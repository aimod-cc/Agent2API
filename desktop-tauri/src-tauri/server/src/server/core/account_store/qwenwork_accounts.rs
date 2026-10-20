//! 千问办公（QwenWork 中国版）账号：导入本机登录态 / 手动粘贴、读记录、公开形态。
//!
//! ── 与各家添加路径的关系 ──────────────────────────────────────
//! 凭证解析在 `providers::qwenwork::auth`（本机 `auth-v2.dat` 解密 →
//! [`Credentials`]），本文件只负责**落盘**：接收已解析、已校验的凭据，写一条
//! 账号记录。
//!
//! ── 记录里的字段 ────────────────────────────────────────────
//! ```text
//! { id(=qwenwork-<userId 摘要>), provider: "qwenwork", name, uid(=userId),
//!   accessToken(=JWT), tokenTail, expiresAt, refreshToken, planName,
//!   source, priority, enabled, addedAt, updatedAt }
//! ```
//! `accessToken` 存的是 **JWT**（只打 `gateway.qwenwork.cn`）；MaaS 推理密钥
//! 是 2 小时短命的派生凭据，**不落账号库**，由适配器进程内缓存现签现用
//! （见 `providers::qwenwork` 的 `maas_cache`）。有状态转发会把签发后的密钥
//! 临时写进会话副本，那是编排层的事，本文件不掺和。
//!
//! id 撞到**别的 provider** 时拒绝（与 KukuAI / CatPaw 同一策略：id 空间独立，
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
use crate::server::core::providers::qwenwork::auth::Credentials;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

/// 备注名长度上限
const MAX_NAME_LENGTH: usize = 100;

/// 千问办公 provider id（**从注册表推导**，与别家同一口径）。
pub(crate) const QWENWORK_PROVIDER_ID: &str = kind_id(ProviderKind::Qwenwork);

fn qwenwork_id() -> &'static str {
    QWENWORK_PROVIDER_ID
}

impl AccountStore {
    // ─── 读：账号记录 ────────────────────────────────────────

    /// 取千问办公账号的**原始记录**（含 JWT）。
    ///
    /// `account_id` 为空 → 取本组内优先级最小的启用账号（与转发选路同一判据）；
    /// 非空 → 按 id 直查并核对归属（取不到返回 None，**不**回落队首 ——
    /// 那会把「点名的账号坏了」变成「静默用了别人的额度」）。
    pub fn qwenwork_account_record(&self, account_id: &str) -> Option<Value> {
        let _guard = self.guard();
        let provider = qwenwork_id();
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

    /// 添加/更新一个千问办公账号（`POST /api/accounts` 的 qwenwork 分支共用
    /// 入口：手动粘贴与导入本机登录态都先解析 + 校验成 [`Credentials`] 再到这里）。
    ///
    /// `source` 记来源（`"manual"` / `"desktop"`）。
    ///
    /// id = `qwenwork-<userId 的 sha256 前 16 位>`：与 Qoder 的
    /// `qoder-<region>-<hash>` 同一考虑 —— 上游 userId 是 UUID，直接当主键会让
    /// 「用户手改过的 id」与「新建」两套规则并存；摘要后也天然不会撞别家的
    /// `user-<uid>` 形态。
    pub fn add_qwenwork_account(
        &self,
        credentials: &Credentials,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let user_id = credentials.user_id.trim();
        if user_id.is_empty() {
            return Err(AccountStoreError::new(
                "千问办公账号缺少用户 id，无法标识账号",
                400,
            ));
        }
        if credentials.token.trim().is_empty() {
            return Err(AccountStoreError::new(
                "千问办公账号缺少登录令牌（token）",
                400,
            ));
        }
        let guard = self.guard();
        let digest = format!("{:x}", Sha256::digest(user_id.as_bytes()));
        let id = format!("{}-{}", qwenwork_id(), truncate_chars(&digest, 16));
        // 身份匹配：本家组内按 `uid` 找既有记录（重新导入同一账号 → 更新，
        // 而不是靠 id 再去猜一次）
        let existing = self
            .records_for_provider(&guard, qwenwork_id())
            .into_iter()
            .find(|record| record.uid() == user_id);
        // 新建时确认这个 id 没被任何人（含别家）占用 —— 主键查询，只读一行
        if existing.is_none() && self.record_by_id(&guard, &id).is_some() {
            return Err(AccountStoreError::new(
                "千问办公账号 ID 已被其它账号占用，请先核对账号记录",
                409,
            ));
        }
        let nickname = credentials.name.trim().to_string();
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
            .unwrap_or_else(|| format!("千问办公 {user_id}"));
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
            Value::String(qwenwork_id().to_string()),
        );
        merged.insert(
            "name".to_string(),
            Value::String(truncate_chars(&record_name, MAX_NAME_LENGTH)),
        );
        mark_name_custom(&mut merged, explicit, existing.as_ref());
        // `uid` 是账号身份字段（公开形态与撞 id 判定都读它）；`user.userId`
        // 是几家共用的身份字段名，这里一并写上，免得读者按家查表。
        merged.insert("uid".to_string(), Value::String(user_id.to_string()));
        merged.insert("userId".to_string(), Value::String(user_id.to_string()));
        merged.insert(
            "accessToken".to_string(),
            Value::String(credentials.token.clone()),
        );
        merged.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&credentials.token)),
        );
        // 过期时刻：凭证里给了就用，没给就从 JWT 的 `exp` 补一次 —— 与
        // `providers::qwenwork::auth::from_payload` 同一条（本机登录态文件通常
        // 不写过期时刻，而 JWT 里本来就有）。空值不覆盖既有内容：重新导入时
        // 若明文回退那份连 JWT 也解不出 exp，不该把上次的过期时间洗掉。
        let expires_at = credentials
            .expires_at
            .or_else(|| {
                crate::server::core::providers::qwenwork::auth::expires_at_from_jwt(
                    &credentials.token,
                )
            });
        if let Some(expires_at) = expires_at {
            merged.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        if !credentials.refresh_token.trim().is_empty() {
            merged.insert(
                "refreshToken".to_string(),
                Value::String(credentials.refresh_token.clone()),
            );
        }
        if !credentials.plan_name.trim().is_empty() {
            merged.insert(
                "planName".to_string(),
                Value::String(credentials.plan_name.clone()),
            );
        }
        merged.insert("source".to_string(), Value::String(source.to_string()));
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
            &format!("✅ 千问办公账号已保存（优先级 {priority}）"),
        );
        Ok(account)
    }

    // ─── 公开形态 ───────────────────────────────────────────

    /// 千问办公的公开形态：通用形状 + `expiresAt` 的读时兜底。
    ///
    /// ── 为什么要补这一层 ──────────────────────────────────
    /// 早期落库的那一批账号（那时键名不同，而本机登录态文件里本来就没有过期
    /// 字段）`expiresAt` 是 null，界面「有效期」一列就空着。让它自己长出来要
    /// 用户重导一次；而本家**没有自动续期接口**（见 accounts-domain 的说明：
    /// 过期必须回客户端重登再导入），这一列空着等于让用户靠「突然转发 401」
    /// 才发现令牌过期。
    ///
    /// 补的是**同一份事实的另一种写法**：JWT 的 `exp` 就存在记录里
    /// （`accessToken` 本身就是那枚 JWT），这里只是把它解出来 —— 不猜、不写回
    /// （写回留给下一次导入 / 换令牌，那时由 `add_qwenwork_account` 与
    /// `update_qwenwork_token` 落盘）。
    pub(crate) fn to_qwenwork_public_account(&self, record: &StoredAccount) -> Value {
        let mut shaped = self.to_public_account(record);
        if let Value::Object(fields) = &mut shaped {
            let missing = fields.get("expiresAt").map_or(true, Value::is_null);
            if missing {
                // ⚠️ 令牌必须从**原始记录**里读：公开形态把 accessToken 抹掉了
                // （它是密钥，不该出现在界面与接口响应里），在那里找恒为空
                let token = record
                    .fields()
                    .get("accessToken")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(at) =
                    crate::server::core::providers::qwenwork::auth::expires_at_from_jwt(token)
                {
                    fields.insert("expiresAt".to_string(), Value::from(at));
                }
            }
        }
        shaped
    }

    /// 回写一枚新的 JWT（用户重新登录、或手动换了令牌时用）。
    ///
    /// ── 为什么要连带清 MaaS 缓存 ────────────────────────────────
    /// 推理密钥是「用这枚 JWT 签出来的」，换了 JWT 就等于换了身份。
    /// 不清缓存的话，在过期或 401 之前会一直用**旧身份**签出来的
    /// `sk-mm-v1.*` 打推理 —— 那枚密钥按 user_id 缓存，新旧身份的 user_id
    /// 通常不同，所以这里按新令牌里解出的 userId 一并作废两处。
    pub fn update_qwenwork_token(
        &self,
        account_id: &str,
        access_token: &str,
    ) -> Result<(), AccountStoreError> {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return Err(AccountStoreError::new("千问办公登录令牌不能为空", 400));
        }
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::new("该千问办公账号不存在", 404));
        };
        if record.provider() != qwenwork_id() {
            return Err(AccountStoreError::new("该账号不是千问办公账号", 400));
        }
        let old_uid = record.uid().to_string();
        let mut merged = record.fields().clone();
        merged.insert(
            "accessToken".to_string(),
            Value::String(access_token.to_string()),
        );
        merged.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(access_token)),
        );
        // 新令牌可能属于**另一个人**（用户在客户端里换了账号再导出）。JWT 的
        // `sub` 就是 user id，解出来同步两个身份键 —— 不同步的话
        // `record.uid()` 拿到的仍是旧值，下面「两个身份都作废」退化成只作废
        // 一个（`new_uid == old_uid`），而 MaaS 推理密钥恰恰是按 user_id 建
        // 缓存的（见本函数注释里「为什么要连带清 MaaS 缓存」），旧身份那份
        // 会一直留到进程结束。
        if let Some(user_id) =
            crate::server::core::providers::qwenwork::auth::user_id_from_jwt(access_token)
        {
            merged.insert("uid".to_string(), Value::String(user_id.clone()));
            merged.insert("userId".to_string(), Value::String(user_id));
        }
        if let Some(expires_at) = crate::server::core::providers::qwenwork::auth::expires_at_from_jwt(
            access_token,
        ) {
            merged.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        merged.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        record = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        drop(guard);
        // 两个身份都要作废：旧 uid 那份缓存不会再用到，但留着就是内存泄漏
        let new_uid = record.uid().to_string();
        let invalidate = crate::server::core::providers::qwenwork::QwenworkAdapter::invalidate_maas_cache;
        if !old_uid.is_empty() {
            invalidate(Some(&old_uid));
        }
        if !new_uid.is_empty() && new_uid != old_uid {
            invalidate(Some(&new_uid));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// provider id 必须从注册表推导（别写死字符串 —— 改名时只该改一处）。
    #[test]
    fn provider_id_matches_registry() {
        assert_eq!(QWENWORK_PROVIDER_ID, "qwenwork");
        assert_eq!(qwenwork_id(), QWENWORK_PROVIDER_ID);
    }

    /// 账号 id 的形态：`qwenwork-<16 位十六进制>`。
    /// 这条锁的是「同一个人重复导入命中同一条记录」的基础 —— 摘要只依赖
    /// userId，与令牌、备注名、导入路径都无关。
    #[test]
    fn account_id_is_stable_hash_of_user_id() {
        let user_id = "b57d9570-1111-2222-3333-444455556666";
        let digest = format!("{:x}", Sha256::digest(user_id.as_bytes()));
        let id = format!("{}-{}", qwenwork_id(), truncate_chars(&digest, 16));
        assert!(id.starts_with("qwenwork-"));
        assert_eq!(id.len(), "qwenwork-".len() + 16);
        // 同一 userId 再算一次必须一致（幂等）
        let again = format!(
            "{}-{}",
            qwenwork_id(),
            truncate_chars(&format!("{:x}", Sha256::digest(user_id.as_bytes())), 16)
        );
        assert_eq!(id, again);
    }
}
