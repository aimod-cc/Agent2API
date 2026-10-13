//! Antigravity 的「登录」：**粘贴式归一化 + 一次校验刷新**（本步不做网页登录）。
//!
//! ── 为什么本步只有粘贴式（评估结论见 `mod.rs` 的模块头）──────────
//! 网页登录要 loopback 回调 + 授权码换 token，是本仓既有能力（raccoon / Trae
//! 都接了），但 Antigravity 的授权页与 scope 还需要逐一实测（尤其 Google 对
//! 「非官方客户端 + 动态 loopback 端口」的容忍度，规格 §8.5 把它列为未确认），
//! 因此本步只做**用户自己拿到 refresh_token 后粘贴**这一条（与 Command Code /
//! MonkeyCode 的粘贴式入口同一形态）。
//!
//! ── 归一化做什么（用户粘的东西形态很杂）────────────────────────
//! 认得的形态（都剥成裸值）：
//! ```text
//!   1//0gxxxx…                      裸 refresh token（1// 是令牌本体，必须留着）
//!   "1//0gxxxx…"                    带成对引号
//!   refresh_token=1//0gxxxx…        表单 / 环境变量整行
//!   "refresh_token": "1//0gxxxx…"   JSON 片段（DevTools 复制）
//!   Bearer ya29.xxxx                access token 整行（剥 Bearer）
//! ```
//! 判据是**扫描 `1//` 出现的位置**：从那儿取到行尾，再削掉引号 / 尾逗号 / 尾花
//! 括号 / 空白 —— 这一步同时完成「前缀包装剥离」与「尾部杂字段截断」。
//! 扫不到 `1//` 时只剥引号与空白，把整串当令牌（Google 的 refresh token 一定
//! 有 `1//`，但**不因为形态不符就拒绝**：上游会用 401 明确告诉用户串不对，
//! 而形态猜错会把一个能用的串挡在门外）。
//!
//! ── 校验顺序：本地归一化 → 一次刷新（真校验）───────────────────
//! refresh_token 的有效性**只能问 Google**（不像 `user_` key 有形态可判），
//! 所以添加账号时打一次 token 端点：成功即证明「这枚串能用」，并顺手拿到
//! access_token 与到期时间（落盘后第一次转发不必现刷）。
//! `invalid_grant` 会由 `oauth::refresh_access_token` 翻成 401 + 可照做的文案。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::REFRESH_TOKEN_PREFIX;
use super::{oauth, project};

/// 输入长度上限（超过这个长度的输入一定是粘错了东西）
pub const MAX_INPUT_LENGTH: usize = 8192;

/// 去掉成对的引号与两侧的分隔符（JSON / 表单复制常见的包装字符）
fn trim_wrappers(text: &str) -> String {
    text.trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | ',' | ';'))
        .trim()
        .to_string()
}

/// 剥掉整行前缀：`refresh_token=` / `refreshToken:` / `access_token=` 之类。
///
/// 只在「前缀确实存在」时剥，且剥完还要继续走后面的扫描 —— 于是
/// `refresh_token=1//xxx` 与 `"refresh_token": "1//xxx"` 两条路都收敛到
/// 「扫描 `1//`」那一步。
fn strip_label(text: &str) -> String {
    const LABELS: [&str; 6] = [
        "refresh_token",
        "refreshtoken",
        "refresh token",
        "access_token",
        "accesstoken",
        "token",
    ];
    let lowered = text.to_ascii_lowercase();
    for label in LABELS {
        if !lowered.starts_with(label) {
            continue;
        }
        let rest = text.get(label.len()..).unwrap_or("").trim_start();
        let Some(rest) = rest
            .strip_prefix('=')
            .or_else(|| rest.strip_prefix(':'))
            .or_else(|| rest.strip_prefix("=>"))
        else {
            continue;
        };
        return trim_wrappers(rest);
    }
    text.to_string()
}

/// 归一化粘贴的 refresh token：剥掉整行包装与引号，返回**裸令牌**。
///
/// `1//` 前缀是令牌本体的一部分，**绝不要剥**（见模块头）。
pub fn normalize_refresh_token(raw: &str) -> String {
    let cleaned = strip_label(&trim_wrappers(raw));
    if let Some(start) = cleaned.find(REFRESH_TOKEN_PREFIX) {
        let tail = cleaned.get(start..).unwrap_or("");
        // `1//` 之后可能跟 `"` / `,` / `}` / 空白（JSON 片段）；令牌本体只含
        // 可见 ASCII，因此在这里就地把它们削掉
        let token: String = tail
            .chars()
            .take_while(|ch| !ch.is_whitespace() && !matches!(ch, '"' | '\'' | ',' | '}' | ']' | ';'))
            .collect();
        if token.len() > REFRESH_TOKEN_PREFIX.len() {
            return token;
        }
    }
    // 扫不到 `1//`：只把空白去掉（不是合法形态也交给上游 401 说话，
    // 而不是在这里把用户挡在门外）
    cleaned.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 归一化粘贴的 access token：剥 `Bearer ` 前缀、引号与空白。
///
/// 用户可能从 DevTools 的 Authorization 头整行复制过来 —— 那行**不能**原样
/// 落盘（发出去会变成 `Bearer Bearer ya29…`）。
pub fn normalize_access_token(raw: &str) -> String {
    let cleaned = strip_label(&trim_wrappers(raw));
    let trimmed = cleaned.trim();
    // `get` 而不是切片：非 ASCII 首字符时不做越界的字符边界切片
    let has_bearer = trimmed
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("bearer"));
    let without_bearer = if has_bearer {
        trimmed.get(6..).unwrap_or("").trim()
    } else {
        trimmed
    };
    without_bearer.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 从 payload 里取第一个非空字符串（界面的表单键名与落盘名都认）
fn pick(payload: &Value, keys: &[&str]) -> String {
    for key in keys {
        let value = payload
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

/// 粘贴式添加的入口：归一化 → 校验刷新（真打 Google）→ best-effort 补 project/email。
///
/// 返回可直接交给落账号入口的 payload：
/// `{refreshToken, accessToken?, expiresAt?, projectId?, email?}`。
///
/// ── 为什么校验刷新是必需的 ────────────────────────────────────
/// 「用户粘了一串东西」与「这串能用」之间没有任何本地判据（不像 API Key 有
/// 前缀形态）。不校验就落账号，用户会在第一次转发时才发现串是坏的 ——
/// 而那时错误文案混在转发链路里，比在添加页直接说清楚难查得多。
///
/// ── project / email 是 best-effort ───────────────────────────
/// 两者都不影响「账号能不能用」：project 只影响聊天请求（规格 §3.4），
/// email 只影响展示。拿不到就留空，**绝不因为它失败就判登录失败**
/// （与 Trae `profile::get_user_info` 的「登录已成功却因为读不到昵称而判失败
/// 是本家明确要避免的行为」同一条理由）。
pub async fn verify_paste(
    payload: &Value,
    proxy: Option<&ResolvedProxy>,
) -> Result<Value, GatewayError> {
    let raw_refresh = pick(payload, &["refreshToken", "refresh_token", "token"]);
    if raw_refresh.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            "请粘贴 Antigravity 的 Google refresh token（以 1// 开头）",
        ));
    }
    if raw_refresh.chars().count() > MAX_INPUT_LENGTH {
        return Err(GatewayError::with_status(
            400,
            "粘贴的内容过长，请确认复制的是 refresh token 本身",
        ));
    }
    let refresh_token = normalize_refresh_token(&raw_refresh);
    if refresh_token.is_empty() {
        return Err(GatewayError::with_status(
            400,
            "请粘贴 Antigravity 的 Google refresh token（以 1// 开头）",
        ));
    }
    // 校验刷新：成功即证明串可用（失败时 oauth 模块给出可照做的文案）
    let response = oauth::refresh_access_token(&refresh_token, proxy).await?;
    let access_token = normalize_access_token(&response.access_token);
    let expires_in = if (1..=7 * 24 * 3600).contains(&response.expires_in) {
        response.expires_in
    } else {
        3600
    };
    let expires_at = logging::now_ms() + expires_in * 1000;
    // 表单里手填的 project 优先（用户可能自己知道）；没有再 best-effort 发现
    let provided_project = pick(payload, &["projectId", "project_id"]);
    let mut project_id = provided_project.trim().to_string();
    if project_id.is_empty() {
        match project::discover_project(&access_token, proxy).await {
            Ok(found) => project_id = found,
            Err(error) => logging::verbose(
                "[Login]",
                &format!("Antigravity project 未发现（不影响账号添加）：{}", error.message),
            ),
        }
    }
    let email = pick(payload, &["email"]);
    logging::log(
        "[Login]",
        &format!(
            "✅ Antigravity refresh token 校验通过（{}{}）",
            if email.is_empty() { "email 未填".to_string() } else { format!("email {email}") },
            if project_id.is_empty() {
                "，project 未发现（刷模型清单时会再试）".to_string()
            } else {
                format!("，project {project_id}")
            }
        ),
    );
    Ok(json!({
        "refreshToken": refresh_token,
        "accessToken": access_token,
        "expiresAt": expires_at,
        "projectId": project_id,
        "email": email,
    }))
}
