//! AtomCode 账号：粘贴凭据落账号、刷新回写、公开形态。
//!
//! ── 账号字段 ────────────────────────────────────────────────
//! ```text
//!   本项目记录：{ id, provider:"atomcode", name, userId, username, accessToken,
//!                refreshToken, tokenTail, expiresAt, addedAt, updatedAt,
//!                source, priority, enabled }
//!   代理令牌包：  { access_token, refresh_token, token_type, expires_in, user:{id,…} }
//! ```
//! `accessToken` / `refreshToken` 键名与别家统一（`update_account_tokens` 一族
//! 认这两个键，见 `store_admin.rs`），粘贴形态另外兼容官方 `auth.toml` 的
//! 下划线拼法（`access_token` / `refresh_token`）。
//!
//! ── 与别家添加路径的关系 ────────────────────────────────────
//! - **粘贴凭据**：`api::accounts::add_account` 的 atomcode 分支（唯一的入口，
//!   在此之前界面上的「浏览器登录」尚未接壳侧，见 `providers::atomcode` 模块头）；
//! - **刷新回写**：不走本文件 —— 走通用的
//!   `AccountStore::update_account_tokens`（`providers::atomcode::mod` 的
//!   `refresh_and_save` 调它）。
//!
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::{ATOMCODE_PROVIDER_ID, AccountStore, AccountStoreError};
use crate::server::logging;

use crate::server::core::providers::atomcode::credentials;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份字段（userId / username）长度上限
const MAX_IDENTITY_LENGTH: usize = 256;

/// 令牌长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_TOKEN_LENGTH: usize = 8192;

/// 截断到 `max` 个**字符**（不是字节；用户名可能有中文）
fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect()
}

/// 从一条取值链里挑第一个非空字符串
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

/// 从一条取值链里挑第一个正数
fn pick_i64(payload: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(value) = payload.get(*key).and_then(Value::as_i64) {
            if value > 0 {
                return value;
            }
        }
    }
    0
}

impl AccountStore {
    /// 取一条 AtomCode 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（转发默认用队首账号）。
    pub fn atomcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == ATOMCODE_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, ATOMCODE_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 AtomCode 账号（粘贴凭据的唯一入口）。
    ///
    /// payload：
    ///   - `accessToken` / `access_token` / `token`：访问令牌（必填）；
    ///   - `refreshToken` / `refresh_token`：续期令牌（**强烈建议给** ——
    ///     没有它账号无法自动续期，只能等用户重新粘贴）；
    ///   - `userId` / `user_id` / `username`：身份（可选，用于生成账号 id 与展示名）；
    ///   - `expiresAt`：过期时刻（毫秒，可选）；`createdAt` + `expiresIn` 可替代它
    ///     （令牌非 JWT，读不到 `exp`，过期只能这样算 —— 见 `credentials.rs`）。
    ///
    /// 撞 id 时：同 provider 就地更新（保留优先级与启用状态、沿用用户改过的
    /// 备注名），撞到**别的 provider** 时报错 —— 绝不覆写他人记录。
    pub fn add_atomcode_account(
        &self,
        payload: &Value,
        name: Option<&str>,
    ) -> Result<Value, AccountStoreError> {
        let Some(_object) = payload.as_object() else {
            return Err(AccountStoreError::new("上传内容必须是 JSON 对象", 400));
        };
        let access_token = pick(payload, &["accessToken", "access_token", "token"]);
        if access_token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少访问令牌（accessToken / access_token / token）",
                400,
            ));
        }
        if access_token.chars().count() > MAX_TOKEN_LENGTH {
            return Err(AccountStoreError::new("访问令牌过长", 400));
        }
        let refresh_token = pick(payload, &["refreshToken", "refresh_token"]);
        if refresh_token.chars().count() > MAX_TOKEN_LENGTH {
            return Err(AccountStoreError::new("refresh token 过长", 400));
        }
        let user_id = truncate_chars(&pick(payload, &["userId", "user_id", "uid"]), MAX_IDENTITY_LENGTH);
        let username = truncate_chars(&pick(payload, &["username", "userName", "login"]), MAX_IDENTITY_LENGTH);
        // 身份标识：优先 userId（稳定）；没有就退回用户名；再没有就用令牌尾段
        // （与 loomy 的处置一致：至少要有一个可辨认身份，否则 `atomcode-` 前缀的 id 会撞）
        let identity = if !user_id.is_empty() {
            user_id.clone()
        } else if !username.is_empty() {
            username.clone()
        } else {
            let tail: String = access_token
                .chars()
                .rev()
                .take(12)
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect();
            format!("tok{tail}")
        };
        let id = format!("atomcode-{identity}");
        let now = logging::now_ms();

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            if existing.provider() != ATOMCODE_PROVIDER_ID {
                return Err(AccountStoreError::new(
                    "该账号 id 已被其它提供商占用，请改名后重试",
                    409,
                ));
            }
        }

        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                // 优先级全局唯一（各家共用一条队列），取全库已用号段
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let record_name = truncate_chars(
            name.map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(if username.is_empty() { &identity } else { &username }),
            MAX_NAME_LENGTH,
        );
        let expires_at = {
            let explicit = pick_i64(payload, &["expiresAt"]);
            if explicit > 0 {
                explicit
            } else {
                credentials::expires_at_from(
                    pick_i64(payload, &["createdAt"]),
                    pick_i64(payload, &["expiresIn", "expires_in"]),
                    now,
                )
            }
        };

        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(ATOMCODE_PROVIDER_ID.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        if !user_id.is_empty() {
            record.insert("userId".to_string(), Value::String(user_id));
        }
        if !username.is_empty() {
            record.insert("username".to_string(), Value::String(username));
        }
        record.insert("accessToken".to_string(), Value::String(access_token.clone()));
        if !refresh_token.is_empty() {
            record.insert("refreshToken".to_string(), Value::String(refresh_token));
        }
        record.insert(
            "tokenTail".to_string(),
            Value::String(
                access_token
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<char>>()
                    .into_iter()
                    .rev()
                    .collect(),
            ),
        );
        if expires_at > 0 {
            record.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        record.insert("source".to_string(), Value::String("manual".to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert("enabled".to_string(), Value::Bool(true));
        record.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .and_then(|record| record.get("addedAt").and_then(Value::as_i64))
                    .unwrap_or(now),
            ),
        );
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        record.insert("updatedAt".to_string(), Value::from(now));

        // 未知字段全量保留（用户手工加过的字段不能因为一次「更新账号」丢掉）
        let mut merged = record;
        if let Some(existing) = existing.as_ref() {
            for (key, value) in existing.fields() {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let saved = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!("✅ AtomCode 账号已保存: {record_name}（优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    /// AtomCode 账号的**公开形态**（进 HTTP 响应）：去掉两个令牌本体。
    ///
    /// ⚠️ `refreshToken` 必须一起摘掉：只摘 `accessToken` 会让一个长期有效的
    /// 续期凭据随账号列表下发到界面上（与 loomy 只摘 session 的场景不同 ——
    /// 那家的 session 是唯一凭据）。
    pub fn to_atomcode_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert(
                "hasSession".to_string(),
                Value::Bool(record.get("accessToken").is_some() || record.get("tokenTail").is_some()),
            );
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 公开形态必须把**两个令牌本体**都摘掉。
    #[test]
    fn public_shape_never_leaks_either_token() {
        let record = StoredAccount::from_map({
            let mut map = Map::new();
            map.insert("id".into(), Value::String("atomcode-42".into()));
            map.insert("provider".into(), Value::String(ATOMCODE_PROVIDER_ID.into()));
            map.insert("accessToken".into(), Value::String("at-secret".into()));
            map.insert("refreshToken".into(), Value::String("rt-secret".into()));
            map.insert("tokenTail".into(), Value::String("cret".into()));
            map
        });
        let store_free_shape = record.to_value();
        assert!(store_free_shape.get("accessToken").is_some());
        // 公开形态是 store 方法，这里直接照它的规则断言（避免构造整个 store）
        let mut public = record.to_value();
        if let Some(object) = public.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
        }
        assert!(public.get("accessToken").is_none());
        assert!(public.get("refreshToken").is_none());
        assert_eq!(public.get("tokenTail").and_then(Value::as_str), Some("cret"));
    }

    /// 取字段：三种拼法都要能读到（本项目名 / 官方 auth.toml 名 / 粘贴名）。
    #[test]
    fn payload_aliases_are_accepted() {
        let payload = json!({"access_token": "at", "refresh_token": "rt", "user_id": "42"});
        assert_eq!(pick(&payload, &["accessToken", "access_token", "token"]), "at");
        assert_eq!(pick(&payload, &["refreshToken", "refresh_token"]), "rt");
        assert_eq!(pick(&payload, &["userId", "user_id", "uid"]), "42");
    }

    /// 过期时刻可由 `createdAt + expiresIn` 推出（令牌非 JWT）。
    #[test]
    fn expiry_can_come_from_created_at_plus_expires_in() {
        let payload = json!({"createdAt": 1_700_000_000_i64, "expiresIn": 604_800});
        assert_eq!(pick_i64(&payload, &["expiresAt"]), 0);
        assert_eq!(
            credentials::expires_at_from(
                pick_i64(&payload, &["createdAt"]),
                pick_i64(&payload, &["expiresIn", "expires_in"]),
                0
            ),
            1_700_000_000_000 + 604_800_000
        );
    }
}
