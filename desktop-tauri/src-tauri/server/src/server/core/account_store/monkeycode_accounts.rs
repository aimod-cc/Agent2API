//! MonkeyCode 账号以「地区 + userId」识别；凭证是粘贴来的 session cookie，
//! 外加创建任务必需的 `imageId`。
//!
//! ── 地区为什么不落进记录 JSON ────────────────────────────────
//! 与 ZCode 同一条理由：地区**已经编码在 provider id 里**
//! （`monkeycode` / `monkeycode-intl`），而记录自带 `provider` 字段 ——
//! 地区可由 `Region::from_provider_id` 直接还原，不需要再存一份 region 字段
//! （存了就有两处事实来源，漂移时会静默打到错误的站点）。
//!
//! ── 三个落盘字段 ────────────────────────────────────────────
//! `accessToken`（= session，键名与别家统一，别的模块的 `has_token()` 一族认它）、
//! `imageId`（创建任务必需的镜像 UUID，可能为空）、`userId`（上游用户 uuid）。
//! 备注名与公开形态的处置照 Loomy / ZCode 的样子写。
//!
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{json, Map, Value};

use crate::server::core::providers::monkeycode::Region;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份 / 凭证字段长度上限
const MAX_IDENTITY_LENGTH: usize = 256;
const MAX_SESSION_LENGTH: usize = 8192;

/// session 有效期（30 天硬限制；上游不返回明确过期时刻，按登录时刻估算）
const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

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

/// 没有上游 userId 时的账号 id：`{地区前缀}anon-` + 12 位 hex。
///
/// 与 ZCode 同一处置：这类账号没有任何稳定标识可派生，而 id 的唯一性不能打折
/// （固定串会让第二次添加撞上第一条并被合并掉）。随机源失败时如实报错。
fn anonymous_account_id(region: Region) -> Result<String, AccountStoreError> {
    let mut bytes = [0u8; 6];
    getrandom::getrandom(&mut bytes).map_err(|_| {
        AccountStoreError::new(
            "无法生成安全的随机账号 id（系统随机源不可用），请重试",
            500,
        )
    })?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}anon-{hex}", region.account_id_prefix()))
}

impl AccountStore {
    /// 取一条 MonkeyCode 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于**本地区**返回 None —— 两个地区是
    /// 两家 provider，不能互相取到）；为空 → 本地区组内**启用中**账号里优先级
    /// 最靠前的一条（转发默认用队首账号）。
    pub fn monkeycode_account_record(&self, region: Region, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        let provider_id = region.provider_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == provider_id).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, provider_id)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 MonkeyCode 账号（粘贴 session 登录的唯一落账号入口）。
    ///
    /// payload（`login::verify_session` 的返回，或界面直接传来的形状）：
    ///   - `session` / `accessToken` / `token`：登录 session（必填）；
    ///   - `imageId` / `image_id`：创建任务必需的镜像 UUID（可选，可能为空）；
    ///   - `userId` / `user_id`：上游用户 uuid（可选，用于生成稳定 id 与展示）。
    ///
    /// 同一地区、同一 userId 的记录就地更新（保留优先级与启用状态、沿用用户
    /// 改过的备注名）；撞到**别的 provider / 别的地区**的 id 时报错，绝不覆写。
    pub fn add_monkeycode_account(
        &self,
        region: Region,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
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
        let user_id = truncate_chars(&pick(payload, &["userId", "user_id", "userid"]), MAX_IDENTITY_LENGTH);
        let image_id = truncate_chars(&pick(payload, &["imageId", "image_id"]), MAX_IDENTITY_LENGTH);
        let provider_id = region.provider_id();

        // 身份标识：优先 userId（稳定）；没有就用随机段（不去重，理由见
        // `anonymous_account_id` 的注释）
        let guard = self.guard();
        let existing = if user_id.is_empty() {
            None
        } else {
            self.records_for_provider(&guard, provider_id)
                .into_iter()
                .find(|record| record.user_id() == user_id)
        };
        let id = match existing.as_ref() {
            Some(record) => record.id().to_string(),
            None if user_id.is_empty() => anonymous_account_id(region)?,
            None => format!("{}{user_id}", region.account_id_prefix()),
        };
        if existing.is_none() {
            if let Some(occupied) = self.record_by_id(&guard, &id) {
                if occupied.provider() != provider_id {
                    return Err(AccountStoreError::new(
                        format!(
                            "账号 id「{id}」已被{}账号占用，无法添加同一身份的 MonkeyCode 账号（请先处理那个账号）",
                            occupied.provider()
                        ),
                        400,
                    ));
                }
            }
        }
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let payload_name = pick(payload, &["name", "nickname"]);
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                if !payload_name.is_empty() {
                    payload_name
                } else if !user_id.is_empty() {
                    format!("MonkeyCode {user_id}")
                } else {
                    "MonkeyCode 账号".to_string()
                }
            });
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert("provider".to_string(), Value::String(provider_id.to_string()));
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        if !user_id.is_empty() {
            record.insert("userId".to_string(), Value::String(user_id));
        }
        // imageId 空值**不写**：重新粘贴一次 session 不该把上次发现的 image_id
        // 洗掉（与 ZCode 的 jwt / deviceMid 同一条规则）
        if !image_id.is_empty() {
            record.insert("imageId".to_string(), Value::String(image_id));
        } else if let Some(previous) = existing
            .as_ref()
            .and_then(|record| record.fields().get("imageId").cloned())
        {
            record.insert("imageId".to_string(), previous);
        }
        record.insert("accessToken".to_string(), Value::String(session.clone()));
        record.insert("tokenTail".to_string(), Value::String(token_tail_of(&session)));
        record.insert("expiresAt".to_string(), Value::from(now + SESSION_TTL_MS));
        record.insert("source".to_string(), Value::String(source.to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)),
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
            &format!("✅ MonkeyCode {} 账号已保存: {record_name}（优先级 {priority}）", region.label()),
        );
        Ok(self.public_account(&saved))
    }

    /// MonkeyCode 账号的**公开形态**（进 HTTP 响应）：去掉 session 本体，只留
    /// 尾 4 位；带上 `imageId` 与 `edition`（地区）供界面显示。
    pub fn to_monkeycode_public_account(&self, record: &StoredAccount) -> Value {
        let region = Region::from_provider_id(&record.provider());
        let available = record.has_token() && region.is_some();
        let mut public = Map::new();
        for key in ["id", "provider", "name", "userId", "source", "tokenTail", "imageId"] {
            public.insert(
                key.to_string(),
                record.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        public.insert(
            "edition".to_string(),
            region
                .map(|value| Value::String(value.provider_id().to_string()))
                .unwrap_or(Value::Null),
        );
        public.insert(
            "editionLabel".to_string(),
            region
                .map(|value| Value::String(value.label().to_string()))
                .unwrap_or(Value::Null),
        );
        // 有没有 image_id：没有时界面上「创建任务」注定失败，前端据此提示先补
        public.insert(
            "hasImageId".to_string(),
            Value::Bool(
                record
                    .get("imageId")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty()),
            ),
        );
        // 本家没有续期协议（30 天硬限制，见 `credentials.rs` 的模块头），恒 false
        public.insert("hasRefreshToken".to_string(), Value::Bool(false));
        public.insert("desktop".to_string(), Value::Bool(false));
        public.insert("expiresAt".to_string(), Value::from(record.expires_at().unwrap_or(0.0) as i64));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert(
            "proxy".to_string(),
            crate::server::core::proxies::describe_account_proxy(Some(&record.proxy())),
        );
        public.insert(
            "rateLimits".to_string(),
            record.get("rateLimits").cloned().unwrap_or_else(|| json!({})),
        );
        public.insert("available".to_string(), Value::Bool(available));
        Value::Object(public)
    }
}

/// 本家的两个 provider id 常量（别处按它判「是不是 MonkeyCode」时用注册表口径）
pub(crate) const _MONKEYCODE_PROVIDER_ID: &str = kind_id(ProviderKind::MonkeyCode);
