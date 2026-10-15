//! **OpenCode 官方客户端形态伪装**（提供商记录上的 `clientEmulation: "opencode"`）。
//!
//! ── 为什么需要它（上游实测，2026-10-14 对着 opencode v2.0.27 重测）──
//! `https://opencode.ai/inference/v1` 的**免费档**模型（`*-free` 等）在直连时
//! 会回 403：
//! ```text
//! {"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free
//!  tier can only be used from within OpenCode. Download it at
//!  https://opencode.ai and keep building."}}
//! ```
//! 这次重测用的是**官方客户端自己的出站请求**（`opencode serve` 起一个实例，
//! `BUN_CONFIG_VERBOSE_FETCH=curl` 把它的请求原文打出来）做字节级对照，再逐项
//! 消融。当前这道门要求四条同时成立：
//!   1. 凭证是**字面量** `public`（`Authorization: Bearer public`）——
//!      匿名通道，不需要注册、也不需要登录；
//!   2. **四个会话头同时在场且同值**：`x-opencode-session`、
//!      `x-opencode-session-id`、`x-session-affinity`、`x-session-id`。
//!      缺 `x-opencode-session-id` 实测即 403；
//!   3. `x-opencode-request` 与 `x-opencode-project` **一个都不能发** —— 实测
//!      带上任何取值（含 `msg_…`、真实 sha1）都 403，官方 v2 客户端这两个头
//!      本来也不发；
//!   4. 请求体须是「智能体形态」：`stream: true`，且 `tools` 里同时含一个
//!      shell 类工具（`bash` 或 `shell`）与一个 `read`（名字要小写逐字相同，
//!      参数与描述不查；见 [`SHELL_TOOL_ALIASES`]）。
//!
//! 会话 id 只校验**形状与取值**（`ses_<12位小写 hex><14位 Base62>`，低 12 位
//! ≥ [`LOW12_FLOOR`]），**不校验它是否在服务端注册过** —— 本模块的派生算法
//! （[`session_id`]，低 12 位钉 [`LOW12_TAG`]）实测直接放行，不需要本机装或跑
//! 官方客户端。端点也换过：`/zen/v1` 已废弃（打它一律 403），当前是
//! `/inference/v1`（预置卡的 baseUrl 已同步，存量记录走 `custom_providers`
//! 的读侧迁移）。
//!
//! 本模块就实现这四条。**它是有意为之的 opt-in**：只在提供商记录显式写了
//! `clientEmulation: "opencode"` 时才生效（预置卡「OpenCode Zen」会带上
//! 它），手写的家与本网关的其它路径完全不受影响。
//!
//! ── 还有第二个面：OpenCode **Go**（`/zen/go/v1`）─────────────
//! Go 是订阅制端点，要求与免费档**完全不同**：它只要客户端**带一个稳定的
//! 会话头**（上游文档 `docs/go/#where-can-i-use-it` 的公开路由要求：「Send a
//! stable session ID in `x-opencode-session` for each conversation」），
//! 不要求任何伪装，也拒绝被塞桩工具的请求（社区实测 #44300/#44382）。
//! 本模块对 Go 面只做一件事 —— [`apply_go_headers`]：补会话头与产品 UA，
//! 凭证与请求体一概不碰。两个面的分派由 `providers::custom::forward` 按
//! **端点**做（判据 `custom_providers::is_opencode_go_endpoint`），
//! 且 Go 面**不受 `clientEmulation` 开关控制** —— 那条要求是端点自身的
//! 协议要求，不是「伪装」，没有开关可关。
//!
//! ── 代价与边界（用它之前要知道的事）─────────────────────────
//!   · 它会**改写用户的请求体**（补桩工具、强制 `stream: true`）——
//!     与「自定义家透传」的默认语义相反，所以只在显式开启时做；
//!   · 桩工具是上游校验的产物，不是给模型用的：客户端**原本没有工具**时
//!     一并下发 `tool_choice: "none"`，免得模型真的去调它。客户端自带工具时
//!     保留其 `tool_choice`（改掉会破坏调用工具的客户端），此时桩工具与真
//!     工具并列存在 —— 上游要的就是这个形态；
//!   · 上游随时可能收紧这套校验（2026-09、2026-10-09、2026-10-13、2026-10-14
//!     都动过），失效时表现为 403 `FreeTierError`。再遇到这类 403，排查顺序是
//!     **端点是不是 `/inference/v1` → 四个会话头齐不齐 → 有没有多带
//!     `x-opencode-request` / `x-opencode-project` → 会话 id 的低 12 位 →
//!     请求体的 `stream` 与工具对**，最后才怀疑上游又改了规则（判据是「四条的
//!     产物都对、却仍然 403」）。
//!     排这个门的正解是**抓官方客户端自己的出站请求**做对照
//!     （`BUN_CONFIG_VERBOSE_FETCH=curl opencode serve`），别照着社区方案猜 ——
//!     那批方案大多还打在废弃的 `/zen/v1` 上，照着抄只会一起坏。
//!
//! ── 与其它模块的分工 ────────────────────────────────────────
//! 本模块**只回答四件事**：请求体补齐（[`ensure_agent_shape`]）、会话 id 与
//! 它是否合格（[`session_id`] / [`is_acceptable_session_id`]）、伪装头集合
//! （[`apply_headers`]）。发不发、发给谁由 `providers::custom::forward` 决定；
//! 记录字段的读写与校验在 `core::custom_providers`。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// 提供商记录上的合法取值（与 `custom_providers::CLIENT_EMULATION_OPENCODE` 同字面量；
/// 常量在那边定义是为了让读写与校验在同一处，这里只借用）。
pub use crate::server::core::custom_providers::CLIENT_EMULATION_OPENCODE;

/// 匿名凭证：上游免费档认的字面量（无需注册、无轮换）。
pub const ANONYMOUS_KEY: &str = "public";

/// 伪装成官方 CLI 的 User-Agent（对照实现实测可通过的版本号）
const USER_AGENT: &str = "opencode/1.18.31 (windows amd64; node22)";

/// 免费档校验要求的桩工具名（大小写敏感，实测只有小写通过）：一个 shell 类
/// 工具 + 一个 `read`，两者齐了才放行。
const STUB_TOOLS: [&str; 2] = ["bash", "read"];

/// 「shell 类工具」的等价名 —— 上游认 `bash` 或 `shell` 任一（官方 v2 客户端
/// 发的是 `shell`）。本模块补桩一律用 [`STUB_TOOLS`] 里的 `bash`。
const SHELL_TOOL_ALIASES: [&str; 2] = ["bash", "shell"];

/// 会话标识的 Base62 字母表（与对照实现逐字一致）
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// 会话种子：**首个 user 消息**的 content 序列化。
///
/// 为什么用它而不是随机值：多轮对话里历史不断增长，取首轮才能让同一场对话
/// 一直映射到同一个会话 —— 上游按会话做提示词缓存亲和，每次换 id 等于每轮
/// 都冷启动。取不到（没有 user 消息 / content 为空）时给空串，调用方退化成
/// 随机会话（单次对话，无亲和需求）。
pub fn conversation_seed(body: &Value) -> String {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return String::new();
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = message.get("content") else { continue };
        if content.is_null() {
            continue;
        }
        // 字符串直接取；数组（多模态）与其它形态走序列化 —— 与对照实现
        // （`json.Marshal(m["content"])`）同一口径：**不看内容细节**，
        // 只要同一场对话算出同一个串即可
        let seed = match content {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if !seed.is_empty() {
            return seed;
        }
    }
    String::new()
}

/// 任意种子 → 上游要求的 `ses_<12位小写 hex><14位 Base62>` 会话 id。
///
/// 合格（形状与取值，见 [`is_acceptable_session_id`]）时**原样返回** ——
/// 保住调用方自带的亲和键；不合格的种子当普通字符串重新派生。
///
/// 派生时低 12 位钉死为 [`LOW12_TAG`]（官方客户端的产出值），其余 36 位与
/// 14 位尾段都来自 `SHA-256(种子)`：**同一场对话仍映射到同一个 id**，上游
/// 按会话做的提示词缓存亲和因此不受影响（这条是取首轮 user 消息当种子的
/// 原因，见 [`conversation_seed`]）。
pub fn session_id(seed: &str) -> String {
    if is_acceptable_session_id(seed) {
        return seed.to_string();
    }
    let seed = if seed.is_empty() {
        // 空种子（没有可派生的对话内容）用随机值：单次会话，无亲和需求，
        // 但形状必须合法 —— 否则上游一律 403
        random_seed()
    } else {
        seed.to_string()
    };
    let digest = Sha256::digest(format!("ses\u{0}{seed}").as_bytes());
    // 低 12 位按官方客户端钉死（理由见 LOW12_TAG 的注释）：直接用散列前
    // 6 字节的话，低 12 位是均匀随机的，只有 64/4096 落在放行区间。
    let mut bits = 0u64;
    for byte in &digest[..6] {
        bits = (bits << 8) | u64::from(*byte);
    }
    let bits = (bits & !0xFFF) | u64::from(LOW12_TAG);
    format!("ses_{bits:012x}{}", base62_fixed(&digest[6..16], 14))
}

/// `ses_` + 12 位小写 hex + 14 位 Base62（上游 2026-09-16 起对其它形状一律 403）
fn valid_session_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("ses_") else { return false };
    if rest.len() != 12 + 14 {
        return false;
    }
    let (hex, tail) = rest.split_at(12);
    hex.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && tail.bytes().all(|byte| BASE62.contains(&byte))
}

/// 会话 id 低 12 位的**取值下界**。
///
/// 2026-10-13 对 `POST /zen/v1/chat/completions` 实测的翻转点：低 12 位
/// `0xFBF`（4031）被拒、`0xFC0`（4032）放行。上游**只看这 12 位** ——
/// `ses_0000000fffff`（数值极小）放行、`ses_ecbcb548a000`（数值极大）被拒，
/// 可见与 id 的数值大小无关；本机历史上 23 个真实 id 的低 12 位全是 `0xFFE`。
/// 5 月、6 月的真实 id 一样放行，阈值在 146 秒内没有移动 —— 这一条**不是**
/// 时间新鲜度校验，是纯粹的位模式校验。
///
/// 2026-10-14 换成 `/inference/v1` 端点后复测，此下界**原样成立**（低 12 位
/// `0x100` 被拒、`0xFFE` 放行），端点迁移没有动这条。
const LOW12_FLOOR: u16 = 0xFC0;

/// 会话 id 低 12 位的**产出值**：官方客户端实测恒为该值，落在放行区间正中。
///
/// 判据用 [`LOW12_FLOOR`]、产出用本值，两者刻意分开：判据是「上游当时认
/// 什么」，本值是「官方客户端发什么」，将来上游若再收紧，改哪里一目了然。
const LOW12_TAG: u16 = 0xFFE;

/// 这个字符串能不能**直接当会话头发出去**：形状（[`valid_session_id`]）与
/// 取值（低 12 位 ≥ [`LOW12_FLOOR`]）都合格才算。
///
/// 拆成两个判据是刻意的：形状是 2026-09 立的规矩、取值是 2026-10 加的，
/// 合成一个函数会让「这个 id 为什么被重算」将来无从追查。
pub fn is_acceptable_session_id(text: &str) -> bool {
    if !valid_session_id(text) {
        return false;
    }
    // `ses_` 之后 12 位是 hex（valid_session_id 已保证是 ASCII），取末 3 位
    // 即低 12 位。用 get 而不是直接切片：本模块的硬约束是不 panic。
    let Some(hex) = text.get(4..16) else { return false };
    let Some(low) = hex.get(9..12).and_then(|tail| u16::from_str_radix(tail, 16).ok()) else {
        return false;
    };
    low >= LOW12_FLOOR
}

/// 随机种子（`getrandom` 失败时退回时间戳 —— 这里只是会话 id 的熵，
/// 不是密钥，退化的后果仅是「同一场对话的亲和键不稳定」，不该让请求失败）
fn random_seed() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_ok() {
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    format!("fallback-{}", crate::server::logging::now_ms())
}

/// 定长 Base62（左侧补零到 `width` 位；不足则从低位截断）
fn base62_fixed(bytes: &[u8], width: usize) -> String {
    // 把字节串当一个 128 位大数：逐字节「乘 256 加」并用 62 进制取位。
    // 用 u32 位宽逐位除法实现（不引大整数库）——宽度 14 位 Base62 约 83 bit，
    // 16 字节输入足够，且结果稳定。
    let mut digits = vec![0u32; width];
    for byte in bytes {
        let mut carry = u32::from(*byte);
        for digit in digits.iter_mut().rev() {
            let value = *digit * 256 + carry;
            *digit = value % 62;
            carry = value / 62;
        }
        // 溢出的高位（超出 width 位 Base62 所能表达的范围）直接丢掉：
        // 上游只校验形状与长度，不做唯一性校验
    }
    digits
        .iter()
        .map(|digit| BASE62[(*digit as usize).min(61)] as char)
        .collect()
}

/// 出站头的伪装集合：把官方 v2 客户端会带的头补齐到 `headers` 上。
///
/// `headers` 的键名沿用本网关的默认头（`Authorization` / `Content-Type`），
/// 同名覆盖 —— 于是**自定义 key 优先**：账号填了 key 就用它（付费模型必须），
/// 没填才用匿名字面量（免费档）。这与 `ProviderQuirks::apply_to` 的合并口径
/// 相同，转发侧先铺默认头、再调本函数，顺序不能反过来。
///
/// ── 头集合是**实测**出来的，多一个少一个都不行 ────────────────
///   · **四个会话头必须同时在场且同值**（缺 `x-opencode-session-id` → 403）；
///   · **不发** `x-opencode-request` / `x-opencode-project` —— 官方 v2 客户端
///     这两个头本来也不发，实测带上任何取值都 403。旧实现发的
///     `req_…` / `prj_…` 正是 2026-10 这轮 403 的成因之一，别再加回来。
pub fn apply_headers(headers: &mut Vec<(String, String)>, api_key: &str, session: &str) {
    let credential = if api_key.trim().is_empty() {
        ANONYMOUS_KEY
    } else {
        api_key.trim()
    };
    let pairs = [
        ("Authorization".to_string(), format!("Bearer {credential}")),
        // 官方 CLI 的形状头：缺了「看起来就不像官方客户端」，实测会掉进 403
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("x-opencode-client".to_string(), "cli".to_string()),
        // 四个会话头（同一场对话同一个 id，上游按它做提示词缓存亲和）
        ("x-opencode-session".to_string(), session.to_string()),
        ("x-opencode-session-id".to_string(), session.to_string()),
        ("x-session-affinity".to_string(), session.to_string()),
        ("X-Session-Id".to_string(), session.to_string()),
    ];
    merge(headers, pairs);
}

/// **Go 面**（`/zen/go/v1`）的会话头名。免费档的四个头在 Go 上一个都不需要，
/// Go 只看这一个（上游文档点名的就是它）。
pub const GO_SESSION_HEADER: &str = "x-opencode-session";

/// Go 面的 User-Agent：Go 的文档要求客户端「自报家门」（`my-coding-agent/1.0`
/// 这类），**明确不要通用 HTTP 库名** —— 网关默认的 `undici` 正好是它劝阻的
/// 那种，所以这条路径必须换掉。版本取自 crate 版本，随发布自动更新。
fn go_user_agent() -> String {
    format!("agent2api/{}", env!("CARGO_PKG_VERSION"))
}

/// **Go 面**（`/zen/go/v1`）的出站头：只补会话头与 UA。
///
/// 与免费档的伪装是两回事：Go 要的只是「每场对话一个稳定会话头」这条**公开的
/// 路由要求**（缺了 400 MissingSessionID，实测这道检查在鉴权之前、且不校验
/// 形状）。所以本函数：
///   · **不动凭证** —— Go 用账号自己的 API Key，这里一个 Authorization 都不加；
///   · **不动请求体** —— Go 面拒绝被塞桩工具（社区实测 #44300/#44382），
///     `ensure_agent_shape` 那条路对 Go 一律不叫；
///   · 会话 id 仍走 [`session_id`] 的派生（同一场对话同一个 id），
///     Go 拿它做路由与提示词缓存亲和。
pub fn apply_go_headers(headers: &mut Vec<(String, String)>, session: &str) {
    merge(headers, [
        ("User-Agent".to_string(), go_user_agent()),
        (GO_SESSION_HEADER.to_string(), session.to_string()),
    ]);
}

/// 把一对键值按名合并进头表（ASCII 不区分大小写；同名覆盖，其余追加）
fn merge(
    headers: &mut Vec<(String, String)>,
    pairs: impl IntoIterator<Item = (String, String)>,
) {
    for (key, value) in pairs {
        match headers.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(&key)) {
            Some(slot) => slot.1 = value,
            None => headers.push((key, value)),
        }
    }
}

/// 请求体补齐「智能体形态」：`stream: true` + `tools` 里必须同时含一个
/// shell 类工具与一个 `read`（见模块头第 4 条）。
///
/// 返回是否改动了请求体（只给日志用）。四条细节：
///   · `stream` 恒置 true（上游免费档只接受流式；非流式下游由网关聚合器收敛
///     —— 这与编排层对**所有**上游的既有策略一致，不是这里新加的行为）；
///   · **缺哪个补哪个，客户端自带的工具一律原样保留**（不覆盖它的参数定义）。
///     注意是「缺就补」而不是「没有才补」：客户端只带自己的业务工具时，若不含
///     shell / `read` 这两样，照样过不了检 —— 实测这种请求一律 403；
///   · shell 侧认**两个等价名**：客户端自己带了 `bash` 或 `shell`，都不再补
///     （上游两者都认，见 [`SHELL_TOOL_ALIASES`]）；
///   · 客户端**原本没有 tools** 时补 `tool_choice: "none"`：桩工具只为过检，
///     明确禁止模型调用它。客户端自带工具时不动 `tool_choice` —— 那会破坏
///     它本来能用的工具调用。
pub fn ensure_agent_shape(body: &mut Value) -> bool {
    let mut changed = false;
    let Some(object) = body.as_object_mut() else { return false };
    if object.get("stream").and_then(Value::as_bool) != Some(true) {
        object.insert("stream".to_string(), Value::Bool(true));
        changed = true;
    }
    let had_tools = object
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    let mut tools: Vec<Value> = object
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let declared = tool_names(&tools);
    for name in STUB_TOOLS {
        // shell 侧用等价名匹配：`bash` 这条只要客户端带了 `bash` 或 `shell` 就算满足
        let satisfied = if name == "bash" {
            declared.iter().any(|item| SHELL_TOOL_ALIASES.contains(&item.as_str()))
        } else {
            declared.iter().any(|item| item.as_str() == name)
        };
        if satisfied {
            continue;
        }
        tools.push(stub_tool(name));
        changed = true;
    }
    object.insert("tools".to_string(), Value::Array(tools));
    if !had_tools {
        object.insert("tool_choice".to_string(), Value::String("none".to_string()));
    }
    changed
}

/// 请求体里 `tools` 已声明的 function 名（chat 形态）。大小写不归一 ——
/// 上游认的是小写逐字相同（`Bash` 这种不算数，调用方补一个小写桩即可）。
fn tool_names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// 一个桩工具（参数与描述不参与上游校验，给可辨识的名字是为了让人读日志时
/// 一眼看出它是网关补的）
fn stub_tool(name: &str) -> Value {
    let property = if name == "bash" { "command" } else { "filePath" };
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": "(internal placeholder — do not call)",
            "parameters": {
                "type": "object",
                "properties": { property: { "type": "string" } },
            },
        },
    })
}
