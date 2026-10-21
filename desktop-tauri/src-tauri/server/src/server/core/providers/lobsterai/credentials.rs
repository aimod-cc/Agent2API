//! LobsterAI 凭证：**uuid 与两个 keyfrom 是载荷件，丢了账号就永远续不了期**。
//!
//! ── 为什么这一家的凭证字段比别家多 ──────────────────────────────
//! 上游的续期接口不认「只给 refresh token」：它要的是**一组身份字段**再加
//! refresh token（参考实现 `auth.go:191-196` 的 `refreshBody`）：
//!
//! ```text
//!   { firstKeyfrom, latestKeyfrom, version, [uuid], [userId], refreshToken }
//! ```
//!
//! 而 `uuid` 与那两个 keyfrom 只在**换码那一刻**由上游下发，此后再也拿不到。
//! 所以落账号时**必须把它们一起存下来** —— 只存 access/refresh 两个 token 的
//! 账号，看起来能用到 access 过期，之后既续不了期也修不回来（只能重新登录）。
//! 参考实现把这条写成模块头的第一句警告，这里照抄这个约束。
//!
//! `latest_keyfrom` 还额外有个「故意不更新」的口径：续期响应里即使带了新的
//! keyfrom 也不覆盖它，永远保持登录时那一份 —— 这是**唯一经过生产验证**的形态
//! （Go 参考 `auth.go:49-53` 与 TS 版 `lobsterai.ts:378-410` 都这么做，并在
//! 注里写明「不是为了省事，是照实测」）。本实现跟随。
//!
//! ── 过期时刻 ────────────────────────────────────────────────
//! 上游给 `expiresIn`（秒），落盘时算成 `expiresAt`（毫秒）。access token 是
//! HS512 的 JWT（30 天），所以**判不出 `expiresAt` 时可以退回读 JWT 的 `exp`**；
//! 两条都没有就返回 0 = 判不出来（不猜，见 `expiring`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

use crate::server::errors::GatewayError;
use crate::server::logging;

/// 一次转发 / 查询用的 LobsterAI 凭证快照
#[derive(Clone, Debug)]
pub struct LobsterCredentials {
    /// 账号记录 id
    pub id: String,
    /// 访问令牌（HS512 JWT，约 30 天）
    pub access_token: String,
    /// 续期令牌（可能为空 —— 那种账号只能重新登录）
    pub refresh_token: String,
    /// 过期时刻（毫秒）。0 = 推不出来
    pub expires_at: i64,
    /// 上游用户 id（积分接口与 keyfrom 都用）
    pub uid: String,
    /// keyfrom 载荷件之二（见模块头）
    pub user_id: String,
    /// 展示名
    pub nickname: String,
    /// **载荷件**：换码时下发，续期必须原样回传
    pub uuid: String,
    /// **载荷件**：登录时那一份，续期时原样回传且**永不更新**
    pub first_keyfrom: String,
    /// **载荷件**：同 [`Self::first_keyfrom`]
    pub latest_keyfrom: String,
}

impl LobsterCredentials {
    /// 还能不能续期：有 refresh token **且** 载荷件齐全。
    ///
    /// 缺任一载荷件时上游会拒（或更糟：给一个不认识的错误），所以这里先判死 ——
    /// 让「这个账号续不了期」在界面上是一句可读的话，而不是一次玄学失败。
    pub fn can_refresh(&self) -> bool {
        !self.refresh_token.trim().is_empty()
            && !self.first_keyfrom.trim().is_empty()
            && !self.latest_keyfrom.trim().is_empty()
    }

    /// 是否已过期或进入提前续期窗口（到期前 1 小时）。
    pub fn expiring(&self) -> bool {
        const WINDOW_MS: i64 = 3600 * 1000;
        self.expires_at > 0 && self.expires_at - WINDOW_MS <= logging::now_ms()
    }

    /// keyfrom 载荷（`firstKeyfrom` / `latestKeyfrom` / `version` 恒在，
    /// `uuid` 与 `userId` 非空才带）。
    ///
    /// 两个消费点共用这一份：目录端点把它拼成 **query**（`models.go:523-551`，
    /// 特意不带 refresh token —— query 会进服务端访问日志），续期端点把它当
    /// **body** 再补一个 `refreshToken`。
    pub fn keyfrom_fields(&self, client_version: &str) -> Vec<(&'static str, String)> {
        let mut fields: Vec<(&'static str, String)> = vec![
            ("firstKeyfrom", self.first_keyfrom.clone()),
            ("latestKeyfrom", self.latest_keyfrom.clone()),
            ("version", client_version.to_string()),
        ];
        if !self.uuid.trim().is_empty() {
            fields.push(("uuid", self.uuid.clone()));
        }
        if !self.user_id.trim().is_empty() {
            fields.push(("userId", self.user_id.clone()));
        }
        // 与 Go 的 `url.Values.Encode()` 同序（按 key 排序），
        // 让同样的账号每次拼出同样的串 —— 便于逐字节对拍与排错
        fields.sort_by(|a, b| a.0.cmp(b.0));
        fields
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

/// 读一个毫秒时刻（数字或数字字符串都认）
fn pick_ms(record: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(value) = record.get(*key) {
            if let Some(number) = value.as_i64() {
                if number > 0 {
                    return number;
                }
            }
            if let Some(text) = value.as_str() {
                if let Ok(number) = text.trim().parse::<i64>() {
                    if number > 0 {
                        return number;
                    }
                }
            }
        }
    }
    0
}

/// 读一个秒数（`expiresIn`）
fn pick_secs(record: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(number) = record.get(*key).and_then(Value::as_i64) {
            if number > 0 {
                return number;
            }
        }
    }
    0
}

/// 从 JWT 里读 `exp`（毫秒）。读不出来返回 None。
///
/// access token 是 HS512 的三段点分 JWT，`exp` 在第二段的 claims 里。
/// **只解码不验签**：这是本地判过期用的，签名由上游校验。
pub fn jwt_exp_ms(token: &str) -> Option<i64> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let raw = URL_SAFE_NO_PAD.decode(payload.trim()).ok()?;
    let claims: Value = serde_json::from_slice(&raw).ok()?;
    let exp = claims.get("exp").and_then(Value::as_f64)?;
    if !exp.is_finite() || exp <= 0.0 {
        return None;
    }
    Some((exp * 1000.0) as i64)
}

/// 由 `createdAt + expiresIn` 或 JWT `exp` 推出过期时刻（毫秒），都没有则 0。
pub fn expires_at_from(created_at_ms: i64, expires_in_secs: i64, access_token: &str) -> i64 {
    if created_at_ms > 0 && expires_in_secs > 0 {
        return created_at_ms + expires_in_secs * 1000;
    }
    // 退回 JWT 的 exp（HS512，实测 30 天）
    jwt_exp_ms(access_token).unwrap_or(0)
}

/// 从账号记录解析凭证（**唯一入口**；`record` 为 None 时报「没有可用账号」）。
///
/// 字段兼容：本项目的驼峰名（`accessToken` / `refreshToken` / `firstKeyfrom`…）
/// 与 Go 参考里的下划线名（`access_token` / `first_keyfrom`…）都认。
pub fn from_record(record: Option<&Value>) -> Result<LobsterCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的 LobsterAI 账号：请在账号页用浏览器登录添加",
        ));
    };
    let access_token = pick(record, &["accessToken", "access_token"]);
    if access_token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "LobsterAI 账号缺少访问令牌，请重新登录",
        ));
    }
    let created_at = pick_ms(record, &["createdAt", "created_at"]);
    let expires_in = pick_secs(record, &["expiresIn", "expires_in"]);
    let explicit_expiry = pick_ms(record, &["expiresAt", "expires_at"]);
    let expires_at = if explicit_expiry > 0 {
        explicit_expiry
    } else {
        expires_at_from(created_at, expires_in, &access_token)
    };
    Ok(LobsterCredentials {
        id: pick(record, &["id"]),
        refresh_token: pick(record, &["refreshToken", "refresh_token"]),
        access_token,
        expires_at,
        uid: pick(record, &["uid", "userId", "user_id"]),
        user_id: pick(record, &["userId", "user_id"]),
        nickname: pick(record, &["name", "nickname"]),
        uuid: pick(record, &["uuid", "UUID"]),
        first_keyfrom: pick(record, &["firstKeyfrom", "first_keyfrom"]),
        latest_keyfrom: pick(record, &["latestKeyfrom", "latest_keyfrom"]),
    })
}

/// 账号展示名（昵称优先，其次 uid，最后账号 id）
pub fn display_name(nickname: &str, uid: &str, id: &str) -> String {
    for candidate in [nickname, uid, id] {
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "LobsterAI 账号".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_record() -> Value {
        json!({
            "id": "lobsterai-1",
            "accessToken": "at", "refreshToken": "rt",
            "uuid": "u-1", "firstKeyfrom": "fk", "latestKeyfrom": "lk",
            "userId": "42", "name": "龙虾用户",
            "createdAt": 1_700_000_000_000_i64, "expiresIn": 2_592_000
        })
    }

    /// 载荷件三件套都要读出来 —— 缺一个账号就续不了期（模块头的第一约束）。
    #[test]
    fn payload_fields_are_read_and_gate_refresh() {
        let record = base_record();
        let credentials = from_record(Some(&record)).expect("应可解析");
        assert_eq!(credentials.uuid, "u-1");
        assert_eq!(credentials.first_keyfrom, "fk");
        assert_eq!(credentials.latest_keyfrom, "lk");
        assert!(credentials.can_refresh());

        // 少一个 keyfrom ⇒ 判「不可续期」，而不是让它去撞一次必败的请求
        let mut broken = base_record();
        if let Some(object) = broken.as_object_mut() {
            object.remove("latestKeyfrom");
        }
        let credentials = from_record(Some(&broken)).expect("仍应可解析");
        assert!(!credentials.can_refresh());
    }

    /// keyfrom 载荷：三个恒在 + 两个可选；顺序按 key 排序（与 Go 的 Encode 同序）。
    #[test]
    fn keyfrom_fields_are_sorted_and_omit_empty_optionals() {
        let credentials = from_record(Some(&base_record())).expect("应可解析");
        let fields = credentials.keyfrom_fields("2026.9.4");
        let keys: Vec<&str> = fields.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, vec!["firstKeyfrom", "latestKeyfrom", "userId", "uuid", "version"]);
        assert_eq!(fields[4].1, "2026.9.4");

        // uuid / userId 为空时不带这两个键
        let mut record = base_record();
        if let Some(object) = record.as_object_mut() {
            object.remove("uuid");
            object.remove("userId");
        }
        let credentials = from_record(Some(&record)).expect("应可解析");
        let keys: Vec<&str> = credentials
            .keyfrom_fields("2026.9.4")
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(keys, vec!["firstKeyfrom", "latestKeyfrom", "version"]);
    }

    /// 过期时刻：`createdAt + expiresIn` 优先；都没有时退回 JWT 的 `exp`。
    #[test]
    fn expiry_prefers_created_at_then_falls_back_to_jwt() {
        assert_eq!(
            expires_at_from(1_700_000_000_000, 2_592_000, "irrelevant"),
            1_700_000_000_000 + 2_592_000_000
        );
        // 两个输入都没有 ⇒ 去读 JWT；这里给一个造出来的三段 token
        let claims = serde_json::json!({"exp": 1_700_000_000_i64});
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap_or_default());
        let token = format!("h.{payload}.s");
        assert_eq!(expires_at_from(0, 0, &token), 1_700_000_000_000);
        // 都不是 JWT ⇒ 判不出来（0），不猜
        assert_eq!(expires_at_from(0, 0, "not-a-jwt"), 0);
    }

    /// 缺访问令牌时报「重新登录」，而不是拿空串去发请求。
    #[test]
    fn missing_token_is_an_auth_error() {
        let error = from_record(Some(&json!({"refreshToken": "rt"}))).expect_err("应报错");
        assert_eq!(error.status_code, 401);
        assert!(error.message.contains("重新登录"));
        assert_eq!(from_record(None).expect_err("无账号应报错").status_code, 401);
    }

    /// 展示名优先级：昵称 → uid → id。
    #[test]
    fn display_name_falls_back_in_order() {
        assert_eq!(display_name("虾", "42", "id"), "虾");
        assert_eq!(display_name("", "42", "id"), "42");
        assert_eq!(display_name("", "", "id"), "id");
        assert_eq!(display_name("", "", ""), "LobsterAI 账号");
    }

    /// 常量哨兵存在（防止有人把兜底版本号删掉）
    #[test]
    fn fallback_version_sentinel_is_present() {
        assert!(super::super::endpoints::FALLBACK_CLIENT_VERSION.starts_with("2026."));
    }
}
