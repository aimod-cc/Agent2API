//! Loomy 账号：手机号登录落账号、粘贴 session、公开形态。
//!
//! ── 账号字段（对照客户端登录响应的实际字段）────────────────────
//! ```text
//!   本项目记录：{ id, provider:"loomy", name, userId, phone, accessToken(=session),
//!                tokenTail, expiresAt, addedAt, updatedAt, source, priority, enabled }
//!   客户端登录响应：{ session, userid, phone }（data.data，见 providers/loomy/login.rs）
//! ```
//! `accessToken` 落盘存的是**登录 session**（它同时是模型网关的 `token`）——
//! 键名与别家统一（`has_token()` / `access_token()` 一族认这个键），
//! 粘贴形态另外兼容 `session` / `token`。
//!
//! ── 与别家添加路径的关系 ────────────────────────────────────
//! 三条来源共用这一个入口：短信登录（`api::session::login_loomy_sms_verify`）、
//! 粘贴 session（`api::accounts::add_account` 的 loomy 分支）。**没有桌面端导入**
//! —— Loomy 的登录态不在可读文件里（见 `credentials.rs` 的模块头）。
//!
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::{AccountStore, AccountStoreError, LOOMY_PROVIDER_ID};
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份字段（userId / phone）长度上限
const MAX_IDENTITY_LENGTH: usize = 256;

/// session 长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_SESSION_LENGTH: usize = 8192;

/// 会话有效期（登录请求里的 `expire`：14 天；上游不返回明确过期时刻，
/// 落盘时按登录时刻 + 此值估算）
const SESSION_TTL_MS: i64 = 14 * 24 * 3600 * 1000;

/// 截断到 `max` 个**字符**（不是字节；昵称可能有中文）
fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect()
}

/// session 尾 4 字符（公开形态的展示字段，别家同款）
fn session_tail(session: &str) -> String {
    let chars: Vec<char> = session.chars().collect();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

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

impl AccountStore {
    /// 取一条 Loomy 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（与 `autoclaw_account_record`
    /// 的语义一致：转发默认用队首账号）。
    pub fn loomy_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == LOOMY_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, LOOMY_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 Loomy 账号（短信登录与粘贴 session 共用的落账号入口）。
    ///
    /// payload（两种来源形状一致）：
    ///   - `session` / `accessToken` / `token`：登录 session（必填）；
    ///   - `userId` / `userid`：讯飞 userid；
    ///   - `phone`：手机号（可选，展示用）；
    ///   - `expiresAt`：过期时刻（可选；短信登录底层算、粘贴形态可省）。
    ///
    /// 校验：session 非空且不超长；userId 与 phone 至少有一个（否则记不出
    /// 可辨认的身份，`loomy-` 前缀的 id 会撞在一起）。
    ///
    /// 撞 id（本机已有同 id 记录）时：同 provider 就地更新（保留优先级与启用
    /// 状态、沿用用户改过的备注名），撞到**别的 provider** 时报错 ——
    /// 绝不覆写他人记录（与另外几家同一策略）。
    pub fn add_loomy_account(
        &self,
        payload: &Value,
        name: Option<&str>,
    ) -> Result<Value, AccountStoreError> {
        let Some(object) = payload.as_object() else {
            return Err(AccountStoreError::new("上传内容必须是 JSON 对象", 400));
        };
        let session = pick(payload, &["session", "accessToken", "token"]);
        if session.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 session（accessToken / session / token）",
                400,
            ));
        }
        if session.chars().count() > MAX_SESSION_LENGTH {
            return Err(AccountStoreError::new("session 过长", 400));
        }
        let user_id = truncate_chars(&pick(payload, &["userId", "userid", "user_id"]), MAX_IDENTITY_LENGTH);
        let phone = truncate_chars(&pick(payload, &["phone"]), MAX_IDENTITY_LENGTH);
        if user_id.is_empty() && phone.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 userId 或手机号：至少给一个才能生成可辨认的账号身份",
                400,
            ));
        }
        // 身份标识：优先 userid（稳定）；没有就退回手机号
        let identity = if user_id.is_empty() { phone.clone() } else { user_id.clone() };
        let id = format!("loomy-{identity}");

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != LOOMY_PROVIDER_ID {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一身份的\
                         Loomy 账号（请先处理那个账号）"
                    ),
                    400,
                ));
            }
        }
        // 备注名兜底：显式传入 → 既有记录里的备注名 → 脱敏手机号 → userId
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
            .unwrap_or_else(|| {
                if phone.len() >= 11 {
                    format!("Loomy {}", crate::server::core::providers::loomy::credentials::mask_phone(&phone))
                } else if !user_id.is_empty() {
                    format!("Loomy {user_id}")
                } else {
                    "Loomy 账号".to_string()
                }
            });
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                // 优先级全局唯一（各家共用一条队列），取全库已用号段
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let expires_at = object
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|record| {
                        record
                            .fields()
                            .get("expiresAt")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                    })
                    .filter(|value| *value > 0)
            })
            .unwrap_or(now + SESSION_TTL_MS);

        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert("provider".to_string(), Value::String(LOOMY_PROVIDER_ID.to_string()));
        record.insert("name".to_string(), Value::String(record_name.clone()));
        // 备注名标记：用户显式传了名或既有记录已打标时置位，界面据此决定
        // 「备注名赢过昵称等默认口径」还是「维持原展示行为」。与其余八家同一
        // 口径（见 `state::mark_name_custom`）—— 本家走自己的保存路径（不经过
        // `store_crud::upsert_account` 的统一打标），所以必须在这里显式调用，
        // 漏了会让添加时填的备注名在将来被昵称顶掉。
        mark_name_custom(&mut record, name.is_some_and(|value| !value.trim().is_empty()), existing.as_ref());
        if !user_id.is_empty() {
            record.insert("userId".to_string(), Value::String(user_id));
        }
        if !phone.is_empty() {
            record.insert("phone".to_string(), Value::String(phone));
        }
        record.insert("accessToken".to_string(), Value::String(session.clone()));
        record.insert("tokenTail".to_string(), Value::String(session_tail(&session)));
        record.insert("expiresAt".to_string(), Value::from(expires_at));
        record.insert("source".to_string(), Value::String("manual".to_string()));
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
        // 单行落地：`put` = DELETE + INSERT（与别家一致）
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!("✅ Loomy 账号已保存: {record_name}（优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    /// Loomy 账号的**公开形态**（进 HTTP 响应）：去掉 session 本体，只留尾 4 位。
    pub fn to_loomy_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert("hasSession".to_string(), Value::Bool(true));
        }
        value
    }
}
