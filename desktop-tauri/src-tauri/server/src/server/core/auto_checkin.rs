//! 定时签到 —— 按定时条目（`core::checkin_schedules`）在各自时刻执行一次签到。
//!
//! 签到执行体与 /api/accounts/checkin 是同一段（`core::billing::checkin::run_checkin`），
//! 因此限额跳过、国际版排除、串行防风的规则完全一致，不存在两套行为。
//! 本模块只负责**什么时候跑、跑哪些**：总开关 + 每条条目自己的时刻与范围，
//! 条目的存储与 CRUD 在 `core::checkin_schedules`。
//!
//! ── 触发方式：轮询 + 当天去重（不是单个定时器）────────────────
//! Node 版用 `setInterval(tick, 30s)`：单个 setTimeout 在系统休眠、锁屏、时钟被改
//! 之后会漂移甚至整段错过，而每 30 秒比一次「当前是否已过今天的触发点」，
//! 唤醒后自然补上。Rust 侧用等价的 `sleep(30s)` 循环，每个 tick 都从**墙上时钟**
//! （北京时间，见 `core::beijing`）重新判定，因此与 Node 一样具备自愈能力：
//! libuv 与 tokio 的定时器都基于单调时钟，机器休眠期间都不推进，
//! 醒来的第一次 tick 会把错过的时点补上 —— 两者在这点上是同一套语义。
//! 多条目之后每个 tick **逐条**判定（各自到点 + 各自的当天去重），同一时刻
//! 到点的多条靠 fire 的全局「同时只跑一次」串行：抢不到的那条下个 tick 再试。
//!
//! ── 时区口径：固定 UTC+8，不跟机器时区（issue #138）────────────
//! 「今天是否已触发」与触发时刻都按**北京时间**算（`core::beijing`）：
//! 签到的自然日是上游的自然日（北京时间零点重置），网关跑在 NAS / Docker /
//! 海外 VPS 上时跟机器时区走会让定时整体漂移 —— 美西机器上 00:01 的签到
//! 实际发生在北京时间 15:01，当天额度可能已被更早的调用用掉，面板的
//! 「今日已签到」也按错误的日期显示。移植自 Node 版（作者在国内，`Local`
//! 恰好等于 UTC+8）时没有显式钉住这个前提，这里补上。
//!
//! ── 补签（有意的行为，别「顺手优化」）────────────────────────
//! Node 版 `start()` 的条件是**「今天还没签过」**，并不判「时间点是否已过」：
//! 开着自动签到却在 00:01 时没开机、白天才启动 → 立刻补一次。
//! 签到的每日额度按自然日重置，用户开着开关却整天没签到才是反直觉的结果；
//! 上游签到接口幂等，重复调用只返回「已领取」。实测（2026-09-17）在 18:44
//! 把 enabled 设为 true、time 设为 23:00（未来时点）时，Node 版立即执行了
//! 一次 reason='启动补签' 的签到 —— 所以这里的条件同样只看日期键。
//! 多条目之后按条目各补各的：今天没跑过的**启用**条目每条补一次。
//!
//! ── 配置读写 ────────────────────────────────────────────────
//! 状态存在 config.json 的 `autoCheckin` 字段：`enabled` 是总开关，
//! 条目数组在 `schedules`（读写见 `core::checkin_schedules`）。Node 每次
//! getState/tick 都重读 config.json；Rust 侧读 `config::current()` 的内存快照
//! （config.rs 已经全量保留未知字段，写回也走它），于是「别人写进 config.json
//! 的字段不会被吃掉」这条契约继续成立。差异：Node 会看到用户手工改盘上的
//! config.json 后的新值，Rust 的内存快照只在启动与写入时刷新 —— 手工改文件
//! 需要重启（与 config.rs 的既有设计一致）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, TimeZone};
use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::beijing;
use crate::server::core::billing::checkin;
use crate::server::core::billing::BillingService;
use crate::server::core::checkin_schedules::{self, Schedule};
use crate::server::logging;

/// 轮询间隔：30 秒足够精确到分钟，又不会让计时器显得忙（Node 版 TICK_MS）
pub const TICK_MS: u64 = 30_000;

/// 默认触发时刻：零点一分（Node 版 DEFAULT_TIME）
pub const DEFAULT_TIME: &str = "00:01";

/// 缺省触发时刻（`DEFAULT_TIME` 的 owned 形态，读侧兜底用）
pub fn default_time() -> String {
    DEFAULT_TIME.to_string()
}

/// 可勾选的签到提供商（界面上的复选框）。默认全选。
///
///   - **WorkBuddy 国内版**：腾讯的每日签到接口；
///   - **WorkBuddy 国际版**：每日活跃任务（活动探测 + 条件领取 + 免费模型保活，
///     `billing::activity` 的 `workbuddy_daily_activity`）。国际版没有国内版的
///     普通签到接口，但上游客户端把这条活跃链接在同一个每日调度上 —— 保活的
///     成功不落 `checkinAt`、单独统计在 `active`；免费模型链可在签到中心自定义
///     （`billing::keepalive`）。
///   - **小浣熊**：「桌面登录积分」链路（`providers::raccoon` 的每日积分发放）；
///     首次登录奖励（电脑端 / 手机端各一条端点）是一次性新手福利，不在这条
///     每日链路里（见 `providers::raccoon::onboarding`，挂新手任务分组）；
///   - **AutoClaw 国内版 / 国际版**：通用任务接口的 `daily_signin` 任务
///     （`providers::autoclaw::checkin`）。两个地区**都支持** —— 任务接口在
///     两地是同一套路径、同一套任务 id，只是站点不同（已实测），因此两家
///     都列进来；地区由 `billing::checkin` 从账号的 provider 反查。
///   - **Qoder 中国版**：活动（campaign）领取链路（`providers::qoder::checkin`）。
///     Free 套餐账号也可能没有被下发活动（实测如此），那种情况实现返回一条
///     中性结果（「当前没有可领取的签到活动」），不算失败。
///   - **Qoder 国际版**（2026-10，issue #140）：同一条 campaign 链路，但上游
///     **只对带设备风控头的请求**下发「每天领 100 Credits」—— 风控身份由本机
///     Qoder 客户端 / qodercli 的 UMID 组件生成（`providers::qoder::risk`），
///     Linux/Docker 可一键安装组件。缺组件时得到的是明确的说明而不是报错
///     （也不落当日台账，第二天组件就位后照常领）。
///   - **Trae**：SOLO 的 `checkin_credits` 领取（`providers::trae::checkin`）。
///     加它不是因为"别家有"，而是这条链**有钱**：SOLO 转积分制后，模型调用花的
///     正是签到钱包那份积分，不签就是每天白丢一笔额度。它的风险不在"该不该签"
///     而在"怎么签"：设备号每轮全新（复用是风控可疑项）、请求体恒空对象、
///     claim 的 `code:0` 要用同设备号回查确认（幂等假成功）。三条都在那个
///     模块里钉着。⚠️ 本清单同时是**缺省全选**的来源：老配置的 `autoCheckin`
///     里没有 `providers` 字段时，读出来就是"包括 trae"——上线后第二天零点
///     就会多发一轮签到请求。不想要就在签到中心「自动签到」里取消勾选。
///   - **Loomy**：**没有独立签到接口** —— 它的「每日赠送积分」由每日首次登录
///     触发刷新（`POST /api/v1/points/first-login`，见 `providers::loomy::checkin`）。
///     自动签到对它就是每天替账号打一次这个接口，长期不登录的账号也能把
///     赠送积分续上。
///   - **KukuAI**：「免费领积分」活动的每日任务（每日登录 / 完成一次对话，
///     `freepoint/taskComplete`，见 `providers::kuku::checkin`）。接口幂等，
///     重复领取只返回 reward_point=0；业务会话由换发的 genflowpro STOKEN
///     保障（`kuku::engine`）。
///   - **OfficeAce 果办**：**一次调用**领当天全部奖励（每日签到 + 一次性新人礼
///     是同一条上游动作，`POST /v1/subscription/bonus/claim`，见
///     `providers::officeace::checkin`）。幂等由上游保证：一天一个活动只能领一次，
///     重复领不报错、返回同一份列表 —— 所以它也没有「已领过」分支。
///   - **CodeArts**：当日**ops 福利**（`/v1/ops` 的 delivery → claim → confirm，
///     见 `providers::codearts::welfare`）。它与别家的形态差别最大（三个接口、
///     两套状态、一个幂等键），所以 2026-10 之前一直**不进**这条链、只在签到
///     中心留一颗手动「领福利」按钮；按用户要求纳入本链后，它就是那一家账号的
///     「签到」：列进每日签到清单、被批量与定时执行，收尾（落 `checkinAt`、
///     计 succeeded、记签到记录台账）与别家完全一致。
///     ⚠️ 两点与别家不同，改这块前先读 `welfare` 的模块头：
///     ① 它是**写请求有限流闸**的（每天 ≤6 次写、间隔 ≥10 分钟）—— 自动路径
///     受闸（`checkin_for` 传 manual=false），用户在面板上点不受闸；
///     ② 「暂无可领取活动 / 等待重试」是**中性未领取**（不落当日台账，定时链
///     稍后重试），不是失败 —— `claim_pending` 认这两个文案。
///
/// 这是「有签到或每日活跃任务」的清单，不是「有积分概念」的清单：CatPaw 有积分
/// 查询但没有签到，因此不在此列 —— 它的账号在批量签到里被算作 `skipped`。
/// 加一家之前先确认它的签到链路真的存在（一个点了必然报错的复选框比没有更糟）。
pub const CHECKIN_PROVIDERS: [&str; 12] = [
    "workbuddy",
    "workbuddy-intl",
    "raccoon",
    "autoclaw",
    "autoclaw-intl",
    "qoder",
    "qoder-intl",
    "trae",
    "loomy",
    "kuku",
    "officeace",
    // 追加在**末尾**：这个数组的顺序就是界面复选框与签到中心分组行的顺序，
    // 插在中间会让所有存量用户的清单顺序变一下（没有功能影响，但没必要动）。
    "codearts",
];

/// 缺省的签到提供商集合（全选）
pub fn default_providers() -> Vec<String> {
    CHECKIN_PROVIDERS.iter().map(|id| id.to_string()).collect()
}

/// 签到范围清单的**增量版本**：每给 [`CHECKIN_PROVIDERS`] 追加新家就 +1，
/// 并把新家连同它的版本登记进 [`PROVIDERS_ADDED_IN`]。
///
/// ── 为什么需要一个版本号（而不是读的时候无条件补齐）───────────
/// 「签到范围」是**每条定时条目自己勾的清单**（`checkin_schedules`）。新加进来的
/// 家不可能出现在存量条目的清单里，而用户的期望是**默认就在范围内** ——
/// 2026-10 用户明确要求 OfficeAce 果办与 CodeArts 默认开启。可是「每次读都补齐」
/// 会让这一家的勾选框再也取消不掉（读完就补回来，界面上表现为「取消勾选没反应」）。
/// 所以补齐只做一次，靠配置里的版本号记账；版本一旦写上去，用户之后再取消勾选
/// 就真的生效了。执行点见 `checkin_schedules::extend_providers_once`。
pub const PROVIDERS_VERSION: i64 = 2;

/// 「哪一版加入的家」——条目清单的版本低于它 = 这家还没被提供过 ⇒ 补进去。
/// 版本 1 = 多条目改造上线时的形态（9 家；`qoder-intl` 由 `normalize_providers`
/// 的存量兼容规则跟着 `qoder` 带上，不在这里重复登记）。
const PROVIDERS_ADDED_IN: [(&str, i64); 2] = [("officeace", 2), ("codearts", 2)];

/// 版本 `version` 之后才加入清单的家（按注册顺序）。空数组 = 这一版没有新家，
/// 或配置比代码还新（装过更新的版本又退回来）——两种情况都只记账、不动清单。
pub fn providers_added_after(version: i64) -> Vec<String> {
    CHECKIN_PROVIDERS
        .iter()
        .filter(|id| {
            PROVIDERS_ADDED_IN
                .iter()
                .any(|(added, at)| added == *id && *at > version)
        })
        .map(|id| id.to_string())
        .collect()
}

/// 提供商的展示名（从注册表查，查不到就原样回显 id）。
///
/// 这是**签到语境**的展示名（签到中心的分组、自动签到的提供商勾选共用这一份）。
/// WorkBuddy 两个地区在注册表里就叫「WorkBuddy 国内版 / 国际版」（拆家后的
/// 既定口径），签到语境沿用注册名即可 —— 国际版执行的是活跃任务而不是普通
/// 签到，这件事由签到中心 `PROVIDER_DESC` 的链路说明与它的分组描述讲清楚，
/// 不在标签里堆字。Qoder 拆家（2026-10）后注册名就是「Qoder 中国版 /
/// 国际版」，不再需要这里的覆盖。其余几家不需要后缀：小浣熊没有版本区分，
/// AutoClaw 两地的签到链路都存在且同形 —— 展示名已带「国内版 / 国际版」。
pub fn provider_label(id: &str) -> &str {
    crate::server::core::providers::PROVIDERS
        .iter()
        .find(|meta| meta.id == id)
        .map(|meta| meta.label)
        .unwrap_or(id)
}

/// 归一化配置里的提供商清单：只认 CHECKIN_PROVIDERS 里的 id（去重、保持顺序），
/// 缺失 / 空数组 / 全是非法值都回落到「全选」—— 旧配置文件里没有这个字段，
/// 读出来必须是合法的默认行为。
///
/// 存量兼容：Qoder 拆家（2026-10）前的老清单只有 9 项、没有 `qoder-intl`，
/// 而显式清单不会被「缺啥补啥」—— 不补的话，拆家前勾了 Qoder 的用户拆家后
/// 自动签到会静默漏掉国际版账号。这里按账号迁移的同一口径补齐：勾了
/// `qoder` 的，紧跟其后补上 `qoder-intl`（要签中国版就要签国际版，拆家只是
/// 同一套能力的地区分身）；两者都没勾的（明确不签 Qoder）不动。
pub fn normalize_providers(value: Option<&Value>) -> Vec<String> {
    let Some(list) = value.and_then(Value::as_array) else {
        return default_providers();
    };
    let picked: Vec<String> = CHECKIN_PROVIDERS
        .iter()
        .filter(|id| {
            list.iter()
                .any(|item| item.as_str() == Some(*id))
        })
        .map(|id| id.to_string())
        .collect();
    let mut picked = if picked.is_empty() {
        default_providers()
    } else {
        picked
    };
    if let Some(pos) = picked.iter().position(|id| id == "qoder") {
        if !picked.contains(&"qoder-intl".to_string()) {
            picked.insert(pos + 1, "qoder-intl".to_string());
        }
    }
    picked
}

/// config.json 里承载定时签到状态的键。
/// `pub(crate)`：多条目配置（`core::checkin_schedules`）在同一个键下读写。
pub(crate) const CONFIG_KEY: &str = "autoCheckin";

/// 配置类错误（对应 Node 版 AutoCheckinConfigError）。
///
/// **注意 statusCode 是 400 而不是 409**：Node 版 `AutoCheckinConfigError`
/// 构造里写死 `statusCode = 400`，于是 server.mjs 的 `throw new
/// AutoCheckinConfigError('签到正在执行中，请稍候')`（run 路由）经 errorPayload
/// 得到的是 **400 + OpenAI 风格 body**，而不是 409。实测（2026-09-17）用非法
/// time 打 POST /api/auto-checkin 拿到的正是
/// `400 {"error":{"message":"时间超出范围（00:00 - 23:59）","type":"invalid_request_error"}}`。
/// 契约以 Node 版为准，这里保持一致（`CHECKIN_BUSY_STATUS` 单独抽出，
/// 万一确认要用 409，改一处即可）。
#[derive(Clone, Debug)]
pub struct AutoCheckinConfigError {
    pub message: String,
    pub status_code: i32,
}

/// run 路由「正在执行中」的状态码：与 Node 的 AutoCheckinConfigError 一致为 400
pub const CHECKIN_BUSY_STATUS: i32 = 400;

impl AutoCheckinConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: 400 }
    }
}

impl std::fmt::Display for AutoCheckinConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

// ─── 时间工具（对照 Node 的同名函数）────────────────────────

/// JS `String(value)` 的近似：只用于 normalizeTime 的输入转换。
/// `value ?? ''` 在 Node 里把 null/undefined 变成空串，其余走 String()：
/// 数字/布尔取字面量，对象/数组在 JS 里是 "[object Object]" / 逗号拼接 ——
/// 这些形态只可能来自手改的请求体或 config.json，最终都会被正则拒掉。
fn js_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// 校验并归一化 HH:MM，返回补零后的文本（对应 Node 版 normalizeTime）。
///
/// 正则 `^(\d{1,2}):(\d{1,2})$` 手写等价实现：冒号两侧各 1-2 位 ASCII 数字，
/// 且**整串**必须如此（`\d` 在 JS 里不含全角数字、不允许前后空白 ——
/// 前后空白在 Node 里由 `String(value).trim()` 提前去掉）。
pub fn normalize_time(value: Option<&Value>) -> Result<String, AutoCheckinConfigError> {
    let text = js_text(value);
    let text = text.trim();
    let parsed = text
        .split_once(':')
        .filter(|(head, tail)| {
            let digits = |part: &str| {
                !part.is_empty()
                    && part.len() <= 2
                    && part.bytes().all(|byte| byte.is_ascii_digit())
            };
            digits(head) && digits(tail) && !tail.contains(':')
        })
        .and_then(|(head, tail)| Some((head.parse::<u32>().ok()?, tail.parse::<u32>().ok()?)));
    let Some((hour, minute)) = parsed else {
        return Err(AutoCheckinConfigError::new("时间格式应为 HH:MM（例如 00:01）"));
    };
    if hour > 23 || minute > 59 {
        return Err(AutoCheckinConfigError::new("时间超出范围（00:00 - 23:59）"));
    }
    Ok(format!("{hour:02}:{minute:02}"))
}

/// 北京日期键 YYYY-MM-DD：用于「今天是否已触发」的判定。
///
/// 为什么不能用 UTC 算：UTC+8 的凌晨 00:01 在 UTC 下还是前一天，会让
/// 「今天签过没有」的判重错一整天（移植来源 Node 版用本地时区，作者在
/// 国内恰好等于 UTC+8 —— 这里显式钉住北京时间，见 `core::beijing`）。
pub fn date_key(now: DateTime<FixedOffset>) -> String {
    beijing::date_key(now)
}

/// 北京时间的今天（`YYYY-MM-DD`）——「今天是否已触发」判定的统一入口。
pub fn today_key() -> String {
    beijing::today_key()
}

/// 今天的触发时刻（北京时间）；已过则返回 Some，未到返回 None。
/// 对应 Node 版 dueNow：非法的 time 文本在这里同样表现为 None（Node 是抛异常，
/// 但调用方一律 catch 后 return，结果一致）。
fn due_now(time_text: &str, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    let target = today_at(time_text, now)?;
    if now >= target {
        Some(target)
    } else {
        None
    }
}

/// 「下次执行」的绝对时刻（毫秒时间戳），给界面展示用。
///
/// 对应 Node 版 msUntilNext 的语义（目标时刻已过顺延到明天）外加**已触发**
/// 分支：启动补签会在条目时刻之前就把今天落账（补签只看日期不看时点），
/// 这时「下次执行」必须是明天 —— 只按时刻算会得到今天的未来时刻，
/// 界面显示一个其实不会跑的点。time 非法 → None（调度起不来的情形
/// 不该编一个时间出来）。
fn next_run_at_ms(time_text: &str, fired_today: bool, now: DateTime<FixedOffset>) -> Option<i64> {
    let mut target = today_at(time_text, now)?;
    if fired_today || target <= now {
        target += chrono::Duration::days(1);
    }
    Some(target.timestamp_millis())
}

/// 今天的 HH:MM 时刻（北京时间）；time 文本非法时 None。
///
/// Node 用 `target.setHours(hour, minute, 0, 0)`（就地改到当天）；
/// chrono 没有 set_* 的就地 API，用 `with_ymd_and_hms` 重建。
/// 固定偏移没有夏令时问题（构造恒为 `Single`），因此不需要 Node 那种
/// 「DST 当天该时刻不存在」的兜底 —— `single()` 取不到就当 None，
/// 也就是这台机器上的时钟环境异常到连北京时间的固定偏移都构造不出来。
fn today_at(time_text: &str, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    let normalized = normalize_time(Some(&Value::String(time_text.to_string()))).ok()?;
    let (hour, minute) = normalized.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    let naive = now.date_naive().and_hms_opt(hour, minute, 0)?;
    beijing::offset()
        .from_local_datetime(&naive)
        .single()
}

// ─── 状态读写 ───────────────────────────────────────────────

/// `autoCheckin` 字段的原始对象（非对象/缺失都给空对象）。
/// `pub(crate)`：多条目配置（`core::checkin_schedules`）在同一个键下读写，
/// 合并写回的逻辑只有这一份。
pub(crate) fn raw_object() -> Map<String, Value> {
    match config::current().raw().get(CONFIG_KEY) {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    }
}

/// 总开关（`autoCheckin.enabled`）：关了所有条目都不跑。
/// 条目自己的开关在 `checkin_schedules::Schedule::enabled`，两层都开着才跑。
pub fn master_enabled() -> bool {
    raw_object().get("enabled").and_then(Value::as_bool) == Some(true)
}

/// 合并写回 `autoCheckin`（对应 Node 版 writeState 的 `{...current, ...patch}`）。
/// 走 config.rs 的 `update_raw_field`，config.json 里的其它字段原样保留。
/// `pub(crate)`：多条目配置在同一键下写回，合并逻辑只有这一份。
pub(crate) fn write_state(patch: Value) {
    let mut merged = raw_object();
    if let Value::Object(patch) = patch {
        for (key, value) in patch {
            merged.insert(key, value);
        }
    }
    config::update_raw_field(CONFIG_KEY, Value::Object(merged));
}

// ─── 服务句柄 ───────────────────────────────────────────────

/// 句柄内部状态：下载/签到这类长任务只在这里留「是否在跑」与调度任务句柄。
struct Inner {
    /// 是否已有一次签到在执行（对应 Node 的闭包变量 running）
    running: bool,
    /// 调度循环任务句柄（对应 Node 的 timer）
    task: Option<tokio::task::JoinHandle<()>>,
    /// 停机标志：stop() 置位后循环退出（幂等；再次 start 会重置）
    stopped: Arc<AtomicBool>,
}

/// 定时签到服务句柄：内部一把锁 + Clone，与 update / models 同构。
///
/// 锁只用来读写 running 与任务句柄这类微秒级操作；签到执行全程在锁外
/// （硬约束：持锁不做网络请求、不 await）。`running` 用 CAS 抢锁的方式表达
/// 「同时只跑一次」，与 Node 单线程下 `if (running) return` 的语义一致。
#[derive(Clone)]
pub struct AutoCheckin {
    store: AccountStore,
    billing: BillingService,
    inner: Arc<Mutex<Inner>>,
}

/// 抢到「执行中」标记的守卫：无论正常返回还是中途被取消（任务 abort），
/// Drop 时都会把 running 复位 —— 对应 Node 的 `finally { running = false }`，
/// 同时避免 abort 把 running 永久卡在 true。
struct RunningGuard {
    inner: Arc<Mutex<Inner>>,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        match self.inner.lock() {
            Ok(mut guard) => guard.running = false,
            Err(poisoned) => poisoned.into_inner().running = false,
        }
    }
}

impl AutoCheckin {
    pub fn new(store: AccountStore, billing: BillingService) -> Self {
        Self {
            store,
            billing,
            inner: Arc::new(Mutex::new(Inner {
                running: false,
                task: None,
                stopped: Arc::new(AtomicBool::new(false)),
            })),
        }
    }

    /// 取锁；锁中毒（持锁 panic）不致命，接管内部数据继续用
    /// （与账号存储、脱敏器同一策略）
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 是否正在执行签到（getState().running）
    fn is_running(&self) -> bool {
        self.lock().running
    }

    // ─── 对外状态 ───────────────────────────────────────────

    /// 对外暴露的状态（对应 Node 版 getState 的多条目版）。
    ///
    /// 形状分两层：
    ///   - `schedules`：条目视图（每条含自己的 enabled / time / providers /
    ///     lastFiredToday / nextRunAt / lastResult），签到中心的「定时签到」页直接渲染；
    ///   - 顶层 `time` / `providers` / `lastFiredDate` / `lastFiredToday` /
    ///     `lastResult` / `nextRunAt`：**兼容视图**——顶层 nextRunAt 是所有启用
    ///     条目里最近的一次（顶栏徽标用），其余字段取默认条目（合成的或显式的
    ///     id="default"；删光了就给缺省值），旧读方（启动日志）不用改。
    pub fn state(&self) -> Value {
        let master = master_enabled();
        let now = beijing::now();
        let today = date_key(now);
        let schedules = checkin_schedules::list();
        let entries: Vec<Value> = schedules
            .iter()
            .map(|schedule| {
                let fired_today = schedule.last_fired_date.as_deref() == Some(today.as_str());
                let next_run_at = if master && schedule.enabled {
                    next_run_at_ms(&schedule.time, fired_today, now)
                } else {
                    None
                };
                json!({
                    "id": schedule.id,
                    "label": schedule.label,
                    // 展示名：label 为空时退到 time，界面不必各自兜底
                    "displayLabel": checkin_schedules::display_label(schedule),
                    "enabled": schedule.enabled,
                    "time": schedule.time,
                    "providers": schedule.providers,
                    "lastFiredDate": schedule.last_fired_date,
                    "lastFiredToday": fired_today,
                    "nextRunAt": next_run_at,
                    "lastResult": schedule.last_result,
                })
            })
            .collect();
        // 聚合「下次执行」：启用条目里最近的一次（总开关关着就没了）
        let next_run_at = if master {
            entries
                .iter()
                .filter_map(|entry| entry.get("nextRunAt").and_then(Value::as_i64))
                .min()
        } else {
            None
        };
        // 兼容视图的「默认条目」：显式 default → 第一条 → （删光了）缺省
        let fallback = Schedule {
            id: checkin_schedules::DEFAULT_SCHEDULE_ID.to_string(),
            label: String::new(),
            enabled: false,
            time: default_time(),
            providers: default_providers(),
            last_fired_date: None,
            last_result: None,
        };
        let default_entry = schedules
            .iter()
            .find(|schedule| schedule.id == checkin_schedules::DEFAULT_SCHEDULE_ID)
            .or_else(|| schedules.first())
            .unwrap_or(&fallback);
        json!({
            "enabled": master,
            // 兼容字段（默认条目的投影；语义与改造前的顶层字段一致）
            "time": default_entry.time,
            "providers": default_entry.providers,
            "lastFiredDate": default_entry.last_fired_date,
            "lastFiredToday":
                default_entry.last_fired_date.as_deref() == Some(today.as_str()),
            "lastResult": default_entry.last_result,
            "nextRunAt": next_run_at,
            // 界面的复选框清单（签到范围），顺序即 CHECKIN_PROVIDERS 注册顺序
            "providerOptions": CHECKIN_PROVIDERS.iter().map(|id| {
                json!({ "id": id, "label": provider_label(id) })
            }).collect::<Vec<_>>(),
            "schedules": entries,
            "running": self.is_running(),
        })
    }

    // ─── 执行 ───────────────────────────────────────────────

    /// 执行一次签到并记录结果；失败只记日志，不影响下一次调度。
    ///
    /// 返回 None 的两种情形（Node 同）：① 已有一次在跑（本次跳过）；
    /// ② 执行抛错。路由层据此把 ① 翻成「签到正在执行中，请稍候」。
    ///
    /// 手动全量（`run_now`，无条目归属）走这里：范围是**全部提供商**——多条目
    /// 之后批量动作不再有全局「勾选范围」可借，那条范围概念进了每条定时条目。
    pub async fn fire(&self, reason: &str) -> Option<Value> {
        self.fire_for(reason, None, "manual").await
    }

    /// 执行一次签到，可归属到某条定时条目。
    ///
    /// `schedule_id` 给 Some：范围用**那条条目自己勾选的提供商**，去重标记落
    /// 条目级（`checkin_schedules::mark_fired` / `mark_result`）；给 None
    /// （手动全量）：范围是全部提供商，只落配置级标记。
    /// `source` 是签到记录台账的触发来源（schedule / manual / startup，
    /// 见 `core::checkin_records` 的模块头）。
    pub async fn fire_for(
        &self,
        reason: &str,
        schedule_id: Option<&str>,
        source: &str,
    ) -> Option<Value> {
        // 先抢「执行中」标记；抢不到说明上一次还在跑
        let guard = {
            let mut inner = self.lock();
            if inner.running {
                drop(inner);
                logging::verbose("[Checkin]", "上一次签到仍在执行，本次跳过");
                return None;
            }
            inner.running = true;
            RunningGuard { inner: self.inner.clone() }
        };

        let today = today_key();
        // 先落日期再执行：即便签到中途进程被杀，也不会在重启后反复补签
        write_state(json!({ "lastFiredDate": today }));
        if let Some(id) = schedule_id {
            checkin_schedules::mark_fired(id, Some(&today));
        }
        logging::log("[Checkin]", &format!("⏰ 定时签到开始（{reason}）"));

        // 范围：条目轮次用那条自己勾的提供商；手动全量（无条目）= 全部提供商。
        // 条目可能在执行前被删（罕见）：拿不到就按全量兜，不空跑。
        let providers = match schedule_id {
            Some(id) => checkin_schedules::list()
                .into_iter()
                .find(|schedule| schedule.id == id)
                .map(|schedule| schedule.providers)
                .unwrap_or_else(default_providers),
            None => default_providers(),
        };
        let outcome = match checkin::run_checkin(
            &self.store,
            &self.billing,
            providers.as_slice(),
            None,
            reason,
            source,
        )
        .await
        {
            Ok(result) => {
                let summary = self.record_success(&result, &today, reason);
                if let Some(id) = schedule_id {
                    checkin_schedules::mark_result(id, Some(summary.clone()));
                }
                Some(summary)
            }
            Err(error) => {
                // 失败也写 lastResult（failed 里放错误文案），与 Node 的 catch 分支一致
                let failure = json!({
                    "at": logging::now_ms(),
                    "date": today,
                    "reason": reason,
                    "succeeded": 0,
                    "active": 0,
                    "total": 0,
                    "skipped": 0,
                    "failed": [error.message.clone()],
                    "failedCount": 1,
                });
                write_state(json!({ "lastResult": failure.clone() }));
                if let Some(id) = schedule_id {
                    checkin_schedules::mark_result(id, Some(failure));
                }
                logging::log("[Checkin]", &format!("❌ 定时签到失败: {}", error.message));
                None
            }
        };
        drop(guard);
        outcome
    }

    /// 本轮是否有「Qoder 国际版账号没领到且不算已签」的行 —— 有则今天保持
    /// 未落账状态，等窗口开了重试（见 tick 的说明）。
    fn qoder_intl_pending(result: &Value) -> bool {
        result
            .get("results")
            .and_then(Value::as_array)
            .map(|items| {
                items.iter().any(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id.starts_with("qoder-global-"))
                        && crate::server::core::billing::checkin::claim_pending(
                            &item.get("claim").cloned().unwrap_or(Value::Null),
                        )
                })
            })
            .unwrap_or(false)
    }

    /// 成功分支：汇总 + 记录 lastResult + 日志（对应 Node fire 里的 try 主体）
    fn record_success(&self, result: &Value, today: &str, reason: &str) -> Value {
        let number = |key: &str| {
            result
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let succeeded = number("succeeded");
        let active = number("active");
        let total = number("total");
        let skipped = number("skipped");
        // 失败明细：`名字（错误）`，名字缺失时退到 id，再退到「未知账号」
        let failures: Vec<String> = result
            .get("results")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let error = item.get("error").and_then(Value::as_str)?;
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                            .or_else(|| item.get("id").and_then(Value::as_str))
                            .unwrap_or("未知账号");
                        Some(format!("{name}（{error}）"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let summary = json!({
            "at": logging::now_ms(),
            "date": today,
            "reason": reason,
            "succeeded": succeeded,
            "active": active,
            "total": total,
            "skipped": skipped,
            // 只留前 5 条：面板展示用，避免 config.json 被长列表撑大（Node 同）
            "failed": failures.iter().take(5).cloned().collect::<Vec<_>>(),
            "failedCount": failures.len(),
        });
        write_state(json!({ "lastResult": summary }));
        logging::log(
            "[Checkin]",
            &format!(
                "定时签到完成: {succeeded}/{total} 个账号成功领取{}{}{}",
                if skipped > 0 { format!("，跳过 {skipped} 个") } else { String::new() },
                if failures.is_empty() {
                    String::new()
                } else {
                    format!("，失败 {} 个", failures.len())
                },
                if active > 0 {
                    format!("，活跃保活 {active} 个")
                } else {
                    String::new()
                },
            ),
        );
        // qoderIntlPending 不进 lastResult（那是给界面看的快照），单独放在
        // fire 的返回值上供 tick 决定今天要不要保持未落账
        let mut summary = summary;
        if let Some(object) = summary.as_object_mut() {
            object.insert(
                "qoderIntlPending".to_string(),
                Value::Bool(Self::qoder_intl_pending(result)),
            );
        }
        summary
    }

    // ─── 调度 ───────────────────────────────────────────────

    /// 轮询回调：每条启用的条目各自「到点 + 今天没跑过」就执行（对应 Node 版
    /// tick 的多条目版）。
    ///
    /// ── Qoder 国际版的窗口补领（2026-10，issue #140）────────────
    /// 国际版「每日 100 Credits」的官方窗口**每天 10:00（UTC+8）开启**，而
    /// 缺省触发时刻是 00:01 —— 那时窗口还没开，活动列表里没有积分活动，
    /// 领取轮次对它只能得到「无活动」。若就此落条目的 `lastFiredDate` 整天
    /// 不再重试，国际版账号等于永远领不到。所以条目轮次**照常在配置时刻跑**，
    /// 但当轮次里存在「Qoder 国际版无活动未落账」的行时，**这一条**今天暂不落
    /// 到点标记：之后每个 tick 会重新进来，直到 10:00 窗口开启领到（或活动行
    /// 出现 CLAIMED）才落账 —— 与 10router「no-activity 刻意不记忆、下个
    /// tick 再看」同一语义。窗口判定按条目独立：没勾 qoder-intl 的条目
    /// 不会有这种行，到点即落账。
    pub async fn tick(&self) {
        if !master_enabled() {
            return;
        }
        let now = beijing::now();
        let today = date_key(now);
        for schedule in checkin_schedules::list() {
            // 条目自己的开关（总开关已在上面拦掉）
            if !schedule.enabled {
                continue;
            }
            // 未到点直接看下一条（time 在读取时已归一，这里不会解析失败）
            if due_now(&schedule.time, now).is_none() {
                continue;
            }
            // 条目级当天去重
            if schedule.last_fired_date.as_deref() == Some(today.as_str()) {
                continue;
            }
            let reason =
                format!("到点触发（{}）", checkin_schedules::display_label(&schedule));
            match self.fire_for(&reason, Some(&schedule.id), "schedule").await {
                Some(result) => {
                    // 领取轮次完成，但里面有「Qoder 国际版无活动」的行 → 这一条
                    // 不落账，今天窗口开了之后由后续 tick 重试
                    if result.get("qoderIntlPending").and_then(Value::as_bool) == Some(true) {
                        checkin_schedules::mark_fired(&schedule.id, None);
                        logging::log(
                            "[Checkin]",
                            "⏳ Qoder 国际版的活动窗口未开（官方每日 10:00 UTC+8），今天会自动重试到领到为止",
                        );
                    }
                }
                // None = 已有轮次在跑或执行出错：照旧落账（fire 内部已写），行为不变
                None => {}
            }
        }
    }

    /// 起调度循环（对应 Node 版 schedule 的 `if (timer) return`）：
    /// 已有存活任务就不重复起，避免 configure 每次保存都叠一个循环。
    fn schedule(&self) {
        {
            let mut inner = self.lock();
            if let Some(task) = &inner.task {
                if !task.is_finished() {
                    return;
                }
            }
            inner.stopped = Arc::new(AtomicBool::new(false));
            let stopped = inner.stopped.clone();
            let service = self.clone();
            // 用 crate::spawn_task：本函数可能从 setup 钩子（主线程）或 axum handler
            // 调用，`crate::spawn_task` 兜底进全局运行时，
            // 不会像裸 `tokio::spawn` 那样在非运行时上下文 panic（release 是 panic=abort）
            inner.task = Some(crate::spawn_task(async move {
                loop {
                    // 先睡再 tick：与 setInterval 的「间隔后首次触发」一致，
                    // 也避开 start() 里那次补签刚发起的窗口
                    tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    service.tick().await;
                }
            }));
        }
    }

    /// 启动调度（对应 Node 版 start 的多条目版）。
    ///
    /// 开启状态下立即检查一次：**只看「今天签过没有」，不看时点是否已过**
    /// （理由见模块头部「补签」）。多条目之后按条目各补各的：今天没跑过的
    /// **启用**条目每条补一次，串在一个任务里跑（fire 的全局防重本来就只允许
    /// 一轮在跑，串行省得互相抢空）。
    pub fn start(&self) {
        if !master_enabled() {
            return;
        }
        self.schedule();
        let today = today_key();
        let pending: Vec<Schedule> = checkin_schedules::list()
            .into_iter()
            .filter(|schedule| {
                schedule.enabled
                    && schedule.last_fired_date.as_deref() != Some(today.as_str())
            })
            .collect();
        if pending.is_empty() {
            logging::verbose("[Checkin]", "定时签到条目均已执行，无需补签");
            return;
        }
        let service = self.clone();
        crate::spawn_task(async move {
            for schedule in pending {
                let reason = format!(
                    "启动补签（{}）",
                    checkin_schedules::display_label(&schedule)
                );
                service.fire_for(&reason, Some(&schedule.id), "startup").await;
            }
        });
    }

    /// 停调度（进程退出时用）。幂等：重复调用不会报错。
    ///
    /// 与 Node 的 `clearInterval` 语义对齐：只结束**循环**，不打断正在跑的
    /// 那一次签到（abort 只会让任务在下一个 await 点结束，config.json 的写入是
    /// 同步的，已落盘的 lastFiredDate 不会被回滚）。
    pub fn stop(&self) {
        let task = {
            let mut inner = self.lock();
            if inner.task.is_none() {
                return;
            }
            inner.stopped.store(true, Ordering::SeqCst);
            inner.task.take()
        };
        if let Some(task) = task {
            task.abort();
        }
    }

    // ─── 设置 ───────────────────────────────────────────────

    /// 保存设置（对应 Node 版 configure 的兼容入口）。
    ///
    /// 多条目之后字段落点分两层：
    ///   - `enabled` → **总开关**（`autoCheckin.enabled`，语义不变）；
    ///   - `time` / `providers` → 落到**默认条目**（`checkin_schedules::upsert`）：
    ///     老入口没有条目 id 概念，写的就是合成的 default 条目——第一次保存会把
    ///     条目固化进 `schedules`（去重状态一并带过去，见 upsert 的存量兼容）。
    ///
    /// 关闭时调度循环继续跑，但 tick 读到总开关关会直接返回 —— 与 Node 一致
    /// （Node 的 configure 也只 start 不 stop，关闭靠 tick 早退生效）。
    pub fn configure(&self, payload: &Value) -> Result<Value, AutoCheckinConfigError> {
        let has_enabled = payload.get("enabled").is_some();
        let has_time = payload.get("time").is_some();
        let has_providers = payload.get("providers").is_some();
        if !has_enabled && !has_time && !has_providers {
            return Err(AutoCheckinConfigError::new("没有需要更新的字段"));
        }
        if has_enabled {
            // Node: `enabled !== undefined` → `patch.enabled = enabled === true`，
            // 也就是说 null / 字符串 / 0 都会把开关置为 false（不是「忽略」）
            let on = payload.get("enabled") == Some(&Value::Bool(true));
            write_state(json!({ "enabled": on }));
            logging::log(
                "[Checkin]",
                &format!("定时签到总开关已{}", if on { "开启" } else { "关闭" }),
            );
        }
        if has_time || has_providers {
            let mut entry = Map::new();
            entry.insert(
                "id".to_string(),
                Value::String(checkin_schedules::DEFAULT_SCHEDULE_ID.to_string()),
            );
            if has_time {
                entry.insert(
                    "time".to_string(),
                    Value::String(normalize_time(payload.get("time"))?),
                );
            }
            if let Some(value) = payload.get("providers") {
                // 勾选清单：只认注册过的提供商 id（去重、按注册顺序落盘）；
                // 一个都不勾没有意义 —— 定时到点也无账号可签，直接拒绝并说明。
                let Some(list) = value.as_array() else {
                    return Err(AutoCheckinConfigError::new("providers 必须是字符串数组"));
                };
                let picked: Vec<Value> = CHECKIN_PROVIDERS
                    .iter()
                    .filter(|id| list.iter().any(|item| item.as_str() == Some(*id)))
                    .map(|id| Value::String(id.to_string()))
                    .collect();
                if picked.is_empty() {
                    let names = CHECKIN_PROVIDERS
                        .iter()
                        .map(|id| provider_label(id))
                        .collect::<Vec<_>>()
                        .join("、");
                    return Err(AutoCheckinConfigError::new(format!(
                        "至少勾选一家签到提供商（{names}）"
                    )));
                }
                entry.insert("providers".to_string(), Value::Array(picked));
            }
            // 不带 enabled：条目自己的开关保持原样（这里只改时刻/范围）
            checkin_schedules::upsert(&Value::Object(entry))
                .map_err(|error| AutoCheckinConfigError::new(error))?;
        }
        // 总开关开着就走一遍 start()：确保调度循环在跑（幂等），并且刚开启 /
        // 刚改完配置的今天还没跑过的条目立刻补签（模块头「补签」的实测行为）
        if master_enabled() {
            self.start();
        }
        Ok(self.state())
    }

    /// 账号页「全部签到」的范围：**全部提供商**。
    ///
    /// 改造前这里返回的是配置勾选的清单（`autoCheckin.providers`）——那份清单
    /// 现在属于定时条目，不该再管手动批量：条目管「定时跑哪些」，手动全量
    /// 就该把所有能签的都签了（单账号的定点签到另有 /api/accounts/{id}/checkin）。
    pub fn batch_providers(&self) -> Vec<String> {
        default_providers()
    }

    /// 立即执行一次（界面上的「立即签到」按钮走这里，与定时触发同一条路径）。
    /// 正在执行中返回 None —— 路由层翻成「签到正在执行中，请稍候」。
    pub async fn run_now(&self) -> Option<Value> {
        self.fire("手动触发").await
    }

    /// 立即执行**某条定时条目**（定时签到页每条的「立即执行」按钮）。
    ///
    /// 与到点触发同一条 fire 路径（自带全局防重与条目落账），但不要求条目
    /// 或总开关开着 —— 用户点了就是明确要跑这一条。条目不存在返回 None 与
    /// 「在跑」不可区分，路由层先查存在性（见 api::checkin_schedules）。
    pub async fn run_schedule_now(&self, id: &str) -> Option<Value> {
        let schedule = checkin_schedules::list().into_iter().find(|entry| entry.id == id)?;
        let reason = format!("手动触发（{}）", checkin_schedules::display_label(&schedule));
        self.fire_for(&reason, Some(id), "manual").await
    }
}

// ─── 进程级句柄（供停机路径调用 stop）─────────────────────────

/// 进程级定时签到句柄。
///
/// 为什么不用 ServerState 就够了：停机入口是 `backend::shutdown(&AppState)`，
/// 那里只有 Tauri 的应用状态，拿不到 ServerState；而 Node 版退出时明确会
/// `autoCheckin.stop()`（server.mjs 1047-1049 的 closeAll）。用进程级句柄
/// （与 config / logging 同一个模式）让停机路径能直接拿到它。
/// ServerState 里那份与全局这份是**同一实例**（bootstrap 时装入）。
static GLOBAL: OnceLock<AutoCheckin> = OnceLock::new();

/// 初始化进程级句柄（启动时调用一次，幂等）
pub fn init_global(service: AutoCheckin) -> AutoCheckin {
    let _ = GLOBAL.set(service.clone());
    service
}

/// 停掉进程级调度；未初始化时什么都不做（路由以外的调用点用）
pub fn stop_global() {
    if let Some(service) = GLOBAL.get() {
        service.stop();
    }
}
