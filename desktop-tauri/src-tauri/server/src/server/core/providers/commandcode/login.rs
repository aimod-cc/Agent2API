//! Command Code 的「登录」：**粘贴式归一化 + 校验**（没有 OAuth / 设备码 / 窗口）。
//!
//! ── 链路（一步本地归一化 + 一次 best-effort 探活）────────────────
//! ```text
//! 用户粘贴的东西（形态很杂）→ normalize_key → 形态检查（user_ 开头）
//!   → GET /alpha/billing/credits 探活（401/403 才判无效）
//!   → {"apiKey": "<裸 key>"} 交给落账号入口
//! ```
//!
//! ── 归一化做什么（用户从 DevTools / 配置文件复制的东西形态很杂）──
//! 认得的形态：裸 key、带成对引号、`Authorization: Bearer user_…`、
//! `x-api-key: user_…`、整行 `Cookie: …`、以及 key 后面还跟着别的字段
//! （`…user_xxx; Path=/` 之类）。做法与参考实现同源：**扫描 `user_` 并取其后
//! 连续合法字符**（正则 `user_[a-zA-Z0-9_-]+` 的手写形态）—— 这一步同时完成
//! 前缀剥离与尾部截断。扫不到 `user_` 时只剥引号与空白，交给形态检查给出
//! 可读的报错（而不是把一整行当 key 发给上游换一个 401）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::{self, KeyCheck, KEY_PREFIX};

/// 输入长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_INPUT_LENGTH: usize = 8192;

/// `user_` 之后的合法字符（与 [`credentials::shape_ok`] 同一字符集）
fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// 扫描第一个 `user_` 并取其后连续合法字符（参考实现的正则提取）
fn extract_key(text: &str) -> Option<String> {
    let start = text.find(KEY_PREFIX)?;
    let rest = &text[start + KEY_PREFIX.len()..];
    let body: String = rest.chars().take_while(|ch| is_key_char(*ch)).collect();
    if body.is_empty() {
        return None;
    }
    Some(format!("{KEY_PREFIX}{body}"))
}

/// 归一化粘贴的凭证：剥引号 / 整行前缀 / 尾部杂字段，返回**裸 key**。
pub fn normalize_key(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | ',' | ';'))
        .trim();
    if let Some(key) = extract_key(trimmed) {
        return key;
    }
    // 没有 `user_` 形态：仍把空白剥掉（key 里不含空白），让形态检查能给出
    // 「应为 user_ 开头」这句可执行的报错
    trimmed.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 归一化 + 校验，返回可直接交给落账号入口的 payload：`{"apiKey": "<裸 key>"}`。
///
/// 校验顺序（先本地、再线上）是刻意的：本地形态不对时**一次网络都不发**
/// （用户粘错了东西，探活没有意义）；线上探活失败**不阻断添加**
/// （见 `credentials::probe_key` 的三档语义）。
pub async fn verify_key(raw: &str) -> Result<Value, GatewayError> {
    if raw.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            format!("请粘贴 Command Code 的 API Key（{KEY_PREFIX} 开头）"),
        ));
    }
    if raw.chars().count() > MAX_INPUT_LENGTH {
        return Err(GatewayError::with_status(
            400,
            "粘贴的内容过长，请确认复制的是 API Key 本身",
        ));
    }
    let api_key = normalize_key(raw);
    if api_key.is_empty() {
        return Err(GatewayError::with_status(
            400,
            format!("请粘贴 Command Code 的 API Key（{KEY_PREFIX} 开头）"),
        ));
    }
    if !credentials::shape_ok(&api_key) {
        return Err(GatewayError::with_status(
            400,
            format!(
                "这不是 Command Code 的 API Key：应以 {KEY_PREFIX} 开头、只含字母数字与 _ -；\
                 请在 commandcode.ai/studio（或本机 ~/.commandcode/auth.json）重新复制"
            ),
        ));
    }
    match credentials::probe_key(&api_key).await {
        KeyCheck::Valid => {
            logging::log(
                "[Login]",
                &format!(
                    "✅ Command Code API Key 校验通过（{}）",
                    credentials::masked(&api_key)
                ),
            );
        }
        KeyCheck::Invalid(reason) => {
            let message = format!(
                "Command Code API Key 无效或已失效（{reason}）：\
                 请在 commandcode.ai/studio 重新获取后粘贴"
            );
            logging::log("[Login]", &format!("❌ {message}"));
            return Err(GatewayError::with_status(401, message));
        }
        KeyCheck::Unknown(note) => {
            // 判不出来就放行：一次上游抖动不该让用户加不进账号。
            // 真的无效会在第一次生成时以 401 暴露（编排层换账号 / 提示重贴）。
            logging::verbose(
                "[Login]",
                &format!("Command Code API Key 未能线上确认（{note}），按可用处理"),
            );
        }
    }
    Ok(json!({ "apiKey": api_key }))
}
