//! Antigravity 的账号凭证：**refresh_token 是主凭证**，access_token 是短期缓存。
//!
//! ── 与别家最大的不同：token 是「Google OAuth 的」，不是上游自研的 ──
//! `access_token` 一小时左右就过期（Google 侧的 `expires_in`，通常 3599 秒），
//! 但 `refresh_token` 是长寿命的（Google 只在首次授权 `prompt=consent` +
//! `access_type=offline` 时下发，规格 §1.5 / §7.17）。因此本家的落盘重点是：
//!
//! ```text
//!   refreshToken   主凭证（长寿命；丢了只能重新授权 —— 规格 §7.17）
//!   accessToken    可选缓存（有它就不必每次转发前现刷一次）
//!   expiresAt      可选（毫秒；= now + expires_in，口径见规格 §2 的
//!                  `expiry_timestamp`）
//!   projectId      可选（cloudaicompanionProject，见 `project.rs`）
//!   email          可选（userinfo；展示名与账号身份的来源）
//! ```
//!
//! ── 过期判定：判不出来就说「不需要刷」─────────────────────────
//! 与其它家同一纪律（见 `ProviderAdapter::credentials_expiring` 的契约）：
//! 记录里没有 `expiresAt`（手工粘贴只给了 refreshToken）时返回 false ——
//! 那种账号靠 401 之后的懒刷新链路救，不该让维护任务每轮都去打一次上游。
//! 提前量取 [`REFRESH_SKEW_MS`]（规格 §1.5：Manager 的
//! `TOKEN_REFRESH_SKEW_SECONDS = 900`，即提前 15 分钟）。
//!
//! ── 为什么不做「停用账号」（与规格 §1.6 的差异）────────────────
//! Manager 在 refresh 返回 `invalid_grant` 时把账号标成 `disabled: true` 并从
//! token 池摘除。**本仓没有这种机制**（账号记录里没有 `disabled` 字段，也没有
//! 「停用」这条产品路径），既有几家（Cline / Qoder / Trae）的处置都是
//! 「报一个可识别的永久性错误，让用户重新登录」——本家照做，见
//! `oauth::refresh_access_token` 的错误分支。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

/// 刷新提前量（毫秒）：规格 §1.5 的 `TOKEN_REFRESH_SKEW_SECONDS = 900`
pub const REFRESH_SKEW_MS: i64 = 900 * 1000;

/// refresh_token 的形态前缀（Google 的 refresh token 以 `1//` 开头）。
///
/// 它是**令牌本体的一部分**，不是可以剥掉的装饰前缀 —— 归一化时只负责把
/// 用户粘贴时带上的一整行包装（变量名、引号、冒号）剥掉，这几位必须留着。
pub const REFRESH_TOKEN_PREFIX: &str = "1//";

/// access_token 的常见前缀（`ya29.…`），仅用于日志脱敏与形态提示
pub const ACCESS_TOKEN_PREFIX: &str = "ya29.";

/// 一次转发 / 刷新 / 目录拉取用的凭证快照。
#[derive(Clone, Debug)]
pub struct AntigravityCredentials {
    /// 账号记录 id
    pub id: String,
    /// 主凭证（长寿命；为空表示这条账号不可用/不可续期）
    pub refresh_token: String,
    /// 短期访问令牌（可能为空 —— 需要先刷新）
    pub access_token: String,
    /// 访问令牌过期时刻（毫秒；0 = 判不出来）
    pub expires_at: i64,
    /// `cloudaicompanionProject`（可能为空 —— 由 `project.rs` 发现）
    pub project_id: String,
    /// 账号邮箱（可能为空 —— 展示名与身份来源）
    pub email: String,
    /// 展示名（记录的备注名）
    pub name: String,
}

impl AntigravityCredentials {
    /// 有没有续期手段（refresh_token 非空）
    pub fn can_refresh(&self) -> bool {
        !self.refresh_token.trim().is_empty()
    }

    /// 有没有可用的 access_token（注意：它可能已经过期 —— 过期与否看
    /// [`Self::expiring`]）
    pub fn has_access_token(&self) -> bool {
        !self.access_token.trim().is_empty()
    }

    /// 该账号此刻是否**已过期或进入提前量窗口**（[`REFRESH_SKEW_MS`]）。
    ///
    /// 过期时间判不出来（`expires_at <= 0`）→ false（见模块头）。没有
    /// access_token（从未刷过 / 手工粘贴只给了 refreshToken）→ true：那种账号
    /// 必须先刷一次才能用。
    pub fn expiring(&self) -> bool {
        if !self.has_access_token() {
            return true;
        }
        if self.expires_at <= 0 {
            return false;
        }
        self.expires_at - REFRESH_SKEW_MS <= crate::server::logging::now_ms()
    }

    /// 显示用的脱敏 refresh token 尾号（只给尾 4 位；**绝不打印完整串**）
    pub fn masked_refresh(&self) -> String {
        let trimmed = self.refresh_token.trim();
        if trimmed.is_empty() {
            return "(空)".to_string();
        }
        format!("…{}", tail_chars(trimmed, 4))
    }
}

/// 取字符串末尾 n 个字符（不足时给整串；UTF-8 安全）
pub fn tail_chars(text: &str, count: usize) -> String {
    let total = text.chars().count();
    let skip = total.saturating_sub(count);
    text.chars().skip(skip).collect()
}

/// 从一条取值链里挑第一个非空字符串（记录可能被手工编辑过，宽容读取）
fn pick(record: &Value, keys: &[&str]) -> String {
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

/// 记录里的过期时刻（毫秒；接受数字与数字字符串两种形态）。
///
/// 手工编辑过的记录可能写成字符串（`"1758000000000"`），照读不误 ——
/// 读不出给 0，由 [`AntigravityCredentials::expiring`] 按「判不出来」处理。
fn expires_at_of(record: &Value) -> i64 {
    match record.get("expiresAt") {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// 从账号记录解析凭证（`record` 为 None 时报「没有可用账号」）。
///
/// 字段兼容：`refreshToken`（本项目落盘名）/ `refresh_token`；`accessToken` /
/// `access_token`；`projectId` / `project_id`；`email`。
///
/// **只要求 refresh_token 存在**：access_token 为空是正常状态（靠刷新补齐），
/// 而 refresh_token 为空说明这条记录不可用 —— 给一句能照做的报错，而不是让
/// 转发层拿空串去撞上游。
pub fn from_record(record: Option<&Value>) -> Result<AntigravityCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的 Antigravity 账号：请在账号页粘贴 Google refresh token 添加",
        ));
    };
    let refresh_token = pick(record, &["refreshToken", "refresh_token"]);
    if refresh_token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "Antigravity 账号缺少 refresh token，请重新粘贴（Google OAuth 的 refresh_token）",
        ));
    }
    Ok(AntigravityCredentials {
        id: pick(record, &["id"]),
        refresh_token,
        access_token: pick(record, &["accessToken", "access_token"]),
        expires_at: expires_at_of(record),
        project_id: pick(record, &["projectId", "project_id"]),
        email: pick(record, &["email"]),
        name: pick(record, &["name"]),
    })
}

/// 记录里的备注名 + 邮箱的展示口径（账号页显示用；公开形态里 `name` 优先）。
pub fn display_name(credentials: &AntigravityCredentials) -> String {
    if !credentials.name.trim().is_empty() {
        return credentials.name.trim().to_string();
    }
    if !credentials.email.trim().is_empty() {
        return credentials.email.trim().to_string();
    }
    format!("Antigravity {}", credentials.masked_refresh())
}
