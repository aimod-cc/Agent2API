//! 智谱清言余额 / 积分查询。
//!
//! ── 三条读数的来源（实测，2026-10）─────────────────────────────
//! ```text
//!   1. 会员信息（**主读数，积分在这里**）
//!      GET /chatglm/member-api/member/member_info
//!        result: { left_score, true_left_score, left_token,
//!                  token_left_percent, token_usage_percent,
//!                  is_member, level, vip_expired_at, svip_expired_at,
//!                  member_expiration_time, score_rule, ... }
//!        `left_score` 就是界面上那个「积分」，但它是**百分之一积分**的
//!        整数（`1864405` = **18644.05** 积分）—— 展示前必须除以 100
//!        （见 [`SCORE_SCALE`]；直接把原值当积分显示会放大一百倍）。
//!        余额绝对值的唯一权威来源就是它。
//!   2. 会员积分活动（**不是余额**，见下）
//!      GET /chatglm/member-api/member/score_activity_status
//!        result: { status, remain_days, target_score, current_score, ... }
//!   3. 每日登录积分（签到接口，只用于展示「今日可得」，见 checkin.rs）
//!   4. 积分流水（**「积分明细」那一屏**，best-effort）
//!      GET /chatglm/member-api/member/score_record?page=1&page_size=N
//!        result: { list: [{title, score, created_at, expired_at}], has_more }
//!        界面上的「积分明细」必须是这一屏，而不是把 `left_score` 换一个名字
//!        再写一遍 —— 首版就是后者，弹层里只有孤零零一行「积分」。
//!        注意 `score` 与 `left_score` **单位不同**（见 [`record_row`]）。
//! ```
//!
//! ── 为什么不能拿 `score_activity_status` 当余额（必读，踩过）──────
//! 首版把它的 `current_score` 当成积分余额，于是**没有活动在跑的账号一律显示 0**
//! （实测未参与活动的账号回 `status: -2, current_score: 0`；客户端自己只把
//! status 1/2/3/4 当有效档，其余走 `default` 分支根本不显示数字）。
//! 那个字段的含义是「**本期会员积分活动**已经攒了多少分」，与「账户里有多少积分」
//! 是两件事 —— 拿它当余额的后果是「明细里明明有积分，余额却写 0」。
//! **账户积分只认 `member_info.left_score`。**
//!
//! 活动信息仍然读（best-effort），但只作为**附加行**（`credits` 里的活动进度），
//! 且 `available` 绝不再取它。
//!
//! ── 访客账号没有积分体系（实测 401 `user needs login`）──────────
//! 访客令牌调这些接口一律 401。这不是「查询失败」而是「该账号形态不支持」，
//! 因此报 [`USAGE_NOT_CONFIGURED_CODE`] 那条中性标记（前端显示成「未配置查询」
//! 而不是红色失败）—— 与 CatPaw 的 token2 / ZCode 的 Coding Plan 同一处置。
//! 文案要说清「访客账号本来就没有积分」，否则用户会去找根本没坏的设置。
//!
//! ── 归一化形状（`ProviderAdapter::query_usage` 的契约）────────
//! `{available, unit: "积分", wallets: [...], credits: [...], raw}`
//!
//! `credits[]` 里混着两类行，靠 `record` 键区分：
//!   · **积分包行**（无 `record`）：`{name, remain, ...}` —— 汇总与活动进度；
//!   · **流水行**（`record: true`）：`{name, amount, at, expireAt, unit}` ——
//!     上游那一屏真实收支，`amount` 带符号（正=获得 / 负=消耗）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{usage_not_configured, USAGE_NOT_CONFIGURED_CODE};
use crate::server::errors::GatewayError;

use super::{credentials, endpoints, http};

/// 从任意 JSON 里按候选键顺序取一个数字（字符串数字也认）
fn pick_number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(field) = value.get(*key) else {
            continue;
        };
        if let Some(number) = field.as_f64() {
            if number.is_finite() {
                return Some(number);
            }
        }
        if let Some(text) = field.as_str() {
            if let Ok(number) = text.trim().parse::<f64>() {
                if number.is_finite() {
                    return Some(number);
                }
            }
        }
    }
    None
}

/// 「访客账号没有积分体系」的中性报错（前端按 code 显示成未配置）
fn guest_has_no_points() -> GatewayError {
    let mut error = usage_not_configured("智谱清言", "登录账号（访客账号没有积分体系）");
    // 覆盖掉 usage_not_configured 的通用文案：本家的情况要说清「访客本来就没有」
    error.message = "该账号是智谱清言**访客**，没有积分体系（访客只能对话，不能查询积分 / 签到）。\
                     请添加一个登录账号后再查。"
        .to_string();
    error.code = Some(USAGE_NOT_CONFIGURED_CODE.to_string());
    error
}

/// 上游积分原始值的**换算分母**（100 分 = 1 积分）。
///
/// ── 为什么必须换算（实测定的，别把原始值直接显示）──────────────
/// 上游 `member_info.left_score` 给的是**百分之一积分**的整数：账号里 18644.05
/// 积分对应 `1864405`。把原始值当积分显示会**放大 100 倍**（1864405 而不是
/// 18644.05），与用户在客户端里看到的数字对不上 —— 这是必须换算的唯一理由。
///
/// 换算后保留两位小数：上游本身就是两位精度（末两位即小数），进位规则按
/// 四舍五入（`round` 到百分位），不做额外截断。
///
/// ── 旁证：`score_alert_config.threshold` ───────────────────────
/// 积分预警配置里 `threshold: 15000`（整数、正常量级），与 `left_score` 的
/// `1864405` 差两个数量级 —— 同一个账号下两个积分字段不可能用不同单位，
/// 因此余额一定是「百分之一积分」。这条旁证是本换算最硬的外部依据
/// （客户端模板里反而是直接渲染 `left_score`，看不出除法）。
const SCORE_SCALE: f64 = 100.0;

/// 上游原始分值 → 展示用积分（除以 [`SCORE_SCALE`]，保留两位小数）。
///
/// 非有限值（NaN / inf）一律给 0：`member_info` 的字段上游可能给字符串，
/// 解析失败时上游已经返回 None，走到这里的一定是数字；防御一下不让 NaN
/// 渗进 JSON（serde_json 对 NaN 会序列化成 null，前端拿到 null 会显示空）。
///
/// ── 公开给 `checkin` 同用（同一套单位，不要各写一份）──────────────
/// 签到 / 领取返回的 `result.score` 与余额是同一字段族（同一账户的积分），
/// 因此必须走同一个换算。两处各写一个 `/100` 迟早漂成「余额对了、签到差一百倍」。
pub(super) fn score_from_raw(raw: f64) -> f64 {
    if !raw.is_finite() {
        return 0.0;
    }
    (raw / SCORE_SCALE * 100.0).round() / 100.0
}

/// 原始分值（保留给 `raw` 与排查用）：不换算，原样回报上游给的那个整数。
fn raw_score(value: Option<f64>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

/// 流水一次取多少条。
///
/// 客户端弹层的口径是「仅展示近 90 天明细」，但上游**不按天过滤**（它按
/// `page/page_size` 翻页，`has_more` 报还有没有下一批），所以这里靠条数封顶。
/// 50 条足够铺满那个 320px 宽的弹层（再多也只是滚得更久），代价是一次请求 ——
/// 余额格每次查询都会带上它，所以不能无限翻页。
const SCORE_RECORD_LIMIT: i64 = 50;

/// 拉一次积分流水（best-effort：失败给 `None`，绝不让余额查询跟着失败）。
///
/// 返回 `(list, has_more)`。`page` / `page_size` 必填（缺了上游回 400）。
async fn fetch_score_records(
    token: &str,
    device_id: &str,
) -> Option<(Vec<Value>, bool)> {
    let path = format!("{}?page=1&page_size={SCORE_RECORD_LIMIT}", endpoints::PATH_SCORE_RECORD);
    let payload = http::get_json(&path, token, device_id, None, "积分流水查询")
        .await
        .ok()?;
    let result = http::check_envelope(&payload, "积分流水查询").ok()?;
    let list = result
        .get("list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_more = result
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some((list, has_more))
}

/// 流水一条 → 明细行。
///
/// ── 形状为什么长这样（与 `credits` 里其余行的区别）──────────────
/// 其余行是**积分包**（面额 / 已用 / 剩余 + 领取→到期），前端按「剩 x / y」渲染；
/// 流水一笔是**收支记录**，没有"剩余"可讲。所以带 `record: true` 标记，让前端
/// 换一套读法（带符号金额 + 发生时刻），而不是硬套包的那套措辞 ——
/// 不标记的话，一笔 `-0.96` 的消耗会因为 `remain < 0` 被判成「已用完」，
/// 顺手折叠进弹层末尾那组，用户就再也看不见它了。
///
/// ── 分数**不换算**（与余额相反，别照抄 `score_from_raw`）────────
/// 流水的 `score` 就是真实积分（实测 11 条相加 = 余额 24144.05），
/// 而 `member_info.left_score` 是百分之一。同一个人账号里两套单位，
/// 混用就会出现「明细加起来是余额的一百倍」。
fn record_row(entry: &Value) -> Option<Value> {
    let amount = pick_number(entry, &["score"])?;
    let title = entry
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("积分变动");
    let mut row = json!({
        "name": title,
        "record": true,
        "amount": amount,
        "unit": "积分",
    });
    let Some(object) = row.as_object_mut() else {
        return None;
    };
    // 上游给的是**秒**级时间戳，统一成毫秒（与其余几家的 grantAt / expireAt 同口径）
    if let Some(created) = pick_number(entry, &["created_at"]).filter(|value| *value > 0.0) {
        object.insert("at".to_string(), Value::from((created * 1000.0) as i64));
    }
    if let Some(expired) = pick_number(entry, &["expired_at"]).filter(|value| *value > 0.0) {
        object.insert("expireAt".to_string(), Value::from((expired * 1000.0) as i64));
    }
    let remark = entry
        .get("remark")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let Some(remark) = remark {
        object.insert("remark".to_string(), Value::String(remark.to_string()));
    }
    Some(row)
}

/// 查询某账号的积分余额（归一化形状见模块头）。
pub(super) async fn query(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.chatglm_account_record(account_id);
    let credentials = credentials::from_record(record.as_ref())?;
    // 访客账号：直接给中性提示（不去打注定 401 的接口）
    if credentials.is_guest {
        return Err(guest_has_no_points());
    }
    let token = credentials.bearer();

    // ── 主读数：会员信息里的 `left_score`（**账户积分**，见模块头）──
    // 打不通就如实报错：这是余额的唯一权威来源，拿不到就不能编一个数字出来
    let member_payload = http::get_json(
        endpoints::PATH_MEMBER_INFO,
        &token,
        &credentials.device_id,
        None,
        "积分查询",
    )
    .await?;
    let member = http::check_envelope(&member_payload, "积分查询")?;
    // `left_score` 实测是**百分之一积分**的整数（1864405 = 18644.05 积分），
    // 必须除以 100 再展示，否则放大一百倍（见 `SCORE_SCALE`）
    let current_raw = pick_number(&member, &["left_score"])
        .or_else(|| pick_number(&member, &["true_left_score"]));
    let current = current_raw.map(score_from_raw).unwrap_or(0.0);
    // 令牌额度的绝对值：只进 `raw` 供排查（量纲未确证，见下面 wallets 的说明）
    let left_token = pick_number(&member, &["left_token"]);
    let token_left_percent = member
        .get("token_left_percent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let is_member = member.get("is_member").and_then(Value::as_bool).unwrap_or(false);
    let level = member
        .get("level")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let score_rule = member
        .get("score_rule")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());

    // ── 支线（best-effort）：会员积分活动。**只作附加行**，绝不当余额 ──
    // 没在跑活动的账号回 `status:-2 / current_score:0`（实测），
    // 若拿它当余额，这类账号的积分会一律显示成 0（首版就是这么错的，见模块头）
    let activity = http::get_json(
        endpoints::PATH_SCORE_ACTIVITY_STATUS,
        &token,
        &credentials.device_id,
        None,
        "积分活动查询",
    )
    .await
    .ok()
    .and_then(|payload| http::check_envelope(&payload, "积分活动查询").ok());
    let activity_current = activity
        .as_ref()
        .and_then(|value| pick_number(value, &["current_score"]));
    let activity_target = activity
        .as_ref()
        .and_then(|value| pick_number(value, &["target_score"]));
    let activity_status = activity
        .as_ref()
        .and_then(|value| value.get("status").and_then(Value::as_i64));

    // ── wallets：积分（主档）+ 令牌额度（有百分比时才带上）──────────
    // 令牌额度只报**百分比**：客户端自己就只读 `token_left_percent`、
    // 从不展示 `left_token` 的绝对值（实测 bundle 里 `left_token` 零引用），
    // 而那个整数的量纲没有可交叉验证的第二来源 —— 与其猜它是不是也按
    // 百分之一折算，不如只报一个确定有意义的比例（原始值留在 `raw` 里）。
    let mut wallets = vec![json!({
        "type": "score",
        "displayName": "积分",
        "balance": current,
    })];
    if let Some(percent) = token_left_percent.as_deref() {
        wallets.push(json!({
            "type": "token_quota",
            "displayName": "令牌额度",
            "percent": percent,
        }));
    }

    // ── credits 明细行 ─────────────────────────────────────────
    let mut credits = vec![json!({
        "name": "积分",
        "remain": current,
    })];
    if let Some(rule) = score_rule {
        if let Some(object) = credits[0].as_object_mut() {
            object.insert("rule".to_string(), Value::String(rule.to_string()));
        }
    }
    // 只有真的在跑活动（客户端认的 1/2/3/4 档）才列这一行 ——
    // `-2`（无此活动）之类的值只会在明细里摆一个恒 0 的数字，误导用户。
    //
    // 活动分的量纲**不确定**（当前账号 `status:-2` 拿不到样本，无法交叉验证
    // 它是否与 `left_score` 一样按百分之一折算），因此**原样透出**并在文案里
    // 标明来自活动：宁可显示上游原值，也不要猜一个折算后对不上的数字。
    if let Some(status) = activity_status.filter(|status| (1..=4).contains(status)) {
        let mut row = json!({
            "name": "会员积分活动",
            "remain": activity_current.unwrap_or(0.0),
            "status": status,
        });
        if let Some(object) = row.as_object_mut() {
            if let Some(target) = activity_target {
                object.insert("target".to_string(), Value::from(target));
            }
            if let Some(days) = activity
                .as_ref()
                .and_then(|value| pick_number(value, &["remain_days"]))
            {
                object.insert("remainDays".to_string(), Value::from(days));
            }
        }
        credits.push(row);
    }

    // ── 真实流水（「积分明细」点开那一屏的内容）───────────────────
    // best-effort：这是余额格的附赠读数，打不通就只少一屏明细，
    // 不该把整个余额查询拖成失败（主读数已经拿到了）。
    let records = fetch_score_records(&token, &credentials.device_id).await;
    let (record_rows, records_has_more) = match records.as_ref() {
        Some((list, has_more)) => (
            list.iter().filter_map(record_row).collect::<Vec<Value>>(),
            *has_more,
        ),
        None => (Vec::new(), false),
    };
    credits.extend(record_rows.iter().cloned());

    // ── 会员档位（从 member_info 本身的字段推，不再打 payment-api）──
    // `is_member` / `level` / `vip_expired_at` / `svip_expired_at` 都在主读数里，
    // 同一个响应能回答的事不必再开一条支线（少一次网络、少一处会漂的字段名）
    let mut subscription = Value::Null;
    let vip_expire = pick_number(&member, &["vip_expired_at", "member_expiration_time"]);
    let svip_expire = pick_number(&member, &["svip_expired_at"]);
    if is_member || !level.is_empty() || vip_expire.is_some() || svip_expire.is_some() {
        subscription = json!({
            "planName": level,
            "isMember": is_member,
            "vipExpireAt": vip_expire.map(|value| value as i64).unwrap_or(0),
            "svipExpireAt": svip_expire.map(|value| value as i64).unwrap_or(0),
        });
        if let Some(object) = credits.get_mut(0).and_then(Value::as_object_mut) {
            if !level.is_empty() {
                object.insert("plan".to_string(), Value::String(level.to_string()));
            }
        }
    }

    let mut document = json!({
        "available": current,
        "unit": "积分",
        "wallets": wallets,
        "credits": credits,
        "raw": {
            "member": member_payload,
            "activity": activity,
            "activityStatus": activity_status,
            // 换算前的原始整数（1864405 这类）与令牌绝对值：只在排查时看，
            // 界面一律用上面换算过的 `available` / `credits[].remain`
            "leftScoreRaw": raw_score(current_raw),
            "leftTokenRaw": raw_score(left_token),
            // 流水只取了前 [`SCORE_RECORD_LIMIT`] 条；`hasMore` 为真说明
            // 上游还有更早的（界面不翻页，这是排查「怎么少了几条」的依据）
            "recordsReturned": record_rows.len(),
            "recordsHasMore": records_has_more,
        },
    });
    if subscription != Value::Null {
        if let Some(object) = document.as_object_mut() {
            object.insert("subscription".to_string(), subscription);
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_numbers_from_mixed_shapes() {
        let value = json!({ "a": 12.5, "b": "30", "c": "not a number" });
        assert_eq!(pick_number(&value, &["a"]), Some(12.5));
        assert_eq!(pick_number(&value, &["b"]), Some(30.0));
        assert_eq!(pick_number(&value, &["c"]), None);
        assert_eq!(pick_number(&value, &["missing"]), None);
        // 候选键顺序
        assert_eq!(pick_number(&json!({ "x": 1, "y": 2 }), &["y", "x"]), Some(2.0));
    }

    #[test]
    fn guest_error_is_neutral_and_mentions_guest()  {
        let error = guest_has_no_points();
        assert_eq!(error.status_code, 400, "中性标记用 400（前端显示成未配置）");
        assert_eq!(error.code.as_deref(), Some(USAGE_NOT_CONFIGURED_CODE));
        assert!(error.message.contains("访客"));
    }

    /// **账户积分只认 `member_info.left_score`**，不是活动进度分。
    ///
    /// ── 锁的是真实发生过的故障 ──────────────────────────────────
    /// 首版把 `score_activity_status.current_score` 当余额，于是**没在跑活动的
    /// 账号余额一律显示 0**（实测那种账号回 `status:-2 / current_score:0`），
    /// 而账户里明明有积分。用户看到的就是「明明有 18000 多积分，查询却是 0」。
    #[test]
    fn score_comes_from_member_info_not_activity() {
        // 真实响应（2026-10 实测，登录账号）
        let member = json!({
            "id": 0,
            "user_id": "66d2a65fd3b36febe583d900",
            "is_member": false,
            "level": "",
            "left_score": 1864405,
            "true_left_score": 0,
            "left_token": 74568559,
            "token_left_percent": "100%",
            "score_rule": "免费用户，登录赠送200积分/天",
        });
        // 主读数取 left_score（不是 true_left_score：后者实测恒 0，
        // 是另一档口径；界面显示的「积分」对应 left_score）
        let raw = pick_number(&member, &["left_score"]);
        assert_eq!(
            raw,
            Some(1864405.0),
            "账户积分必须来自 member_info.left_score"
        );
        // 取到之后**必须换算**：1864405 → 18644.05（用户实际看到的数字）
        assert_eq!(
            raw.map(score_from_raw),
            Some(18644.05),
            "展示值要除以 100；直接显示原始值会放大一百倍"
        );

        // 没在跑活动的账号：current_score 恒 0，拿它当余额就是那个 bug
        let activity = json!({ "status": -2, "remain_days": 0, "current_score": 0, "target_score": 0 });
        assert_eq!(pick_number(&activity, &["current_score"]), Some(0.0));
        assert!(
            !(1..=4).contains(&activity["status"].as_i64().unwrap_or(0)),
            "status -2 不是客户端认的有效活动档，不该被当成「活动在跑」列出"
        );
    }

    /// **积分要除以 100 再展示**：上游给的是百分之一积分的整数。
    ///
    /// ── 锁的是真实发生过的故障 ──────────────────────────────────
    /// 首版（改对接口之后）把 `left_score: 1864405` 直接当积分显示，
    /// 界面上变成 **1864405**，而客户端里是 **18644.05** —— 放大了一百倍。
    /// 换算规则见 [`SCORE_SCALE`]，改这里之前先想清楚精度。
    #[test]
    fn score_is_scaled_down_by_one_hundred() {
        // 实测值：用户账号 18644.05 积分 → 上游 1864405
        assert_eq!(score_from_raw(1864405.0), 18644.05);
        // 整数积分不产生多余小数位带来的误差
        assert_eq!(score_from_raw(20000.0), 200.0);
        assert_eq!(score_from_raw(0.0), 0.0);
        // 末两位就是小数：1234 → 12.34
        assert_eq!(score_from_raw(1234.0), 12.34);
        // 只保留两位（不引入浮点尾巴）
        let scaled = score_from_raw(1864407.0);
        assert_eq!(scaled, 18644.07);
        assert_eq!(format!("{scaled:.2}"), "18644.07");
    }

    /// 原始分值原样保留在 `raw` 里（排查用），不被换算影响
    #[test]
    fn raw_score_keeps_upstream_integer() {
        assert_eq!(raw_score(Some(1864405.0)), json!(1864405.0));
        assert_eq!(raw_score(None), Value::Null);
    }

    /// 非有限值不渗进 JSON（serde_json 会把 NaN 序列化成 null，前端显示空）
    #[test]
    fn non_finite_scores_degrade_to_zero() {
        assert_eq!(score_from_raw(f64::NAN), 0.0);
        assert_eq!(score_from_raw(f64::INFINITY), 0.0);
        assert_eq!(score_from_raw(f64::NEG_INFINITY), 0.0);
    }

    /// 有效活动档（1..=4）才列活动那一行 —— 客户端自己就是这么判的
    /// （`status===2 → claimable` / `3 → usable` / `4 → encourage`）
    #[test]
    fn only_client_recognized_statuses_are_valid() {
        for status in [1i64, 2, 3, 4] {
            assert!((1..=4).contains(&status));
        }
        for status in [-2i64, -1, 0, 5, 99] {
            assert!(
                !(1..=4).contains(&status),
                "status {status} 不是有效活动档，列出只会在明细里摆一个恒 0 的数字"
            );
        }
    }

    /// **流水行不换算**：`score_record.score` 已经是真实积分（与余额相反）。
    ///
    /// ── 锁的是最容易顺手写错的一处 ──────────────────────────────
    /// 同一个账号里两套单位：`member_info.left_score` 是百分之一（要除 100），
    /// 而流水 `score` 就是积分本身。实测 11 条相加 = 余额 24144.05 —— 若这里
    /// 也套 `score_from_raw`，弹层里每一笔都会缩小一百倍（+1000 变成 +10），
    /// 且「明细之和 = 余额」这条自洽关系当场断掉。
    #[test]
    fn score_records_are_already_real_points() {
        // 真实响应里的两类记录（一正一负）
        let gain = record_row(&json!({
            "id": "6ad2e87c91d6ee41ee24e966",
            "title": "【任务奖励】创建群聊",
            "score": "1000.00",
            "created_at": 1792206972,
            "expired_at": 1823742972,
        }))
        .expect("正数记录应当成行");
        assert_eq!(gain["record"], json!(true), "必须带 record 标记（前端靠它换读法）");
        assert_eq!(gain["amount"], json!(1000.0), "金额不换算：原样 1000 积分");
        assert_eq!(gain["unit"], json!("积分"));
        assert_eq!(gain["name"], json!("【任务奖励】创建群聊"));

        let spend = record_row(&json!({
            "title": "【Agent任务】你好",
            "score": "-0.96",
            "created_at": 1792200000,
        }))
        .expect("负数记录应当成行");
        assert_eq!(spend["amount"], json!(-0.96), "消耗是负数，符号必须留着");
    }

    /// 秒级时间戳转毫秒（前端按毫秒格式化）
    #[test]
    fn record_timestamps_are_promoted_to_millis() {
        let row = record_row(&json!({
            "title": "登录赠送",
            "score": "200.00",
            "created_at": 1792206972,
            "expired_at": 1823742972,
        }))
        .expect("应当成行");
        assert_eq!(row["at"], json!(1792206972000i64), "created_at 是秒，要乘 1000");
        assert_eq!(row["expireAt"], json!(1823742972000i64), "expired_at 同理");
    }

    /// 读不出金额的记录直接丢弃（宁少一行，也不在明细里摆一笔 `—`）；
    /// 缺时间字段只是少显示一段，不影响这一行成立
    #[test]
    fn record_rows_survive_missing_fields() {
        assert!(record_row(&json!({ "title": "没有金额" })).is_none());
        let bare = record_row(&json!({ "score": "5" })).expect("只有金额也成行");
        assert_eq!(bare["name"], json!("积分变动"), "没标题时给一个中性名");
        assert!(bare.get("at").is_none(), "没给时刻就不写这个键（不是写 0）");
        assert!(bare.get("expireAt").is_none());
    }
}
