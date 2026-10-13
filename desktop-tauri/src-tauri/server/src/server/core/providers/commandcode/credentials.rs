//! Command Code 凭证：**单个粘贴的 API Key**（无 OAuth、无设备码、无续期）。
//!
//! ── 上游的凭证形态（规格 §1）───────────────────────────────────
//! 一个以 `user_` 开头的字符串，从 <https://commandcode.ai/studio> 或本机
//! `~/.commandcode/auth.json` 自取。反代侧只需要把它透传成
//! `Authorization: Bearer <key>`（参考实现用正则 `user_[a-zA-Z0-9_-]+`
//! 从入站头里**提取**，多余前后缀自动剥离）。
//! 上游**没有** refresh 接口、key 也不会续期 → `supports_refresh = false`。
//!
//! ── 校验：本地形态检查为主，线上探活是 best-effort ───────────────
//! 参考实现（commandcode-proxy）**不在添加时校验**，全靠上游在生成时报
//! 401/403；10router 另用 `GET /alpha/billing/credits` 探活（401/403 即视为
//! key 无效）。本模块取两者之长：形态检查**必定执行**（本地、零网络），
//! 线上探活**尽力而为** —— 只有拿到 401/403 才判无效，网络失败 / 5xx / 未知
//! 一律按「暂时判不出来」放行（否则一次上游抖动就会让用户加不进账号）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::auth_http::send_raw;
use crate::server::errors::GatewayError;

use super::endpoints;

/// Key 前缀（上游要求的唯一硬性形态）
pub const KEY_PREFIX: &str = "user_";

/// Key 字符集（规格 §1：`user_` 之后是 `[a-zA-Z0-9_-]+`）
fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// 形态检查：以 `user_` 开头、其后至少一个合法字符、且整串都在字符集内。
///
/// 这是**唯一的本地校验**（零网络）：`sk-xxx` 之类会在添加时就被拒（参考
/// 实现同样把「`sk-` 直接拒绝」列为契约）。
pub fn shape_ok(key: &str) -> bool {
    let Some(rest) = key.trim().strip_prefix(KEY_PREFIX) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(is_key_char)
}

/// 展示用的脱敏形态（只露前缀与尾 4 位；日志与报错文案都用它）
pub fn masked(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    if chars.len() <= 12 {
        return format!("{KEY_PREFIX}…");
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{KEY_PREFIX}…{tail}")
}

/// 一次转发 / 目录刷新用的凭证快照
#[derive(Clone, Debug)]
pub struct CommandCodeCredentials {
    /// 账号记录 id
    pub id: String,
    /// 上游 API Key（`user_` 开头的明文）
    pub api_key: String,
    /// 展示名（记录的备注名）
    pub name: String,
}

/// 从一条取值链里挑第一个非空字符串（记录可能被手工编辑过，宽容读取）
fn pick<'a>(record: &'a Value, keys: &[&str]) -> String {
    for key in keys {
        let value = record
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// 从账号记录解析凭证（`record` 为 None 时报「没有可用账号」）。
///
/// 字段兼容：`apiKey`（本项目落盘名）/ `accessToken` / `token` / `key`。
/// 形态不合格时给**可执行**的报错（去哪儿重新取 key），而不是笼统的 401。
pub fn from_record(record: Option<&Value>) -> Result<CommandCodeCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的 Command Code 账号：请在账号页粘贴 API Key（user_ 开头）添加",
        ));
    };
    let api_key = pick(record, &["apiKey", "accessToken", "token", "key"]);
    if api_key.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "Command Code 账号缺少 API Key，请重新粘贴（user_ 开头）",
        ));
    }
    if !shape_ok(&api_key) {
        return Err(GatewayError::with_status(
            401,
            format!(
                "Command Code API Key 形态不对（应为 user_ 开头、只含字母数字与 _ -，当前 {}）：\
                 请在 commandcode.ai/studio 重新获取",
                masked(&api_key)
            ),
        ));
    }
    Ok(CommandCodeCredentials {
        id: pick(record, &["id"]),
        api_key,
        name: pick(record, &["name"]),
    })
}

/// 线上探活的结果
#[derive(Clone, Debug)]
pub enum KeyCheck {
    /// 上游认这个 key（2xx）
    Valid,
    /// 上游明确拒绝（401/403）—— key 无效 / 已失效
    Invalid(String),
    /// 这次判不出来（网络失败 / 其他状态码）—— **按可用放行**
    Unknown(String),
}

/// 探活超时（比目录查询短：它只是「顺手问一句」，不该拖住添加流程）
const PROBE_TIMEOUT_MS: u64 = 10_000;

/// `GET /alpha/billing/credits` 探活（10router 的判据：401/403 即无效）。
///
/// 为什么用 billing 而不是随便一个需要鉴权的端点：`/alpha/generate` 会把
/// 一次**真实生成**的额度花掉；billing 是只读的额度/滚动窗口查询，
/// 对账号零副作用（规格 §2.1 的端点表）。
pub async fn probe_key(api_key: &str) -> KeyCheck {
    let mut headers = endpoints::cli_headers();
    headers.extend(endpoints::auth_headers(api_key));
    let result = send_raw(
        "GET",
        &endpoints::url(endpoints::BILLING_CREDITS_PATH),
        None,
        &headers,
        None,
        Some(PROBE_TIMEOUT_MS),
    )
    .await;
    match result {
        Ok(response) => {
            if response.status == 401 || response.status == 403 {
                KeyCheck::Invalid(format!("上游返回 HTTP {}", response.status))
            } else if response.ok {
                KeyCheck::Valid
            } else {
                KeyCheck::Unknown(format!("上游返回 HTTP {}", response.status))
            }
        }
        Err(error) => KeyCheck::Unknown(format!("探活请求失败：{error}")),
    }
}
