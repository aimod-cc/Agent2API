//! 「更新设置」里的 GitHub 令牌：加密落库、读取与状态。
//!
//! ── 加解密已委托给 `core::crypto` ───────────────────────────
//! 本模块**不再自带加解密实现**：算法、`enc1:` 信封、密钥文件的加载/生成/缓存
//! 统统抽到了 `crate::server::core::crypto`（账号表要复用同一套，抽出理由见它的
//! 模块头）。这里只剩两件令牌专属的事：
//!   1. 把密钥域钉死成 `SecretScope::UpdateToken`（密钥文件 `update-token.key`）；
//!   2. 令牌自己的规矩：校验规则、与环境变量 `GITHUB_TOKEN` 的优先级、解不开时
//!      的回落，以及 `status_json` 给用户看的那句提示。
//!
//! ── 为什么落库前要加密 ─────────────────────────────────────
//! 令牌能提高 GitHub API 的限额（匿名 60 次/小时 → 5000 次/小时），是**有代价的
//! 凭证**：泄露后别人能以你的身份读仓库、触发 CI。配置库（`agent2api.db`）会被
//! 整库导出 / 备份 / 拷机迁移，明文躺在里面等于「拷走一个文件就带走令牌」。
//! 因此这里存的是 AES-256-GCM 密文（形态与密钥存放规则见 `core::crypto`）：
//!   · 密钥：32 字节随机，存**库外**的 `{config_dir}/update-token.key`（首次保存
//!     令牌时生成；Unix 上 0600）。库被单独拿走时没有密钥文件，解不出令牌。
//!   · 密文形态：`enc1:<base64(nonce(12) || 密文+tag)>`，作为配置键 `githubToken`
//!     的值落库（见 config 模块的 KEY_GITHUB_TOKEN）。`enc1` 是版本前缀，
//!     将来换算法（比如换平台原生密钥库）时能识别出旧格式。
//!
//! ── 明文只进内存，不出三处口子 ─────────────────────────────
//!   1. `effective_token`：`github_headers` 拼 Authorization 头时用；
//!   2. `stored_token`：只给「这次保存有没有真的换掉令牌」的判定用（不解密就
//!      无法与已存值逐字比较，重复粘贴会被误当成换凭证）；
//!   3. 保存与清除路径：加密后立刻丢明文，不落任何日志；
//!   4. API **只报「有没有、来自哪」**（`status_json`），不回显本体 ——
//!      「已填写」三个字是界面能拿到的全部（回显 = 白加密）。
//!
//! ── 优先级：界面保存的 > 环境变量 ──────────────────────────
//! 环境变量 `GITHUB_TOKEN` / 旧名 `WORKBUDDY_GITHUB_TOKEN`（原先是唯一来源，
//! Docker 部署仍靠它）作为兜底。与 config 模块的既有约定一致（「配置里的值 >
//! 环境变量」）：在界面上保存是用户的**最后一次明确动作**，应当生效。
//!
//! ── 解密失败怎么办 ────────────────────────────────────────
//! 密钥文件被删 / 手改过 / 拷库没拷密钥时：**不当作「没配」**——配置里明明有
//! 一段密文，静默按匿名跑会让用户以为令牌还在生效。`effective_token` 回落
//! 环境变量并留一条 verbose；`status_json` 把原因带出去，界面上提示重新保存
//! 一次（新密钥 + 新密文，自愈）。「重存一次令牌即可自愈」这句提示本身由
//! `core::crypto` 按密钥域生成（不同域提示不同），本模块不再自己拼。

use crate::server::config;
use crate::server::core::crypto::{self, SecretScope};
use crate::server::logging;

/// 本模块使用的密钥域（GitHub 令牌专用；密钥文件是 `update-token.key`）
const SCOPE: SecretScope = SecretScope::UpdateToken;

/// 令牌长度上限：GitHub 令牌最长 255 左右，再宽就当脏数据处理
const MAX_TOKEN_LEN: usize = 255;

// ─── 信封的解与读 ──────────────────────────────────────────

/// 解开一段 `enc1:` 信封 → UTF-8 明文。
///
/// 只是「绑定额外的密钥域 + 收口成 String」的薄包装：真正的格式解析、长度校验、
/// AES-GCM 解密与 UTF-8 闸门都在 `core::crypto::open`。`open` 已保证字节是合法
/// UTF-8（见其模块头），所以这里的转换不会替换任何字符 —— 用 `from_utf8_lossy`
/// 只是为了拿 `String`，不引入第二条错误路径（与旧实现逐字等价的错误文案仍由
/// `open` 给出）。
fn decode_envelope(envelope: &str) -> Result<String, String> {
    let bytes = crypto::open(envelope, SCOPE)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 环境变量里的令牌（Docker / 命令行部署的配置口，两个名字都认、空串当没配）
fn env_token() -> Option<String> {
    ["GITHUB_TOKEN", "WORKBUDDY_GITHUB_TOKEN"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// 界面保存的那一份令牌（解密后）。未保存 / 解不开 → None。
///
/// 与 `effective_token` 的分工：这条**只看界面保存的那份**（不含环境变量兜底），
/// 供「这次保存有没有真的换掉令牌」的判定使用（api::update 据此决定要不要清
/// 检查冷却 —— 重复粘贴同一个令牌不该算一次更换）。
pub fn stored_token() -> Option<String> {
    let envelope = config::github_token_envelope()?;
    decode_envelope(&envelope).ok()
}

/// 实际生效的令牌：界面保存的（解密）> 环境变量。
///
/// `github_headers` 每次检查更新都调它：解密失败的回落与日志都在这里
/// 统一处理，调用方拿到 `Option<String>` 就够了。
pub fn effective_token() -> Option<String> {
    if let Some(envelope) = config::github_token_envelope() {
        match decode_envelope(&envelope) {
            Ok(token) => return Some(token),
            Err(reason) => logging::verbose(
                "[Update]",
                &format!("已保存的 GitHub 令牌解密失败（{reason}），本次跳过使用"),
            ),
        }
    }
    env_token()
}

/// `set_token` 的结果。
///
/// `saved`：是否落盘成功（false 时内存已生效，与 config 各 setter 同一约定）。
/// `changed`：令牌是否**真的发生了替换/清除** —— 调用方（api::update）只有看到
/// true 才清「检查更新」的失败冷却，免得「重复粘贴同一个令牌」被当成换凭证、
/// 变成绕开冷却的口子。
#[derive(Clone, Copy, Debug)]
pub struct TokenUpdate {
    pub saved: bool,
    pub changed: bool,
}

/// 保存 / 清除令牌（`None` = 清除）。
///
/// 校验只做三件事：非空、不超长、字符集限定在「字母数字下划线连字符」——
/// GitHub 令牌都在这个集合里，而令牌要进 HTTP 头，放进任何空白或控制字符
/// 都是请求头注入，必须在入口拦死。
pub fn set_token(token: Option<&str>) -> Result<TokenUpdate, String> {
    let previous = stored_token();
    let Some(raw) = token else {
        // 清除：库里原本存在过密文（哪怕解不开）就算一次变化
        let changed = previous.is_some() || config::github_token_envelope().is_some();
        return Ok(TokenUpdate {
            saved: config::set_github_token_envelope(None),
            changed,
        });
    };
    let token = raw.trim();
    if token.is_empty() {
        return Err("令牌不能为空（要清除请传 null）".to_string());
    }
    if token.len() > MAX_TOKEN_LEN {
        return Err(format!("令牌过长（最多 {MAX_TOKEN_LEN} 字符）"));
    }
    if !token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') {
        return Err("令牌只能包含字母、数字、下划线与连字符".to_string());
    }
    // 与已存令牌逐字比较（解不开时存储侧那一份算「非现值」，会正确判为变化）
    let changed = previous.as_deref() != Some(token);
    let envelope = crypto::seal(token.as_bytes(), SCOPE)?;
    Ok(TokenUpdate {
        saved: config::set_github_token_envelope(Some(envelope)),
        changed,
    })
}

/// 令牌状态 → API 的 JSON 形态（**不含本体**，见模块头「三处口子」）。
///
/// `origin`：`stored`（界面保存的）/ `env`（环境变量）/ null（没配）。
/// 已存令牌解不开时 `error` 带原因 —— 界面据此提示「重新保存一次」，
/// 而不是把「配置坏了」显示成「没配置」。
pub fn status_json() -> serde_json::Value {
    if let Some(envelope) = config::github_token_envelope() {
        return match decode_envelope(&envelope) {
            Ok(_) => serde_json::json!({ "filled": true, "origin": "stored", "error": null }),
            Err(reason) => {
                let env = env_token().is_some();
                let origin = if env { Some("env") } else { None };
                serde_json::json!({
                    "filled": env,
                    "origin": origin,
                    "error": reason,
                })
            }
        };
    }
    if env_token().is_some() {
        serde_json::json!({ "filled": true, "origin": "env", "error": null })
    } else {
        serde_json::json!({ "filled": false, "origin": null, "error": null })
    }
}
