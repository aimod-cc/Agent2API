//! 账号路由的**运行期状态**：轮转游标 / 会话粘性 / 失败退避 / 限流垫底 / 熔断。
//!
//! 对应账号页「路由」区域的五样设置在**转发时**要记的那点状态。全部是
//! **进程内存**（不落盘、不跨进程）：这些判定回答的是「接下来这一会儿请求
//! 怎么分」，重启清零是正确语义（与 magpie 的 Sink / resting 同口径）；
//! 真正要跨重启保留的读数是账号记录里的 `rateLimits`（限额冷却，已有机制）。
//!
//! ── 与选路纯函数的分工 ────────────────────────────────────────
//! `routing.rs` 只做**纯判定**（候选过滤 + 排序 + 原因句，输入即输出）；
//! 本模块是那套判定要读 / 要写的**状态**。于是：
//!   - 选路时：从本模块取一份快照（克隆小 Map），传进纯函数；
//!   - 请求有了结果时：调本模块的 mark_* 更新状态。
//! 「决策」与「状态」分开后，queue 端点（只看不写）与转发路径（看 + 写）
//! 读到的口径永远一致 —— 它们调的是同一组纯函数。
//!
//! ── 粘性键为什么是「模型 + 首条用户消息」的哈希 ────────────────
//! 网关不做会话管理（那是客户端的事），拿不到 magpie 那种 session id。
//! 但「同一个对话」在下游请求里的**稳定特征**是：模型不变、首条用户消息
//! 不变（后续轮次只是在尾部追加）。哈希它得到会话键，进程内 DefaultHasher
//! 即可（不落盘、不需要抗碰撞的密码学强度，键空间是本进程自己产生的串）。
//!
//! ── 退避与熔断的分工（两个都开时互不吞并）─────────────────────
//!   - 退避（backoff）：**单次失败**后的短期休息（15 分钟起，连续失败翻倍，
//!     上限 1 小时）。目的是「让这次失败先凉一会儿」，惩罚轻、恢复自动；
//!   - 熔断（breaker）：**连续失败到阈值**才打开，且要连续成功若干次才关。
//!     目的是「这个账号现在整体不对劲，别再一条条地试」。
//! 两者都以账号为键、都影响排序（垫底），但判定独立：退避只影响位次，
//! 熔断打开期间直接跳过（除非全场只剩它是开的 —— 那时仍按「试一次」放行，
//! 与并发挤占同一取舍：宁可打一次试试，也不把请求直接拒掉）。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use axum::http::HeaderMap;

/// 失败退避的起步时长（15 分钟）与上限（1 小时）。
pub(super) const BACKOFF_BASE_MS: i64 = 15 * 60 * 1000;
const BACKOFF_MAX_MS: i64 = 60 * 60 * 1000;

/// 限流垫底的时长（30 分钟）。
///
/// ── 为什么需要它（上游原实现没有，2026-10-13 修）─────────────────
/// 上游的 `sink` 存的是「沉底时刻」但**从不看过没过期**（`snapshot` 原样克隆
/// 整张表），而唯一的解除路径是 `sink_clear` —— 它只在**该账号成功转发一次**
/// 之后才被调用。于是形成自锁：429 → 垫底 → 排到队尾 → 轮不到它 → 永远没机会
/// 成功 → 永远解除不了。实测现场：国际版 11 个账号在一天内被逐个撞完 429 后
/// 全部永久沉底，「家之间按优先级」于是每次都把整个家排到最后，家内轮询再也
/// 轮不到它（详见 `projects/agent2api/deploy-to-03` 与那次排查）。
///
/// ── 为什么取 30 分钟 ────────────────────────────────────────
/// 垫底的原意是「让它先凉一会儿、别反复撞」（`routing.rs` 的原因句写的是
/// 「轮一圈才回来」），而不是永久流放。30 分钟与另两层的量级相称：
///   - 冷却：上游给的恢复时刻（WorkBuddy 常见 10 分钟窗口）；
///   - 退避：15 分钟起步、翻倍封顶 1 小时；
///   - 垫底：固定 30 分钟（不翻倍 —— 它标记的是「疑似共享池被别人抢」，
///     不是「这个账号本身有问题」，重犯不该加重）。
/// 到点自动回到队列，与「冷却到点自动回来」同一口径。
const SINK_TTL_MS: i64 = 30 * 60 * 1000;

/// 一次退避记录：休息到 `until`；`strikes` 是连续失败笔数（决定翻倍档）。
#[derive(Clone, Copy, Debug)]
struct BackoffEntry {
    until: i64,
    strikes: u32,
}

/// 一个账号的熔断状态。
#[derive(Clone, Copy, Debug, Default)]
struct BreakerEntry {
    /// 当前连续失败数（成功清零）
    failures: u32,
    /// 熔断打开到什么时候（毫秒；0 = 未打开）
    open_until: i64,
    /// 打开期过后（半开）已连续成功多少次
    half_open_successes: u32,
}

/// 全部路由运行期状态。挂在 `UpstreamService` 上（与 `Connections` 同款）。
#[derive(Default)]
pub(super) struct RouteState {
    /// 轮转游标：模型键（请求名）→ 下一个起始位。
    ///
    /// 为什么按模型分键：不同模型的可用账号集合不同，共用一个游标会让
    /// 「A 模型的第 2 位」错误地决定 B 模型从谁开始。空模型（客户端没点名）
    /// 用空串键，全局一条。
    cursors: Mutex<HashMap<String, usize>>,
    /// 会话粘性：会话键 → (账号 id, 最近一次使用时刻)
    sticky: Mutex<HashMap<String, (String, i64)>>,
    /// 失败退避：账号 id → 休息记录
    backoff: Mutex<HashMap<String, BackoffEntry>>,
    /// 限流垫底：账号 id → 沉底时刻（还有额度时被 429 的；超 [`SINK_TTL_MS`] 自动失效）
    sink: Mutex<HashMap<String, i64>>,
    /// 熔断：账号 id → 状态
    breaker: Mutex<HashMap<String, BreakerEntry>>,
}

/// 粘性 / 垫底 / 退避的快照（选路纯函数的输入）。
pub(super) struct RouteSnapshot {
    /// 会话键命中的账号 id（没有 = None）
    pub sticky_hit: Option<String>,
    /// 沉底账号（值 = 沉底时刻，仅排障展示用）
    pub sunk: HashMap<String, i64>,
    /// 休息中的账号（值 = 休息截止时刻）
    pub resting: HashMap<String, i64>,
    /// 熔断打开的账号
    pub open: std::collections::HashSet<String>,
}

impl RouteState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 显式标识的**长度上限**（字符）。
    ///
    /// 客户端可以自选这个值，而它会成为进程内存里粘性表的一条键（表只在每次查表时
    /// 按 TTL 清理），所以长度不该由客户端决定 —— 真实会话标识都是 UUID 或十六进制
    /// 串，256 足够宽；超过就当「没有显式标识」回落到内容哈希，是安全降级。
    const EXPLICIT_ID_MAX_CHARS: usize = 256;

    /// 会话键：**显式标识优先**，都没有才回落到「模型 + 首条用户消息」的内容哈希。
    ///
    /// ── 显式标识从哪来（顺序即优先级）──────────────────────────
    ///   1. 头 `AH-Thread-Id` —— AxonHub 的**会话级**标识（一个 thread = 一整段
    ///      多轮对话）。同族的 `AH-Trace-Id` **不能**拿来当会话键：它的粒度是
    ///      「单条用户消息 + 它触发的所有请求」（AxonHub 的层级是 Thread →
    ///      Trace → Request），用它会让每来一轮新消息就换一个键、粘性逐轮断裂。
    ///   2. OpenCode 系头：`X-Opencode-Session` → `X-Session-Id` → `X-Session-Affinity`
    ///   3. body 的 `prompt_cache_key` —— **经翻译网关接入时唯一能拿到的那条**
    ///      （AxonHub 出站时只发 Content-Type / Accept，会话 id 它写进 body）
    ///   4. Claude Code 的 `metadata.user_id`（v2 JSON 与 legacy 两种形状）
    ///   5. Codex：头 `Session_id` / `Session-Id`，或 `X-Codex-Turn-Metadata` 里的 `session_id`
    ///
    /// ── 为什么显式标识优先，而不是只留内容哈希 ──────────────────
    /// 只按「模型 + 首条用户消息」哈希的话覆盖面更广（任何带 messages 的请求
    /// 都能粘），但那是**推断** —— 首条消息一旦被客户端改写（压缩、编辑、
    /// 重放）就换了一个会话。客户端明确给了会话 id 时那个值是权威的，直接用它
    /// 才不会被一次消息改写抹掉。两者是「精确」与「兜底」的关系而非替代：
    /// 没带标识的老客户端仍走内容哈希，行为与上游逐字相同。
    ///
    /// 空串 = 既没有显式标识、也没有可用的会话特征（没有 messages / 没有用户
    /// 消息），调用方按「不粘」处理。
    pub(super) fn conversation_key(headers: &HeaderMap, body: &serde_json::Value) -> String {
        // 长度闸加在这一个收口点上，五个来源一次覆盖（来源内部各自校验形状会
        // 变成五份判据）。超限当「没有显式标识」，回落到内容哈希。
        if let Some(explicit) = Self::explicit_conversation_id(headers, body)
            .filter(|value| value.chars().count() <= Self::EXPLICIT_ID_MAX_CHARS)
        {
            // 带前缀：显式值是客户端给的任意串，与下面 16 位十六进制的内容哈希
            // 分处两个命名空间，免得某个客户端的 id 恰好长成哈希的样子。
            return format!("id:{explicit}");
        }
        Self::content_conversation_key(body)
    }

    /// 内容哈希兜底（上游原实现，逐字保留）：模型 + 首条用户消息（截断 4KB）。
    fn content_conversation_key(body: &serde_json::Value) -> String {
        let model = body.get("model").and_then(serde_json::Value::as_str).unwrap_or("");
        let first_user = body
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .and_then(|messages| {
                messages.iter().find(|message| {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                })
            })
            .map(|message| message.get("content").cloned().unwrap_or(serde_json::Value::Null));
        let Some(content) = first_user else {
            return String::new();
        };
        let material = match &content {
            serde_json::Value::String(text) => {
                let end = text
                    .char_indices()
                    .take_while(|(index, _)| *index < 4096)
                    .last()
                    .map(|(index, ch)| index + ch.len_utf8())
                    .unwrap_or(0);
                text[..end].to_string()
            }
            other => other.to_string(),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hasher);
        0x1fu8.hash(&mut hasher);
        material.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// 显式会话标识的提取链（顺序即优先级，见 [`Self::conversation_key`]）。
    ///
    /// **端点无关**：同一套标识在不同协议端点上语义相同，按路径收窄反而会在
    /// 客户端换端点时丢掉同一个会话（上游的内容哈希同样是端点无关的）。
    fn explicit_conversation_id(headers: &HeaderMap, body: &serde_json::Value) -> Option<String> {
        // ① AxonHub 的**会话级**头。⚠️ 别换成 `AH-Trace-Id`：那个的粒度是
        // 「单条用户消息 + 它触发的所有请求」（AxonHub 的层级是 Thread → Trace
        // → Request），一个客户端每轮都会给它一个新值，拿它当会话键会让粘性
        // 逐轮断裂 —— 比下面那条内容哈希（跨轮稳定）还差。上游原实现读的正是
        // Trace，2026-10-14 订正为 Thread。
        if let Some(value) = Self::header_text(headers, "ah-thread-id") {
            return Some(value);
        }
        // ② OpenCode 系（三个头按新旧顺序）
        for name in ["x-opencode-session", "x-session-id", "x-session-affinity"] {
            if let Some(value) = Self::header_text(headers, name) {
                return Some(value);
            }
        }
        // ③ OpenAI Chat 的 prompt_cache_key：**翻译网关这条路唯一的会话标识**。
        //
        // 典型场景：客户端 → AxonHub（Anthropic 入口）→ 翻译成 Chat 格式 →
        // 本网关的 Chat 入口。AxonHub 出站时只发 Content-Type / Accept 两个头
        // （`llm/transformer/openai/outbound.go`），会话 id 它写进 **body 的
        // `prompt_cache_key`**（`outbound_convert.go` 把共享会话 id 填进
        // `req.PromptCacheKey`）。上面两条全落空时读它，粘性才接得上。
        //
        // 值取纯字符串、非空即用：`prompt_cache_key` 本就是「缓存命名空间」语义
        // （同一会话稳定复用），拿它当会话标识与该字段的用途一致；它也没有
        // 历史包袱，任何非空值都是调用方自己给的稳定键，不需要形状校验。
        //
        // ⚠️ 不校验形状就等于要求调用方真的按「一会话一键」用它：把它当**固定
        // 常量**发的客户端（所有会话共用一个值）会让整个客户端钉死在同一个账号上
        // —— 粘性置顶修好之后这会真的生效，等于把轮转关掉。长度另有一道闸
        // （见 [`Self::EXPLICIT_ID_MAX_CHARS`]）：它是这条链里唯一没有形状校验的
        // 来源，而键会长住进内存。
        if let Some(value) = body
            .get("prompt_cache_key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
        // ④ Claude Code：`metadata.user_id`（两种可辨认的形状，见
        // [`Self::parse_claude_code_user_id`]）。解析不出就当没有 —— 宁可没有
        // 粘性，也不要把一个不认识的串当会话 id（别的客户端往这个字段塞了
        // 别的东西时，编出来的 id 会把不相干的请求粘到一起）。
        if let Some(value) = body
            .get("metadata")
            .and_then(|metadata| metadata.get("user_id"))
            .and_then(serde_json::Value::as_str)
            .and_then(Self::parse_claude_code_user_id)
        {
            return Some(value);
        }
        // ⑤ Codex：会话头，或 turn metadata 里的 session_id
        for name in ["session_id", "session-id"] {
            if let Some(value) = Self::header_text(headers, name) {
                return Some(value);
            }
        }
        if let Some(metadata) = headers
            .get("x-codex-turn-metadata")
            .and_then(|value| value.to_str().ok())
        {
            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(metadata) {
                if let Some(session) = payload
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(session.to_string());
                }
            }
        }
        None
    }

    /// 头值读取：缺失 / 非 ASCII 一律当「没有」（不做 URL 解码等额外处理）。
    ///
    /// `HeaderMap::get` 大小写不敏感（HTTP 头名不区分大小写），用小写名查询
    /// 即可。**非 ASCII 头值（中文、emoji）会让 `to_str` 失败、该头被当作
    /// 「没有」**——这是 HTTP 规范的限制（头值是 ISO-8859-1 字节串），不是
    /// 这里的选择：真实客户端发的会话标识都是 UUID 或十六进制串。手工用 curl
    /// 测粘性时要留意 —— `-H "AH-Thread-Id: 中文值"` 发出去的请求不带可用
    /// 标识，看起来就像「粘性不生效」。
    fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    /// 从 Claude Code 的 `metadata.user_id` 里取 `session_id`（照抄 AxonHub
    /// 的 `claudecode.ParseUserID`）。
    ///
    /// 两种格式：
    ///   - **v2（>=2.1.78）**：JSON 文本
    ///     `{"device_id":"…","account_uuid":"…","session_id":"…"}`；
    ///   - **legacy**：`user_<64位十六进制>_account__session_<uuid>`。
    fn parse_claude_code_user_id(raw: &str) -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // v2：JSON 形态
        if raw.starts_with('{') {
            let payload: serde_json::Value = serde_json::from_str(raw).ok()?;
            let session = payload.get("session_id").and_then(serde_json::Value::as_str)?;
            let session = session.trim();
            return if session.is_empty() { None } else { Some(session.to_string()) };
        }
        // legacy：user_<64hex>_account__session_<uuid>
        let rest = raw.strip_prefix("user_")?;
        let (device, tail) = rest.split_once("_account__session_")?;
        let is_hex64 = device.len() == 64 && device.chars().all(|c| c.is_ascii_hexdigit());
        if is_hex64 && Self::is_uuid_shape(tail) {
            return Some(tail.to_string());
        }
        None
    }

    /// `8-4-4-4-12` 的十六进制 UUID 形态（不校验版本位 —— 正则原版也没校验）
    fn is_uuid_shape(text: &str) -> bool {
        let parts: Vec<&str> = text.split('-').collect();
        let lengths = [8, 4, 4, 4, 12];
        parts.len() == lengths.len()
            && parts
                .iter()
                .zip(lengths)
                .all(|(part, len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
    }

    /// 取粘性命中的账号（命中即刷新最近使用时刻；过期的顺手清掉）。
    pub(super) fn sticky_lookup(&self, key: &str, now: i64, ttl_ms: i64) -> Option<String> {
        if key.is_empty() || ttl_ms <= 0 {
            return None;
        }
        let mut table = self.sticky.lock().ok()?;
        // 顺手清过期项：表规模 = 活跃会话数，一次线性扫描可接受；
        // 不在后台起清扫线程 —— 网关没有「会话结束」的信号，惰性清理最省事。
        table.retain(|_, (_, last_seen)| now - *last_seen <= ttl_ms * 8);
        let entry = table.get_mut(key)?;
        if now - entry.1 > ttl_ms {
            table.remove(key);
            return None;
        }
        entry.1 = now;
        Some(entry.0.clone())
    }

    /// 绑定会话 → 账号（成功转发后调用；只对新账号改写）
    pub(super) fn sticky_bind(&self, key: &str, account_id: &str, now: i64) {
        if key.is_empty() || account_id.is_empty() {
            return;
        }
        if let Ok(mut table) = self.sticky.lock() {
            table.insert(key.to_string(), (account_id.to_string(), now));
        }
    }

    /// 取轮转游标并前进一格，返回**计数** —— 取模交给调用方做。
    ///
    /// ── 为什么这里不取模（2026-10-14 修）────────────────────────
    /// 本函数能拿到的候选数（`plan_route` 之前那份）与调用方环绕时用的长度
    /// （过滤掉禁用 / 冷却 / 并发满 / 无凭证之后的）**不是同一把尺子**。旧写法
    /// 按前者取模、又把取模结果回写，等于让位次在两个模数之间反复换算：某家
    /// 3 人里有 1 人冷却时，位次序列 `0,1,2,0,1,2…` 按 2 环绕就成了
    /// `0,1,0,0,1,0…` —— **同一个账号被连续选中两次**。
    ///
    /// 只存计数后，取模只发生一次、只在一把尺子上，候选数怎么变都是均匀轮转；
    /// 候选数恒定的场景下序列与旧实现逐位相同（`0,1,…,len-1,0,…`）。
    pub(super) fn rotate_take(&self, model_key: &str) -> usize {
        let Ok(mut table) = self.cursors.lock() else {
            return 0;
        };
        let cursor = table.entry(model_key.to_string()).or_insert(0);
        let counter = *cursor;
        // 只增不减的计数：用不回绕的加法，免得 debug 构建在理论上限处 panic。
        *cursor = counter.wrapping_add(1);
        counter
    }

    /// 取「每个家各自的轮转计数」（家内轮询策略用），取模同样交给调用方。
    ///
    /// 与 [`Self::rotate_take`] 共用同一张游标表，键多加一维 provider ——
    /// 「同一个模型下，每个家自己一条环」。家之间不轮转（先后由优先级定），
    /// 所以游标**必须每家一份**：全局共用一个会让 A 家轮到的位次错误地决定
    /// B 家从谁开始，那正是「家内轮询」要避免的。
    ///
    /// `sizes` 是各家的候选数（provider id → 账号数），这里只用来产出键名 ——
    /// 取模不在这一层做（理由见 [`Self::rotate_take`]），调用方按各家**过滤后**
    /// 的实际成员数取模；`len == 0` 的家跳过（没有候选，计划里也不会有它的账号）。
    pub(super) fn rotate_take_per_provider(
        &self,
        model_key: &str,
        sizes: &[(String, usize)],
    ) -> HashMap<String, usize> {
        let Ok(mut table) = self.cursors.lock() else {
            return HashMap::new();
        };
        let mut counters = HashMap::new();
        for (provider, len) in sizes {
            if *len == 0 {
                continue;
            }
            // 键用 `\u{1}` 分隔而不是可读的分隔符：模型名是客户端给的，什么
            // 字符都可能出现，一个不会出现在模型名里的控制字符才不会让两把
            // 不同的键撞成同一把。
            let key = format!("{model_key}\u{1}{provider}");
            let cursor = table.entry(key).or_insert(0);
            let counter = *cursor;
            *cursor = counter.wrapping_add(1);
            counters.insert(provider.clone(), counter);
        }
        counters
    }

    /// 记一笔失败退避（连续失败翻倍，上限 1 小时），返回休息截止时刻。
    pub(super) fn backoff_mark(&self, account_id: &str, now: i64) -> i64 {
        let Ok(mut table) = self.backoff.lock() else {
            return 0;
        };
        let entry = table.entry(account_id.to_string()).or_insert(BackoffEntry {
            until: 0,
            strikes: 0,
        });
        entry.strikes = entry.strikes.saturating_add(1);
        let factor = 1i64 << entry.strikes.saturating_sub(1).min(3); // 1x / 2x / 4x / 8x
        let wait = (BACKOFF_BASE_MS * factor).min(BACKOFF_MAX_MS);
        entry.until = now + wait;
        entry.until
    }

    /// 成功一次 → 清退避记录（连续失败的链子断了）。
    pub(super) fn backoff_clear(&self, account_id: &str) {
        if let Ok(mut table) = self.backoff.lock() {
            table.remove(account_id);
        }
    }

    /// 记一笔垫底（还有额度时被 429 的账号）。
    pub(super) fn sink_mark(&self, account_id: &str, now: i64) {
        if let Ok(mut table) = self.sink.lock() {
            table.insert(account_id.to_string(), now);
        }
    }

    /// 成功一次 → 从垫底里放出来（它证明自己能干活）。
    pub(super) fn sink_clear(&self, account_id: &str) {
        if let Ok(mut table) = self.sink.lock() {
            table.remove(account_id);
        }
    }

    /// 记一笔失败；返回熔断是否**就此打开**（用于打日志）。
    pub(super) fn breaker_failure(
        &self,
        account_id: &str,
        threshold: i64,
        timeout_seconds: i64,
        now: i64,
    ) -> bool {
        let Ok(mut table) = self.breaker.lock() else {
            return false;
        };
        let entry = table.entry(account_id.to_string()).or_default();
        let was_open = entry.open_until > now;
        entry.failures = entry.failures.saturating_add(1);
        entry.half_open_successes = 0;
        if !was_open && entry.failures as i64 >= threshold.max(1) {
            entry.open_until = now + timeout_seconds.max(0) * 1000;
            return true;
        }
        // 半开期再失败：重新打开
        if entry.open_until > 0 && entry.open_until <= now {
            entry.open_until = now + timeout_seconds.max(0) * 1000;
            return true;
        }
        false
    }

    /// 成功一次 → 熔断的恢复计数（连续成功到阈值才真正关闭）。
    pub(super) fn breaker_success(&self, account_id: &str, success_threshold: i64) {
        let Ok(mut table) = self.breaker.lock() else {
            return;
        };
        let Some(entry) = table.get_mut(account_id) else {
            return;
        };
        entry.failures = 0;
        if entry.open_until > 0 {
            entry.half_open_successes = entry.half_open_successes.saturating_add(1);
            if entry.half_open_successes as i64 >= success_threshold.max(1) {
                table.remove(account_id);
            }
        }
    }

    /// 选路快照：粘性命中 + 沉底 / 休息 / 熔断三张表（一次取齐）。
    pub(super) fn snapshot(&self, conv_key: &str, now: i64, ttl_ms: i64) -> RouteSnapshot {
        let sticky_hit = self.sticky_lookup(conv_key, now, ttl_ms);
        // 垫底按 [`SINK_TTL_MS`] 过滤 —— 与下面 resting / open 同款。
        // 上游原实现是整张表原样克隆（没有时间判定），于是垫底永不失效；
        // 唯一的解除路径是「该账号成功转发一次」，而它正排在队尾 —— 自锁。
        let sunk = self
            .sink
            .lock()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, at)| now - **at < SINK_TTL_MS)
                    .map(|(id, at)| (id.clone(), *at))
                    .collect()
            })
            .unwrap_or_default();
        let resting = self
            .backoff
            .lock()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, entry)| entry.until > now)
                    .map(|(id, entry)| (id.clone(), entry.until))
                    .collect()
            })
            .unwrap_or_default();
        let open = self
            .breaker
            .lock()
            .map(|table| {
                table
                    .iter()
                    .filter(|(_, entry)| entry.open_until > now)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        RouteSnapshot {
            sticky_hit,
            sunk,
            resting,
            open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};
    use serde_json::json;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("合法头名"),
                HeaderValue::from_str(value).expect("合法头值"),
            );
        }
        map
    }

    fn chat_body() -> serde_json::Value {
        json!({ "model": "m", "messages": [{ "role": "user", "content": "hello" }] })
    }

    /// 显式标识优先于内容哈希：同一个 body，只差一个头，必须落在两个键上。
    #[test]
    fn an_explicit_id_beats_the_content_hash() {
        let body = chat_body();
        let keyed = RouteState::conversation_key(&headers(&[("ah-thread-id", "thread-abc")]), &body);
        let hashed = RouteState::conversation_key(&HeaderMap::new(), &body);
        assert_eq!(keyed, "id:thread-abc");
        assert_ne!(keyed, hashed);
        // 没带标识时走的仍是上游那条内容哈希（16 位十六进制）
        assert_eq!(hashed.len(), 16);
        assert!(hashed.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// `AH-Trace-Id` 是「单条用户消息」级，**不是**会话级 —— 不能被当会话键。
    ///
    /// AxonHub 的层级是 Thread（多轮会话）→ Trace（每条用户消息一个）→ Request。
    /// 拿 Trace 当会话键，客户端每来一轮新消息就换一个键 ⇒ 粘性逐轮断裂，比
    /// 内容哈希（首条用户消息不变、跨轮稳定）还差。上游原实现读的正是 Trace，
    /// 2026-10-14 订正为 Thread 并把 Trace 从链里去掉。
    #[test]
    fn an_axonhub_trace_id_is_not_a_session_key() {
        let body = chat_body();
        // 只有 Trace 时落回内容哈希，而不是 id:at-abc
        let key = RouteState::conversation_key(&headers(&[("ah-trace-id", "at-abc")]), &body);
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        // 会话级的 Thread 头则被采纳
        assert_eq!(
            RouteState::conversation_key(&headers(&[("ah-thread-id", "thread-abc")]), &body),
            "id:thread-abc"
        );
    }

    /// 五段提取链逐段验证（顺序即优先级）。
    #[test]
    fn every_source_of_an_explicit_id_is_recognised() {
        let body = chat_body();
        let cases: Vec<(HeaderMap, serde_json::Value, &str)> = vec![
            (headers(&[("ah-thread-id", "thread-abc")]), body.clone(), "id:thread-abc"),
            (headers(&[("x-opencode-session", "sess-opencode")]), body.clone(), "id:sess-opencode"),
            (headers(&[("x-session-id", "sess-x")]), body.clone(), "id:sess-x"),
            (headers(&[("x-session-affinity", "sess-affinity")]), body.clone(), "id:sess-affinity"),
            (headers(&[("session-id", "sess-codex")]), body.clone(), "id:sess-codex"),
            (
                headers(&[("x-codex-turn-metadata", "{\"session_id\":\"sess-turn\"}")]),
                body.clone(),
                "id:sess-turn",
            ),
            (HeaderMap::new(), json!({ "prompt_cache_key": "sess-cache" }), "id:sess-cache"),
            (
                HeaderMap::new(),
                json!({ "metadata": { "user_id": "{\"session_id\":\"sess-claude\"}" } }),
                "id:sess-claude",
            ),
        ];
        for (headers, body, expected) in &cases {
            assert_eq!(RouteState::conversation_key(headers, body), *expected);
        }
    }

    /// 形状不对的 `metadata.user_id` 不该被当成会话 id —— 宁可没有粘性。
    #[test]
    fn a_malformed_claude_user_id_falls_back_to_the_content_hash() {
        let body = json!({
            "model": "m",
            "messages": [{ "role": "user", "content": "hello" }],
            "metadata": { "user_id": "随便一个串" },
        });
        let key = RouteState::conversation_key(&HeaderMap::new(), &body);
        assert!(!key.starts_with("id:"));
        assert_eq!(key.len(), 16);
    }

    /// 游标存的是**单调计数**，取模由调用方做 —— 两个模数不一致时也不会重复选。
    ///
    /// 钉的是 2026-10-14 修的一处缺陷：旧实现按「调用方传进来的候选数」取模、
    /// 又把取模结果回写，而环绕用的是**过滤后**的长度。候选 3 个、其中 1 个
    /// 被冷却（真正参与环绕的只有 2 个）时，游标值在 `0,1,2` 之间回绕、再按 2
    /// 定位次就成了 `0,1,0,0,1,0` —— 第 3、4 次连续选中同一个账号。
    #[test]
    fn rotation_counters_are_not_wrapped_inside_the_state() {
        let state = RouteState::new();
        // 候选 3 个、可见 2 个：每次取计数，按可见的 2 定位次
        let positions: Vec<usize> = (0..6)
            .map(|_| state.rotate_take("m") % 2)
            .collect();
        // 计数 0..5 取模 = 0,1,0,1,0,1 —— 严格轮流（旧实现会是 0,1,0,0,1,0）
        assert_eq!(positions, vec![0, 1, 0, 1, 0, 1]);
    }

    /// 家内游标：每家一条独立计数，互不影响；模型不同则另算一套。
    #[test]
    fn family_counters_advance_per_family_and_per_model() {
        let state = RouteState::new();
        let sizes = vec![("A".to_string(), 3usize), ("B".to_string(), 2usize)];
        let first = state.rotate_take_per_provider("m", &sizes);
        let second = state.rotate_take_per_provider("m", &sizes);
        // 返回的是计数本身（不回绕），且两家各走各的
        assert_eq!(first.get("A"), Some(&0));
        assert_eq!(first.get("B"), Some(&0));
        assert_eq!(second.get("A"), Some(&1));
        assert_eq!(second.get("B"), Some(&1));
        // 换模型 → 另一套键
        assert_eq!(state.rotate_take_per_provider("m2", &sizes).get("A"), Some(&0));
    }

    /// 垫底必须**到点自动失效**。
    ///
    /// 上游原实现里 `snapshot` 把整张 sink 表原样克隆（没有时间判定），
    /// 而唯一的解除路径 `sink_clear` 只在「该账号成功转发一次」后调用 ——
    /// 可它正排在队尾，永远轮不到，于是自锁。实测现场：国际版 11 个账号
    /// 在一天内被逐个撞完 429 后全部永久沉底。
    #[test]
    fn sunk_accounts_come_back_when_their_ttl_expires() {
        let state = RouteState::new();
        let now = 1_700_000_000_000i64;
        state.sink_mark("a1", now);

        // 刚垫上：在快照里
        let fresh = state.snapshot("", now, 0);
        assert!(fresh.sunk.contains_key("a1"), "刚垫底的账号应当在快照里");

        // 差一点到 TTL：仍在
        let almost = state.snapshot("", now + SINK_TTL_MS - 1, 0);
        assert!(almost.sunk.contains_key("a1"), "未到 TTL 前不该提前解除");

        // 到点：自动消失（这就是本次修复的全部内容）
        let expired = state.snapshot("", now + SINK_TTL_MS, 0);
        assert!(
            !expired.sunk.contains_key("a1"),
            "超过 TTL 后应当自动回到队列 —— 上游原实现在这里恒为「仍在」"
        );
    }

    /// `sink_clear` 仍要能立即解除（成功转发后不该等 TTL）。
    #[test]
    fn a_successful_forward_clears_the_sink_immediately() {
        let state = RouteState::new();
        let now = 1_700_000_000_000i64;
        state.sink_mark("a1", now);
        state.sink_clear("a1");
        let snap = state.snapshot("", now, 0);
        assert!(!snap.sunk.contains_key("a1"), "成功后应当立刻解除垫底");
    }
}
