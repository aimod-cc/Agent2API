//! OrcaTerm 账号：粘贴 accessToken、导入桌面端登录态、续期回写、公开形态。
//!
//! ── 与各家添加路径的关系 ──────────────────────────────────────
//! 每条添加路径各自独立，因为凭证形态完全不同：OrcaTerm 的凭证是**一枚
//! OAuth JWT**（`Authorization: Bearer`，约 2 小时时效），解析在
//! `providers::orcaterm::credentials`（裸 token / `Bearer ` 整行 /
//! data.bin JSON 三种形态）。本文件只负责**落盘**：接收已解析、已验证的
//! [`OrcatermCredentials`]，写一条账号记录。
//!
//! ── 记录里的字段 ────────────────────────────────────────────
//! ```text
//! { id, provider: "orcaterm", name, uid(=userId), loginName(=昵称),
//!   accessToken(JWT 本体), tokenExpireAt(毫秒), tokenTail, source,
//!   priority, enabled, addedAt, updatedAt }
//! ```
//! id = `orcaterm-<userId>`（userId 来自 JWT payload / `GetUserInfo`；
//! 判不出时用 token 尾兜底）。id 撞到**别的 provider** 时拒绝（与 KukuAI
//! 同一策略：id 空间独立，撞到别家不能静默覆写）。
//!
//! ── 续期回写 ────────────────────────────────────────────────
//! [`AccountStore::update_orcaterm_token`] 由凭证层的单飞刷新调用：
//! 只改 `accessToken` / `tokenExpireAt` / `updatedAt` 三个字段（其余不动）。
//! 简单覆写而不是比较-再写：续期是单飞的（同账号同 token 只有一轮），
//! 与用户重新导入的竞态窗口极小，且即便撞上也自愈（下一轮临期会用
//! 记录里的 token 再刷一次；上游的 `OAuthRefreshToken` 对任意有效旧 token
//! 都能换新）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，不持锁做网络。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::providers::orcaterm::credentials::OrcatermCredentials;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

/// 备注名长度上限
const MAX_NAME_LENGTH: usize = 100;

/// OrcaTerm provider id（账号存储内部多处要用；**从注册表推导**，同
/// [`crate::server::core::account_store::kuku_accounts::KUKU_PROVIDER_ID`] 的口径）。
pub(crate) const ORCATERM_PROVIDER_ID: &str = kind_id(ProviderKind::OrcaTerm);

fn orcaterm_id() -> &'static str {
    kind_id(ProviderKind::OrcaTerm)
}

impl AccountStore {
    // ─── 读：账号记录 ────────────────────────────────────────

    /// 取 OrcaTerm 账号的**原始记录**（含 accessToken）。
    ///
    /// `account_id` 为空 → 取 OrcaTerm 组内优先级最小的启用账号（与转发选路同一
    /// 判据）；找不到返回 None（调用方据此报「没有可用的登录凭证」）。
    pub fn orcaterm_account_record(&self, account_id: &str) -> Option<Value> {
        let _guard = self.guard();
        let orcaterm = orcaterm_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&_guard, account_id)?;
            return (record.provider() == orcaterm).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&_guard, orcaterm)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    // ─── 写：落账号 ──────────────────────────────────────────

    /// 添加/更新一个 OrcaTerm 账号（`POST /api/accounts` 的 orcaterm 分支共用
    /// 入口：粘贴 accessToken 与导入桌面端登录态都先解析+验证成
    /// [`OrcatermCredentials`] 再到这里）。`source` 记来源（`"manual"` /
    /// `"desktop"`）。
    ///
    /// id 的生成：`orcaterm-<userId>` 优先；userId 判不出时用
    /// `orcaterm-` + token 尾 8 位兜底。
    pub fn add_orcaterm_account(
        &self,
        credentials: &OrcatermCredentials,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let user_key = credentials.user_key();
        if user_key.len() > 256 {
            return Err(AccountStoreError::new("OrcaTerm 账号标识过长", 400));
        }
        let _guard = self.guard();
        let id = user_key;
        let existing = self.record_by_id(&_guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != orcaterm_id() {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一标识的\
                         OrcaTerm 账号（请先处理那个账号）"
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
        let token = credentials.access_token.clone();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert("provider".to_string(), Value::String(orcaterm_id().to_string()));
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(&mut record, explicit, existing.as_ref());
        record.insert("uid".to_string(), Value::String(credentials.user_id.clone()));
        record.insert("loginName".to_string(), Value::String(nickname));
        record.insert("accessToken".to_string(), Value::String(token.clone()));
        record.insert("tokenTail".to_string(), Value::String(token_tail_of(&token)));
        record.insert(
            "tokenExpireAt".to_string(),
            Value::from(credentials.expires_at * 1000),
        );
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
            &format!("✅ OrcaTerm 账号已保存: {record_name}（{id}，优先级 {priority}）"),
        );
        Ok(self.public_account(&saved))
    }

    // ─── 写：续期回写 ────────────────────────────────────────

    /// 把续期得到的新 token 回写进账号记录（`credentials::refresh` 调用）。
    ///
    /// `account_id` 为空（无账号记录的临时凭证）时静默跳过 —— 没有可回写的
    /// 落点，本次进程内的新 token 已经在调用方手上。找不到记录同样跳过
    /// （账号刚被删除）：刷新不该把一条已删除的账号"救活"回来。
    pub fn update_orcaterm_token(&self, account_id: &str, token: &str, expires_at: i64) {
        if account_id.is_empty() || token.is_empty() {
            return;
        }
        let _guard = self.guard();
        let Some(record) = self.record_by_id(&_guard, account_id) else {
            return;
        };
        if record.provider() != orcaterm_id() {
            return;
        }
        let mut fields = record.fields().clone();
        fields.insert("accessToken".to_string(), Value::String(token.to_string()));
        fields.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(token)),
        );
        fields.insert("tokenExpireAt".to_string(), Value::from(expires_at * 1000));
        fields.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        let saved = StoredAccount::from_map(fields);
        if let Ok(()) = self.with_conn(&_guard, |conn| sql::put(conn, &saved)) {
            drop(_guard);
            logging::verbose(
                "[OrcaTerm]",
                &format!("账号 {account_id} 的续期结果已回写（新过期时刻 {expires_at}）"),
            );
        }
    }

    // ─── 读：公开形态 ────────────────────────────────────────

    /// 账号的公开形态（前端账号列表 / 会话摘要用；**不含凭证**）。
    pub fn to_orcaterm_public_account(&self, record: &StoredAccount) -> Value {
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
        // tokenExpireAt 原样透出（毫秒；0 = 未知）：前端据此显示「到期时间」，
        // 也让「续期是否在工作」可被肉眼校验
        public.insert(
            "tokenExpireAt".to_string(),
            Value::from(record.get("tokenExpireAt").and_then(Value::as_i64).unwrap_or(0)),
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
