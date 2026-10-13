//! Command Code 账号：粘贴 `user_` API Key 落账号 + 公开形态。
//!
//! ── 账号字段（与别家的对照）────────────────────────────────────
//! ```text
//!   本项目记录：{ id, provider:"commandcode", name, accessToken(= API Key),
//!                tokenTail, source, priority, enabled, addedAt, updatedAt }
//! ```
//! `accessToken` 落盘存的是**API Key 本体**（键名与别家统一：`has_token()` /
//! `access_token()` 一族认这个键，转发时从 `auth.accessToken` 取）。
//! **没有** `expiresAt` / `refreshToken`：本家 key 不续期、也不过期
//! （`supports_refresh = false`，见 `providers/commandcode/credentials.rs`）。
//!
//! ── 账号 id 的派生（为什么不是前缀 + 尾号）──────────────────────
//! 本家没有「userId」那种稳定身份可读（key 里也没有可解析的用户标识），
//! 而 id 必须**对同一个 key 稳定**（再粘一次要更新同一条记录，而不是新增一条）。
//! 因此取 `sha256("commandcode:account-id:v1\0" + key)` 的前 12 个 hex：
//! 确定性、不可反推 key、不同 key 撞 id 的概率可忽略。
//!
//! 本文件全是「读-改-写」存储操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::commandcode::credentials;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::COMMANDCODE_PROVIDER_ID;
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 输入长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_KEY_LENGTH: usize = 8192;

/// 从 payload 里取第一个非空字符串（候选键覆盖本项目落盘名与粘贴形态）
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

/// key → 稳定的账号 id（见模块头）
fn account_id_for(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"commandcode:account-id:v1\0");
    hasher.update(api_key.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("commandcode-{}", &digest[..12])
}

impl AccountStore {
    /// 取一条 Command Code 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（与 `loomy_account_record`
    /// 的语义一致：转发默认用队首账号）。
    pub fn commandcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == COMMANDCODE_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, COMMANDCODE_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 Command Code 账号（粘贴 API Key 的唯一落账号入口）。
    ///
    /// payload（`login::verify_key` 的返回，或界面直接传来的形状）：
    ///   - `apiKey` / `accessToken` / `token` / `key`：API Key（必填，`user_` 开头）；
    ///
    /// 同一 key 的记录就地更新（保留优先级与启用状态、沿用用户改过的备注名）；
    /// 撞到**别的 provider** 的 id 时报错，绝不覆写（与另外几家同一策略）。
    pub fn add_commandcode_account(
        &self,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let api_key = pick(payload, &["apiKey", "accessToken", "token", "key"]);
        if api_key.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 API Key（apiKey / accessToken / token）",
                400,
            ));
        }
        if api_key.chars().count() > MAX_KEY_LENGTH {
            return Err(AccountStoreError::new("API Key 过长", 400));
        }
        if !credentials::shape_ok(&api_key) {
            return Err(AccountStoreError::new(
                format!(
                    "这不是 Command Code 的 API Key：应以 {} 开头、只含字母数字与 _ -",
                    credentials::KEY_PREFIX
                ),
                400,
            ));
        }
        let id = account_id_for(&api_key);

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != COMMANDCODE_PROVIDER_ID {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一个 Key 的\
                         Command Code 账号（请先处理那个账号）"
                    ),
                    400,
                ));
            }
        }
        // 备注名兜底：显式传入 → 既有记录里的备注名 → 「Command Code <尾号>」
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| format!("Command Code {}", token_tail_of(&api_key)));
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                // 优先级全局唯一（各家共用一条队列），取全库已用号段
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(COMMANDCODE_PROVIDER_ID.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        // 备注名标记：用户显式传了名（或既有记录已打标）时置位 —— 本家走自己的
        // 保存路径（不经过 `store_crud::upsert_account` 的统一打标），必须显式调用
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        record.insert("accessToken".to_string(), Value::String(api_key.clone()));
        record.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&api_key)),
        );
        record.insert("source".to_string(), Value::String(source.to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(
                existing
                    .as_ref()
                    .map(StoredAccount::enabled)
                    .unwrap_or(true),
            ),
        );
        record.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .filter(|value| *value != 0)
                    .unwrap_or(now),
            ),
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
            &format!(
                "✅ Command Code 账号已保存: {record_name}（优先级 {priority}，key {}）",
                credentials::masked(&api_key)
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// Command Code 账号的**公开形态**（进 HTTP 响应）：去掉 key 本体，只留尾几位。
    pub fn to_commandcode_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            // 有没有 key（等价于 has_credentials，但这里给出的是**本家语义**的
            // 字段名，界面按它显示「已配置 / 需重贴」）
            object.insert(
                "hasKey".to_string(),
                Value::Bool(
                    record
                        .get("accessToken")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                ),
            );
        }
        value
    }
}
