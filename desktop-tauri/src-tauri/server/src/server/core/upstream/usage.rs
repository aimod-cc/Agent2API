//! usage 旁路提取（请求统计用）：从上游 chunk 里抄出 token 用量与承载账号。
//!
//! ── 为什么叫「旁路」──────────────────────────────────────────
//! 调用点都在**已经解析好、马上就要原样下发**的帧旁边（`sse.rs` 的逐行解析
//! 分支、`aggregate.rs` 的聚合分支）：只读一眼 JSON、往共享槽位写一份拷贝，
//! 不参与帧的构造、不改动任何要下发的字节。所以无论提取成功、失败还是字段
//! 缺失，客户端收到的内容与接入前逐字一致 —— 这是本文件的硬约束。
//!
//! ── 为什么字段名要兼容多种写法 ───────────────────────────────
//! 上游是 OpenAI 兼容接口，但不同版本/不同接入点对用量字段的命名并不统一：
//!   prompt_tokens（OpenAI 标准） / input_tokens（部分兼容实现）/ promptTokens
//!   / inputTokens（驼峰形态，见 `protocol::commandcode_outbound` 里同一族的键）
//!   completion_tokens / output_tokens / completionTokens / outputTokens
//!   缓存命中：prompt_tokens_details.cached_tokens（OpenAI 标准）
//!            / cache_read_tokens / cache_read_input_tokens（Anthropic 风格）
//!            / 驼峰的 cacheReadTokens / cachedInputTokens
//!            / input_tokens_details.cached_tokens（Responses 系的新形态）
//!   积分消耗：credit（WorkBuddy 的 `usage.credit`，可带小数）
//!            / credits / credit_used / creditUsed / credit_cost 等常见别名
//! 取到哪个算哪个：宁可少记一项，也不要因为字段名对不上而把整条记成 0。
//!
//! ── 为什么同一个含义的多个别名要「取第一个正数」而不是「取第一个存在」 ──
//! 上游会**同时**给出嵌套与顶层两种写法，且嵌套那份常是废值。一手形态：
//! `{"prompt_tokens_details":{"cached_tokens":0}, "cache_read_input_tokens":4800}`
//! —— 按「存在与否」挑，先命中的是那条恒 0 的嵌套项，整条缓存用量就记成 0，
//! 报表的命中率列跟着变成 0（而 0 是「这家的缓存没生效」的读数，比缺值更难查）。
//! 按「第一个正数」挑，0 与缺失同样视为「这一项没说话」，交给下一个别名。
//! 对 prompt/completion/total 同一口径：`total_tokens: 0` 是废值，
//! 该回落到 prompt + completion，而不是把总量记成 0。
//!
//! ── 为什么这里**不设** cache_write / cache_creation 槽位 ────────────
//! `usage.cache_read` 唯一的落点是请求日志的 `cache_read_tokens` 列
//! （`db/schema.rs:354`），而它进的是**命中率**的分子
//! （`request_stats::report::safe_rate` = hit / input，`aggregate.rs:216` 的日聚合
//! 也叫 `cache_hit_tokens`）。「写入缓存」与「命中缓存」是两笔不同的计费，
//! 折进同一列会把命中率抬高（`protocol::anthropic_outbound.rs:801` 那种
//! `cache_read + cache_creation` 的合并是给 Anthropic 出站报文的 `cached_tokens`
//! 字段用的，语义本来就是「prompt 里被缓存处理的总量」，与这里的命中率不同物）。
//! 真要分列，得先给存储契约加列 —— 那不是「口径对齐」的改动面，别顺手塞进本文件。
//!
//! ── 为什么回调语义是「每次都上报、由读取侧覆盖」────────────────
//! usage 通常只在流的最后一个 chunk 出现（也有实现会在中途补发增量帧），
//! 于是「最后一次上报即最终值」。把覆盖策略留给读取侧的好处是上报侧完全
//! 无状态：不需要知道「这一帧是不是最后一帧」，也就不可能因为判断错而丢数据。
//!
//! ── 为什么这里不返回 Result ─────────────────────────────────
//! 上报是纯附加动作：任何一步（字段类型不对、锁中毒）都不能影响转发链路。
//! 所以所有方法都吞掉异常、不 panic —— 统计少记一条是可接受的，把用户请求
//! 搞坏不可接受（release 是 panic=abort，一次 panic 会带走整个桌面应用）。

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Map, Value};

mod attempt;
pub use attempt::{
    AttemptDetail, RetryEvent, MAX_ATTEMPT_DETAILS, MAX_ATTEMPT_ERROR_CHARS,
    MAX_ATTEMPT_ACCOUNT_CHARS, MAX_ATTEMPT_RETRY_REASON_CHARS, MAX_ATTEMPT_NOTICE_CHARS,
};

use crate::server::logging;

/// 一次上报的用量（四种 token 计数 + 可选的积分消耗，字段名对应存储契约）。
///
/// 四类 token 是整数计数；`credit` 是**计费读数**（上游按请求计价的家才给，
/// 例如 WorkBuddy 的 `usage.credit`），可以为小数，因此单独用 `Option<f64>`
/// 表达 —— `None` = 这一家没上报，与「上报了 0」是两件事。
///
/// 只有这几槽是因为存储契约只有这几列（见模块头部「为什么不设 cache_write」那条）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageTokens {
    pub prompt: i64,
    pub completion: i64,
    pub total: i64,
    pub cache_read: i64,
    pub credit: Option<f64>,
}

/// 「输入 token 数」的别名表（含驼峰形态）。**表序就是优先序**：多项都命中时按表序
/// 取第一个**正数**（见 [`first_positive`]），不是求和、也不是取末位。
const PROMPT_KEYS: &[&str] = &["prompt_tokens", "input_tokens", "promptTokens", "inputTokens"];
/// 「输出 token 数」的别名表。
const COMPLETION_KEYS: &[&str] = &[
    "completion_tokens",
    "output_tokens",
    "completionTokens",
    "outputTokens",
];
/// 「总量」的别名表；全缺（或全为废值 0）时由 prompt + completion 补。
const TOTAL_KEYS: &[&str] = &["total_tokens", "totalTokens"];
/// 「缓存命中 token 数」的**顶层**别名表。
///
/// `cache_read_input_tokens` 是 Anthropic 系的写法（本仓的入站/出站两侧都在读它，
/// 见 `protocol::anthropic.rs` 与 `protocol::responses_outbound.rs`）；驼峰两项是
/// Command Code 一族上游的原样形态（`protocol::commandcode_outbound.rs` 归一前读的
/// 就是这一族键）；`cached_*` 是部分兼容实现把它当顶层字段直接给的做法。
///
/// **表序就是优先序**（与 [`PROMPT_KEYS`] 同一条规矩）：`cached_tokens` 排在最前，
/// 因为它是 OpenAI 标准形态；排它的理由不是「更权威」而是「更常见」，多项都为正数时
/// 按表序取第一项。真正要挡的是**这一族里某一项恒为 0 时的短路**：取第一个**正数**
/// 而不是第一个「存在的」，否则嵌套容器里那个恒 0 的 `cached_tokens` 会把顶层真有
/// 值的 `cache_read_input_tokens` 挡掉（0 在前端不是缺值，是「这家的缓存没生效」的
/// 读数，比少记一项更难查）。
const CACHE_READ_KEYS: &[&str] = &[
    "cached_tokens",
    "cached_input_tokens",
    "cache_read_tokens",
    "cache_read_input_tokens",
    "cacheReadTokens",
    "cacheReadInputTokens",
    "cachedInputTokens",
];
/// 「缓存命中 token 数」的**嵌套容器**（容器里再找 [`CACHE_READ_KEYS`]）。
///
/// `prompt_tokens_details` 是 OpenAI 标准（里面放 `cached_tokens`）；
/// `input_tokens_details` 是 Responses 系把 prompt 改叫 input 之后的同名容器。
/// 两个容器都查：上游同时给两份、其中一份是废值的情况已经出现过（模块头部）。
const CACHE_READ_CONTAINERS: &[&str] = &["prompt_tokens_details", "input_tokens_details"];
/// 「积分消耗」的别名表（顺序即优先序）。
///
/// WorkBuddy 的 `usage.credit` 是已实测的一手形态；其余是兼容实现常见的
/// 下划线 / 驼峰写法。`points_*` 一组来自 Loomy 生图的 `points_consumed`
/// （同样是实测形态），也一并认，好让「上游把积分叫 points」的家不需要再改表。
/// 末尾的 `cost` 来自 Cline 的 `usage.cost`：它是**美元成本**而不是积分，
/// 按「积分列就是计费读数」的产品口径与积分共用一列（排在最后，同时存在时
/// 仍优先取 credit）。与 token 的别名表同一条规矩：取**第一个正数**，避免
/// 某个恒 0 的别名把真值挡在后面；一个别名都没有时才返回 None。
const CREDIT_KEYS: &[&str] = &[
    "credit",
    "credits",
    "credit_used",
    "creditUsed",
    "credits_used",
    "creditsUsed",
    "credit_cost",
    "creditCost",
    "credit_consumed",
    "creditConsumed",
    "points_consumed",
    "pointsConsumed",
    "points_used",
    "pointsUsed",
    "cost",
];

/// 从 usage 对象里提取 token 数（字段名兼容与「第一个正数」口径见模块头部）。
///
/// 返回 None 只有一种情况：`usage` 不是对象（null / 数字 / 字符串 / 数组）。
/// 对象内单个字段缺失**或为非正数**按「这项没说话」处理，交给别名表的下一项；
/// 全部落空才按 0 算。`total` 的兜底是 prompt + completion。
pub fn extract_usage(usage: &Value) -> Option<UsageTokens> {
    let object = usage.as_object()?;
    let prompt = first_positive(object, PROMPT_KEYS).unwrap_or(0);
    let completion = first_positive(object, COMPLETION_KEYS).unwrap_or(0);
    let total = first_positive(object, TOTAL_KEYS).unwrap_or(prompt + completion);
    // 嵌套容器先查（标准写法优先），再查顶层别名；两边都只认正数。
    let cache_read = CACHE_READ_CONTAINERS
        .iter()
        .filter_map(|container| object.get(*container).and_then(Value::as_object))
        .find_map(|details| first_positive(details, CACHE_READ_KEYS))
        .or_else(|| first_positive(object, CACHE_READ_KEYS))
        .unwrap_or(0);
    let credit = first_amount(object, CREDIT_KEYS);
    Some(UsageTokens { prompt, completion, total, cache_read, credit })
}

/// 从一个 JSON 对象里按积分别名取读数。
///
/// 给「本次消耗直接挂在响应顶层、不在 `usage` 里」的家用：Loomy 生图的
/// `points_consumed` 就是这种形态（`{created, data:[…], points_consumed}`）。
/// 与 [`extract_usage`] 共用同一张 [`CREDIT_KEYS`] 别名表，所以别名一改两边同时生效。
pub fn credit_from_object(object: &Map<String, Value>) -> Option<f64> {
    first_amount(object, CREDIT_KEYS)
}

/// 按别名表依次取**第一个正数**（口径见模块头部）。
fn first_positive(object: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| positive_field(object.get(*key)))
}

/// 取数值字段，**只认正数**。
///
/// 三种上游形态都要认：
///   - 整数（`as_i64`）；
///   - `1.0` 这类浮点形态（`as_i64` 对它失败，所以补一次 f64 转换再截断；
///     上游不同实现给整数/浮点都有先例）；
///   - **字符串**数字（`"34"`，部分兼容实现把 usage 里的数当文本吐出；
///     走 f64 解析所以 `"12.0"` 也认）。
///
/// 0 与负数一律返回 None（= 「这一项没说话」，交给别名表的下一项）：
///   - 0 是上游最常给的废值，按「有值」用掉就会把真值挡在后面那个别名上
///     （模块头部那条嵌套 `cached_tokens` 恒 0 就是这一条）；
///   - 负数多半是手改的库或上游溢出。这个值会进日聚合的**加数**
///     （`request_stats::aggregate` 的 `cache_hit_tokens += …`），一条负值能把
///     整天的读数压低，而读侧的 `report::safe_rate` 只挡比率、挡不住聚合和。
///     夹成 0 同样能防住压低，但那会让这条负值继续扮演「有值」而埋掉后面的真值，
///     所以按 None 处理。
///
/// 非数值（bool / null / 数组 / 解析不出的字符串）同样 None，交给别名表兜底。
fn positive_field(value: Option<&Value>) -> Option<i64> {
    let raw = match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|number| number as i64)),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .map(|number| number as i64),
        _ => None,
    };
    raw.filter(|number| *number > 0)
}

/// 按别名表取**第一个正数**；全是 0 时返回显式的 0（「上报了 0」）。
///
/// 与 [`first_positive`] 的差别只有两处，都是为了计费读数：
///   - **不截断小数**（`0.06` 必须原样保留，不能变成 0）；
///   - 0 不再是废值：所有别名都显式给 0 时，这条请求确实「零积分」，
///     与「一个别名都没有」（None = 这一家不提供该读数）必须分开。
///
/// 负数仍然当废值丢掉（手改的库 / 上游溢出）：它既不能当「有值」，
/// 也不能当「零消耗」。
fn first_amount(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    let mut zero = None;
    for key in keys {
        let Some(value) = amount_field(object.get(*key)) else { continue };
        if value > 0.0 {
            return Some(value);
        }
        zero = zero.or(Some(0.0));
    }
    zero
}

/// 取非负有限浮点数：认数字与「数字字符串」（与 [`positive_field`] 同一兼容面，
/// 但不做 i64 截断）。
fn amount_field(value: Option<&Value>) -> Option<f64> {
    let raw = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    raw.filter(|number| number.is_finite() && *number >= 0.0)
}

/// 在途请求当前所处的**转发阶段**（请求日志状态列的第一个读数）。
///
/// ── 四个取值的含义 ──────────────────────────────────────────
///   - `Connecting` 连接中：请求刚受理，选路 / 取凭证 / 建立连接 / 构造请求体
///     都在这段里（上游请求还没发出去）；
///   - `Waiting` 等待响应：本轮上游请求**已发出**，等第一个字节回来
///     （模型思考的时间也在这一段，与 OmniProxy 的「等待响应（含模型思考）」同义）；
///   - `Streaming` 响应中：首帧已到、内容正在下发；
///   - `Retrying` 重试中：本轮失败了，退避等待 / 换号前的过渡（下一轮发出即回到
///     `Waiting`）。
///
/// ── 与 OmniProxy 的对应关系 ─────────────────────────────────
/// 前四个取值与那边的 `LogPhase`（`connecting` / `waiting` / `streaming` /
/// `retrying`）逐字对应，第五个（`queued`）是本项目的扩展（排队制上游的
/// 等待阶段，OmniProxy 没有对应物）：`as_str()` 的返回值就是落库的字符串，
/// 前端按它选文案
/// （连接中 / 等待响应 / 响应中 / 重试中 —— 同一套文案）。阶段计时的语义也一致：
/// **进入阶段的时刻**记一次（[`TelemetrySnapshot::phase_started_at`]），
/// 前端那第二行显示的是「在当前阶段里待了多久」而不是总耗时。
///
/// ── 为什么阶段与「首帧」是两个读数 ──────────────────────────
/// `Streaming` 与 `first_response_at` 在同一次调用里确定（`note_first_frame`），
/// 但两者不等价：首帧是**一次性事实**（收尾后仍要留着，明细的「首响」列读它），
/// 阶段是**当前状态**（收尾即清空，只在途有意义）。混用一个字段会让
/// 「请求结束了但阶段还写着响应中」这种状态有机会出现。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogPhase {
    /// 请求已受理，还没把上游请求发出去（槽位的初始值）
    #[default]
    Connecting,
    /// 上游请求已发出，等首字节
    Waiting,
    /// 首帧已到，内容正在下发
    Streaming,
    /// 本轮失败，退避 / 换号过渡中
    Retrying,
    /// **上游排队中**：上游没报错，只是把请求排进了队列（模型繁忙），
    /// 网关按上游建议的时长等着重发（见 `qoder::queue_backoff`）。
    ///
    /// 与 `Retrying` 分开的理由：两者的**成因与处置完全不同** —— 重试是「这一轮
    /// 失败了再打一次」，排队是「这一轮还没开始，在等上游放行」。用户看到「排队中」
    /// 就知道「不是网关卡住、也不是账号有问题，是模型忙」，而「重试中」会让人以为
    /// 上游报错了。首次等待由 `note_queued` 进入，退避结束后的重发把它推回
    /// `Waiting`（`note_attempt_started`）。
    Queued,
}

impl LogPhase {
    /// 落库与 API 透出的字面量（与 OmniProxy 的 `LogPhase` 逐字相同）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Waiting => "waiting",
            Self::Streaming => "streaming",
            Self::Retrying => "retrying",
            Self::Queued => "queued",
        }
    }
}

/// 记账点要的最终快照（一次性读走，避免读字段时逐次加锁）
#[derive(Clone, Debug, Default)]
pub struct TelemetrySnapshot {
    /// 实际尝试的**账号数**（0 = 还没走到选路就失败了）。
    ///
    /// 口径说明：一次「尝试」= 选路循环的一轮 = 向一个账号发一次上游请求
    /// （见 `forward` 的选路循环）。同一账号内的 11128 退避重试
    /// （`request_with_waf_retry` 的 10s/25s 两次）**不计入** —— 它换的是时间
    /// 不是账号，报表里「换了几个账号才成功」比「总共打了几次上游」更常用。
    ///
    /// 存储契约是「含首次、恒 ≥1」，所以 0 只出现在「一次都没发出去」时，
    /// 由记账点 `.max(1)` 归一。
    pub attempts: i64,
    /// 本条请求的关联 id（转发开始前生成一次，全链路不变）。
    ///
    /// 用途：请求日志条目与调试模式的原始报文（`core::debug_traffic`）用同一个
    /// id 关联 —— 日志页的「详情」列拿它去取报文。空串 = 该条没有（本字段引入
    /// 前落盘的旧行），前端据此不显示详情入口。
    pub id: String,
    /// 最终承载本次请求的账号（用默认登录态转发、或选路结果无账号时为空串）
    pub account_id: String,
    /// 账号展示名，取值顺序与限额日志一致：账号名 → 会话昵称 → 账号 id
    /// （所以账号 id 为空但会话带昵称时，这里仍可能非空）
    pub account_name: String,
    /// 最终承载本次请求的 **provider id**（Agent2API W2b-T3 新增；
    /// `None` = 还没走到选路就失败了，例如请求体非法 / 模型不存在）。
    ///
    /// 为什么存 id 字符串而不是 `ProviderKind`：这个值要跨模块交给记账点
    /// （`api::chat` → `request_stats`），落进请求日志的 `provider` 字段，
    /// 最终出现在报表与前端 —— 契约里它就是 id 字符串（architecture §3.6）。
    /// 存字符串省掉一次「kind → id」的转换与两处枚举依赖。
    pub provider: Option<String>,
    /// 实际发给上游的模型名（映射 + 备援按家改写后的**最终值**；空串 =
    /// 一次都没发出去，例如转发前就失败）。
    ///
    /// 与 `provider` 同一「最后一次为准」口径：429 换账号（可能换家）后，
    /// 请求日志的「上游模型」应该跟着实际承载的那一次走。采集点在发送体
    /// 决定处（`upstream::payload::send_body`，每家首次尝试时覆盖一次）。
    /// 空串口径与 `account_id` 一致：键恒在、空串表示没有。
    pub upstream_model: String,
    /// 实际随上游请求发出的**思考等级**（空串 = 没有等级随行）。
    ///
    /// 与 `upstream_model` 同点同时采集（`payload::send_body`，发送体定稿处）、
    /// 同一「最后一次为准」口径：429 换家后留下实际承载那一次的等级。
    /// 空串的三种成因：客户端没指定且映射没绑、承载家不接等级（适配器的
    /// `outbound_reasoning` 返回 None）、「关闭思考」档被注入闸拦下。
    /// 请求日志的模型列用它给上游模型名带 `(等级)` 后缀。
    pub upstream_reasoning: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    /// 上游按请求上报的**积分消耗**（WorkBuddy 的 `usage.credit`；`None` = 这一家
    /// 不提供该读数，落库为 NULL，前端显示「—」而不是 0）。
    ///
    /// 与 token 四件套同一「最后一次上报覆盖」口径：一次请求可能有多轮尝试，
    /// 终态记账只保留最后那次上游 `usage` 里的值（与 prompt / completion 同源）。
    pub credit: Option<f64>,
    pub quota_channel: String,
    /// 上游首帧到达的**绝对时刻**（毫秒 Unix 时间戳；None = 全程没有帧到达，
    /// 例如转发前就失败 / 请求尚未开始下发）。
    ///
    /// 为什么存绝对时刻而不是相对耗时：采集点（响应流）与记账点（请求收尾）
    /// 分布在两处，「请求开始时刻」只有记账点知道 —— 存绝对值让采集点完全
    /// 不需要知道口径，减法在记账点做一次（`record_entry`），与 durationMs
    /// 用同一个 `started_at`，两列的参考点不可能各说各话。
    ///
    /// 对应 OmniProxy 请求日志的 `ttfb_ms`（time to first byte）：它回答
    /// 「上游多久开始吐内容」，把「等上游出首字」与「生成完整段内容」两段
    /// 耗时分开 —— 只有 durationMs 时，一个 30 秒的请求看不出是上游慢
    /// 还是内容长。
    pub first_response_at: Option<i64>,
    /// 当前所处的转发阶段（见 [`LogPhase`]；槽位一建出来就是「连接中」）。
    ///
    /// 只在**在途**期间有意义：收尾记账不把它写进明细（终态行的 `phase` 列
    /// 一律清空，见 `request_stats::sql` 的各条收尾语句），读点只有进行中行的
    /// 状态列（`phaseElapsedMs` 的起点见下一个字段）。
    pub phase: LogPhase,
    /// **进入当前阶段**的绝对时刻（毫秒 Unix 时间戳；None = 还停在槽位的初始
    /// 阶段「连接中」上，一次都没切换过）。
    ///
    /// 与 `first_response_at` 同一手法存绝对值：采集点不知道「请求什么时候
    /// 开始的」，而展示层要的是「在这个阶段里待了多久」——
    /// 在那里减一次即可（在途回写把阶段起点写进 `requests.phase_started_at`，
    /// API 序列化时现算 `phaseElapsedMs`）。
    ///
    /// ── 为什么初值是 None 而不是建槽时的 now ────────────────────
    /// 「连接中」的起点就是**请求开始时刻**（`record_started` 写进行的 `ts`），
    /// 那个值是记账点带进来的 `started_at`，槽位这里拿不到。None 表达
    /// 「还没换过阶段」，在途回写用 `started_at` 兜底（见
    /// `api::pipeline::live_row_sink`）—— 两边算出来的是同一个时刻，
    /// 不会出现「行说 3 秒、阶段说 2.9 秒」这种对不上的读数。
    pub phase_started_at: Option<i64>,
    /// 中断 / 异常原因（成功为 None）
    pub error: Option<String>,
    /// **本次请求命中的脱敏规则**（规则标签 + 次数；空表 = 一个都没命中）。
    ///
    /// ── 数据从哪来 ──────────────────────────────────────────────
    /// `core::sanitize` 的 `sanitize_body` 在剥离指纹时顺手算出命中明细
    /// （按次数降序的每条规则命中数）。字段名与形状**未随规则集更换而改动**
    /// —— 列名 `sensitive_hits`、元素 `{word, count}`、前端的「敏」标签与
    /// 悬停面板都照常工作，只是 `word` 里装的从「命中的敏感词」变成
    /// 「命中的规则标签」（如 `11128` / `cc_*=` / 模板句原文）。
    ///
    /// 采集点在 `payload::send_body`（某一家即将发送、拿到脱敏结果的那一刻），
    /// 由 [`Self::note_sensitive_hits`] 合并进来。
    ///
    /// ── 为什么是并集累加（与 attempts 的覆盖式相对）───────────────
    /// 见 `payload::send_body` 的说明：跨家降级时每个在范围内的家各处理一次，
    /// 合并计数才是「这次请求命中了什么」的正确读数。
    pub sensitive_hits: Vec<SensitiveHit>,
    /// **每一次上游尝试的明细**（按发生顺序，与账号轮换链一一对应）。
    ///
    /// ── 为什么需要它（Agent2API 请求日志改造）───────────────────
    /// 原先只有 `attempts`（次数）与 `provider`（最终承载者）两个读数，
    /// 于是请求日志里那一列只能显示一个「重试」标记 —— 用户看到「重试过」，
    /// 但看不到**换了谁、哪一次失败在哪、最后是谁扛下来的**。而这些信息在
    /// 转发链路里每一轮都是现成的（provider id、HTTP 状态码、错误摘要），
    /// 只是此前没有落点。
    ///
    /// ── 口径（与 `attempts` / `provider` 刻意不同）──────────────
    /// `attempts` 与 `provider` 都是「最后一次为准」的**覆盖式**读数；
    /// 本字段相反，是**追加式**的历史。两者不冲突：覆盖式回答「最终算谁头上」
    /// （报表聚合要的就是这个），追加式回答「这一路是怎么走过来的」。
    ///
    /// ── 为什么记「已发送」而不是「选路选到了」───────────────────
    /// 采集点在 `note_attempt` 旁边（该函数在**凭证已就绪、即将发送之前**
    /// 被调用），所以与 `attempts` 同源同频。选路过程中被跳过的账号
    /// （禁用 / 限额冷却 / 不支持该模型）**不进这条链** —— 它们没有产生任何
    /// 上游往返，混进来会让「切换路径」显示一串从没被请求过的名字。
    ///
    /// 条数上限见 [`MAX_ATTEMPT_DETAILS`]：这是一条请求内的内存小数组，
    /// 落库时序列化成 JSON 文本，必须有上界。
    pub attempts_detail: Vec<AttemptDetail>,
    pub route: Option<Value>,
    route_revision: u64,
}

/// 一个被命中的敏感词及其次数（`TelemetrySnapshot::sensitive_hits` 的元素）。
///
/// 字段名与前端 `sensitiveHits` 契约里的键一致（`word` / `count`），
/// 与 OmniProxy 的 `SensitiveTermHit` 同形 —— 那边的 `sensitive_masked_terms`
/// 也是一份 `[{word, count}]`。同形不是巧合：两边的信息结构本来就一样
/// （命中词 × 次数），照抄形态省掉一层没意义的转换。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SensitiveHit {
    /// 命中的词（词表里的原样字符串，未做大小写归一 —— 用户填的是什么就显示什么）
    pub word: String,
    /// 本次请求里命中的次数
    pub count: i64,
}

/// 按字符截断（超出部分用 `…` 收尾）。
///
/// 不引 `request_stats::truncate_chars`：同名的实现住在 `api::pipeline` 与
/// `account_store::store_util` 里，本模块在 `core::upstream`，跨模块取一个
/// 五行的私有工具只会让依赖方向变乱（core 不认识 api）。按**字符**而不是字节：
/// 中文摘要按字节截会切出半个字。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// 在途回写钩子：把当前快照交给存储层写进「进行中」行（见
/// [`RequestTelemetry::set_live_sink`]）。
///
/// 类型是 `Arc<dyn Fn>` 而不是某个具体存储类型：本模块在 `core::upstream` 下，
/// **不认识** `request_stats`（依赖方向见 `core/mod.rs` 的约定）。接线在 api 层
/// （`api::pipeline::live_row_sink`），这里只负责在状态变化时调用它。
pub type LiveSink = Arc<dyn Fn(&TelemetrySnapshot) + Send + Sync>;

/// 在途回写的**内容指纹**（见 [`RequestTelemetry::flush_live`]）。
///
/// 字段是「进行中行会显示的那些」的一对一摘要：值本身，或「有没有 / 有几条」。
///
/// ── 为什么「先整份构造、再比较」而不是「先逐字段比对」─────────
/// 构造一份指纹要克隆四个短字符串，而流式路径每收一帧都会调一次 `flush_live`
/// （`note_first_frame` 挂在透传流的每个分片上）—— 看似该省。省的办法是再写一个
/// 逐字段比对函数，但那会多出**第二份字段清单**：加字段时漏改一处，闸门要么永不
/// 打开（界面不再实时）、要么次次打开（每帧一次写库），两种都很难发现。这里选
/// 「一份清单」：比同一条路径上每帧拷贝响应正文缓冲便宜得多，不值得为它换一个
/// 静默失效的风险。
///
/// ── 为什么不含 `error` ──────────────────────────────────────
/// 那一列一旦有值，前端就不再把这一行当「进行中」（判据见 `ui/requests-panel.js`
/// 的 `isRunning`），而转发中途的失败往往只是换号前的一次尝试失败 —— 让它提前
/// 把整行标成失败，正是「进行中」这个状态要避免的误读。失败原因由尝试明细
/// （`last_error`）如实带出，不必借 error 列。
///
/// ── 为什么不含 token 四件套 ─────────────────────────────────
/// 与 `RunningProgress` 同理：进行中行不显示用量，为一次看不见的更新写库没有意义
/// （usage 通常只在上游最后一个 chunk 才出现，收尾记账紧接着就会写它）。
///
/// ── 为什么含 `phase` 却不含 `phase_started_at` ───────────────
/// 阶段计时是「现在 - 阶段起点」，它每一拍都在变 —— 放进指纹会让闸门次次打开
/// （流式路径每帧一次写库，正是指纹要挡掉的东西）。而阶段起点**只在换阶段时
/// 变**，于是「阶段变了」这一个布尔量就足以覆盖两个字段的变化：比 `phase`
/// 即可，计时读侧自己算。
#[derive(Default, PartialEq)]
struct LiveStamp {
    provider: String,
    account_id: String,
    account_name: String,
    upstream_model: String,
    upstream_reasoning: String,
    attempts: i64,
    /// 首响的**绝对时刻**（与快照同形，转换在 api 层做）
    first_response_at: Option<i64>,
    /// 当前阶段（换阶段必写一次 —— 状态列的徽章与第二行计时都读它）
    phase: LogPhase,
    /// 尝试明细的条数 + **最后一条**的定局情况（状态码 / 有没有错误 / 退避次数 /
    /// 提示）。只看最后一条：明细是严格串行的，前面的轮次一旦定局就不再变化。
    details: usize,
    last_status: Option<i64>,
    last_error: bool,
    last_retries: usize,
    last_notice: bool,
    last_response_at: Option<i64>,
    last_finished_at: Option<i64>,
    route_revision: u64,
    sensitive: usize,
}

impl LiveStamp {
    fn of(snapshot: &TelemetrySnapshot) -> Self {
        let last = snapshot.attempts_detail.last();
        Self {
            provider: snapshot.provider.clone().unwrap_or_default(),
            account_id: snapshot.account_id.clone(),
            account_name: snapshot.account_name.clone(),
            upstream_model: snapshot.upstream_model.clone(),
            upstream_reasoning: snapshot.upstream_reasoning.clone(),
            attempts: snapshot.attempts,
            first_response_at: snapshot.first_response_at,
            phase: snapshot.phase,
            details: snapshot.attempts_detail.len(),
            last_status: last.and_then(|item| item.status),
            last_error: last.map(|item| item.error.is_some()).unwrap_or(false),
            last_retries: last.map(|item| item.retries.len()).unwrap_or(0),
            last_notice: last.map(|item| item.notice.is_some()).unwrap_or(false),
            last_response_at: last.and_then(|item| item.response_at),
            last_finished_at: last.and_then(|item| item.finished_at),
            route_revision: snapshot.route_revision,
            sensitive: snapshot.sensitive_hits.len(),
        }
    }
}

/// 在途回写的接线状态（钩子为空 = 不启用，例如转发前就失败的记账路径 ——
/// 那些请求没有进行中行，回写无处可写）。
#[derive(Default)]
struct LiveWrite {
    sink: Option<LiveSink>,
    /// 上一次**已经回写**的那份指纹（首次回写前是全空：任何真实状态都与之不同）
    stamp: LiveStamp,
}

/// usage 上报槽：转发链路往里写，记账点在收尾时读。
///
/// 为什么用共享槽位而不是把数据一路 return 出来：流式转发的收尾发生在
/// **handler 返回之后**（由响应流自己被 axum 拉取），那时转发函数早已返回，
/// 只能靠一个双方都能拿到的句柄传话。槽位只在一条请求内共享，不存在争用。
pub struct RequestTelemetry {
    inner: Mutex<TelemetrySnapshot>,
    /// 调试模式的原始报文采集器（`core::debug_traffic`；None = 未开启调试模式）。
    ///
    /// 挂在这里而不是层层传参：流式路径的采集发生在 `ForwardStream::poll_next`
    /// （handler 早已返回），非流式发生在聚合函数里，两者手上都只有 telemetry
    /// —— 转发层在**即将发送前**把它装进来，两条路径各自从同一个槽位取。
    ///
    /// 独立一把锁（不与 `inner` 共用）：采集器的读写都在转发热路径上，
    /// 与「记账字段」的锁分开可以避免两处互不相关的写互相等待。
    capture: Mutex<Option<Arc<super::super::debug_traffic::TrafficCapture>>>,
    /// 在途回写（钩子 + 上一次已回写的指纹；见 [`Self::set_live_sink`]）
    live: Mutex<LiveWrite>,
    /// 手动终止的取消令牌（`core::upstream::cancellation`；None = 这条请求
    /// 没登记 —— 转发前就失败的记账路径，或未接线的调用方）。
    ///
    /// 与 `capture` 同一挂法：转发链的各个等待点（等响应头、退避睡眠、流式
    /// 轮询）手上只有 telemetry，令牌放这里它们才够得着；接线在入口 handler
    /// （`api::chat` / `api::protocol` 在 `record_started` 之后装入）。
    cancel: Mutex<Option<Arc<super::cancellation::CancelToken>>>,
}

impl Default for RequestTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestTelemetry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TelemetrySnapshot::default()),
            capture: Mutex::new(None),
            live: Mutex::new(LiveWrite::default()),
            cancel: Mutex::new(None),
        }
    }

    /// 建一个已带关联 id 的槽位（三个对话入口都走这条：转发开始前生成一次）。
    ///
    /// 为什么不把 id 生成塞进 `new()`：`RequestTelemetry::new()` 还被
    /// 「转发前就失败」的记账路径用（`record_early_failure`），那些请求没有
    /// 原始报文可采，凭空生成一个 id 只会让日志里多出一批没有详情的行。
    pub fn with_id() -> Self {
        let telemetry = Self::new();
        telemetry.ensure_id(&super::request::new_request_id());
        telemetry
    }

    /// 装入调试模式的采集器（转发层即将发送前调一次）。
    pub fn set_capture(&self, capture: Arc<crate::server::core::debug_traffic::TrafficCapture>) {
        let mut guard = self
            .capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(capture);
    }

    /// 取采集器（未开启调试模式时为 None，调用点据此完全跳过采集）
    pub fn capture(
        &self,
    ) -> Option<Arc<crate::server::core::debug_traffic::TrafficCapture>> {
        self.capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 装入手动终止的取消令牌（入口 handler 在 `record_started` 之后调一次）。
    ///
    /// 与 `ensure_id` 同一时机与同一「首次为准」纪律的变体：令牌只在请求
    /// 开始时装一次，转发链上各处只读不写（置位走注册表 `cancellation::cancel`，
    /// 不从这里进）。
    pub fn set_cancel_token(&self, token: Arc<super::cancellation::CancelToken>) {
        let mut guard = self
            .cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(token);
    }

    /// 取取消令牌（未接线时为 None，各等待点据此退化成「不可取消」的原行为）
    pub fn cancel_token(&self) -> Option<Arc<super::cancellation::CancelToken>> {
        self.cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 本条请求是否已被手动终止（循环顶的同步判据；令牌缺省时恒 false）
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token()
            .map(|token| token.is_cancelled())
            .unwrap_or(false)
    }

    /// 生成并记下本条请求的关联 id（转发开始前调一次）。
    ///
    /// **首次为准**：重复调用不覆盖 —— id 一旦生成，请求日志与调试报文就必须
    /// 认它，中途换一个会让已经落盘的报文变成孤儿。空串参数不写（调用方拿不到
    /// id 时保持空，前端据此不显示详情入口）。
    pub fn ensure_id(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let mut guard = self.lock();
        if guard.id.is_empty() {
            guard.id = id.to_string();
        }
    }

    /// 本条请求的关联 id（未生成时为空串）
    pub fn id(&self) -> String {
        self.lock().id.clone()
    }

    /// 装入在途回写钩子（**转发开始前**由调用方装一次；见 `LiveSink`）。
    ///
    /// ── 它解决什么 ──────────────────────────────────────────────
    /// 「进行中」行（`RequestStats::record_started` 插的那条）只有 id / ts / 模型名：
    /// 选路与发送体定稿都发生在它之后，于是整段转发期间列表里那一行看不出**谁在
    /// 承载、转发的是哪个模型、已经试了几轮**。这些读数在本槽位里本来就有，钩子
    /// 把它们实时回写进那一行。
    ///
    /// ── 为什么由调用方装，而不是本模块自己建 ────────────────────
    /// 本模块在 core 下、不认识存储层（理由见 `LiveSink`）；调用方
    /// （`api::chat` / `api::protocol`）手里既有 `Arc<RequestStats>` 又有本条请求
    /// 的 id 与开始时刻，正好在 `record_started` 之后装。**不装**的路径：转发前就
    /// 失败的记账（`record_early_failure`）—— 那些请求没有进行中行，回写无处可写。
    pub fn set_live_sink(&self, sink: impl Fn(&TelemetrySnapshot) + Send + Sync + 'static) {
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        live.sink = Some(Arc::new(sink));
    }

    /// 状态**真的变了**才把快照交给在途回写钩子（每个会改状态的方法收尾处调一次）。
    ///
    /// ── 为什么是「内容指纹」而不是时间节流 ──────────────────────
    /// 流式路径每收到一帧都会 `note_first_frame`（幂等），时间节流会把「窗口内
    /// 真实发生的变化」丢掉 —— 而进行中行的全部价值就在实时性上。指纹是精确的：
    /// 同一份状态重复上报只写一次，真变了立刻写。于是库上的写入次数等于**状态
    /// 变化次数**（一条普通请求 3~4 次），而不是上报次数（流式可达每帧一次）。
    ///
    /// ── 调用时机在持 `inner` 锁期间 ─────────────────────────────
    /// 各方法刚改完就调：这样「读快照 → 比指纹 → 回写」不会被另一处上报插进来，
    /// 写进库的必然是某个真实状态，而不是两次修改拼出来的中间态。回写本身是
    /// 一次单行 UPDATE（在途字段那几列），与收尾记账同一条纪律。
    fn flush_live(&self, snapshot: &TelemetrySnapshot) {
        // 钩子与指纹同锁：取钩子、比指纹、记新指纹必须是一次原子判断，否则两次
        // 上报可能都判「变了」而各写一遍（无害，但没必要）。`live` 这把锁在调用
        // 钩子**之前**就放掉 —— 它保护的是「要不要写」，而写库的等待不该占着它
        // （`inner` 仍由调用方持有，见上面「调用时机」那段）。
        let sink = {
            let mut live = self
                .live
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(sink) = live.sink.clone() else {
                return;
            };
            let next = LiveStamp::of(snapshot);
            if next == live.stamp {
                return;
            }
            live.stamp = next;
            sink
        };
        sink(snapshot);
    }

    /// 切换转发阶段（同阶段重复调用是**无操作**）。
    ///
    /// ── 为什么同阶段不重置计时 ──────────────────────────────────
    /// 阶段计时的语义是「在这个阶段里待了多久」，同一个阶段的多次上报
    /// （例如流式路径每帧都会走到 `note_first_frame`）绝不能把起点越改越晚 ——
    /// 否则「响应中」的计时会永远停在 0 秒附近。所以只有**真的换了阶段**
    /// 才更新起点，这正是与 OmniProxy 的 `updateLogPhase` 唯一的口径差异：
    /// 那边每次调用都重置 `phase_started_at`，靠调用点保证「只在换阶段时调」，
    /// 这里把这条不变量钉在函数自己身上。
    ///
    /// 调用方必须正持有 `inner` 锁（各 `note_*` 方法的 `guard`）—— 与
    /// `flush_live` 同一纪律：改状态与「要不要回写」是一次原子判断。
    fn enter_phase(&self, guard: &mut TelemetrySnapshot, phase: LogPhase) {
        if guard.phase == phase {
            return;
        }
        guard.phase = phase;
        guard.phase_started_at = Some(logging::now_ms());
    }

    /// 把阶段推到 `phase` 并回写（**给转发链的少数几个精确时点用**）。
    ///
    /// 与各 `note_*` 内部那几处 `enter_phase` 的分工：那几处把阶段绑在
    /// 「已发生的事实」上（首帧到达、尝试起头、退避重试），本方法是给
    /// 「即将发生的事」用的 —— 目前唯一调用点是 `send_with_retry` 里
    /// **真正发出请求之前**那一下，它覆盖了同账号内的退避重发：那一轮
    /// 失败后阶段是「重试中」（睡着等退避），睡醒再发时必须回到「等待响应」，
    /// 否则重试期间的阶段会一直停在「重试中」直到首帧到达。
    pub fn note_phase(&self, phase: LogPhase) {
        let mut guard = self.lock();
        self.enter_phase(&mut guard, phase);
        self.flush_live(&guard);
    }

    /// 记一次「已向这个账号发出上游请求」（选路循环每轮调一次）。
    ///
    /// 账号取**最后一次**：429 轮换后真正承载请求的是最后那个账号，
    /// 报表里「这条请求算在谁头上」应该跟着实际承载者走。
    /// attempts 是累计值 —— 它要回答的是「这条请求换了几个账号才发出」。
    ///
    /// `provider` 同样是**最后一次**（多提供商轮询后真正承载请求的那家）。
    pub fn note_attempt(&self, account_id: Option<&str>, account_name: &str, provider: &str) {
        let mut guard = self.lock();
        guard.attempts += 1;
        guard.account_id = account_id.unwrap_or("").to_string();
        guard.account_name = account_name.to_string();
        if !provider.is_empty() {
            guard.provider = Some(provider.to_string());
        }
        // 在途回写：选路一确定，进行中行就该显示「谁在承载」
        self.flush_live(&guard);
    }

    pub fn note_quota_channel(&self, account: Option<&Value>) {
        self.lock().quota_channel = account.map(crate::server::core::limiter::counter::channel_of).unwrap_or_default();
    }

    /// 给**最后一条**尝试明细补上实际发出的体字节数。
    ///
    /// 与 [`Self::note_attempt_started`] 配对：起头那条只知「谁」，真值只有 provider
    /// 在发送点握着自己序列化出来的 `Vec<u8>` 时才知道（各家还会改写 body：抬
    /// `max_tokens`、塞会话 id），所以由 provider 侧回报，而不是编排层从 `Value` 估。
    ///
    /// 没有明细可挂（超上限被丢弃 / 压根没起头）时静默跳过 —— 这是观测，
    /// 不该反过来影响转发结论。
    pub fn note_attempt_body_bytes(&self, bytes: i64) {
        let mut guard = self.lock();
        if let Some(last) = guard.current_attempt_mut() {
            last.body_bytes = Some(bytes.max(0));
            self.flush_live(&guard);
        }
    }

    /// 给**最后一条**尝试明细追加一次内部退避重试（`send_with_retry` 每退避
    /// 一次调一次）。
    ///
    /// 空 `reason` 直接丢弃：明细里出现一条「重试了，但不知道为什么」，
    /// 对排障没有任何价值，还会让弹层多一行噪音。
    pub fn note_attempt_retry(&self, reason: &str, status: Option<i64>, delay_ms: u64) {
        let debug = self.capture().is_some();
        let reason = reason.trim();
        if reason.is_empty() {
            return;
        }
        let mut guard = self.lock();
        let Some(last) = guard.current_attempt_mut() else {
            return;
        };
        last.retries.push(RetryEvent {
            reason: if debug { reason.to_string() } else { truncate_chars(reason, MAX_ATTEMPT_RETRY_REASON_CHARS) },
            status,
            delay_ms,
        });
        // 阶段：同账号的退避重发 —— 接下来是睡一段时间再发，这段等待就是「重试中」
        // （下一轮 `note_attempt_started` 会把它推回「等待响应」）
        self.enter_phase(&mut guard, LogPhase::Retrying);
        // 在途回写：退避重试也是「进行中」期间就值得看到的进展
        // （前端那枚标签的判据含重试次数，见 `hasProcessFacts`）
        self.flush_live(&guard);
    }

    /// 上游排队中：记一次「内部等待」并把阶段切到「排队中」。
    ///
    /// ── 为什么不是 `note_attempt_retry` ─────────────────────────
    /// 那条走的是「重试中」阶段。排队不是失败重试，是**等上游放行**：阶段要
    /// 单独显示成「排队中」，用户才知道这次慢跟账号、限额、登录态都无关
    /// （文案里也写着这句）。重试事件本身照记 —— 详情弹窗的「内部重试」列表
    /// 与阶段是两套读数，排队同样该在那份明细里留下一行。
    pub fn note_queued(
        &self,
        reason: &str,
        status: Option<i64>,
        delay_ms: u64,
        wait_number: usize,
        max_waits: usize,
    ) {
        let debug = self.capture().is_some();
        let reason = reason.trim();
        if reason.is_empty() {
            return;
        }
        let mut guard = self.lock();
        let Some(last) = guard.current_attempt_mut() else {
            return;
        };
        // 序号与总数拼进原因里：详情弹窗那一列直接显示它，用户能看出
        // 「这是第几次排队等待、上限几次」
        let label = if max_waits > 0 {
            format!("{reason}（第 {wait_number}/{max_waits} 次）")
        } else {
            reason.to_string()
        };
        last.retries.push(RetryEvent {
            reason: if debug { label } else { truncate_chars(&label, MAX_ATTEMPT_RETRY_REASON_CHARS) },
            status,
            delay_ms,
        });
        self.enter_phase(&mut guard, LogPhase::Queued);
        self.flush_live(&guard);
    }

    /// 给**最后一条**尝试明细记一条提示（目前只有代理回退直连）。
    ///
    /// **首次为准**（与 `note_error` 同一取向）：同一轮里代理提示只会有一条，
    /// 后来的（例如重试时又算了一次）不该覆盖先到的那条根因。
    /// 空串不写，理由同 [`Self::note_attempt_retry`]。
    pub fn note_attempt_notice(&self, notice: &str) {
        let debug = self.capture().is_some();
        let notice = notice.trim();
        if notice.is_empty() {
            return;
        }
        let mut guard = self.lock();
        let Some(last) = guard.current_attempt_mut() else {
            return;
        };
        if last.notice.is_none() {
            last.notice = Some(if debug { notice.to_string() } else { truncate_chars(notice, MAX_ATTEMPT_NOTICE_CHARS) });
        }
        // 在途回写：提示同样是「这一轮怎么走的」的一部分
        self.flush_live(&guard);
    }

    /// 给**最后一条**尝试明细补上结果（成功也要调，此时 `status` 有值、
    /// `error` 传空）。
    ///
    /// 为什么只认最后一条：一次尝试的发送链是严格串行的（`send_with_retry`
    /// 内部循环 + 401 刷新重试都在同一轮里），响应回来时「最后起头的那条」
    /// 必然是它 —— 中间不会有别的尝试插进来。这条性质由 `provider_loop`
    /// 的 `'accounts` 循环保证（每轮只调一次起头，轮换才进入下一轮）。
    ///
    /// `status` 为 None 时（传输层失败）**不覆盖**已有的 None，`error` 照写：
    /// 于是那一条明细显示成「A → 失败（无状态码）：连接被拒绝」，
    /// 而不是显示一个凭空捏造的状态码。
    ///
    /// 调试请求保留完整错误，普通请求按列表摘要上限保存。
    pub fn finish_last_attempt(&self, status: Option<i64>, error: Option<&str>) {
        let debug = self.capture().is_some();
        let mut guard = self.lock();
        let Some(last) = guard.current_attempt_mut() else {
            return;
        };
        if status.is_some() {
            last.status = status;
        }
        if last.response_at.is_none() && error.is_none()
            && status.is_none_or(|status| (200..300).contains(&status))
        {
            last.response_at = Some(logging::now_ms());
        }
        // 空串摘要按「没有错误」处理（与 RequestEntry.error 的归一同一口径）：
        // 上游返回空 body 时 read_upstream_error 会给出空 message，
        // 那不该渲染成「失败：」后面跟着一片空白
        if let Some(text) = error.filter(|text| !text.is_empty()) {
            last.error = Some(if debug { text.to_string() } else { truncate_chars(text, MAX_ATTEMPT_ERROR_CHARS) });
        }
        // 阶段：这一轮**失败**收场 → 重试中（退避 / 换号前的过渡）。
        // 紧随其后的三种出口都由下一轮或收尾接管：换号走 `note_attempt_started`
        // （推回等待响应）、就地重发走 `note_attempt_retry`（留在重试中）、
        // 整个请求失败则收尾把阶段清空（终态行没有阶段）。
        // 成功收场（`error` 为空）不动阶段：那一轮的成功就是这条请求的结局，
        // 收尾紧接着就会到，中间没有可展示的过渡态。
        if last.error.is_some() || status.is_some_and(|status| !(200..300).contains(&status)) {
            last.finished_at = Some(logging::now_ms());
        }
        if last.error.is_some() {
            self.enter_phase(&mut guard, LogPhase::Retrying);
        }
        // 在途回写：这一轮定局（成功 / 失败）是「切换路径」上最值得看到的一步
        self.flush_live(&guard);
    }

    /// 记录「这一次尝试实际发给上游的模型名」（覆盖式，最后一次为准）。
    ///
    /// 与 `note_attempt` 的 provider 同一口径：429 换家后，最终值是最后一次
    /// 尝试发出的名字。空串不写（没有「清空」的语义 —— 槽位初始就是空串，
    /// 写空只可能来自调用方的兜底路径，那些路径不该覆盖已采到的值）。
    pub fn note_upstream_model(&self, model: &str) {
        if model.is_empty() {
            return;
        }
        let mut guard = self.lock();
        guard.upstream_model = model.to_string();
        if let Some(last) = guard.current_attempt_mut().filter(|last| last.finished_at.is_none()) { last.model = model.to_string(); }
        // 在途回写：上游真名是发送体定稿那一刻就确定的，比请求真正发出去还早
        // —— 模型列因此能在转发期间就显示「⬆️ 上游 / ⬇️ 下游」两行
        self.flush_live(&guard);
    }

    /// 记录「这一次尝试实际随请求发给上游的思考等级」（覆盖式，最后一次为准）。
    ///
    /// 与模型名同点采集；None 表示本次没有等级随行，清除上一次尝试的等级。
    pub fn note_upstream_reasoning(&self, level: Option<String>) {
        let level = level.filter(|text| !text.is_empty()).unwrap_or_default();
        let mut guard = self.lock();
        guard.upstream_reasoning = level.clone();
        if let Some(last) = guard.current_attempt_mut().filter(|last| last.finished_at.is_none()) { last.reasoning = level; }
        // 在途回写：与模型名同一时刻定稿，进行中行的模型列一并显示等级
        self.flush_live(&guard);
    }

    /// 上报一次 usage（覆盖式，最后一次为准；字段名兼容见 `extract_usage`）。
    ///
    /// **不做在途回写**：进行中行在前端不显示用量（`usageCell` 对进行中的行给空），
    /// 而 usage 通常只在上游最后一个 chunk 才出现 —— 收尾记账紧接着就会写它。
    pub fn report_usage(&self, usage: &Value) {
        let Some(tokens) = extract_usage(usage) else {
            return;
        };
        let mut guard = self.lock();
        guard.prompt_tokens = tokens.prompt;
        guard.completion_tokens = tokens.completion;
        guard.total_tokens = tokens.total;
        guard.cache_read_tokens = tokens.cache_read;
        guard.credit = tokens.credit;
    }

    /// 记一次「上游首帧已到达」。
    ///
    /// **首次为准**（与 note_error 相同、与 report_usage 相反）：首帧只有一次，
    /// 重跑计数只会把「第一个字节什么时候到的」改写成「后来某次调用的时刻」。
    /// 采集点在响应流上（Streaming 的 RecordingStream / 聚合的第一个 chunk），
    /// 同一条流上会被反复调用，幂等性由这里的 None 判断保证。
    ///
    /// ── 阶段为什么不受「首次为准」限制 ──────────────────────────
    /// 首帧事实只有一次（首响那一列），但阶段是**当前状态**：换号重试之后
    /// 新的一轮同样要回到「响应中」。所以阶段切换在 if **之外** ——
    /// 第二次及以后的首帧都该把阶段推回来（`enter_phase` 对同阶段是空操作，
    /// 所以流式路径每帧的调用不会造成重复写库）。
    pub fn note_first_frame(&self) {
        let mut guard = self.lock();
        if guard.first_response_at.is_none() {
            guard.first_response_at = Some(logging::now_ms());
        }
        // 阶段：首帧到了 → 响应中
        self.enter_phase(&mut guard, LogPhase::Streaming);
        // 在途回写：首响在转发期间就值得看（一条跑几分钟的流式请求，首响其实
        // 一秒内就有了）。**幂等**，所以每帧调到这里也只会触发一次回写 ——
        // 靠的是指纹闸（`flush_live`），不是这个 if
        self.flush_live(&guard);
    }

    /// 记一条中断 / 异常原因（成功请求不会被调用）。
    ///
    /// **首次为准**（与 usage 的「最后一次为准」相反）：先到的那条是根因，
    /// 之后到达的多半是它的连带现象（例如断流后客户端断开又触发一次收尾），
    /// 覆盖掉反而把根因埋了。
    ///
    /// **不做在途回写**：error 列一旦有值，前端就不再把这一行当「进行中」
    /// （判据见 `ui/requests-panel.js` 的 `isRunning`），而转发中途的失败往往只是
    /// 换号前的一次尝试失败 —— 那次失败由尝试明细如实带出，整行仍应是「进行中」。
    pub fn note_error(&self, message: &str) {
        if message.is_empty() {
            return;
        }
        let mut guard = self.lock();
        if guard.error.is_none() {
            guard.error = Some(message.to_string());
        }
    }

    /// 记一批敏感词命中（**并集累加**，见 `TelemetrySnapshot::sensitive_hits`）。
    ///
    /// 入参是脱敏模块的 `term_counts` 原样形态（`&[(String, usize)]`）——
    /// 不在这里重新排序或过滤：那份表已经是「按次数降序」的（`engine::Counter`
    /// 的口径），重排一遍只会引入第二套顺序定义。前端要展示的顺序就是它。
    ///
    /// 同一个词再次出现（跨家降级时另一家的词表也命中）时**合并计数**而不是
    /// 追加第二条：一份 `[{word, count}]` 里同一个词出现两次，没有任何读侧
    /// 会把它当成两件事，只会让「命中 3 次」这种读数变成「两条各 2 次」。
    ///
    /// 条数上界：规则集是硬编码的（7 条特征串 + 3 条正则 + 5 条改写，见
    /// `core::sanitize`），远小于改造前那份可维护词表，因此命中标签的条数
    /// 天然有界。这里不设额外的截断闸 —— 命中表落库是 JSON 文本，
    /// 前端展示时还会按次数降序截断（见 requests-panel 的弹层）。
    pub fn note_sensitive_hits(&self, term_counts: &[(String, usize)]) {
        if term_counts.is_empty() {
            return;
        }
        let mut guard = self.lock();
        for (term, count) in term_counts {
            let count = *count as i64;
            match guard
                .sensitive_hits
                .iter_mut()
                .find(|hit| hit.word == *term)
            {
                Some(hit) => hit.count += count,
                None => guard.sensitive_hits.push(SensitiveHit {
                    word: term.clone(),
                    count,
                }),
            }
        }
        // 在途回写：脱敏命中在发送体处理那一刻就有值，不必等收尾
        // （「敏」那枚标签因此能在转发期间就出现）
        self.flush_live(&guard);
    }

    /// 取当前快照（记账点收尾时调一次）
    pub fn snapshot(&self) -> TelemetrySnapshot {
        self.lock().clone()
    }

    /// 取锁；中毒时继续用内部值 —— 统计数据的完整性远不如「转发不因统计而崩」
    /// 重要（与 RequestStats / LogStore 同一取向）
    fn lock(&self) -> MutexGuard<'_, TelemetrySnapshot> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod usage_extraction {
    //! usage 旁路提取的字段口径（别名表与「第一个正数」的缘由见模块头部）。

    use serde_json::json;

    use super::*;

    /// 废值必须让位给下一个别名 —— **旧实现在这条上会红**。
    ///
    /// 一手形态：嵌套的 `prompt_tokens_details.cached_tokens` 恒 0、真值在顶层的
    /// `cache_read_input_tokens` 上。旧写法是 `.and_then(嵌套).or_else(顶层)`，
    /// 一命中就短路，于是整条请求的缓存用量记成 0；而 0 不是「缺值」而是
    /// 「这家缓存没生效」的读数（报表的命中率列直接归零），比少记一项更难查。
    #[test]
    fn a_zero_nested_value_yields_to_the_real_top_level_alias() {
        let usage = json!({
            "prompt_tokens": 1200,
            "completion_tokens": 300,
            "total_tokens": 1500,
            "prompt_tokens_details": { "cached_tokens": 0 },
            "cache_read_input_tokens": 4800,
        });
        let tokens = extract_usage(&usage).expect("对象就该出值");
        assert_eq!(4800, tokens.cache_read, "嵌套那份是废值，不该当「命中 0」用掉");
        assert_eq!(1200, tokens.prompt, "其余三项照常");
    }

    /// 字符串与浮点形态都要认（`"34"`、`12.0`）。
    ///
    /// 旧 `number_field` 只走 `as_i64` → `as_f64`，字符串一律 None；上游把 usage
    /// 里的数当文本吐出是有先例的（`protocol::commandcode_outbound` 归一前读的就是
    /// 那族驼峰/文本混合形态）。这里若整体落成 0，报表会显示「一次没花钱的请求」。
    #[test]
    fn string_and_float_shapes_are_read() {
        let usage = json!({
            "prompt_tokens": "34",
            "completion_tokens": 12.0,
            "input_tokens_details": { "cached_tokens": "7" },
        });
        let tokens = extract_usage(&usage).expect("对象就该出值");
        assert_eq!(34, tokens.prompt, "字符串数字要落数");
        assert_eq!(12, tokens.completion, "12.0 这类浮点截断成 12");
        assert_eq!(46, tokens.total, "total 缺值时按 prompt + completion 补");
        assert_eq!(7, tokens.cache_read, "驼峰/浮点之外的容器键也走同一张别名表");
    }

    /// 非正值（0 与负数）都当废值：不写进聚合，也不占住别名表。
    ///
    /// 三条各有分工：`completion_tokens: -5` 验证负数不会带着日聚合的加数往下压；
    /// `total_tokens: 0` 验证 0 不遮蔽 prompt + completion 的兜底；
    /// `cache_read_tokens: -12` 验证废值不冒充「命中」。若实现写成「夹成 0 但算有值」，
    /// 后两条照样绿（前两条本来就落 0），所以这里把「0 也不许遮蔽」一起钉住。
    #[test]
    fn non_positive_values_do_not_speak_and_do_not_mask() {
        let usage = json!({
            "prompt_tokens": 90,
            "completion_tokens": -5,
            "total_tokens": 0,
            "cache_read_tokens": -12,
        });
        let tokens = extract_usage(&usage).expect("对象就该出值");
        assert_eq!(0, tokens.completion, "负的输出去落 0，别把负数带进聚合");
        assert_eq!(90, tokens.total, "total 为 0 是废值，回落 prompt + completion");
        assert_eq!(0, tokens.cache_read, "负的命中同样是废值");

        // 负值必须让位：同一含义的另一项有真值时，读得到的是真值而不是 0
        let masked = json!({
            "prompt_tokens": 90,
            "completion_tokens": 10,
            "cache_read_tokens": -12,
            "cached_tokens": 33,
        });
        assert_eq!(33, extract_usage(&masked).unwrap().cache_read, "废值不许扮演「有值」");
    }

    /// 反空跑：两项**都是正数**时按别名表的顺序取第一项，不是求和也不是取末位。
    ///
    /// 若实现写成「把所有别名相加」或「后面的别名覆盖前面的」，前面几条用例照样绿
    /// （它们的第二项都是 0 或缺失），只有这条能把它钉住。
    /// 表序把 OpenAI 标准名 `cached_tokens` 排在 Anthropic 的 `cache_read_tokens` 之前，
    /// 所以这一发赢的是前者 —— 顺序本身是刻意的，但这里要断言的是「只取一项」。
    #[test]
    fn two_positive_aliases_do_not_add_up() {
        let usage = json!({
            "prompt_tokens": 100,
            "input_tokens": 999,
            "cache_read_tokens": 40,
            "cached_tokens": 77,
        });
        let tokens = extract_usage(&usage).expect("对象就该出值");
        assert_eq!(100, tokens.prompt, "标准写法在前，别把别名加上去");
        assert_eq!(77, tokens.cache_read, "按表序取第一项正的，不相加");
    }

    /// 驼峰形态与标准形态同一张表；真值不会被同容器里的废值挡掉。
    ///
    /// `totalTokens` 故意写成与 `inputTokens + outputTokens` **不相等**的数：
    /// 若两值相同，这条断言就分不清「读到了驼峰总量」和「走了 prompt+completion 兜底」，
    /// 别名表整段没被覆盖也算绿。
    #[test]
    fn camel_case_aliases_and_container_precedence() {
        let usage = json!({
            "inputTokens": 500,
            "outputTokens": 20,
            "totalTokens": 610,
            "input_tokens_details": { "cached_tokens": 0 },
            "prompt_tokens_details": { "cacheReadTokens": 300 },
        });
        let tokens = extract_usage(&usage).expect("对象就该出值");
        assert_eq!(500, tokens.prompt, "驼峰的 inputTokens 就是 prompt");
        assert_eq!(20, tokens.completion);
        assert_eq!(610, tokens.total, "读的是 totalTokens，不是兜底加出来的 520");
        assert_eq!(300, tokens.cache_read, "容器内同样按「第一个正数」挑");
    }

    /// 非对象仍是一个都不记（None 由调用侧直接跳过上报）。
    #[test]
    fn a_non_object_usage_reports_nothing() {
        assert!(extract_usage(&json!(null)).is_none());
        assert!(extract_usage(&json!("34")).is_none());
        assert!(extract_usage(&json!([1, 2])).is_none());
        // 空对象是「对象但一个字段都没有」：出值、四项全 0，与 None 的区别在于
        // 调用侧不会覆盖掉先前那一帧已经采到的数（report_usage 只在 Some 时写）
        assert_eq!(
            Some(UsageTokens::default()),
            extract_usage(&json!({})),
            "空对象也要出值：None 与「全 0」在调用侧是两条不同分支"
        );
    }
}

#[cfg(test)]
mod body_bytes_observation {
    //! 发出量这条观测的采集侧（存储侧的透传见 `request_stats::record` 与 `api::pipeline`）。

    use super::*;

    /// 回报只落**最后一条**明细；没起头时不许凭空造一条。
    ///
    /// 后半个断言是反空跑：若这个方法在明细为空时自己补一条，或写成「只有第一条能拿到」，
    /// 前半个断言照样绿。真实路径里本地就拒的请求根本没有明细项，必须把那一趟也钉住。
    #[test]
    fn the_body_bytes_observation_lands_on_the_latest_attempt() {
        let telemetry = RequestTelemetry::new();
        telemetry.note_attempt_body_bytes(123);
        assert!(
            telemetry.snapshot().attempts_detail.is_empty(),
            "没有起头时不该凭空多一条明细（本地就拒的请求根本没有发出量）"
        );
        telemetry.note_attempt_started("codearts", "GT-a");
        telemetry.note_attempt_body_bytes(6_372_244);
        telemetry.note_attempt_started("workbuddy", "wb-1");
        telemetry.note_attempt_body_bytes(1_024);
        let details = telemetry.snapshot().attempts_detail;
        assert_eq!(2, details.len());
        assert_eq!(
            Some(6_372_244),
            details[0].body_bytes,
            "第一轮记下的是第一轮真实发出去的字节（串行就等于假归因）"
        );
        assert_eq!(Some(1_024), details[1].body_bytes);
        // 负数只可能来自手改的库：夹成 0，展示层不为一位数写分支
        telemetry.note_attempt_body_bytes(-5);
        assert_eq!(Some(0), telemetry.snapshot().attempts_detail[1].body_bytes);
    }
}
