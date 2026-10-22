//! Coze 账号：手动添加（粘贴整包 Cookie）、网页登录落库、公开形态。
//!
//! ── 与各家添加路径的关系 ──────────────────────────────────────
//! 本家凭证是 `.coze.cn` 域**一整包 Cookie**（`sessionid_ss` / `sid_guard` /
//! `ttwid` / `odin_tt` …，见 `providers::coze::credentials` 的模块头）。
//! 本文件只负责**落盘**：接收已解析、已复核的 [`CozeCredentials`]，写一条记录。
//! 解析与复核在 `providers::coze::credentials`，PAT 铸造也在那里（按需、不落盘）。
//!
//! ── 记录里的字段 ────────────────────────────────────────────
//! ```text
//! { id, provider: "coze", name, uid(=passport user_id_str), loginName(=昵称),
//!   accessToken(=整包 Cookie 头串), tokenTail, source, priority,
//!   enabled, addedAt, updatedAt }
//! ```
//! `accessToken` 存 Cookie 头串（与 KukuAI 同一处置：那一列本来就是「凭据本体」，
//! 换一个名字只会让账号存储的分支分叉）；`uid` 是 `passport/account/info/v2/`
//! 的 `user_id_str`，也是 id 的首选来源。id 撞到**别的 provider** 时拒绝
//! （id 空间独立，撞到别家不能静默覆写）。
//!
//! ⚠️ **PAT 不进账号记录**：它 10 分钟就换一枚，落盘只会变成一份过期垃圾，
//! 还会让「账号凭证」这件事出现两个真相。它由 `credentials::ensure_pat`
//! 进程内缓存（key = 账号 id）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，不持锁做网络。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::providers::coze::credentials::{self, CozeCredentials};
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

/// 备注名长度上限
const MAX_NAME_LENGTH: usize = 100;

/// Coze provider id（从注册表推导，`const` 上下文可用）
pub(crate) const COZE_PROVIDER_ID: &str = kind_id(ProviderKind::Coze);

fn coze_id() -> &'static str {
    kind_id(ProviderKind::Coze)
}

impl AccountStore {
    // ─── 读：账号记录 ────────────────────────────────────────

    /// 取 Coze 账号的**原始记录**（含 accessToken）。
    ///
    /// `account_id` 为空 → 取 Coze 组内优先级最小的启用账号（与转发选路同一
    /// 判据）；找不到返回 None（调用方据此报「没有可用的登录凭证」）。
    pub fn coze_account_record(&self, account_id: &str) -> Option<Value> {
        let _guard = self.guard();
        let coze = coze_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&_guard, account_id)?;
            return (record.provider() == coze).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&_guard, coze)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    // ─── 写：落账号 ──────────────────────────────────────────

    /// 添加/更新一个 Coze 账号（`POST /api/accounts` 的 coze 分支与登录收尾
    /// 共用入口：手动粘贴 Cookie 与网页登录都先解析+复核成
    /// [`CozeCredentials`] 再到这里）。
    ///
    /// `source` 记来源（`"manual"` / `"web"`）。
    ///
    /// id 生成：`uid`（passport 的 `user_id_str`）优先；拿不到时
    /// `coze-` + `sessionid_ss` 尾 8 位兜底（复核失败的粘贴也能落，
    /// 余额 / 签到照常）。
    pub fn add_coze_account(
        &self,
        credentials: &CozeCredentials,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let user_key = credentials::account_id_of(credentials);
        if user_key.trim().is_empty() {
            return Err(AccountStoreError::new("Coze 账号标识为空（Cookie 里没有可用标识）", 400));
        }
        if user_key.len() > 256 {
            return Err(AccountStoreError::new("Coze 账号标识过长", 400));
        }
        let _guard = self.guard();
        let id = user_key;
        let existing = self.record_by_id(&_guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != coze_id() {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一标识的\
                         Coze 账号（请先处理那个账号）"
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
        record.insert("provider".to_string(), Value::String(coze_id().to_string()));
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(&mut record, explicit, existing.as_ref());
        record.insert(
            "uid".to_string(),
            Value::String(credentials.user_id.trim().to_string()),
        );
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
            &format!("✅ Coze 账号已保存: {record_name}（{id}，优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    /// 账号的公开形态（前端账号列表 / 会话摘要用；**不含凭证**）。
    ///
    /// 与 KukuAI 同形：`uid` / `loginName` / `tokenTail` / `desktop` / `source` /
    /// `priority` / `enabled` / `addedAt` / `updatedAt` / `proxy` / `rateLimits` /
    /// `available`。**不带 `expiresAt`** —— 本家 Cookie 没有到期时刻可读、
    /// 也没有续期（`supports_refresh = false`），前端按
    /// `expiry: ''` 处置（不展示假的「会自动续期」）。
    pub fn to_coze_public_account(&self, record: &StoredAccount) -> Value {
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
