//! Antigravity 账号的**公开形态**与归属探测（本仓库保留的部分）。
//!
//! ── 这份文件为什么这么小 ────────────────────────────────────
//! 本家的凭证解析、Google OAuth 刷新、模型目录、`v1internal` 信封构造与
//! 会话转发全部搬到了**提供商插件**（`agent2api-providers` 的 `antigravity/`），
//! 不随本仓库发布 —— 那是风险隔离的一部分。
//!
//! 留在这里的只有两件纯本地的事：
//! ```text
//!   antigravity_account_record     这条账号是不是本家的（管理面分派用）
//!   to_antigravity_public_account  存量内置记录的展示投影（字段映射，无协议知识）
//! ```
//! 新增账号由插件的 `account.normalize` 归一化后落进 `pluginSecret`，公开形态
//! 走 `plugin_accounts::to_plugin_public_account` —— 那条路径不经过本文件。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use super::state::StoredAccount;
use super::store::AccountStore;
use super::ANTIGRAVITY_PROVIDER_ID;

impl AccountStore {
    /// 取一条 Antigravity 账号记录（原始 JSON）。
    ///
    /// 只服务管理面的**归属判定**（`api::accounts::refresh_account` 用它确认
    /// 「这个 id 属于本家」）。`account_id` 为空时返回队首启用账号 —— 这个形态
    /// 已经没有调用方，保留是为了与别家的同名探测函数口径一致。
    pub fn antigravity_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == ANTIGRAVITY_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, ANTIGRAVITY_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// Antigravity 账号的**公开形态**（进 HTTP 响应）。
    ///
    /// 只对**没有** `pluginSecret` 的存量记录生效（判据在
    /// `store_view::public_account`，插件记录先走插件那份投影）。
    ///
    /// 与别家同一条硬约束：**绝不透出令牌本体** —— 只给 `tokenTail`（refresh
    /// token 尾 4 位）与 `hasRefreshToken` / `hasAccessToken` 两个存在性标记；
    /// `expiresAt`、`projectId`、`email` 是展示/排障需要的事实，可以给。
    pub fn to_antigravity_public_account(&self, record: &StoredAccount) -> Value {
        let refresh_token = record.refresh_token();
        let mut public = Map::new();
        for key in ["id", "provider", "name", "userId", "email", "projectId", "source", "tokenTail"] {
            public.insert(
                key.to_string(),
                record.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        public.insert(
            "hasRefreshToken".to_string(),
            Value::Bool(!refresh_token.trim().is_empty()),
        );
        public.insert("refreshTokenLongLived".to_string(), Value::Bool(true));
        public.insert(
            "hasAccessToken".to_string(),
            Value::Bool(
                record
                    .get("accessToken")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty()),
            ),
        );
        public.insert(
            "expiresAt".to_string(),
            Value::from(record.expires_at().unwrap_or(0.0) as i64),
        );
        public.insert("desktop".to_string(), Value::Bool(false));
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
        // 可用 = 有主凭证（refresh token 是这家唯一的续期手段）
        public.insert(
            "available".to_string(),
            Value::Bool(!refresh_token.trim().is_empty()),
        );
        Value::Object(public)
    }

    /// 返回 `小写邮箱 -> 账号展示名` 映射，供本机直连采集将识别到的邮箱对齐用户在面板里起的备注名。
    pub fn antigravity_email_names(&self) -> HashMap<String, String> {
        let guard = self.guard();
        let mut records = self.records_for_provider(&guard, ANTIGRAVITY_PROVIDER_ID);
        records.sort_by_key(StoredAccount::order_key);
        let mut map = HashMap::new();
        for record in records {
            let Some(email) = record
                .identity_field("email")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let display_name = record.name();
            let name = display_name.trim();
            if name.is_empty() {
                continue;
            }
            map.entry(email.to_ascii_lowercase())
                .or_insert_with(|| name.to_string());
        }
        map
    }
}
