//! MonkeyCode 凭证：**只有一条来源** —— 账号记录里的登录 session（粘贴形态）。
//!
//! ── 与别家最大的不同：粘贴式登录、没有续期 ────────────────────
//! 本家的密码登录要 go-cap 验证码、OAuth 要百智云 SCaptcha + 短信（见
//! `endpoints.rs` 的模块头），两条都不适合网关代跑。参考给出的结论是：
//! 从浏览器复制 `monkeycode_ai_session` cookie 是最可靠的一条路。
//! 上游**没有 refresh 接口**（session 30 天硬限制，`docs/02-auth/03-login-methods.md`），
//! 过期只能重新粘贴 —— 因此 `supports_refresh = false`，本模块比别家薄。
//!
//! ── image_id 为什么是账号字段 ────────────────────────────────
//! 创建任务（`task.rs` 的会话转发第一步）必须带 `image_id`（VM 镜像 UUID），
//! 而它**不在**登录响应里：参考实现 `discoverImageId` 从**已有任务列表**的
//! `task.image.id` 里取 —— 新用户一个任务都没有时取不到，必须手动填。因此本
//! 模块把它作为账号记录的一个字段（可为空串），并在 `login.rs` 里做一次
//! best-effort 自动发现。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::region::Region;

/// 会话有效期（`docs/02-auth/03-login-methods.md`：30 天硬限制）
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

/// 临期窗口：到期前 1 天提示重新粘贴（上游无续期，只能重登）
pub const EXPIRY_MARGIN_MS: i64 = 24 * 3600 * 1000;

/// 一次转发 / 查询用的 MonkeyCode 凭证快照。
#[derive(Clone, Debug)]
pub struct MonkeyCodeCredentials {
    /// 账号记录 id
    pub id: String,
    /// 地区（由记录里的 provider id 还原）
    pub region: Region,
    /// 登录 session（`monkeycode_ai_session` 的值）
    pub session: String,
    /// VM 镜像 UUID（创建任务必需；可能为空 —— 见模块头）
    pub image_id: String,
    /// 上游用户 uuid（展示与去重用）
    pub user_id: String,
    /// 展示名（记录的备注名）
    pub name: String,
    /// 估算的过期时刻（毫秒；来自记录 `expiresAt`，缺失时用 `addedAt + 30 天`）
    pub expires_at: i64,
}

impl MonkeyCodeCredentials {
    /// 是否已过期或进入临期窗口（上游无续期手段，这个判定只用于提示重登）
    pub fn expiring(&self) -> bool {
        self.expires_at > 0
            && self.expires_at - EXPIRY_MARGIN_MS <= crate::server::logging::now_ms()
    }

    /// 创建任务是否具备必要条件（session + image_id）。转发入口（`chat.rs`）
    /// 用它在自己真正建任务前给出可读的错误，而不是把空 image_id 发给上游。
    pub fn ready_for_task(&self) -> bool {
        !self.session.trim().is_empty() && !self.image_id.trim().is_empty()
    }
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
/// 地区由记录里的 `provider` 还原（两个地区是两家 provider，记录自带归属）；
/// 认不出归属时按国内版处置（调用方只会在本家 kind 上走到这里，落到这一步
/// 只可能是手工改过数据 —— 与「回落到一个能用的站」比报错更少打断用户，
/// 且 401 会立刻暴露站点选错）。
pub fn from_record(record: Option<&Value>) -> Result<MonkeyCodeCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的 MonkeyCode 账号：请在账号页粘贴 session 添加",
        ));
    };
    let session = pick(record, &["accessToken", "session", "token"]);
    if session.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "MonkeyCode 账号缺少 session，请重新粘贴登录态",
        ));
    }
    let provider = pick(record, &["provider"]);
    let region = Region::from_provider_id(&provider).unwrap_or_default();
    let image_id = pick(record, &["imageId", "image_id"]);
    let user_id = pick(record, &["userId", "user_id", "userid"]);
    let name = pick(record, &["name"]);
    let id = pick(record, &["id"]);
    let explicit_expiry = record.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    let added_at = record.get("addedAt").and_then(Value::as_i64).unwrap_or(0);
    let expires_at = if explicit_expiry > 0 {
        explicit_expiry
    } else if added_at > 0 {
        added_at + SESSION_TTL_MS
    } else {
        0
    };
    Ok(MonkeyCodeCredentials {
        id,
        region,
        session,
        image_id,
        user_id,
        name,
        expires_at,
    })
}
