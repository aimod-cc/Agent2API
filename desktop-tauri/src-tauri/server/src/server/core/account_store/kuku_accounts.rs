//! KukuAI 账号：手动添加（粘贴 Cookie）、导入本机登录态、公开形态。
//!
//! ── 与各家添加路径的关系 ──────────────────────────────────────
//! 每条添加路径各自独立，因为凭证形态完全不同：KukuAI 的凭证是**三个 Cookie**
//! （`BDUSS` / `STOKEN` / `gfprotpl=genflowpro`，后一个恒附加不落盘），
//! 解析在 `providers::kuku::credentials`（Cookie 头 / Cookie 编辑器 JSON /
//! 对象三种形态）。本文件只负责**落盘**：接收已解析、已验证的
//! [`KukuCredentials`]，写一条账号记录。
//!
//! ── 记录里的字段 ────────────────────────────────────────────
//! ```text
//! { id, provider: "kuku", name, uid(=uk), loginName(=昵称),
//!   accessToken(="BDUSS=…; STOKEN=…"), tokenTail, source, priority,
//!   enabled, addedAt, updatedAt }
//! ```
//! `accessToken` 存 Cookie 头串（**不含** `gfprotpl`，它是恒定值，拼头时附加）；
//! `uid` 是 `userreport` 的 `uk`（账号数字标识），也是 id 的首选来源。
//! id 撞到**别的 provider** 时拒绝（与 CatPaw 同一策略：id 空间独立，
//! 撞到别家不能静默覆写）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，不持锁做网络。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::providers::kuku::credentials::KukuCredentials;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

/// 备注名长度上限
const MAX_NAME_LENGTH: usize = 100;

/// KukuAI provider id（账号存储内部多处要用；**从注册表推导**，同
/// [`LOOMY_PROVIDER_ID`] 的口径）。账号形态见本文件。
pub(crate) const KUKU_PROVIDER_ID: &str = kind_id(ProviderKind::Kuku);

fn kuku_id() -> &'static str {
    kind_id(ProviderKind::Kuku)
}

impl AccountStore {
    // ─── 读：账号记录 ────────────────────────────────────────

    /// 取 KukuAI 账号的**原始记录**（含 accessToken）。
    ///
    /// `account_id` 为空 → 取 KukuAI 组内优先级最小的启用账号（与转发选路同一
    /// 判据）；找不到返回 None（调用方据此报「没有可用的登录凭证」）。
    pub fn kuku_account_record(&self, account_id: &str) -> Option<Value> {
        let _guard = self.guard();
        let kuku = kuku_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&_guard, account_id)?;
            return (record.provider() == kuku).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&_guard, kuku)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    // ─── 写：落账号 ──────────────────────────────────────────

    /// 添加/更新一个 KukuAI 账号（`POST /api/accounts` 的 kuku 分支共用入口：
    /// 手动粘贴与导入本机登录态都先解析+验证成 [`KukuCredentials`] 再到这里）。
    ///
    /// `source` 记来源（`"manual"` / `"desktop"`）。
    ///
    /// id 的生成：`uid`（userreport 的 uk）优先；拿不到 uid 时用
    /// `kuku-` + BDUSS 尾 8 位兜底（粘贴的 Cookie 没有验证时也能落，转发照常）。
    pub fn add_kuku_account(
        &self,
        credentials: &KukuCredentials,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let user_key = if !credentials.uid.trim().is_empty() {
            credentials.uid.trim().to_string()
        } else {
            format!("kuku-{}", credentials.token_tail())
        };
        if user_key.len() > 256 {
            return Err(AccountStoreError::new("KukuAI 账号标识过长", 400));
        }
        let _guard = self.guard();
        let id = user_key;
        let existing = self.record_by_id(&_guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != kuku_id() {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一标识的\
                         KukuAI 账号（请先处理那个账号）"
                    ),
                    400,
                ));
            }
        }
        let nickname = credentials.nickname.trim().to_string();
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let explicit = explicit_name.is_some();
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .or_else(|| {
                if !nickname.is_empty() {
                    Some(nickname.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| format!("账号 {id}"));
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&_guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert("provider".to_string(), Value::String(kuku_id().to_string()));
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(&mut record, explicit, existing.as_ref());
        record.insert("uid".to_string(), Value::String(credentials.uid.clone()));
        record.insert("loginName".to_string(), Value::String(nickname));
        let cookie = credentials.cookie_header();
        record.insert("accessToken".to_string(), Value::String(cookie.clone()));
        record.insert("tokenTail".to_string(), Value::String(token_tail_of(&cookie)));
        record.insert("source".to_string(), Value::String(source.to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)),
        );
        record.insert(
            "addedAt".to_string(),
            Value::from(existing.as_ref().map(StoredAccount::added_at).unwrap_or(now)),
        );
        record.insert("updatedAt".to_string(), Value::from(now));
        let mut merged = record;
        if let Some(existing) = existing.as_ref() {
            for (key, value) in existing.fields() {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let saved = StoredAccount::from_map(merged);
        self.with_conn(&_guard, |conn| sql::put(conn, &saved))?;
        drop(_guard);
        logging::log(
            "[Accounts]",
            &format!("✅ KukuAI 账号已保存: {record_name}（{id}，优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    /// 账号的公开形态（前端账号列表 / 会话摘要用；**不含凭证**）。
    pub fn to_kuku_public_account(&self, record: &StoredAccount) -> Value {
        let proxy = crate::server::core::proxies::describe_account_proxy(Some(&record.proxy()));
        let mut public = Map::new();
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        public.insert("provider".to_string(), Value::String(record.provider()));
        public.insert("name".to_string(), Value::String(record.name()));
        public.insert(
            "uid".to_string(),
            Value::String(record.get("uid").and_then(Value::as_str).unwrap_or("").to_string()),
        );
        public.insert(
            "loginName".to_string(),
            Value::String(record.get("loginName").and_then(Value::as_str).unwrap_or("").to_string()),
        );
        public.insert(
            "tokenTail".to_string(),
            Value::String(record.get("tokenTail").and_then(Value::as_str).unwrap_or("").to_string()),
        );
        public.insert("desktop".to_string(), Value::Bool(record.is_desktop()));
        public.insert("source".to_string(), Value::String(record.source()));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert("proxy".to_string(), proxy);
        public.insert(
            "rateLimits".to_string(),
            record.get("rateLimits").cloned().unwrap_or_else(|| Value::Object(Map::new())),
        );
        public.insert("available".to_string(), Value::Bool(true));
        Value::Object(public)
    }
}
