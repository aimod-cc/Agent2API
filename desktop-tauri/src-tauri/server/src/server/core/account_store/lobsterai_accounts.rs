//! LobsterAI 账号：浏览器登录落账号、公开形态。
//!
//! ── 这一家的账号记录比别家多三样东西（**别省**）──────────────
//! `uuid` 与两个 `keyfrom` 只在**换码那一刻**由上游下发，续期时必须原样回传。
//! 只存 access/refresh 两个 token 的账号，能用到 access 过期为止，之后既续不了
//! 期也修不回来（只能重新登录）。理由与字段语义见 `providers::lobsterai::credentials`
//! 的模块头。
//!
//! ── 与别家添加路径的关系 ────────────────────────────────────
//! 只有一个入口：网页登录（`api::session` 的 loopback 回调 → 本文件的
//! `add_lobsterai_account`）。**没有桌面端导入**（客户端登录态在它自己的
//! sqlite 里，不在稳定可读的文件形态里），也没有粘贴路径（粘贴给不出载荷件三件套）。
//!
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::{AccountStore, AccountStoreError, LOBSTERAI_PROVIDER_ID};
use crate::server::core::providers::lobsterai::credentials;
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份字段长度上限
const MAX_IDENTITY_LENGTH: usize = 256;

/// 令牌长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_TOKEN_LENGTH: usize = 8192;

/// keyfrom / uuid 的长度上限（它们是短标识，不该有几千字符）
const MAX_PAYLOAD_FIELD_LENGTH: usize = 512;

/// 截断到 `max` 个**字符**（不是字节；昵称可能有中文）
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

/// 读一个毫秒时刻（数字或数字字符串都认）
fn pick_ms(payload: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(value) = payload.get(*key) {
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
fn pick_secs(payload: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(number) = payload.get(*key).and_then(Value::as_i64) {
            if number > 0 {
                return number;
            }
        }
    }
    0
}

impl AccountStore {
    /// 取一条 LobsterAI 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条。
    pub fn lobsterai_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == LOBSTERAI_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, LOBSTERAI_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 落一个 LobsterAI 账号（网页登录换码后的唯一入口）。
    ///
    /// payload（换码结果的形状）：
    ///   - `accessToken` / `access_token`：访问令牌（必填）；
    ///   - `refreshToken`：续期令牌（**强烈建议有** —— 没有它账号只能重新登录）；
    ///   - `uuid`：**载荷件**，续期必须回传；
    ///   - `firstKeyfrom` / `latestKeyfrom`：**载荷件**，同上；
    ///   - `userId` / `uid`：身份与展示；
    ///   - `name` / `nickname`：展示名；
    ///   - `expiresAt` 或 `createdAt` + `expiresIn`：过期时刻（毫秒）。
    ///
    /// ⚠️ 载荷件与 refreshToken 缺失**不阻止落账号**：登录当时换码是成功的，
    /// 账号能一直用到 access 过期；只是那种账号续不了期，界面上会按
    /// `can_refresh()` 如实标出来（与「不给落」相比，用户至少能用一阵子）。
    pub fn add_lobsterai_account(
        &self,
        payload: &Value,
        name: Option<&str>,
    ) -> Result<Value, AccountStoreError> {
        if payload.as_object().is_none() {
            return Err(AccountStoreError::new("上传内容必须是 JSON 对象", 400));
        }
        let access_token = pick(payload, &["accessToken", "access_token"]);
        if access_token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少访问令牌（accessToken / access_token）",
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
        let user_id = truncate_chars(
            &pick(payload, &["userId", "user_id"]),
            MAX_IDENTITY_LENGTH,
        );
        let uid = truncate_chars(&pick(payload, &["uid", "id"]), MAX_IDENTITY_LENGTH);
        let nickname = truncate_chars(
            &pick(payload, &["name", "nickname"]),
            MAX_NAME_LENGTH,
        );
        let uuid = truncate_chars(&pick(payload, &["uuid", "UUID"]), MAX_PAYLOAD_FIELD_LENGTH);
        let first_keyfrom = truncate_chars(
            &pick(payload, &["firstKeyfrom", "first_keyfrom"]),
            MAX_PAYLOAD_FIELD_LENGTH,
        );
        let latest_keyfrom = truncate_chars(
            &pick(payload, &["latestKeyfrom", "latest_keyfrom"]),
            MAX_PAYLOAD_FIELD_LENGTH,
        );

        // 身份标识：优先 userId（稳定）→ uid → 令牌尾段
        let identity = if !user_id.is_empty() {
            user_id.clone()
        } else if !uid.is_empty() {
            uid.clone()
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
        let id = format!("lobsterai-{identity}");
        let now = logging::now_ms();

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            if existing.provider() != LOBSTERAI_PROVIDER_ID {
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
                .unwrap_or_else(|| {
                    if nickname.is_empty() {
                        &identity
                    } else {
                        &nickname
                    }
                }),
            MAX_NAME_LENGTH,
        );

        // 过期时刻：显式 > createdAt+expiresIn > JWT 的 exp
        let explicit = pick_ms(payload, &["expiresAt", "expires_at"]);
        let expires_at = if explicit > 0 {
            explicit
        } else {
            credentials::expires_at_from(
                pick_ms(payload, &["createdAt", "created_at"]).max(now),
                pick_secs(payload, &["expiresIn", "expires_in"]),
                &access_token,
            )
        };

        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(LOBSTERAI_PROVIDER_ID.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        if !uid.is_empty() {
            record.insert("uid".to_string(), Value::String(uid));
        }
        if !user_id.is_empty() {
            record.insert("userId".to_string(), Value::String(user_id));
        }
        if !nickname.is_empty() {
            record.insert("nickname".to_string(), Value::String(nickname));
        }
        record.insert(
            "accessToken".to_string(),
            Value::String(access_token.clone()),
        );
        if !refresh_token.is_empty() {
            record.insert(
                "refreshToken".to_string(),
                Value::String(refresh_token),
            );
        }
        // ── 载荷件三件套（续期要用，见模块头）────────────────────
        if !uuid.is_empty() {
            record.insert("uuid".to_string(), Value::String(uuid));
        }
        if !first_keyfrom.is_empty() {
            record.insert("firstKeyfrom".to_string(), Value::String(first_keyfrom));
        }
        if !latest_keyfrom.is_empty() {
            record.insert("latestKeyfrom".to_string(), Value::String(latest_keyfrom));
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
        record.insert("source".to_string(), Value::String("web".to_string()));
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
            &format!("✅ LobsterAI 账号已保存: {record_name}（优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    /// LobsterAI 账号的**公开形态**（进 HTTP 响应）：去掉令牌本体。
    ///
    /// `refreshToken` 与载荷件**都要摘干净**：前者是长期有效的续期凭据，后者是
    /// 续期的身份字段 —— 一起下发等于把「这个账号能被别人续期」的条件送出去。
    /// 界面需要的「能不能续期」由 `canRefresh` 布尔表达，不给原件。
    pub fn to_lobsterai_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        let can_refresh = record.get("refreshToken").is_some()
            && record.get("firstKeyfrom").is_some()
            && record.get("latestKeyfrom").is_some();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
            object.remove("uuid");
            object.remove("firstKeyfrom");
            object.remove("latestKeyfrom");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert("hasSession".to_string(), Value::Bool(true));
            object.insert("canRefresh".to_string(), Value::Bool(can_refresh));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 公开形态必须把**令牌本体 + 载荷件**都摘掉（只留一个 canRefresh 布尔）。
    #[test]
    fn public_shape_never_leaks_tokens_or_payload_fields() {
        let record = StoredAccount::from_map({
            let mut map = Map::new();
            map.insert("id".into(), Value::String("lobsterai-42".into()));
            map.insert("provider".into(), Value::String(LOBSTERAI_PROVIDER_ID.into()));
            map.insert("accessToken".into(), Value::String("at".into()));
            map.insert("refreshToken".into(), Value::String("rt".into()));
            map.insert("uuid".into(), Value::String("u-1".into()));
            map.insert("firstKeyfrom".into(), Value::String("fk".into()));
            map.insert("latestKeyfrom".into(), Value::String("lk".into()));
            map
        });
        let mut public = record.to_value();
        let can_refresh = public.get("refreshToken").is_some()
            && public.get("firstKeyfrom").is_some()
            && public.get("latestKeyfrom").is_some();
        if let Some(object) = public.as_object_mut() {
            for key in [
                "accessToken",
                "refreshToken",
                "uuid",
                "firstKeyfrom",
                "latestKeyfrom",
            ] {
                object.remove(key);
            }
            object.insert("canRefresh".into(), Value::Bool(can_refresh));
        }
        for key in [
            "accessToken",
            "refreshToken",
            "uuid",
            "firstKeyfrom",
            "latestKeyfrom",
        ] {
            assert!(public.get(key).is_none(), "{key} 不该出现在公开形态里");
        }
        assert_eq!(public.get("canRefresh"), Some(&Value::Bool(true)));
    }

    /// 载荷件两种拼法都要能读到（本项目驼峰 / Go 参考下划线）。
    #[test]
    fn payload_aliases_are_accepted() {
        let payload = json!({
            "access_token": "at", "refresh_token": "rt",
            "first_keyfrom": "fk", "latest_keyfrom": "lk", "user_id": "42"
        });
        assert_eq!(pick(&payload, &["accessToken", "access_token"]), "at");
        assert_eq!(pick(&payload, &["refreshToken", "refresh_token"]), "rt");
        assert_eq!(pick(&payload, &["firstKeyfrom", "first_keyfrom"]), "fk");
        assert_eq!(pick(&payload, &["latestKeyfrom", "latest_keyfrom"]), "lk");
        assert_eq!(pick(&payload, &["userId", "user_id"]), "42");
    }

    /// 过期时刻可由 `createdAt + expiresIn` 推（与 atomcode 同一条教训：单位别混）。
    #[test]
    fn expiry_can_come_from_created_at_plus_expires_in() {
        let payload = json!({"createdAt": 1_700_000_000_000_i64, "expiresIn": 2_592_000});
        assert_eq!(pick_ms(&payload, &["expiresAt"]), 0);
        assert_eq!(pick_secs(&payload, &["expiresIn"]), 2_592_000);
        assert_eq!(
            credentials::expires_at_from(
                pick_ms(&payload, &["createdAt"]),
                pick_secs(&payload, &["expiresIn"]),
                "not-a-jwt"
            ),
            1_700_000_000_000 + 2_592_000_000
        );
    }
}
