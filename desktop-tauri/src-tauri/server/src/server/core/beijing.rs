//! 北京时间（固定 UTC+8）—— 签到链路对「今天」的唯一定义。
//!
//! ── 为什么要有这一层（issue #138）────────────────────────────
//! 签到的「今天」是**上游的自然日**：各家额度都在北京时间零点前后重置
//! （CodeArts 福利按 UTC+8 零点切日、小浣熊积分按天发放、Qoder 国际版的
//! 「每日 100 Credits」窗口每天 10:00（UTC+8）开启）。网关可能跑在 NAS /
//! Docker / 海外 VPS 上，那些机器的本地时区不是 UTC+8 —— 判定跟着机器时区
//! 走会让「今天」的边界整体漂移：美西机器上 00:01 的定时签到实际发生在
//! 北京时间 15:01，面板的「今日已签到」也按错误的日期亮起。
//!
//! ── 为什么是固定偏移而不是时区库 ─────────────────────────────
//! 中国标准时间没有夏令时，`Asia/Shanghai` 在 1970 年之后恒为 +08:00 ——
//! 一个固定偏移就够，且不依赖宿主机的 tzdata / 时区配置。仓库里
//! `core::degrade` 与 `providers::codearts::welfare` 早已是同一口径
//! （各自内联实现）；本模块是签到链路这一份的收敛点。
//!
//! ── 与 `request_stats::clock` 的关系（两套口径，各自有意）────────
//! 请求统计按**操作者本机时区**切天（报表看的是「我的昨天」），那套口径
//! 由 `clock` 单独维护；签到跟随**上游自然日**，用本模块。两者不可互换：
//! 拿报表的日期键去判签到会重演 issue #138。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：零 unwrap/expect/panic，取值一律走 Option 链。

use chrono::{DateTime, FixedOffset, Offset};

/// UTC+8 的固定偏移（秒）。
pub const OFFSET_SECONDS: i32 = 8 * 3600;

/// 固定 UTC+8 时区对象。
///
/// 8 小时恒在 `FixedOffset` 的合法范围（±24h）内，`east_opt` 不会失败；
/// 不用 unwrap 是为了守住 release 无 panic 的约定 —— 兜底给 UTC 零点偏移，
/// 极端情形下日期可能差 8 小时，好过把进程 abort 掉。
pub fn offset() -> FixedOffset {
    FixedOffset::east_opt(OFFSET_SECONDS).unwrap_or_else(|| chrono::Utc.fix())
}

/// 毫秒时间戳 → 北京时间时刻；越界的非法时间戳给 None。
pub fn date_time_of_ms(ms: i64) -> Option<DateTime<FixedOffset>> {
    DateTime::from_timestamp_millis(ms).map(|utc| utc.with_timezone(&offset()))
}

/// 北京时间当前时刻（时钟来源与全仓一致：`logging::now_ms`）。
pub fn now() -> DateTime<FixedOffset> {
    date_time_of_ms(crate::server::logging::now_ms())
        .unwrap_or_else(|| chrono::Utc::now().with_timezone(&offset()))
}

/// 北京日期键 `YYYY-MM-DD`（定长，字典序即时间序）。
pub fn date_key(now: DateTime<FixedOffset>) -> String {
    now.format("%Y-%m-%d").to_string()
}

/// 毫秒时间戳落在北京时间的哪一天（`YYYY-MM-DD`）；非法时间戳给 None。
pub fn date_key_of_ms(ms: i64) -> Option<String> {
    date_time_of_ms(ms).map(date_key)
}

/// 北京时间的今天（`YYYY-MM-DD`）。
pub fn today_key() -> String {
    date_key(now())
}
