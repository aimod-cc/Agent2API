//! ZCode 套餐余额查询（`GET /api/v1/zcode-plan/billing/balance`）。
//!
//! ── 本文件是**候选令牌链**的编排处（两个通道，见 `query_usage`）────
//! 这条链上的两个通道各认一把凭证：
//!
//! | 凭证 | 通道 | 读到的 |
//! |---|---|---|
//! | `jwt`（套餐 JWT） | 本文件（`zcode.z.ai` 的 billing 网关） | 余额桶 / 套餐 / 到期 |
//! | `accessToken`（编码套餐 API Key） | [`super::monitor`]（开放平台监控接口） | 窗口限额与套餐等级 |
//!
//! 文件内下面几段（字段清单出处、`X-Device-Mid`、与官方客户端的差异）讲的都是
//! **billing 通道** —— 监控通道在那边的 `monitor.rs`，两边归一化成同一个形状，
//! 界面因此不必分家。
//!
//! ── 这是 billing 网关上的第三个接口 ──────────────────────────
//! 同一台网关（`zcode.z.ai`）上前两个是 `preview` / `claim`（见 `claim.rs`，
//! 领取链路），本文件补的是**读数**：账号还剩多少、哪几个桶、什么时候过期。
//! 与领取一样，它认的是 **套餐 JWT**（不是推理用的 `accessToken`，两者不能
//! 互相替代，见 `credentials.rs` 的模块头）。
//!
//! ── `X-Device-Mid` 必须带（而且是 UUID 形态）─────────────────
//! 同一个 billing 网关上，**探测接口**对这一条是实测的：缺这个头、或值不是
//! UUID 形态，网关直接回 `400 {"code":3001,"msg":"parameter error"}`，与
//! `app_version` / `platform` / 各种客户端身份头都无关（2026-09-28 实测）。
//! 本接口（balance）上有 token 的账号才能走到参数校验之后，因此**没法用匿名
//! 请求把它单独测出来**；第三方实现（token-monitor 的 zai 探针）记的是同一台
//! 网关上的同一条要求，本家按同一口径发，反正它只多一个头。
//! 值必须**跨请求稳定**（风控据此关联同一设备的请求），所以账号记录里本来就
//! 存着一个（登录时生成、随凭证落盘），缺失时由
//! [`AccountStore::zcode_device_mid_or_create`] 现场生成一次并落盘 ——
//! 不要在调用点现编，每次现编等于「同一个账号天天换设备」。
//!
//! ── 字段清单的出处：官方客户端，不是第三方实现 ────────────────
//! `zai-org/ZCode`（官方仓库）的
//! `packages/services/src/model-provider/zaiStartPlanBilling.ts` 定义了
//! 响应接口 `ZaiStartPlanBalanceEnvelope`：`data.server_time` / `data.plans[]`
//! / `data.balances[]`，字段名逐条照抄（`bucket_id` / `user_plan_id` /
//! `show_name` / `meter` / `unit_type` / `capabilities` / `total_units` /
//! `used_units` / `reserved_units` / `remaining_units` / `available_units` /
//! `period_start` / `period_end` / `expires_at`）。
//! 三处**官方语义**一并移植，别当成优化去掉：
//!   1. 套餐状态**按时间判**（`ends_at` 已过 → `expired`；**生效时间还没到** →
//!      `pending`），`status` 只当兜底 —— 上游偶尔不刷状态，照直读会让过期套餐
//!      一直显示在身边；反过来只认 `status` 字面值又会让**刚领到、还没到生效
//!      时间**的活动套餐整条消失。三态与判法见 `plan_state`；
//!   2. 现在还不能用的套餐（已过期 / 权益**全都**还没到生效时间）名下的余额桶
//!      **整条丢掉**（`user_plan_id` 优先、退回 `plan_id` 配对）—— 否则
//!      「昨天领的 1 亿」会永远挂在那里，而「今天 23:00 才生效的 1 亿」会提前
//!      冒充可用额度；
//!   3. 认不出归属的桶**保留**（宁可在读数里多一行，也不误删另一个有效套餐的余额）。
//!
//! ── 「生效时间」读哪儿（2026-10-10 按官方源码对齐）────────────
//! 生效时间挂在**权益**上（`entitlements[].effective_at`，unix 秒；`0` = 立即生效
//! 哨兵）。套餐级 `starts_at` 是**领取 / 购买时间**，只在没有权益时兜底 —— 早期
//! 实现拿它当生效时间，于是界面上把「领取那一刻」显示成了生效时间（官方客户端
//! 同一份套餐显示的是「待生效 今天 23:00」）。判「到点没有」的基准是上游的
//! `server_time`（官方 availability 同样用服务端时间，不用本机时钟）。细则与
//! 出处见 `plan_effective_times` / `pending_until` / `all_pending`。
//!
//! ── 输出里的 `plans` 数组（界面「套餐明细」用）────────────────
//! 归一化结果里除了 `wallets`（可用读数）还多一段 `plans`：名下**全部**套餐及其
//! 三态，含未生效与已过期的。旧版只把套餐名压进 `subscription` 那一个「代表」里，
//! 界面没地方回答「我名下有哪些套餐、各是什么状态」，刚领到还没生效的那份因此
//! 完全看不见（这次改动的起源）。时间口径是 **unix 秒**，与领取预览一致
//! （`subscription.expireAt` 是毫秒，别混）。
//!
//! ── 与官方客户端的差异（有意为之）────────────────────────────
//!   - 官方按 `show_name` 把同模型的桶**相加**成一个池（token-monitor 等第三方
//!     也这么做）。本家不合并：账号页的余额面板是逐桶列明细的，合并会让
//!     「哪个活动/套餐给的额度还剩多少」消失 —— 而 ZCode 这个账号页恰恰是靠
//!     「今天领的 1 亿还剩多少」说话的（每日活动，见 `claim.rs` 的模块头）。
//!     逐桶列出来，面板上就是「GLM-5.3-Flash 1 亿 / 1 亿 token」这样一行一个来源。
//!   - 官方把「空 balances」当 `unavailable`；本家把两件事分开说：「这个账号名下
//!     没有可读的额度桶」如实呈现（`availableView` = 「无额度」，那确实不是「没读到」），
//!     但**不给数字 0** —— 没有桶 ≠ 余额是 0，额度也可能记在活动套餐那条通道上，
//!     详见 `available` 那段注释（写 0 会让账号被「余额不足」跳过档误剔）。
//!
//! ── 出网代理：跟着账号走（与领取同一条，与余额查询的其它家相反）──
//! `zcode.z.ai` 对国内用户常常需要代理，而账号记录上的出口正是用户为这个账号
//! 配的。所以这里用 `session_proxy`（与 `api::zcode_claim::account_proxy` 同一
//! 取法），不走直连 —— 理由与 `api/zcode_claim.rs` 的模块头逐字相同。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，取值一律走 Option 链。

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http::send_raw;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::claim::{self, app_version, platform};
use super::region::Region;

/// 余额接口的请求超时。与领取同一档（15 秒）：同一个网关、同一种低频只读调用，
/// 而 `egress` 的默认 read_timeout 是 600 秒（给 SSE 长连接留的）——
/// 不设总超时会让前端的「查询余额」转圈十分钟。
const REQUEST_TIMEOUT_MS: u64 = 15_000;

/// 余额路径（官方客户端 `zcodePlanBillingBalanceUrl` 的后半段）
const BALANCE_PATH: &str = "/api/v1/zcode-plan/billing/balance";

/// 查询某账号的套餐余额（归一化形状见 `ProviderAdapter::query_usage` 的文档）。
///
/// ── 候选令牌链：两个通道，谁先有读数用谁 ────────────────────
/// 本家的账号记录里有**两把互不替代的凭证**（见 `credentials.rs` 的模块头），
/// 各自能打开一个读数通道：
///
/// | 凭证 | 通道 | 读到的 |
/// |---|---|---|
/// | `jwt`（套餐 JWT） | 本文件：`{zcode}/api/v1/zcode-plan/billing/balance` | 余额桶 / 套餐 / 到期 |
/// | `accessToken`（编码套餐 API Key） | [`super::monitor`]：`{监控平面}/api/monitor/usage/quota/limit` | 窗口限额（每 N 小时 / 每周的已用比例）与套餐等级 |
///
/// 只粘了 API Key 的账号（手工添加那条路）此前在这条链上是**死路**
/// （「未配置」），而它其实有可读的额度；反过来，套餐 JWT 被上游拒时
/// （换过账号 / 提前失效）也不该整条读数消失。因此顺序是：
///   1. 有 `jwt` → 走 billing（**存量账号的行为逐字不变**：第一次尝试就是原来那一次）；
///   2. 上面没读数（缺 jwt，或 jwt 通道失败）→ 有 `accessToken` 就走监控通道；
///   3. 两条都不行 → 「未配置」（一把凭证都没有）或错误（有凭证但都被拒）。
///
/// 失败时**不合并两个通道的错误**，而是：401 优先透出（它最可执行 ——
/// 「请重新登录」），否则取第一个错误。合并会让用户拿到一句看不出该做什么的话。
/// 每个被跳过的通道都记一条 verbose 日志（`[Usage]`），排障时能看到走了哪条。
///
/// 失败语义按调用方契约：**缺凭证**是「未配置查询凭证」（400 +
/// `usage_not_configured`，前端显示成中性提示），**凭证被拒**原样透出 401
/// （调用方据此走「刷新后重试一次」；本家没有续期协议，那条路会如实报
/// 「请重新登录」）。
pub(super) async fn query_usage(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.zcode_account_record(account_id).ok_or_else(|| {
        GatewayError::with_status(404, "找不到该 ZCode 账号".to_string())
    })?;
    let region = record
        .get("provider")
        .and_then(Value::as_str)
        .and_then(Region::from_provider_id)
        .ok_or_else(|| GatewayError::with_status(400, "该账号不是 ZCode 账号".to_string()))?;
    let text = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let jwt = text("jwt");
    let coding_key = text("accessToken");
    let account_id_owned = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(account_id)
        .to_string();
    let proxy = store
        .get_session_by_id(&account_id_owned)
        .and_then(|entry| crate::server::core::proxies::session_proxy(&entry.session));

    let mut errors: Vec<GatewayError> = Vec::new();
    if !jwt.is_empty() {
        // 设备标识：记录里有就用，没有就现场生成并落盘（见模块头）。
        // 这里分两步调 store 的方法 —— 它们各自取锁，**不能**嵌套（会死锁）。
        // 只有 billing 通道要它：监控通道不需要设备头（见 `monitor.rs`），
        // 「只粘了 API Key」的账号因此不会为一次查询平白落一条设备标识。
        let device_mid = store.zcode_device_mid_or_create(&account_id_owned);
        match billing_channel(region, &jwt, device_mid.as_deref(), proxy.as_ref()).await {
            Ok(value) => {
                // ── 「有套餐、但一个额度桶都没有」时补一条监控通道的窗口读数 ──
                // 活动套餐（start-plan）的额度不落在 billing 的桶里：2026-10-10 实测
                // 一份「生效中、3 亿 token」的套餐，`balances` 是空的 —— 界面上只剩
                // 一个「权益 3亿 token」的干读数，**连进度条都画不出来**（列宽那一格
                // 的进度条要的是比例）。开放平台的监控通道对同一把账号回的是
                // **窗口限额与剩余比例**（每 N 小时 / 每周），那正是画条要的东西。
                //
                // 只在 billing 一个桶都没读到、且账号有编码套餐 API Key 时合并：
                // 有桶就说明 billing 那侧是权威读数，硬塞一份监控通道的窗口只会让
                // 两套口径混在同一列里。合并的只有 `wallets` / `availableView`
                // （这两样本来就描述「这一列显示什么」），**不动** `plans` /
                // `subscription` / `available`（套餐清单与到期仍只认 billing；
                // `available` 保持 null，见它那段注释 —— 窗口比例不是余额数字，
                // 不该参与「余额不足就跳过」的判定）。
                //
                // 代价是这类账号每次余额查询会多打一次监控接口；它们本来就只有
                // 一次 billing 调用（且回的是空），换来的是一条能画条的读数。
                if !has_wallets(&value) && !coding_key.is_empty() {
                    match super::monitor::query(region, &coding_key, proxy.as_ref()).await {
                        Ok(windows) if has_wallets(&windows) => {
                            return Ok(merge_monitor_windows(value, windows))
                        }
                        Ok(_) => {}
                        Err(error) => {
                            logging::verbose(
                                "[Usage]",
                                &format!(
                                    "ZCode {}账号 {account_id_owned} 的套餐令牌通道没有额度桶，\
                                     监控通道也没读到读数（{}）",
                                    region.label(),
                                    error.message
                                ),
                            );
                        }
                    }
                }
                return Ok(value);
            }
            Err(error) => {
                logging::verbose(
                    "[Usage]",
                    &format!(
                        "ZCode {}账号 {account_id_owned} 的套餐令牌通道未读到读数（{}），\
                         退回编码套餐 API Key 通道",
                        region.label(),
                        error.message
                    ),
                );
                errors.push(error);
            }
        }
    }
    if !coding_key.is_empty() {
        match super::monitor::query(region, &coding_key, proxy.as_ref()).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                logging::verbose(
                    "[Usage]",
                    &format!(
                        "ZCode {}账号 {account_id_owned} 的监控通道未读到读数（{}）",
                        region.label(),
                        error.message
                    ),
                );
                errors.push(error);
            }
        }
    }
    if errors.is_empty() {
        // 两把凭证都没有：用可识别的「未配置」而不是失败 —— 用户去账号设置里
        // 补上任意一把就能查（两把各能读一条通道，见上表）
        return Err(crate::server::core::providers::adapter::usage_not_configured(
            &format!("ZCode {}", region.label()),
            "Coding Plan JWT 或编码套餐 API Key",
        ));
    }
    // 401 优先（「请重新登录」是用户能照着做的那一句），否则第一个错误
    let position = errors
        .iter()
        .position(|error| error.status_code == 401)
        .unwrap_or(0);
    Err(errors.swap_remove(position))
}

/// billing 通道：`GET {zcode}/api/v1/zcode-plan/billing/balance`（认套餐 JWT）。
///
/// 这是本家最早的读数路径，行为与候选链上线前逐字相同（`source` 是唯一新增
/// 的键，排障用）。
async fn billing_channel(
    region: Region,
    jwt: &str,
    device_mid: Option<&str>,
    proxy: Option<&ResolvedProxy>,
) -> Result<Value, GatewayError> {
    let url = format!(
        "{}{BALANCE_PATH}?app_version={}&platform={}",
        region.zcode_origin(),
        claim::urlencode(&app_version()),
        claim::urlencode(platform())
    );
    // 头集合：三个，一个不多（见模块头：其余客户端身份头对这台网关无效）。
    // 设备标识没有值时**不发空头**：空头与缺失同效（都是 3001），
    // 但日志里能少一条误导性的记录。
    let mut headers: Vec<(String, String)> = vec![
        ("Authorization".to_string(), format!("Bearer {jwt}")),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if let Some(mid) = device_mid.map(str::trim).filter(|value| !value.is_empty()) {
        headers.push(("X-Device-Mid".to_string(), mid.to_string()));
    }

    let response = send_raw("GET", &url, None, &headers, proxy, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, "ZCode 余额查询超时")
            } else {
                GatewayError::with_status(502, format!("ZCode 余额查询失败: {error}"))
            }
        })?;

    let payload = response.payload.clone().unwrap_or(Value::Null);
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = payload
        .get("msg")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    // 401：JWT 被拒（失效 / 吊销 / 换了账号）。原样透出 401，让调用方走它既有的
    // 「刷新后重试一次」处置 —— 本家没有续期协议，那条路最终会报一句可读的话。
    if response.status == 401 || code == 401 {
        return Err(GatewayError::with_status(
            401,
            "ZCode 套餐令牌已失效，请重新登录该账号",
        ));
    }
    // 3001 = 参数错误：这台网关只有一种常见成因 —— 设备标识缺失或不是 UUID
    if code == 3001 {
        return Err(GatewayError::with_status(
            502,
            "ZCode 余额查询被上游以「参数错误」拒绝（常见成因：设备标识无效），请重新登录该账号",
        ));
    }
    // 429 = 风控限速。上游对这台网关的请求很敏感（实测短时间连续请求就会被挡），
    // 而**批量查询多个账号时是并发打过去的** —— 这一档要给一句能照着做的提示，
    // 而不是一句「HTTP 429」（用户看不出是限流还是账号问题）。
    // 注意响应体常常是空的，所以这条要判在下面那个「取 message」的兜底之前。
    if response.status == 429 {
        return Err(GatewayError::with_status(
            502,
            "ZCode 余额查询被上游限流（429），请稍后重试或改为逐个查询",
        ));
    }
    if !(200..300).contains(&response.status) || code != 0 {
        let detail = if message.is_empty() {
            format!("HTTP {}", response.status)
        } else {
            message.clone()
        };
        // 业务码为 0 表示「响应体里没有 code」（HTTP 层错误，体可能是空的）——
        // 那种情况别拼一个读起来莫名其妙的「（0）」
        let label = if code == 0 { String::new() } else { format!("（{code}）") };
        return Err(GatewayError::with_status(
            502,
            format!("ZCode 余额查询失败{label}：{detail}"),
        ));
    }
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    Ok(normalize(&data))
}

/// 上游 `data` → 账号页的统一形状。
///
/// 纯函数（不碰网络、不碰时钟以外的东西），因此这里能逐条对齐官方的三处语义
/// （见模块头）而不用管调用时机。`now` 取上游的 `server_time`：那是**上游自己
/// 的时间**，拿本机时间判「套餐过没过期」会在时区/时钟不准的机器上误判。
fn normalize(data: &Value) -> Value {
    let now = data
        .get("server_time")
        .and_then(number_of)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or_else(|| (crate::server::logging::now_ms() as f64) / 1000.0);
    let plans: Vec<&Value> = data
        .get("plans")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    // ── 套餐三态（时间优先，见 [`plan_state`]）───────────────────
    // 旧实现是「`status != "active"` 一律当过期」，于是**刚领到的活动套餐**
    // （官方客户端显示「待生效 今天 23:00」的那种：生效时间在未来）被整条丢掉，
    // 界面上表现为「领取成功但看不到套餐」。现在按时间判，并把它原样带出去。
    let state_of = |plan: &Value| -> PlanState { plan_state(plan, now) };
    // 「这份套餐的额度现在能不能用」：已过期的不能；**权益全都还没到生效时间**的
    // 也不能（3 亿还没到点就冒充可用额度，会让用户以为额度已经到账，也会让
    // 余额跳过档少拦一个账号）。其余一律算能用 —— 包括「状态字符串认不出来」的：
    // 那是我们识别能力的边界，不该变成用户的额度凭空消失
    // （旧实现把非 active 一律当过期，正是那一类 bug 的来源）。
    let usable = |plan: &Value| -> bool {
        state_of(plan) != PlanState::Expired && !all_pending(plan, now)
    };
    // 桶的归属：`user_plan_id` 优先、退回 `plan_id`（官方同款配对规则）
    let owners_of = |bucket: &Value| -> Vec<&Value> {
        let user_plan_id = bucket.get("user_plan_id").and_then(Value::as_str);
        let plan_id = bucket.get("plan_id").and_then(Value::as_str);
        plans
            .iter()
            .copied()
            .filter(|plan| match (user_plan_id, plan.get("user_plan_id").and_then(Value::as_str)) {
                (Some(left), Some(right)) => left == right,
                _ => plan_id.is_some() && plan.get("plan_id").and_then(Value::as_str) == plan_id,
            })
            .collect()
    };
    let owner_unusable = |bucket: &Value| -> bool {
        let owners = owners_of(bucket);
        // 认不出归属的桶不丢（官方注释：不能被误删）
        !owners.is_empty() && owners.iter().all(|plan| !usable(plan))
    };
    // 桶所属套餐的展示名（给界面当「这个额度来自哪个套餐」的标签用；认不出就给 None，
    // 界面退回 `subscription.planName`）
    let owner_name = |bucket: &Value| -> Option<String> {
        owners_of(bucket)
            .into_iter()
            .find_map(plan_display_name)
    };

    let mut wallets: Vec<Value> = Vec::new();
    let mut available = 0.0_f64;
    let mut total = 0.0_f64;
    let mut unit = String::new();
    if let Some(buckets) = data.get("balances").and_then(Value::as_array) {
        for bucket in buckets.iter().filter(|bucket| !owner_unusable(bucket)) {
            let show_name = bucket
                .get("show_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("额度")
                .to_string();
            let bucket_unit = bucket
                .get("unit_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("")
                .to_string();
            if unit.is_empty() && !bucket_unit.is_empty() {
                unit = bucket_unit.clone();
            }
            // 剩余量的取值链：`remaining_units` → 总额 − 已用 → `available_units`
            // （官方与第三方实现都是这条链；缺一项就退回下一项，不拿 0 冒充）
            let bucket_total = bucket.get("total_units").and_then(number_of);
            let used = bucket.get("used_units").and_then(number_of);
            let remaining = bucket
                .get("remaining_units")
                .and_then(number_of)
                .or_else(|| match (bucket_total, used) {
                    (Some(total), Some(used)) => Some(total - used),
                    _ => None,
                })
                .or_else(|| bucket.get("available_units").and_then(number_of));
            let Some(remaining) = remaining else {
                continue;
            };
            let unit_for_row = if bucket_unit.is_empty() { unit.clone() } else { bucket_unit };
            let view = match bucket_total {
                Some(total) if total > 0.0 => format!(
                    "{} / {} {}",
                    compact_number(remaining),
                    compact_number(total),
                    unit_for_row
                ),
                _ => format!("{} {}", compact_number(remaining), unit_for_row),
            };
            available += remaining;
            if let Some(value) = bucket_total {
                total += value;
            }
            // 剩余占比（0~100）：账号页那个进度条画的是**剩余**比例（与相邻的读数
            // 「8800万 / 1亿」同一口径 —— 进度条与数字指向相反方向会让人读反）。
            // 总量缺失或为 0 时不给这个字段（前端据此不画进度条，只显示读数）。
            let remaining_percent = bucket_total
                .filter(|value| *value > 0.0)
                .map(|value| (remaining / value * 100.0).clamp(0.0, 100.0));
            wallets.push(json!({
                "type": bucket
                    .get("bucket_id")
                    .and_then(Value::as_str)
                    .or_else(|| bucket.get("plan_id").and_then(Value::as_str))
                    .unwrap_or("zcode_balance"),
                "displayName": show_name,
                "balance": remaining,
                "balanceView": view.trim_end(),
                // 结构化读数（`balanceView` 是给人看的串，这三个是给界面算进度条 /
                // 排序用的）。缺失一律 null：界面按「不知道」处理，不拿 0 冒充。
                "total": bucket_total,
                "used": used,
                "remainingPercent": remaining_percent,
                // 这个额度来自哪个套餐（界面拿它当标签；认不出时不写这个键）
                "planName": owner_name(bucket),
            }));
        }
    }
    if unit.is_empty() {
        unit = "token".to_string();
    }

    let mut subscription = Map::new();
    if let Some(plan) = pick_plan(&plans, now) {
        if let Some(name) = plan_display_name(plan) {
            subscription.insert("planName".to_string(), Value::String(name));
        }
        if let Some(status) = plan.get("status").and_then(Value::as_str) {
            subscription.insert("status".to_string(), Value::String(status.to_string()));
        }
        // 代表套餐自己的状态（机器可读键，见 [`PlanState`]）。界面靠它给余额列
        // 标「待生效」——注意 `status` 是**上游原话**（可能写 active 而生效时间
        // 在未来），判状态一律看 `state`。
        subscription.insert(
            "state".to_string(),
            Value::String(state_of(plan).key().to_string()),
        );
        if let Some(ends_at) = plan.get("ends_at").and_then(number_of).filter(|value| *value > 0.0) {
            // 秒 → 毫秒：账号页各家（qoder / raccoon / workbuddy）的到期一律毫秒，
            // 混用会让时间显示成 1970 年。
            //
            // 只取**套餐本身**的 `ends_at`，不拿额度桶的 `period_end` 顶替：每日桶的
            // period_end 是「今天的 24 点」，写成「套餐到期」会被读成套餐要过期了。
            subscription.insert("expireAt".to_string(), json!((ends_at * 1000.0) as i64));
        }
    }
    if total > 0.0 {
        subscription.insert("totalQuota".to_string(), json!(total));
        subscription.insert("remainQuota".to_string(), json!(available));
    }
    // ── 套餐清单（界面「套餐明细」用）────────────────────────────
    // **全部**套餐都出，不只生效中的：界面上「已领到的套餐」必须看得见刚领到的那份
    // —— 既包括在官方客户端领的，也包括生效时间还没到的（这次的「待生效」需求就
    // 是它）。旧实现只把生效中的桶算进读数、套餐名字又只体现在 `subscription`
    // 那一个代表上，界面上没有第二处能显示「你名下有哪些套餐、各是什么状态」。
    //
    // 时间口径是 **unix 秒**：与领取预览（`api::zcode_claim` 的 `startsAt`/`endsAt`）
    // 同口径，而**不是** `subscription.expireAt` 的毫秒 —— 界面要用同一个渲染器画
    // 「可领取」与「已领取」两份清单，同形状同单位才不容易错；`entitlements` 的键名
    // 也照抄预览，于是那份渲染只有一份代码。
    let plan_items: Vec<Value> = plans
        .iter()
        .map(|plan| {
            json!({
                "planId": plan_id_of(plan),
                "name": plan_display_name(plan).unwrap_or_default(),
                "state": state_of(plan).key(),
                // 缺失一律 null（界面按「不知道」处理，不拿 0 冒充）
                "startsAt": plan_bound(plan, START_KEYS).filter(|value| *value > 0.0).map(|value| value as i64),
                "endsAt": plan_bound(plan, END_KEYS).filter(|value| *value > 0.0).map(|value| value as i64),
                // 待生效时刻（**最早的、还没到的**生效时间；unix 秒）。界面靠它写
                // 「待生效 今天 23:00」—— 与 `startsAt`（领取时间，官方把它当
                // `beginTime`）**不是一回事**，别拿后者顶替（见 `plan_effective_times`）
                "pendingUntil": pending_until(plan, now).map(|value| value as i64),
                "entitlements": entitlements_of(plan),
            })
        })
        .collect();
    json!({
        // ── `available` 只在**真算出了额度桶**时才给数字 ────────────
        // 一个桶都没有时写 `null`（不是 0）：这个键是选路那边「余额不足就跳过」的
        // 判据（`usage_records::extract_remaining` → `limiter::balance_skip_blocked`），
        // 而「一个桶都没有」的含义是**这次读数里没有可用的额度信息**（免费账号没
        // 领过、或套餐的额度记在活动套餐那条通道上，见上面 `plans` 那段），不是
        // 「余额 0」。写 0 会把刚领到套餐的账号判成「余额不足 · 已跳过」、直接从
        // 转发里剔掉 —— 2026-10-10 实测：一份「生效中、3 亿 token」的活动套餐，
        // 读数的 `balances` 是空的，账号页于是挂着「无额度 · 余额不足 · 已跳过」。
        // 口径与 `monitor.rs`（同样不给 `available`）以及 limiter 的既有原则一致：
        // 「跳过是对『这个账号此刻没钱』的断言，断言拿不出证据就不能拦请求」。
        "available": if wallets.is_empty() { Value::Null } else { json!(available) },
        // `availableView` 是给「余额列」的展示串：1 亿这类数字用原始整数
        // （100000000）读起来是 9 位数字，而那一列只有几十像素宽。
        //
        // 没有额度桶时**不写「0 token」**：`code: 0` 且一个桶都没有，含义是
        // 「这个账号名下没有可读的额度桶」（免费账号没领过、或套餐的额度记在
        // 另一个口径上），把它显示成「0」会被读成「额度用光了」。文案取中性的
        // 「无额度」。
        "availableView": if wallets.is_empty() {
            "无额度".to_string()
        } else {
            format!("{} {}", compact_number(available), unit)
        },
        "unit": unit,
        "wallets": wallets,
        "subscription": subscription,
        // 名下全部套餐（含未生效 / 已过期，见上面那段注释）
        "plans": plan_items,
        // 排障用：上游原文（前端默认不展示）
        "raw": data,
        // 这条读数来自哪个通道（候选链会换通道，见 `query_usage`；
        // `monitor.rs` 的归一化输出里是同一个键）
        "source": "zcode.z.ai/billing",
    })
}

/// 套餐的状态：**时间优先**，`status` 只当兜底（理由见 [`plan_state`]）。
///
/// `key()` 是给界面用的**机器可读键**：界面有六种语言，展示文案由前端按这个键
/// 翻译，后端不掺和（返回中文会在非中文界面里露出一串看不懂的汉字）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlanState {
    /// 还没到生效时间（官方客户端显示「待生效」；活动套餐常见 —— 例如周末套餐
    /// 当天 23:00 才开始算）
    Pending,
    /// 生效中：权益现在可用
    Active,
    /// 已过期：不再提供权益
    Expired,
    /// 状态认不出来（时间判不出来、`status` 也不是已知取值）
    Unknown,
}

impl PlanState {
    fn key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }
}

/// 生效时间的候选键链（上游在不同接口/版本里用过不同名字：领取预览那套是
/// `starts_at`，官方 TS 接口里 `effective_at` 挂在权益上）
const START_KEYS: &[&str] = &["starts_at", "start_at", "startsAt", "startAt", "effective_at", "not_before"];
/// 到期时间的候选键链（`ends_at` 是既有口径，其余为容错）
const END_KEYS: &[&str] = &["ends_at", "end_at", "endsAt", "endAt", "expire_at", "expires_at"];

/// 判一份套餐现在是什么状态。
///
/// ── 为什么以时间为主、`status` 只当兜底 ────────────────────────
/// 旧实现只看 `status`：不是 `"active"` 字面值就判过期。而**刚领到的活动套餐**
/// 在生效时间之前常常不是这个字面值（或者虽是 active、但生效时间在未来）——
/// 于是那次领取被归一化整条丢掉：用户在官方客户端看到「待生效 今天 23:00」，
/// 在我们界面上什么都看不到。
///
/// ── 生效时间读哪儿（口径照抄官方客户端）────────────────────────
/// 见 [`plan_effective_times`]：挂在**权益**上的 `effective_at`，套餐级
/// `starts_at` 只在没有权益时兜底 —— 后者是**领取 / 购买时间**，把它当生效时间
/// 就会显示成「领取那刻就生效」（2026-10-10 实测的那个 bug）。
///
/// 认不出来时给 [`PlanState::Unknown`] 而不是 Active：状态字符串认不出是**我们
/// 识别能力**的问题，不该让额度跟着消失（额度桶的取舍见 `normalize` 里的
/// `usable`）—— 这里只影响徽章上写哪三个字。
fn plan_state(plan: &Value, now: f64) -> PlanState {
    // 结束时间已过就是过期（官方同款：`status: active` 但 `ends_at` 过了 → expired）
    if let Some(ends_at) = plan_bound(plan, END_KEYS).filter(|value| *value > 0.0) {
        if ends_at <= now {
            return PlanState::Expired;
        }
    }
    // 还有没到点的权益 → 待生效（官方 UI 口径：只要有一条未来就标「待生效」）
    if pending_until(plan, now).is_some() {
        return PlanState::Pending;
    }
    match plan
        .get("status")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        // 上游认的「在用」字面值（`valid` 来自同族的订阅列表接口）
        Some("active") | Some("valid") => PlanState::Active,
        // 明确表示结束 / 作废的取值（做包含匹配：上游写过 `expired_at` 这类变体）
        Some(value) if value.contains("expire") || value.contains("cancel") || value == "ended" || value == "invalid" => PlanState::Expired,
        // 缺失 / 空串 / 认不出的取值（`paused`、`pending` 之类）：不知道，别猜
        _ => PlanState::Unknown,
    }
}

/// 这份套餐已知的**生效时刻**（unix 秒）。
///
/// ── 口径照抄官方客户端（两处实现交叉验证过）──────────────────
///   · 首要来源是**权益**上的 `entitlements[].effective_at`
///     （`codingPlanProviderAvailability.ts` 的 `plan.entitlements.map(e => e.effective_at)`；
///     界面侧 `CodingPlanStatusMeta` 读同一份，显示成「待生效 {date}」）；
///   · **没有权益**时才退回套餐级 `starts_at`（官方同一条兜底：
///     `plan.entitlements?.length ? … : [plan.starts_at]`）。`starts_at` 是领取 /
///     购买时间（官方 usage-stats 把它映射成 `beginTime`），官方卡片从不拿它当
///     生效时间展示。
///
/// `effective_at = 0` 是**立即生效**的哨兵值（官方注释：投影成 Unix Epoch，
/// 不属于排期权益），所以只收 `> 0` 的；形态是 `number | string | null`
/// （官方 TS 类型原话），一律走 `number_of` 收口。
fn plan_effective_times(plan: &Value) -> Vec<f64> {
    match plan.get("entitlements").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => items
            .iter()
            .filter_map(|item| item.get("effective_at").and_then(number_of))
            .filter(|value| value.is_finite() && *value > 0.0)
            .collect(),
        _ => plan_bound(plan, START_KEYS)
            .filter(|value| value.is_finite() && *value > 0.0)
            .into_iter()
            .collect(),
    }
}

/// 待生效时刻：**最早的、还没到的**那个生效时间（unix 秒）；没有就是 `None`。
///
/// 界面靠它写「待生效 {时间}」—— 官方同款（`resolvePendingStartPlanEffectiveTime`
/// 取未来里最早的一个）。判定基准是 `now`（上游 `server_time`，见 `normalize`；
/// 官方那边也用余额响应的服务端时间，不用本机时钟）。
fn pending_until(plan: &Value, now: f64) -> Option<f64> {
    plan_effective_times(plan)
        .into_iter()
        .filter(|value| *value > now)
        .reduce(f64::min)
}

/// 这份套餐的权益**全都还没到生效时间**（官方 availability 的口径：`every(v > now)`）。
///
/// 与 [`pending_until`] 的差别：那个回答「有没有还没生效的权益」（界面据此标
/// 「待生效」），这个回答「是不是一条已生效的权益都还没有」—— 后者才是额度桶
/// 能不能算进可用读数的判据（见 `normalize` 里 `usable` 的说明）。
fn all_pending(plan: &Value, now: f64) -> bool {
    let times = plan_effective_times(plan);
    !times.is_empty() && times.iter().all(|value| *value > now)
}

/// 套餐的某个时间字段，**统一折算成 unix 秒**。
///
/// 候选键链逐个试；上游在同一族接口上给过秒也给过毫秒（这个网关别处也见过 13 位
/// 毫秒），按量级判：大于 `1e11` 的一律当毫秒 —— unix 秒要到公元 5138 年才够
/// 这个量级，不会误伤。
fn plan_bound(plan: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = plan.get(*key).and_then(number_of).filter(|value| value.is_finite()) {
            return Some(if value > 1e11 { value / 1000.0 } else { value });
        }
    }
    None
}

/// 套餐的权益 → 前端 JSON。
///
/// 键名与领取预览的 `entitlements` 逐字相同（见 `api::zcode_claim::plan_to_json`）：
/// 「套餐明细」弹窗同时画「可领取」与「已领取」两份清单，同形状才能共用一份渲染。
fn entitlements_of(plan: &Value) -> Vec<Value> {
    plan.get("entitlements")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    // 没有展示名的权益不画（与预览同口径：`show_name` 是这条权益
                    // 唯一可读的标识，缺了就没东西可写）
                    let show_name = item
                        .get("show_name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    Some(json!({
                        "showName": show_name,
                        "unitType": item.get("unit_type").and_then(Value::as_str).map(str::trim).unwrap_or(""),
                        // `grant_units` 官方是 number | string | null（同 `total_units`），
                        // 走 `number_of` 收口；认不出的给 0
                        "grantUnits": item.get("grant_units").and_then(number_of).map(|value| value as i64).unwrap_or(0),
                        "period": item.get("period").and_then(Value::as_str).map(str::trim).unwrap_or(""),
                        // 权益的生效时刻（unix 秒；官方类型 `number | string | null`，
                        // 所以走 `number_of`）。**这才是「生效时间」的权威来源**，
                        // 套餐级的 `starts_at` 是领取时间 —— 见 [`plan_effective_times`]。
                        // `0` 是「立即生效」哨兵（原样带出，界面按已生效处理）。
                        "effectiveAt": item.get("effective_at").and_then(number_of).map(|value| value as i64),
                        // ── 已用 / 总量：给不给看上游 ────────────────────
                        // 活动套餐的额度不落在 `balances` 里（见 `query_usage` 的合并
                        // 那段），它的用量**可能**挂在这条权益上。上游给就带上，
                        // 界面据此画一条真进度条（剩余比例）；不给一律 null ——
                        // 界面按「不知道」处理，不拿 0 冒充。
                        "usedUnits": item.get("used_units").and_then(number_of),
                        "totalUnits": item.get("total_units").and_then(number_of),
                        "remainingUnits": item.get("remaining_units").and_then(number_of),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 这条读数里有没有**可用的额度桶**（billing 只在真算出桶时才 push，所以「非空」
/// 就是「读到了」）。
fn has_wallets(value: &Value) -> bool {
    value
        .get("wallets")
        .and_then(Value::as_array)
        .map(|list| !list.is_empty())
        .unwrap_or(false)
}

/// billing 读数 × 监控通道的窗口读数：只把**窗口那一段**接过去（见调用点的说明）。
///
/// 动的是三处：`wallets`（窗口列表）、`availableView`（表头读数 —— 它描述的正是
/// 那一列要显示什么，跟着窗口走）、`walletsFrom`（标记「额度窗口来自哪个通道」，
/// 界面据此在悬停里说明口径）。其余键一个不动。
fn merge_monitor_windows(billing: Value, windows: Value) -> Value {
    let mut merged = billing;
    let monitor_wallets = windows.get("wallets").cloned().unwrap_or(Value::Null);
    let monitor_view = windows.get("availableView").cloned().unwrap_or(Value::Null);
    let Some(object) = merged.as_object_mut() else {
        return merged;
    };
    if !monitor_wallets.is_null() {
        object.insert("wallets".to_string(), monitor_wallets);
    }
    if !monitor_view.is_null() {
        object.insert("availableView".to_string(), monitor_view);
    }
    object.insert(
        "walletsFrom".to_string(),
        Value::String("monitor".to_string()),
    );
    merged
}

/// 挑一个「代表这个账号」的套餐（`subscription` 那段文案读它）。
///
/// 优先级：先只在**生效中**里挑（理由见 [`plan_rank`]）；一份生效中的都没有时，
/// 退回**待生效**的 —— 刚领到、还没到生效时间的套餐比一份已经不提供权益的旧套餐
/// 更该代表这个账号（也是这次「领取成功但显示不出来」的直接来源）；两者都没有才
/// 用全量（保持旧行为的兜底，认不出状态的条目不会被整条丢掉）。
///
/// ── 与官方客户端的取舍不同（别当成抄漏）───────────────────────
/// 官方客户端那个表头固定偏好「有每日权益的套餐」（它自己的面板按每日口径展示）。
/// 本家的账号页那一列问的是**有效期 / 到期**，活动送的额度（Trust Build 那类
/// 一次性包）恰恰是有明确截止的那一份 —— 拿一个「不会到期」的每日套餐当代表，
/// 那一列就空了，用户反而看不到自己刚领的 1 亿什么时候失效。
fn pick_plan<'a>(plans: &[&'a Value], now: f64) -> Option<&'a Value> {
    let pool: Vec<&'a Value> = {
        let of = |state: PlanState| -> Vec<&'a Value> {
            plans
                .iter()
                .copied()
                .filter(|plan| plan_state(plan, now) == state)
                .collect()
        };
        let active = of(PlanState::Active);
        if !active.is_empty() {
            active
        } else {
            let pending = of(PlanState::Pending);
            if pending.is_empty() { plans.to_vec() } else { pending }
        }
    };
    let mut best: Option<&'a Value> = None;
    for plan in pool {
        match best {
            None => best = Some(plan),
            Some(current) => {
                let key = plan_rank(plan, now);
                let current_key = plan_rank(current, now);
                let take = key < current_key
                    || (key == current_key
                        && plan_id_of(plan) < plan_id_of(current));
                if take {
                    best = Some(plan);
                }
            }
        }
    }
    best
}

/// 套餐的「代表度」键：元组比较，**越小越优先**。
///
///   - 档 0：有未来到期时间 —— 第二项就是 `ends_at`，元组比较取小，
///     于是**到期越早的越优先**；
///   - 档 1：没有未来到期、有每日权益（每天续发的额度）；
///   - 档 2：其余（有到期但已过、又不带每日权益）。
///
/// `now` 由 `data.server_time` 给出（上游自己的时间），判「未来」用它是为了避免
/// 本机时钟不准。
fn plan_rank(plan: &Value, now: f64) -> (u8, f64) {
    match plan
        .get("ends_at")
        .and_then(number_of)
        .filter(|value| value.is_finite() && *value > now)
    {
        Some(ends_at) => (0, ends_at),
        None if has_daily_entitlement(plan) => (1, 0.0),
        None => (2, 0.0),
    }
}

/// 套餐 id（缺失时给空串：参与的只是**定序**，不参与展示）
fn plan_id_of(plan: &Value) -> &str {
    plan.get("plan_id").and_then(Value::as_str).unwrap_or("")
}

/// 套餐的展示名：`name` → 退回 `plan_id`（都缺给 None）。
fn plan_display_name(plan: &Value) -> Option<String> {
    plan.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            plan.get("plan_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

/// 套餐里有没有「每日」权益（`entitlements[].period == "daily"`）。
///
/// 官方客户端的表头选取口径同样优先这种套餐：每日活动的权益是每天续发的，
/// 它比一次性的活动包更能代表「这个账号现在靠什么在跑」。
fn has_daily_entitlement(plan: &Value) -> bool {
    plan.get("entitlements")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("period")
                    .and_then(Value::as_str)
                    .map(|value| value.trim().eq_ignore_ascii_case("daily"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 取一个数值字段（数字或**数字字符串**都认）。
///
/// 上游在不同接口上的形态不一致（官方 TS 接口里 `total_units` 就是
/// `number | string | null`），照直 `as_f64()` 会让一个字符串形态的读数整条消失。
/// `pub(super)`：`monitor.rs` 的窗口读数用同一条取值口径（两处各写一份迟早漂）。
pub(super) fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok().filter(|value| value.is_finite()),
        _ => None,
    }
}

/// 大数字 → 中文紧凑串（`100000000` → `1亿`，`3000000` → `300万`）。
///
/// 余额列是本表最窄的几列之一，9 位原始数字在那儿读不动。单位固定用「万 / 亿」
/// （这是中文界面的默认刻度），小数最多两位、末尾的 0 去掉。
/// `pub(super)`：`monitor.rs` 的绝对量读数用同一份格式化（两处各写一份迟早漂）。
pub(super) fn compact_number(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    let abs = value.abs();
    if abs >= 100_000_000.0 {
        format!("{}亿", trim_trailing_zeros(value / 100_000_000.0))
    } else if abs >= 10_000.0 {
        format!("{}万", trim_trailing_zeros(value / 10_000.0))
    } else if value.fract().abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

/// `1.00` → `1`、`1.03` → `1.03`（两位小数，去掉末尾的 0 与孤立的小数点）
fn trim_trailing_zeros(value: f64) -> String {
    let text = format!("{value:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}
