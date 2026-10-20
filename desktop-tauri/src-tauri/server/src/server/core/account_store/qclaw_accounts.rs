//! QClaw 账号：导入本机网关配置 / 手动粘贴、读记录、公开形态。
//!
//! ── 与各家添加路径的关系 ──────────────────────────────────────
//! 凭证发现（读 `openclaw.json` / 解密 localStorage）在
//! `providers::qclaw::local`，本文件只负责**落盘**：接收已探测、已校验的
//! 网关参数，写一条账号记录。
//!
//! ── 记录里的字段 ────────────────────────────────────────────
//! ```text
//! { id(=qclaw-<uid 摘要>), provider: "qclaw", name, uid(=云端用户 id 或令牌摘要),
//!   userId, accessToken(=本机网关令牌), tokenTail,
//!   gatewayPort(=9434), modelsPort(=19000),
//!   expiresAt(=云端 JWT 的 exp，毫秒；解不开时缺席),
//!   source, priority, enabled, addedAt, updatedAt }
//! ```
//! `accessToken` 存的是**本机网关令牌**（`gateway.auth.token`），不是云端
//! JWT —— 转发用的是它。它随 QClaw 重启而变，因此适配器每次请求都现场
//! 重读 `openclaw.json`（见 `providers::qclaw` 模块头），记录里这份只是
//! 「文件读不到时」的兜底快照；刷新链路（401 → `refresh_access_token`）
//! 会把它写回最新值。
//!
//! id 撞到**别的 provider** 时拒绝（与各家同一策略：id 空间独立，
//! 撞到别家不能静默覆写）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

/// 备注名长度上限
const MAX_NAME_LENGTH: usize = 100;

/// QClaw provider id（**从注册表推导**，与别家同一口径）。
pub(crate) const QCLAW_PROVIDER_ID: &str = kind_id(ProviderKind::Qclaw);

fn qclaw_id() -> &'static str {
    QCLAW_PROVIDER_ID
}

/// 「添加账号」时收集到的本机网关参数（已通过 `local::probe_chat_gateway`
/// 校验），落库前的中间形态。
#[derive(Clone, Debug)]
pub struct QclawCredentials {
    /// 本机网关令牌（`gateway.auth.token`）
    pub token: String,
    /// 聊天网关端口
    pub chat_port: u16,
    /// 模型清单端口
    pub models_port: u16,
    /// 云端用户 id（解不开登录态时为空，落库时用令牌摘要补）
    pub uid: String,
    /// 昵称（解不开时为空）
    pub nickname: String,
}

impl AccountStore {
    // ─── 读：账号记录 ────────────────────────────────────────

    /// 取 QClaw 账号的**原始记录**（含网关令牌）。
    ///
    /// `account_id` 为空 → 取本组内优先级最小的启用账号（与转发选路同一判据）；
    /// 非空 → 按 id 直查并核对归属（取不到返回 None，**不**回落队首 ——
    /// 那会把「点名的账号坏了」变成「静默用了别人的登录态」）。
    pub fn qclaw_account_record(&self, account_id: &str) -> Option<Value> {
        let _guard = self.guard();
        let provider = qclaw_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&_guard, account_id)?;
            return (record.provider() == provider).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&_guard, provider)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    // ─── 写：落账号 ──────────────────────────────────────────

    /// 添加/更新一个 QClaw 账号（`POST /api/accounts` 的 qclaw 分支共用入口：
    /// 导入本机网关与手动粘贴都先探测 + 校验成 [`QclawCredentials`] 再到这里）。
    ///
    /// `source` 记来源（`"desktop"` / `"manual"`）。
    /// `expires_at` 是云端 JWT 的 `exp`（毫秒；账号页「有效期」列读它）。
    /// 粘贴路径没有可信的 JWT 来源就传 `None`（那一列空着，与 chatglm 同一处置）。
    ///
    /// id = `qclaw-<uid 的 sha256 前 16 位>`：uid 是云端用户 id（UUID），
    /// 直接当主键会让「用户手改过的 id」与「新建」两套规则并存；摘要后也天然
    /// 不会撞别家形态。uid 解不开时用**令牌摘要** —— 同一台机器同一个登录态
    /// 的令牌稳定，重复导入仍然命中同一条记录。
    pub fn add_qclaw_account(
        &self,
        credentials: &QclawCredentials,
        name: Option<&str>,
        source: &str,
        expires_at: Option<i64>,
    ) -> Result<Value, AccountStoreError> {
        if credentials.token.trim().is_empty() {
            return Err(AccountStoreError::new(
                "QClaw 账号缺少网关令牌（token）",
                400,
            ));
        }
        let uid = if credentials.uid.trim().is_empty() {
            // 登录态解不开时的稳定身份：令牌摘要（换人登录 → 令牌变 → 新账号）
            format!(
                "{:x}",
                Sha256::digest(credentials.token.trim().as_bytes())
            )
        } else {
            credentials.uid.trim().to_string()
        };
        let guard = self.guard();
        let digest = format!("{:x}", Sha256::digest(uid.as_bytes()));
        let id = format!("{}-{}", qclaw_id(), truncate_chars(&digest, 16));
        // 身份匹配：本家组内按 `uid` 找既有记录（重新导入同一账号 → 更新）
        let existing = self
            .records_for_provider(&guard, qclaw_id())
            .into_iter()
            .find(|record| record.uid() == uid);
        // 新建时确认这个 id 没被任何人（含别家）占用 —— 主键查询，只读一行
        if existing.is_none() && self.record_by_id(&guard, &id).is_some() {
            return Err(AccountStoreError::new(
                "QClaw 账号 ID 已被其它账号占用，请先核对账号记录",
                409,
            ));
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
            .unwrap_or_else(|| "QClaw 本机网关".to_string());
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        // 既有记录先整份取过来：未知字段（用户加的备注、代理、并发上限…）
        // 全量保留，本函数只改自己认识的键（不变量见 `store_crud` 模块头）。
        let mut merged = existing
            .as_ref()
            .map(|record| record.fields().clone())
            .unwrap_or_default();
        merged.insert("id".to_string(), Value::String(id.clone()));
        merged.insert(
            "provider".to_string(),
            Value::String(qclaw_id().to_string()),
        );
        merged.insert(
            "name".to_string(),
            Value::String(truncate_chars(&record_name, MAX_NAME_LENGTH)),
        );
        mark_name_custom(&mut merged, explicit, existing.as_ref());
        // `uid` 是账号身份字段（公开形态与撞 id 判定都读它）；`userId` 是几家
        // 共用的身份字段名，一并写上，免得读者按家查表。
        merged.insert("uid".to_string(), Value::String(uid.clone()));
        merged.insert("userId".to_string(), Value::String(uid.clone()));
        merged.insert(
            "accessToken".to_string(),
            Value::String(credentials.token.trim().to_string()),
        );
        merged.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(credentials.token.trim())),
        );
        merged.insert(
            "gatewayPort".to_string(),
            Value::from(credentials.chat_port),
        );
        merged.insert(
            "modelsPort".to_string(),
            Value::from(credentials.models_port),
        );
        merged.insert("source".to_string(), Value::String(source.to_string()));
        // `expiresAt`：只在真有值时写（None = 解不开 JWT，键缺席 →
        // 公开形态给 null，界面按 chatglm 口径不渲染「有效期」列）。
        // 重新导入同一账号时以新值为准（用户在客户端重新登录过）。
        if let Some(expires_at) = expires_at.filter(|ms| *ms > 0) {
            merged.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        merged.insert("priority".to_string(), Value::from(priority));
        merged.insert(
            "enabled".to_string(),
            Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)),
        );
        merged.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .unwrap_or(now),
            ),
        );
        merged.insert("updatedAt".to_string(), Value::from(now));
        merged
            .entry("rateLimits".to_string())
            .or_insert_with(|| json!({}));
        let record = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        let account = self.public_account(&record);
        logging::log(
            "[Accounts]",
            &format!("✅ QClaw 账号已保存（优先级 {priority}，网关 127.0.0.1:{}）", credentials.chat_port),
        );
        Ok(account)
    }

    // ─── 公开形态 ───────────────────────────────────────────

    /// QClaw 的公开形态：通用形状 + 两个端口字段。
    ///
    /// ── 为什么要显式分派（而不是掉进通用兜底）────────────────
    /// 通用兜底会带出 `edition` / `enterpriseId` 一堆与本家无关的空字段
    /// （与 qwenwork / officeace 同一处置）。端口字段透出是有价值的：
    /// 界面上能直接看到「这个账号连的是哪个网关」，排障时不用去翻配置文件。
    /// 「有效期」（`expiresAt`）走通用形状那条路（导入时从云端 JWT 解出
    /// 写进记录），这里不用额外做什么。
    pub(crate) fn to_qclaw_public_account(&self, record: &StoredAccount) -> Value {
        let mut shaped = self.to_public_account(record);
        if let Value::Object(fields) = &mut shaped {
            for key in ["gatewayPort", "modelsPort"] {
                if let Some(value) = record.fields().get(key) {
                    fields.insert(key.to_string(), value.clone());
                }
            }
        }
        shaped
    }

    /// 回写一枚新的网关令牌（QClaw 重启后 `refresh_access_token` 用）。
    ///
    /// ── 为什么不顺带清什么缓存 ──────────────────────────────
    /// 本家没有派生凭据缓存（模型清单与令牌都是现场读），换令牌直接生效。
    pub fn update_qclaw_token(
        &self,
        account_id: &str,
        access_token: &str,
    ) -> Result<(), AccountStoreError> {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return Err(AccountStoreError::new("QClaw 网关令牌不能为空", 400));
        }
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::new("该 QClaw 账号不存在", 404));
        };
        if record.provider() != qclaw_id() {
            return Err(AccountStoreError::new("该账号不是 QClaw 账号", 400));
        }
        let mut merged = record.fields().clone();
        merged.insert(
            "accessToken".to_string(),
            Value::String(access_token.to_string()),
        );
        merged.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(access_token)),
        );
        merged.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        record = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        Ok(())
    }

    /// 回写「有效期」（云端 JWT 的 `exp`，毫秒）。
    ///
    /// 与 [`Self::update_qclaw_token`] 分开而不是并进去：那个跟着「网关
    /// 令牌变了」走（401 自愈链路），而这个跟着「云端 JWT 轮换」走 ——
    /// 两件事的触发条件不同（用户重新登录 QClaw 会换 JWT，但未必重启
    /// 网关）。调用方（适配器的 `refresh_expiry`）只在真解出新 exp 时来。
    pub fn update_qclaw_expiry(
        &self,
        account_id: &str,
        expires_at: Option<i64>,
    ) -> Result<(), AccountStoreError> {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::new("该 QClaw 账号不存在", 404));
        };
        if record.provider() != qclaw_id() {
            return Err(AccountStoreError::new("该账号不是 QClaw 账号", 400));
        }
        let mut merged = record.fields().clone();
        match expires_at.filter(|ms| *ms > 0) {
            Some(expires_at) => {
                merged.insert("expiresAt".to_string(), Value::from(expires_at));
            }
            // 传 None = 「这次没解到」，不是「清除」：擦掉一个真实事实比
            // 留着略旧的值更糟（可能是本机读不到，不是令牌没了）
            None => return Ok(()),
        }
        merged.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        record = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// provider id 必须从注册表推导（别写死字符串 —— 改名时只该改一处）。
    #[test]
    fn provider_id_matches_registry() {
        assert_eq!(QCLAW_PROVIDER_ID, "qclaw");
        assert_eq!(qclaw_id(), QCLAW_PROVIDER_ID);
    }

    /// 账号 id 的形态：`qclaw-<16 位十六进制>`，且只依赖 uid（幂等）。
    #[test]
    fn account_id_is_stable_hash_of_uid() {
        let uid = "b57d9570-1111-2222-3333-444455556666";
        let digest = format!("{:x}", Sha256::digest(uid.as_bytes()));
        let id = format!("{}-{}", qclaw_id(), truncate_chars(&digest, 16));
        assert!(id.starts_with("qclaw-"));
        assert_eq!(id.len(), "qclaw-".len() + 16);
        let again = format!(
            "{}-{}",
            qclaw_id(),
            truncate_chars(&format!("{:x}", Sha256::digest(uid.as_bytes())), 16)
        );
        assert_eq!(id, again);
    }

    /// uid 为空时（登录态解不开）用令牌摘要 —— 同令牌必须得到同一 uid。
    #[test]
    fn token_digest_is_stable_fallback_uid() {
        let token = "31ed8e5b711b856d66a9a8ae562d91c9e9745b95d45ddef9";
        let first = format!("{:x}", Sha256::digest(token.as_bytes()));
        let second = format!("{:x}", Sha256::digest(token.as_bytes()));
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }
}
