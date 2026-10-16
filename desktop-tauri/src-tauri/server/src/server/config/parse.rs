//! 配置字段的**解析**与环境变量兜底：`Map` 进去、解析结果出来。
//!
//! ── 为什么单独一层 ──────────────────────────────────────────
//! `mod.rs` 装的是「配置的读写入口」（内存快照、各 setter、落库），而这一组
//! 函数是**不依赖全局状态**的纯变换。它们被 `build()`（构造快照时全量解析
//! 一遍）与各 `set_*` 的回读逻辑调用，与库、与文件都没有关系 —— 抽出来之后
//! `mod.rs` 不必为了容纳这段注释与实现而挤到 900 行以上（本项目的单文件行数
//! 约定），而且「配置项的取值口径只有这一处」这件事在文件结构上直接可见。
//!
//! ── 读侧的统一口径（改造前后逐字一致）───────────────────────
//! 配置是**手改得到**的东西（改造前是 `config.json`，现在仍可用 `sqlite3`
//! 改库里的那一行），所以每个取值函数都必须回答「值缺失 / 类型不符 / 越界时
//! 怎么办」，且口径统一：**一律回落到默认值，不报错、不夹紧**。
//!   - 不报错：一个写坏的字段不该让整个网关起不来；
//!   - 不夹紧：静默把 5000 改成 3650 会让用户以为「设置生效了」，而回落到
//!     默认值至少与他看到的值不一致，好排查。
//! 走接口写入的值在各自的 PUT 处理器里已按同一范围校验过（那里是 400 报错）——
//! 两侧范围常量共用（`RETENTION_MIN_DAYS` 等），所以不会出现「接口拒绝 60
//! 而手改库接受它」这种两套口径。
//!
//! ── 环境变量为什么也在这里 ──────────────────────────────────
//! 「配置文件的值 > 环境变量 > 内置默认值」这条优先级由 `mod.rs` 的 `build()`
//! 落地，而 `env_api_key()` / `env_text()` 是它的组成部分 —— 它们与取值函数
//! 一起构成「一个字段最终怎么被解析出来」这一个问题的答案，分散两处会让改
//! 优先级的人只改一半。

use serde_json::{Map, Value};

use super::types::*;

/// 环境变量里的 API Key（新名优先，旧名兼容读；去空白，空串当未配置）
pub(super) fn env_api_key() -> Option<String> {
    env_api_key_value()
}

/// 两个候选名按序取第一个有效值（`AGENT2API_PROXY_API_KEY` > 旧名）
pub(crate) fn env_api_key_value() -> Option<String> {
    for name in ["AGENT2API_PROXY_API_KEY", "WORKBUDDY_PROXY_API_KEY"] {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// 环境变量里的非空字符串
pub(super) fn env_text(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 登录页人机验证组件环境变量，默认为1开启，0为关闭。
///
/// 只有配置里**没有** `captchaEnabled`（设置页从未改过）时才兜底 —— 优先级
/// 「配置里的值 > 环境变量 > 内置默认」见模块头。除字面 `0` 之外一律按开启
/// 处理：它是登录 / 注册的防爆破开关，写坏了宁可多一道校验。
pub(super) fn env_captcha_enabled() -> bool {
    std::env::var("AGENT2API_CAPTCHA_ENABLED")
        .map(|value| value.trim() != "0")
        .unwrap_or(true)
}

/// 环境变量 `AGENT2API_STRIP_NEWLINE_KEEPALIVE` 的读法（**默认关**）。
///
/// 与 `env_captcha_enabled` 的"非 0 即开"刻意不同：这个开关改的是**下发给客户端
/// 的字节**（丢掉只含换行的 content 分片），开错的代价是吞掉一个真换行，
/// 所以只认 `1` / `true` / `on`（忽略大小写与首尾空白），其余一律算关 ——
/// 包括空值。`AGENT2API_STRIP_NEWLINE_KEEPALIVE=` 这种"写了个空值"的 compose
/// 写法在 Docker 里太常见了（未设置的变量插值就是这样），按非 0 即开会把它读成开。
pub(super) fn env_strip_newline_keepalive() -> bool {
    switch_on_from_env_text(std::env::var("AGENT2API_STRIP_NEWLINE_KEEPALIVE").ok().as_deref())
}

/// 环境变量 `AGENT2API_AUTO_CONTINUE` 的读法（**默认关**）。
///
/// 判据与 `AGENT2API_STRIP_NEWLINE_KEEPALIVE` 共用一份（[`switch_on_from_env_text`]），
/// 取向也一样：这个开关多打的是**上游往返**、花的是账号额度，空值 / 写坏一律算关
/// —— 宁可功能没开（用户在面板上点一次就有），也不要一个拼错的 compose 变量
/// 替用户开始多烧额度。
pub(super) fn env_auto_continue() -> bool {
    switch_on_from_env_text(std::env::var("AGENT2API_AUTO_CONTINUE").ok().as_deref())
}

/// 环境变量 `AGENT2API_AUTO_CONTINUE_MAX` 的读法。
///
/// 与文件侧同一口径（[`int_in_range`]：越界 / 非整数都算**没配**，不夹紧），
/// 所以 `=0`、`=9`、`=abc`、`=3.5` 与「没设这个变量」四种情形都交回 `None`，
/// 由 [`auto_continue_rounds_from`] 落到默认值。
pub(super) fn env_auto_continue_max_rounds() -> Option<i64> {
    bounded_int_from_env_text(
        std::env::var("AGENT2API_AUTO_CONTINUE_MAX").ok().as_deref(),
        AUTO_CONTINUE_MIN_ROUNDS,
        AUTO_CONTINUE_MAX_ROUNDS,
    )
}

/// 「环境变量里的开关」的唯一判据：只认 `1` / `true` / `on`
/// （忽略大小写与首尾空白），其余一律算关。
///
/// ── 为什么抽出来 ────────────────────────────────────────────
/// 这条规则有两个读者（保活换行丢弃、自动续写），两个都必须**默认关**且都
/// 不接受「非 0 即开」：判据各写一份，将来一方放宽（比如收了 `yes`）另一方不放宽，
/// 用户在 compose 里写同一个词会得到两种结果 —— 这类差异不报错，只能靠一份实现。
///
/// 参数是「环境变量的值」而不是自己去读进程环境：判据要能被单测钉住。
/// `std::env::set_var` 在同一进程里全局可见，并发用例互相踩（本仓的同类取舍见
/// `core::providers::trae::forward::forward_at` 的注释），所以「缺失」由调用方
/// 传 `None` 表达，测试不必碰真环境。
pub(super) fn switch_on_from_env_text(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|value| value.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

/// 环境变量文本 → 范围内的整数：缺失 / 空白 / 非整数 / 越界都是 `None`。
///
/// 判据与 [`int_in_range`] 同一取向（**不夹紧**），只是输入是文本而不是 JSON
/// 数字：`3.0` 在文件里认（JSON 的 3 与 3.0 是同一个数），在环境变量里不认 ——
/// 那里的文本是人在 shell 里手打的，`"3"` 才是常态写法，收下 `3.0` 换不来什么，
/// 只会让 `AGENT2API_AUTO_CONTINUE_MAX=3.O`（打错的字母 O）这类事故变成静默。
pub(super) fn bounded_int_from_env_text(raw: Option<&str>, min: i64, max: i64) -> Option<i64> {
    let text = raw.map(str::trim).filter(|text| !text.is_empty())?;
    let value = text.parse::<i64>().ok()?;
    (min..=max).contains(&value).then_some(value)
}

/// 单个 JSON 数字 → 范围内的整数：类型不符 / 非整数 / 越界都是 `None`
/// （**不夹紧**，理由见模块头）。
///
/// ── 为什么这一层是 `pub(crate)` 而不是 `pub(super)` ──────────
/// 读写两侧必须共用同一条取值口径 —— 模块头那句「不会出现接口拒绝 60 而手改库
/// 接受它」讲的正是这件事，而口径不只是范围常量，还包括**什么算一个整数**：
/// `30.0` 这种整值浮点读侧认，写侧（`api::auto_continue` 的 PUT 校验）就必须也认，
/// 否则前端某次回传出 `3.0` 会 400，而手改库写同一个数却被接受。
pub(crate) fn int_in_range(value: &Value, min: i64, max: i64) -> Option<i64> {
    let parsed = match value {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                .map(|raw| raw as i64)
        }),
        _ => None,
    };
    parsed.filter(|value| (min..=max).contains(value))
}

/// 从原始 JSON 里取「带范围边界的整数」，**不回落默认**：缺字段 / 类型不符 /
/// 非整数 / 越界一律 `None`，交调用方决定下一级（环境变量或默认值）。
///
/// 判据本体在 [`int_in_range`]（读写两侧共用）。
pub(super) fn bounded_int_option(map: &Map<String, Value>, key: &str, min: i64, max: i64) -> Option<i64> {
    map.get(key).and_then(|value| int_in_range(value, min, max))
}

/// 从原始 JSON 里取「带范围边界的整数」：缺字段 / 类型不符 / 非整数 / 越界
/// 一律回落到 `default`。
///
/// **越界回落而非夹紧**：手改 config.json 写了天文数字时，静默夹紧会让人
/// 以为「设置生效了」，回默认值至少与用户在设置页看到的不一致时好排查。
/// 走接口写入的值在各自的 PUT 处理器里已按同一范围校验过（那里是 400 报错）。
///
/// 接受 `30.0` 这种整值浮点（与 `parse_days` 同一口径）：JSON 里
/// `30` 与 `30.0` 都是合法数字，为后者回落到默认值会显得莫名其妙
/// （用户手改文件时把 30 写成 30.0 是完全可能的事）。
pub(super) fn bounded_int_field(map: &Map<String, Value>, key: &str, default: i64, min: i64, max: i64) -> i64 {
    bounded_int_option(map, key, min, max).unwrap_or(default)
}

/// 从原始 JSON 里取非空字符串字段
pub(super) fn string_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 从原始 JSON 里取天数（`bounded_int_field` 的保留期特化）
pub(super) fn days_field(map: &Map<String, Value>, key: &str, default: i64) -> i64 {
    bounded_int_field(map, key, default, RETENTION_MIN_DAYS, RETENTION_MAX_DAYS)
}

/// 由原始 JSON 解析三档保留天数（缺字段各自用默认值）
pub(super) fn retention_from(map: &Map<String, Value>) -> RetentionSettings {
    let defaults = RetentionSettings::default();
    RetentionSettings {
        log_days: days_field(map, KEY_LOG_RETENTION_DAYS, defaults.log_days),
        request_days: days_field(map, KEY_REQUEST_RETENTION_DAYS, defaults.request_days),
        daily_days: days_field(map, KEY_DAILY_RETENTION_DAYS, defaults.daily_days),
    }
}

// ─── 间隔型定时任务的解析（scheduledTasks.*）──────────────────

/// 从 `scheduledTasks` 里取一条任务的原始子对象；缺失 / 类型不符当空对象
/// （于是 `enabled` 用默认值、`interval` 也用默认值，与「用户没配过」等价）。
pub(super) fn task_object(map: &Map<String, Value>, key: &str) -> Map<String, Value> {
    map.get(KEY_SCHEDULED_TASKS)
        .and_then(Value::as_object)
        .and_then(|tasks| tasks.get(key))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// 从一条任务的子对象里取间隔值。
///
/// 与 `days_field` 同一口径（**越界回落而非夹紧**、接受整值浮点、只在
/// `min..=max` 内才采纳），只是范围由调用方给：两类任务的合理区间差着量级
/// （后端维护按分钟、前端刷新按秒），共用一份范围常量会逼其中一类放宽边界。
pub(super) fn interval_field(task: &Map<String, Value>, default: i64, min: i64, max: i64) -> i64 {
    let parsed = task.get("interval").and_then(|value| match value {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                .map(|raw| raw as i64)
        }),
        _ => None,
    });
    parsed.filter(|value| (min..=max).contains(value)).unwrap_or(default)
}

/// 从一条任务的子对象里取开关。
///
/// **缺字段按开启**：这些任务在本次改造前都是无条件运行的（凭证维护每 10 分钟、
/// 两个前端面板每 10 秒、模型刷新随 /v1/models 触发），
/// 升级上来的 config.json 里没有 `scheduledTasks` —— 若把「没配过」读成「关闭」，
/// 用户什么也没动，凭证却不再自动续期了。写侧（`PUT /api/scheduled-tasks`）
/// 则要求显式布尔值。
pub(super) fn task_enabled(task: &Map<String, Value>, default: bool) -> bool {
    task.get("enabled").and_then(Value::as_bool).unwrap_or(default)
}

/// 由原始 JSON 解析六条间隔型任务（缺字段各自用默认值）。
/// 旧配置里可能残留 `usageQuery` 子键（已退役的全局余额查询任务）：
/// 解析按**未知键忽略**处理，读不到就是不存在，首次保存设置时会被自然清掉。
pub(super) fn scheduled_from(map: &Map<String, Value>) -> ScheduledSettings {
    let defaults = ScheduledSettings::default();
    let task = |key: &str, interval: i64, min: i64, max: i64| {
        let object = task_object(map, key);
        IntervalTask {
            enabled: task_enabled(&object, true),
            interval: interval_field(&object, interval, min, max),
        }
    };
    ScheduledSettings {
        credential_maintenance: task(
            KEY_CREDENTIAL_MAINTENANCE,
            defaults.credential_maintenance.interval,
            INTERVAL_MIN_MINUTES,
            INTERVAL_MAX_MINUTES,
        ),
        model_refresh: task(
            KEY_MODEL_REFRESH,
            defaults.model_refresh.interval,
            INTERVAL_MIN_MINUTES,
            INTERVAL_MAX_MINUTES,
        ),
        logs_auto_refresh: task(
            KEY_LOGS_AUTO_REFRESH,
            defaults.logs_auto_refresh.interval,
            INTERVAL_MIN_SECONDS,
            INTERVAL_MAX_SECONDS,
        ),
        requests_auto_refresh: task(
            KEY_REQUESTS_AUTO_REFRESH,
            defaults.requests_auto_refresh.interval,
            INTERVAL_MIN_SECONDS,
            INTERVAL_MAX_SECONDS,
        ),
        report_auto_refresh: task(
            KEY_REPORT_AUTO_REFRESH,
            defaults.report_auto_refresh.interval,
            INTERVAL_MIN_SECONDS,
            INTERVAL_MAX_SECONDS,
        ),
        update_check: task(
            KEY_UPDATE_CHECK,
            defaults.update_check.interval,
            INTERVAL_MIN_MINUTES,
            INTERVAL_MAX_MINUTES,
        ),
    }
}

// ─── 请求重试设置的解析（retryCount / retryCrossProviderCount / retryIntervalSeconds / noRetryStatusCodes）──

/// 由原始 JSON 解析请求重试设置（缺字段各自用默认值）
pub(super) fn retry_from(map: &Map<String, Value>) -> RetrySettings {
    let defaults = RetrySettings::default();
    RetrySettings {
        count: bounded_int_field(
            map,
            KEY_RETRY_COUNT,
            defaults.count,
            RETRY_MIN_COUNT,
            RETRY_MAX_COUNT,
        ),
        account_switch_count: bounded_int_field(
            map,
            KEY_RETRY_ACCOUNT_SWITCH_COUNT,
            defaults.account_switch_count,
            RETRY_MIN_COUNT,
            RETRY_MAX_COUNT,
        ),
        interval_seconds: bounded_int_field(
            map,
            KEY_RETRY_INTERVAL_SECONDS,
            defaults.interval_seconds,
            RETRY_MIN_INTERVAL_SECONDS,
            RETRY_MAX_INTERVAL_SECONDS,
        ),
        no_retry_codes: no_retry_codes_field(map, KEY_RETRY_NO_RETRY_CODES),
    }
}

/// 「指定错误码直接换号」名单的解析。
///
/// ── 各形态的落点 ────────────────────────────────────────────
///   - 键缺失 / 不是数组 → **默认名单**（[402]）：配置从没写过这一项是常态，
///     默认值就该在这时出现；整体写坏（手改成了字符串）与「没配过」无法区分，
///     按同一口径处理（回落默认，见模块头）。
///   - 空数组 → **空名单**：这是用户的明确选择（任何错误都照常重试），
///     必须与「没配过」区分开。
///   - 数组里的垃圾项（非整数 / 越出 100–599）→ **丢弃该项**：接口写入侧
///     已整单校验（400），能到这里的坏项只来自手改库，跳过比整单回落默认
///     更接近手改者的本意。排序去重：判定只问 contains，顺序无意义，
///     存一份规整形态让手改过的 config.json 也保持可读。
fn no_retry_codes_field(map: &Map<String, Value>, key: &str) -> std::sync::Arc<[u16]> {
    let Some(Value::Array(items)) = map.get(key) else {
        return std::sync::Arc::from(DEFAULT_NO_RETRY_CODES);
    };
    let mut codes: Vec<u16> = items
        .iter()
        .filter_map(|item| {
            let number = item.as_i64().or_else(|| {
                item.as_f64()
                    .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                    .map(|raw| raw as i64)
            });
            u16::try_from(number?)
                .ok()
                .filter(|code| (RETRY_CODE_MIN..=RETRY_CODE_MAX).contains(code))
        })
        .collect();
    codes.sort_unstable();
    codes.dedup();
    std::sync::Arc::from(codes)
}

// ─── 上游请求超时的解析（四项，单位秒）────────────────────────

/// 由原始 JSON 解析四项超时（缺字段各自用默认值；越界回落默认，同模块头口径）。
///
/// 四项的键名与默认值见 `types.rs`——「范围 1~3600」在写侧（timeouts_api）
/// 是 400 报错，在这里是回落默认：手改库把 0 或 99999 写进去时，宁可回到
/// 30/300 也不要让转发层拿到一个必然坏事的值（0 毫秒等于禁用该阶段保护）。
pub(super) fn timeouts_from(map: &Map<String, Value>) -> TimeoutSettings {
    let defaults = TimeoutSettings::default();
    TimeoutSettings {
        connect_seconds: bounded_int_field(
            map,
            KEY_TIMEOUT_CONNECT_SECONDS,
            defaults.connect_seconds,
            TIMEOUT_MIN_SECONDS,
            TIMEOUT_MAX_SECONDS,
        ),
        headers_seconds: bounded_int_field(
            map,
            KEY_TIMEOUT_HEADERS_SECONDS,
            defaults.headers_seconds,
            TIMEOUT_MIN_SECONDS,
            TIMEOUT_MAX_SECONDS,
        ),
        stream_idle_seconds: bounded_int_field(
            map,
            KEY_TIMEOUT_STREAM_IDLE_SECONDS,
            defaults.stream_idle_seconds,
            TIMEOUT_MIN_SECONDS,
            TIMEOUT_MAX_SECONDS,
        ),
        body_seconds: bounded_int_field(
            map,
            KEY_TIMEOUT_BODY_SECONDS,
            defaults.body_seconds,
            TIMEOUT_MIN_SECONDS,
            TIMEOUT_MAX_SECONDS,
        ),
    }
}

// ─── 排队等待的解析（两项，次数与秒数）──────────────────────────

/// 由原始 JSON 解析排队等待的两项（缺字段各自用默认值；越界回落默认）。
///
/// 与 `timeouts_from` 同一口径：范围校验在写侧（`queue_api`）是 400 报错，
/// 在这里是回落默认 —— 手改库写进一个越界值时，宁可回到 2 次 / 跟随建议，
/// 也不要让转发层拿到一个必然坏事的值（例如负数次数会让重试变成死循环）。
pub(super) fn queue_from(map: &Map<String, Value>) -> QueueSettings {
    let defaults = QueueSettings::default();
    QueueSettings {
        max_waits: bounded_int_field(
            map,
            KEY_QUEUE_MAX_WAITS,
            defaults.max_waits,
            QUEUE_MIN_MAX_WAITS,
            QUEUE_MAX_MAX_WAITS,
        ),
        wait_seconds: bounded_int_field(
            map,
            KEY_QUEUE_WAIT_SECONDS,
            defaults.wait_seconds,
            QUEUE_MIN_WAIT_SECONDS,
            QUEUE_MAX_WAIT_SECONDS,
        ),
    }
}

// ─── ZCode 验证码令牌池的解析（三项：目标 / 轮询间隔 / 单轮上限）──────

/// 由原始 JSON 解析令牌池的三项（缺字段各自用默认值；越界回落默认）。
///
/// 与 `queue_from` 同一口径：范围校验在写侧（`zcode_captcha_pool_api`）是
/// 400 报错，在这里是回落默认 —— 手改库写进一个越界值时，宁可回到出厂节奏，
/// 也不要让铸造器拿到一个必然坏事的值（例如 0 毫秒的轮询会让它原地空转）。
pub(super) fn captcha_pool_from(map: &Map<String, Value>) -> CaptchaPoolSettings {
    let defaults = CaptchaPoolSettings::default();
    CaptchaPoolSettings {
        target: bounded_int_field(
            map,
            KEY_CAPTCHA_POOL_TARGET,
            defaults.target,
            CAPTCHA_POOL_MIN_TARGET,
            CAPTCHA_POOL_MAX_TARGET,
        ),
        poll_ms: bounded_int_field(
            map,
            KEY_CAPTCHA_POOL_POLL_MS,
            defaults.poll_ms,
            CAPTCHA_POOL_MIN_POLL_MS,
            CAPTCHA_POOL_MAX_POLL_MS,
        ),
        max_per_round: bounded_int_field(
            map,
            KEY_CAPTCHA_POOL_MAX_PER_ROUND,
            defaults.max_per_round,
            CAPTCHA_POOL_MIN_MAX_PER_ROUND,
            CAPTCHA_POOL_MAX_MAX_PER_ROUND,
        ),
    }
}

// ─── 历史路由优先级（providerRoute，只读，供账号迁移）───────────

// ─── 账号路由的解析（策略 / 粘性 / 退避 / 垫底 / 熔断）──────────────

/// 由原始 JSON 解析路由设置（缺字段各自用默认值；非法取值回落默认）。
///
/// 与 `timeouts_from` 同一口径：范围校验在写侧（`routing_api`）是 400 报错，
/// 在这里是回落默认 —— 手改库写进一个离谱值时，宁可回到默认行为也不要让
/// 选路拿到一个必然坏事的值（例如阈值 0 会让熔断第一个失败就打开）。
pub(super) fn routing_from(map: &Map<String, Value>) -> RoutingSettings {
    let defaults = RoutingSettings::default();
    let strategy = map
        .get(KEY_ROUTING_STRATEGY)
        .and_then(Value::as_str)
        .map(RouteStrategy::from_id)
        .unwrap_or(defaults.strategy);
    let sticky = map
        .get(KEY_ROUTING_STICKY)
        .and_then(Value::as_str)
        .map(StickyMode::from_id)
        .unwrap_or(defaults.sticky);
    let bool_field = |key: &str, default: bool| -> bool {
        map.get(key).and_then(Value::as_bool).unwrap_or(default)
    };
    RoutingSettings {
        strategy,
        sticky,
        failover_backoff: bool_field(KEY_ROUTING_FAILOVER_BACKOFF, defaults.failover_backoff),
        sink: bool_field(KEY_ROUTING_SINK, defaults.sink),
        breaker_enabled: bool_field(KEY_ROUTING_BREAKER_ENABLED, defaults.breaker_enabled),
        breaker_threshold: bounded_int_field(
            map,
            KEY_ROUTING_BREAKER_THRESHOLD,
            defaults.breaker_threshold,
            ROUTING_BREAKER_MIN_THRESHOLD,
            ROUTING_BREAKER_MAX_THRESHOLD,
        ),
        breaker_timeout_seconds: bounded_int_field(
            map,
            KEY_ROUTING_BREAKER_TIMEOUT,
            defaults.breaker_timeout_seconds,
            ROUTING_BREAKER_MIN_TIMEOUT,
            ROUTING_BREAKER_MAX_TIMEOUT,
        ),
        breaker_success_threshold: bounded_int_field(
            map,
            KEY_ROUTING_BREAKER_SUCCESS,
            defaults.breaker_success_threshold,
            ROUTING_BREAKER_MIN_SUCCESS,
            ROUTING_BREAKER_MAX_SUCCESS,
        ),
    }
}

// ─── 长度截断的自动续写（autoContinue / autoContinueMaxRounds）────────

/// 由原始 JSON 解析「自动续写」开关：文件的字面布尔 > 环境变量 > 关。
///
/// 与 `build()` 里 `strip_newline_keepalive` 那一条同一口径（**只有字面
/// `true` / `false` 算写了这一项**，写成 `"on"` / `1` 都算没配、让位给环境变量），
/// 差别只是那条的环境变量兜底在调用点显式出现，这里收在 [`resolve_auto_continue`]
/// 里 —— 与本模块「取值口径只有这一处」的分工一致（见模块头）。
pub(super) fn auto_continue_from(map: &Map<String, Value>) -> bool {
    resolve_auto_continue(
        map.get(KEY_AUTO_CONTINUE).and_then(Value::as_bool),
        env_auto_continue(),
    )
}

/// 「文件值 / 环境变量值 → 最终开关」：文件里写了字面布尔就以它为准，**包括 `false`**。
///
/// 之所以做成两个入参的纯函数（而不是在 [`auto_continue_from`] 里就地读环境变量）：
/// 这条优先级链要能被单测钉住，而进程环境不能（见 [`switch_on_from_env_text`]
/// 的注释）。它钉的第二件事是「关掉必须是可表达的」—— 写成
/// `文件值 || 环境变量` 的实现看着也是「文件优先」，实际让用户在面板上关不掉。
pub(super) fn resolve_auto_continue(from_file: Option<bool>, from_env: bool) -> bool {
    from_file.unwrap_or(from_env)
}

/// 由原始 JSON 解析续写轮数上限：文件的合法值 > 环境变量 > 内置默认。
///
/// 与 `routing_from` 同一口径（范围校验在写侧 `api::auto_continue` 是 400 报错，
/// 在这里是回落默认），多出来的一级是环境变量兜底 —— 与 `env_auto_continue`
/// 那个开关对齐：Docker 部署要么两项都从环境来，要么都从配置来，不该一边能兜底
/// 一边不能。
///
/// 注意**文件里越界也让位给环境变量**，而不是直接落默认：这一项「不可用」的
/// 含义是「这一级没给出可用值」（与键缺失等价），与模块头的优先级一致。
pub(super) fn auto_continue_rounds_from(map: &Map<String, Value>) -> i64 {
    resolve_rounds(
        bounded_int_option(
            map,
            KEY_AUTO_CONTINUE_MAX_ROUNDS,
            AUTO_CONTINUE_MIN_ROUNDS,
            AUTO_CONTINUE_MAX_ROUNDS,
        ),
        env_auto_continue_max_rounds(),
    )
}

/// 「文件值 / 环境变量值 → 最终轮数」：两级都不给可用值才落默认。
///
/// 之所以做成两个 `Option` 的纯函数（而不是在 [`auto_continue_rounds_from`]
/// 里就地读环境变量）：优先级链本身要能被单测钉住，而进程环境不能（见
/// [`switch_on_from_env_text`] 的注释）。默认值在这里显式出现一次，
/// 也让「越界不夹紧、只是让位给下一级」这条规则有唯一一处落点。
pub(super) fn resolve_rounds(from_file: Option<i64>, from_env: Option<i64>) -> i64 {
    from_file.or(from_env).unwrap_or(DEFAULT_AUTO_CONTINUE_MAX_ROUNDS)
}

#[cfg(test)]
mod tests {
    //! 三条取值口径：环境变量的「只认 1 / true / on」、越界的「回落默认而非夹紧」、
    //! 以及「文件 > 环境变量 > 默认」这条优先级链。
    //!
    //! ── 为什么这些用例都不碰进程环境 ────────────────────────
    //! `std::env::set_var` 在同一进程里全局可见，并行用例互相踩（同类取舍见
    //! `core::providers::trae::forward::forward_at` 的注释），而本模块里
    //! 「没设环境变量」是有明确表达的输入 —— `None`。所以三段判据都被抽成
    //! 吃参数的纯函数（[`switch_on_from_env_text`] / [`bounded_int_from_env_text`] /
    //! [`resolve_auto_continue`] / [`resolve_rounds`]），`env_*` 只是把它们接到
    //! 真环境上的那一薄层。

    use super::*;
    use serde_json::json;

    /// 一份只含给定几项的原始底稿。
    fn map_of(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    /// 开关的环境变量只认 `1` / `true` / `on`（忽略大小写与首尾空白）。
    ///
    /// 钉住的写歪：「非 0 即开」（`env_captcha_enabled` 那条口径）。按它写会
    /// 把 `AGENT2API_AUTO_CONTINUE=`（compose 里未设置变量的插值形态）读成**开**，
    /// 于是一个谁都没打算开的部署开始替用户多打上游。
    #[test]
    fn the_env_switch_only_accepts_one_true_or_on() {
        for on in ["1", "true", "on", " TRUE ", "On", "ON "] {
            assert!(switch_on_from_env_text(Some(on)), "{on} 应当算开启（大小写与首尾空白都要被吃掉）");
        }
        for off in ["", " ", "0", "yes", "10", "2", "no", "开启", "trueX", "one"] {
            assert!(!switch_on_from_env_text(Some(off)), "{off} 不该被当成开启");
        }
        assert!(!switch_on_from_env_text(None), "环境变量没设（None）必须是关");
    }

    /// 整数型环境变量的口径：只有**纯整数**且在范围内才算数。
    ///
    /// 钉住的写歪：先 `parse::<f64>()` 再取整（于是 `3.7` 变成 3），或越界时
    /// 夹紧到 8 —— 后者会让人以为自己的 99 生效了。
    #[test]
    fn the_env_integer_must_be_a_plain_integer_in_range() {
        assert_eq!(bounded_int_from_env_text(Some("1"), 1, 8), Some(1), "下限要收");
        assert_eq!(bounded_int_from_env_text(Some(" 8 "), 1, 8), Some(8), "上限要收，空白要吃掉");
        for unusable in ["", " ", "0", "9", "-1", "3.0", "3.5", "abc", "8.", "+"] {
            assert_eq!(
                bounded_int_from_env_text(Some(unusable), 1, 8),
                None,
                "{unusable} 不是可用值，必须交回 None 让下一级兜（而不是夹紧）"
            );
        }
        assert_eq!(bounded_int_from_env_text(None, 1, 8), None);
    }

    /// 文件里越界 → **回落默认，不夹紧**（`bounded_int_field` 全线口径）。
    ///
    /// 钉住的写歪：`clamp(min, max)`。写了 99 的人若看到设置页显示 8，
    /// 只会以为「8 是系统给的硬值」，而 99 从未生效这件事再也问不出来。
    #[test]
    fn an_out_of_range_file_value_falls_back_to_the_default_not_the_bound() {
        let too_many = map_of(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(99))]);
        assert_eq!(
            bounded_int_option(&too_many, KEY_AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS, AUTO_CONTINUE_MAX_ROUNDS),
            None,
            "越界必须表达成「这一级没给出可用值」，夹紧就不叫回落了"
        );
        assert_eq!(
            bounded_int_field(&too_many, KEY_AUTO_CONTINUE_MAX_ROUNDS, DEFAULT_AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS, AUTO_CONTINUE_MAX_ROUNDS),
            DEFAULT_AUTO_CONTINUE_MAX_ROUNDS,
            "回落的是默认值 3，不是上限 8"
        );
        // 边界上的两个数与「写坏的类型」各归各位
        let zero = map_of(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(0))]);
        assert_eq!(bounded_int_field(&zero, KEY_AUTO_CONTINUE_MAX_ROUNDS, DEFAULT_AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS, AUTO_CONTINUE_MAX_ROUNDS), DEFAULT_AUTO_CONTINUE_MAX_ROUNDS, "0 在范围外（想关去关开关）");
        let text = map_of(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!("3"))]);
        assert_eq!(bounded_int_option(&text, KEY_AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS, AUTO_CONTINUE_MAX_ROUNDS), None, "字符串数字不算（与写侧同一口径）");
        // `3.0` 算 3：JSON 里 3 与 3.0 是同一个数，手改文件时写成 3.0 完全可能
        let floaty = map_of(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(3.0))]);
        assert_eq!(bounded_int_option(&floaty, KEY_AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS, AUTO_CONTINUE_MAX_ROUNDS), Some(3));
    }

    /// 优先级链：文件写了就以它为准（**包括 false 与 1**），否则环境变量，否则默认。
    ///
    /// 钉住的写歪：把开关写成 `文件值 || 环境变量值` —— 看上去也是「文件优先」，
    /// 实际让用户在面板上关不掉（库里已经是 false，环境里那句 `on` 一直把它顶开）。
    #[test]
    fn the_file_value_beats_the_env_even_when_it_says_off() {
        assert!(!resolve_auto_continue(Some(false), true), "文件里的 false 必须盖过环境变量的开");
        assert!(resolve_auto_continue(Some(true), false), "文件里的 true 必须盖过环境变量的关");
        assert!(resolve_auto_continue(None, true), "文件没配时环境变量说话");
        assert!(!resolve_auto_continue(None, false), "两级都没配 → 默认关");

        assert_eq!(resolve_rounds(Some(1), Some(8)), 1, "文件里的 1 必须盖过环境变量的 8");
        assert_eq!(resolve_rounds(Some(8), None), 8);
        assert_eq!(resolve_rounds(None, Some(8)), 8, "文件不可用时环境变量说话");
        assert_eq!(resolve_rounds(None, None), DEFAULT_AUTO_CONTINUE_MAX_ROUNDS, "两级都不可用才落默认");
        // 越界的文件值在这里已经是 None（上面试用例过了），链上表现为「让位给下一级」
        assert_eq!(resolve_rounds(None, Some(1)), 1);
    }

    /// 整条链的形状：`*_from(&map)` 必须与纯函数那一层给出同一个答案。
    ///
    /// 钉住的写歪：`auto_continue_rounds_from` 里把范围常量写歪（例如上限用了
    /// `AUTO_CONTINUE_MAX_ROUNDS - 1`），那会让 8 在读侧被当成「没配」——
    /// 而写侧刚收下的值读侧不认，正是 `parse` 模块头明令不许出现的两套口径。
    #[test]
    fn the_map_level_helpers_follow_the_same_rules_as_the_pure_ones() {
        let at_the_top = map_of(&[
            (KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(8)),
            (KEY_AUTO_CONTINUE, json!(true)),
        ]);
        assert_eq!(
            auto_continue_rounds_from(&at_the_top),
            8,
            "上限边界值 8 必须能用（读写两侧共用同一对范围常量）"
        );
        assert!(auto_continue_from(&at_the_top), "文件写了 true 就必须是开");

        let off = map_of(&[
            (KEY_AUTO_CONTINUE, json!(false)),
            (KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(1)),
        ]);
        assert!(!auto_continue_from(&off));
        assert_eq!(auto_continue_rounds_from(&off), 1, "下限边界值 1 必须能用");

        // 类型写坏的那一项等于「没配」：越界 / 字符串都不许留下一个可感的值
        let junk = map_of(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(0)), (KEY_AUTO_CONTINUE, json!("true"))]);
        assert_eq!(
            auto_continue_rounds_from(&junk),
            resolve_rounds(None, env_auto_continue_max_rounds()),
            "文件里的 0 不可用，落点必须与「文件没这项」逐字相同"
        );
        assert_eq!(
            auto_continue_from(&junk),
            resolve_auto_continue(None, env_auto_continue()),
            "文件里的字符串 true 不可用，落点必须与「文件没这项」逐字相同"
        );
    }
}
