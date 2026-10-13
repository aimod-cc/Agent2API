//! Antigravity 账号：粘贴 Google refresh token 落账号 + 刷新回写 + 公开形态。
//!
//! ── 账号字段（落盘形态）────────────────────────────────────────
//! ```text
//!   id            派生（见下）
//!   provider      "antigravity"
//!   name          备注名（可空；界面兜底用 email → 尾号）
//!   refreshToken  主凭证（长寿命；Google OAuth）
//!   accessToken   短期令牌（可选；刷新链路会就地更新）
//!   expiresAt     访问令牌过期时刻（毫秒；口径 = now + expires_in）
//!   projectId     cloudaicompanionProject（可选；由 loadCodeAssist 发现）
//!   email         账号邮箱（可选；展示与身份）
//!   userId        身份键（= email；Google 没有 userId 字段，见下）
//!   tokenTail     refresh token 尾 4 位（展示用；**token 本体不进公开形态**）
//!   source/priority/enabled/addedAt/updatedAt  与别家同形
//! ```
//!
//! ── 账号 id 怎么派生（为什么优先用 email 而不是令牌）───────────
//! 身份键的顺序是 **email → refresh token 摘要**：
//!   - email 是对同一个 Google 账号**稳定**的标识：重新授权会换一枚新的
//!     refresh token，但 email 不变 —— 按 email 派生 id 才能让「重新粘贴」
//!     更新同一条记录，而不是留下一堆孤儿账号；
//!   - email 缺失（用户只粘贴了令牌）时退到令牌摘要：同一枚令牌再粘一次仍
//!     命中同一条记录，而重新授权拿到的**新**令牌会新增一条记录（那时用户
//!     自己知道换了一枚，手工删掉旧的即可）。
//! 取 `sha256("antigravity:account-id:v1\0" + 身份)[..12]`：确定性、不可反推
//! 令牌、不同身份撞 id 的概率可忽略（与 `commandcode_accounts` 同一手法）。
//!
//! ── 为什么 `userId` 存的是 email ───────────────────────────────
//! 「从其他工具导入 / 导出」那条链路按**通用字段 `userId`** 认身份
//! （`account_transfer::identity` 的兜底分支）。Google 的 userinfo 里虽然有
//! 数字 `id`，但本仓只把它当展示 / 身份增强（网页登录会调一次 userinfo 取
//! email，粘贴式则由用户填），因此把 email 当作本家的稳定身份存进 `userId`
//! —— 与 MonkeyCode 存上游 user uuid 同一位置、同一语义。email 也缺失时该键
//! 不写（那类账号在导出时会得到一句「缺少 userId」的失败原因，而不是被静默
//! 跳过）。
//!
//! 本文件全是「读-改-写」存储操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::antigravity::credentials;

use super::priority::next_free_priority;
use super::sql;
use super::state::{json_number, mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::{CredentialWrite, ANTIGRAVITY_PROVIDER_ID};
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份 / 令牌字段长度上限
const MAX_IDENTITY_LENGTH: usize = 256;
const MAX_TOKEN_LENGTH: usize = 8192;

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

/// 身份 + 令牌 → 稳定的账号 id（见模块头）
fn account_id_for(identity: &str, refresh_token: &str) -> String {
    let seed = if identity.is_empty() { refresh_token } else { identity };
    let mut hasher = Sha256::new();
    hasher.update(b"antigravity:account-id:v1\0");
    hasher.update(seed.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let head = digest.get(..12).unwrap_or(digest.as_str());
    format!("antigravity-{head}")
}

/// 记录里的 userId（身份键；缺失给空串）
fn user_id_of(record: &StoredAccount) -> String {
    record
        .get("userId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

impl AccountStore {
    /// 取一条 Antigravity 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；为空 → 本家
    /// **启用中**账号里优先级最靠前的一条（转发与目录刷新默认用队首账号）。
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

    /// 添加/更新一个 Antigravity 账号（粘贴 refresh token 的唯一落账号入口）。
    ///
    /// payload（`login::verify_paste` 的返回，或界面直接传来的形状）：
    ///   - `refreshToken` / `refresh_token`：主凭证（**必填**，建议以 `1//` 开头）；
    ///   - `accessToken` / `access_token`：短期令牌（可选；校验刷新已拿到的那个）；
    ///   - `expiresAt`：访问令牌过期时刻（毫秒，可选）；
    ///   - `projectId` / `project_id`：cloudaicompanionProject（可选）；
    ///   - `email`、`name`：展示与身份（可选）。
    ///
    /// 同一身份（email，其次同一枚令牌）的记录就地更新（保留优先级与启用状态、
    /// 沿用用户改过的备注名）；撞到**别的 provider** 的 id 时报错，绝不覆写。
    pub fn add_antigravity_account(
        &self,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let refresh_token = pick(payload, &["refreshToken", "refresh_token"]);
        if refresh_token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 refresh token（refreshToken / refresh_token）",
                400,
            ));
        }
        if refresh_token.chars().count() > MAX_TOKEN_LENGTH {
            return Err(AccountStoreError::new("refresh token 过长", 400));
        }
        let email = truncate_chars(&pick(payload, &["email"]), MAX_IDENTITY_LENGTH);
        let identity = email.trim().to_ascii_lowercase();
        let access_token = truncate_chars(&pick(payload, &["accessToken", "access_token"]), MAX_TOKEN_LENGTH);
        let project_id = truncate_chars(&pick(payload, &["projectId", "project_id"]), MAX_IDENTITY_LENGTH);
        let expires_at = payload
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(0);
        let provider_id = ANTIGRAVITY_PROVIDER_ID;

        // 身份匹配：email 优先，其次同一枚令牌（见模块头）
        let guard = self.guard();
        let existing = self
            .records_for_provider(&guard, provider_id)
            .into_iter()
            .find(|record| {
                let same_email = !identity.is_empty() && user_id_of(record).eq_ignore_ascii_case(&identity);
                same_email || (!refresh_token.is_empty() && record.refresh_token() == refresh_token)
            });
        let id = match existing.as_ref() {
            Some(record) => record.id().to_string(),
            None => account_id_for(&identity, &refresh_token),
        };
        if existing.is_none() {
            if let Some(occupied) = self.record_by_id(&guard, &id) {
                if occupied.provider() != provider_id {
                    return Err(AccountStoreError::new(
                        format!(
                            "账号 id「{id}」已被{}账号占用，无法添加同一身份的 Antigravity 账号（请先处理那个账号）",
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
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                if !email.is_empty() {
                    email.clone()
                } else {
                    format!("Antigravity {}", token_tail_of(&refresh_token))
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
        record.insert("refreshToken".to_string(), Value::String(refresh_token.clone()));
        record.insert("tokenTail".to_string(), Value::String(token_tail_of(&refresh_token)));
        // 空值**不写**：重新粘贴一枚令牌不该把上次刷新到的 access_token /
        // expiresAt / projectId 洗掉（与 ZCode 的 jwt / MonkeyCode 的 imageId 同一条规则）
        let previous = existing.as_ref().map(StoredAccount::fields);
        let prefer_previous = |key: &str, value: &str| -> Option<String> {
            if !value.is_empty() {
                return Some(value.to_string());
            }
            previous
                .and_then(|fields| fields.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        if let Some(token) = prefer_previous("accessToken", &access_token) {
            record.insert("accessToken".to_string(), Value::String(token));
        }
        if let Some(project) = prefer_previous("projectId", &project_id) {
            record.insert("projectId".to_string(), Value::String(project));
        }
        if !email.is_empty() {
            record.insert("email".to_string(), Value::String(email.clone()));
            // userId = 本家的稳定身份（见模块头）
            record.insert("userId".to_string(), Value::String(email));
        } else {
            // 没带 email 时沿用旧记录里的身份键（不把已有身份洗掉）
            for key in ["email", "userId"] {
                if let Some(value) = previous.and_then(|fields| fields.get(key)).cloned() {
                    record.insert(key.to_string(), value);
                }
            }
        }
        if let Some(existing) = existing.as_ref() {
            if let Some(expires) = existing.expires_at() {
                record.insert("expiresAt".to_string(), json_number(expires));
            }
        }
        if expires_at > 0 {
            record.insert("expiresAt".to_string(), Value::from(expires_at));
        }
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
            &format!(
                "✅ Antigravity 账号已保存: {record_name}（优先级 {priority}，refresh token {}）",
                credentials::tail_chars(&refresh_token, 4)
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// 刷新成功后回写新令牌（`oauth::refresh_and_save` 调用）。
    ///
    /// ── 比较-再写（与另外几家同一纪律）─────────────────────────
    /// 只当记录里**此刻的** refresh token 仍等于刷新前那份快照时才写入；
    /// 不一致（用户重新粘贴 / 换号 / 另一轮刷新先落地）返回
    /// [`CredentialWrite::Stale`] —— 调用方改用最新快照，绝不把旧结果当成功。
    ///
    /// `access_token` 为空 / `expires_at <= 0` / `refresh_token` 为 None 或空 /
    /// `project_id` 为 None 或空：一律**保留旧值**（见调用点的说明）。
    pub fn update_antigravity_credentials_if_current(
        &self,
        id: &str,
        expected_refresh_token: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: i64,
        project_id: Option<&str>,
    ) -> Result<CredentialWrite, String> {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id) else {
            // 账号已被删除：结果无处可写，也不该新建记录
            return Ok(CredentialWrite::Stale);
        };
        if record.provider() != ANTIGRAVITY_PROVIDER_ID {
            return Err(format!("账号 {id} 不是 Antigravity 账号"));
        }
        if record.refresh_token() != expected_refresh_token {
            return Ok(CredentialWrite::Stale);
        }
        let access_token = access_token.trim();
        if !access_token.is_empty() {
            record.set("accessToken", Value::String(access_token.to_string()));
            record.set("tokenTail", Value::String(token_tail_of(&record.refresh_token())));
        }
        if let Some(refresh) = refresh_token.map(str::trim).filter(|text| !text.is_empty()) {
            record.set("refreshToken", Value::String(refresh.to_string()));
            record.set("tokenTail", Value::String(token_tail_of(refresh)));
        }
        if expires_at > 0 {
            record.set("expiresAt", Value::from(expires_at));
        }
        if let Some(project) = project_id.map(str::trim).filter(|text| !text.is_empty()) {
            record.set("projectId", Value::String(project.to_string()));
        }
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))
            .map_err(|error| error.message)?;
        Ok(CredentialWrite::Written)
    }

    /// 只写 `projectId`（project 发现路径专用；目录刷新时发现一次就补上）。
    ///
    /// 与令牌回写分开的理由：它**不是凭证**，没有「比较-再写」的必要，而
    /// 令牌回写要求调用方持有刷新前的 refresh token（发现路径上可能没有）。
    /// 空 project 不写；记录不存在返回 `Ok(false)`（调用方只记一条 verbose）。
    pub fn set_antigravity_project(&self, id: &str, project_id: &str) -> Result<bool, String> {
        let project = project_id.trim();
        if project.is_empty() {
            return Ok(false);
        }
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id) else {
            return Ok(false);
        };
        if record.provider() != ANTIGRAVITY_PROVIDER_ID {
            return Err(format!("账号 {id} 不是 Antigravity 账号"));
        }
        record.set("projectId", Value::String(project.to_string()));
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))
            .map_err(|error| error.message)?;
        Ok(true)
    }

    /// Antigravity 账号的**公开形态**（进 HTTP 响应）。
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
}
