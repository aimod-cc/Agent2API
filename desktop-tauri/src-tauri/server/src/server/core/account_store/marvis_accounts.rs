//! Marvis 账号：粘贴「提取出来的凭证三件套」落账号 + 公开形态。
//!
//! ── 账号字段 ────────────────────────────────────────────────
//! ```text
//!   { id, provider:"marvis", name, accessToken(= mv_ 令牌),
//!     openid, guid, qimei36?, tokenTail, source, priority, enabled,
//!     addedAt, updatedAt, expiresAt? }
//! ```
//! `accessToken` 键名与别家统一（`has_token()` / `access_token()` 一族认它，
//! 转发时从 `auth.accessToken` 取）。`openid` / `guid` 是本家独有的第二、第三个
//! 凭据字段（上游要求三者同时出现在 `Ual-Access-*` 头里）。
//!
//! ── 为什么 id 取 openid 而不是令牌哈希 ────────────────────────
//! `openid` 是**登录身份**（换一次 token 还是同一个人），用它能做到
//! 「重新提取令牌后更新同一条记录」，而不是每换一次 token 就多一个账号。
//! Command Code 那种取 `sha256(key)` 的做法在本家是错的 —— 本家的 key 会轮换。
//! 因此 id = `marvis-` + `sha256("marvis:account-id:v1\0" + openid)` 前 12 hex
//! （openid 本身不进 id，避免它出现在 URL 与日志里）。
//!
//! ── 本文件没有任何网络请求 ──────────────────────────────────
//! 全是「读-改-写」存储操作（持锁不做网络）。联网探活见 `providers/marvis/probe.rs`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::marvis::credentials;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::MARVIS_PROVIDER_ID;
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 输入长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_INPUT_LENGTH: usize = 8192;

/// 从 payload 里取第一个非空字符串（候选键覆盖落盘名与粘贴形态）。
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

/// openid → 稳定的账号 id（见模块头）。
fn account_id_for(openid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"marvis:account-id:v1\0");
    hasher.update(openid.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("marvis-{}", &digest[..12])
}

impl AccountStore {
    /// 取一条 Marvis 账号记录。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（转发默认用队首账号）。
    pub fn marvis_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == MARVIS_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, MARVIS_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 Marvis 账号（粘贴凭证三件套的唯一落账号入口）。
    ///
    /// payload（界面表单 / `_recon/marvis-cred-extract.py` 生成的 JSON 直接粘贴）：
    ///   - `accessToken` / `access_token` / `token`：`mv_` 令牌（必填）
    ///   - `openid` / `openId`：登录 openid（必填）
    ///   - `guid` / `deviceGuid`：设备号（必填）
    ///   - `qimei36`：设备指纹（可选，缺省时转发侧按 guid 兜底）
    ///
    /// 同一 openid 的记录就地更新（保留优先级与启用状态、沿用用户改过的备注名）——
    /// 这是本家的关键语义：令牌过期后重新提取一次，应**更新**而不是新增账号。
    /// 撞到别的 provider 的 id 时报错，绝不覆写（与另外几家同一策略）。
    pub fn add_marvis_account(
        &self,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let access_token = pick(payload, &["accessToken", "access_token", "token"]);
        let openid = pick(payload, &["openid", "openId"]);
        let guid = pick(payload, &["guid", "deviceGuid"]);
        let qimei36 = pick(payload, &["qimei36", "qimei"]);
        // 到期时刻（毫秒）。三件套里它**是可选**的（提取器当前只产三项），但给了就
        // 一定落盘：本家没有刷新端点，账号页与维护任务只能靠这个读数回答「这枚令牌
        // 还能撑多久 / 该重新提取了吗」。上游字段名是 `expireTime`（`LoginInfo`），
        // 别处（会话形态）叫 `expiresAt` —— 两个都认，与 `credentials::from_record`
        // 同一口径（那里读的也是这两个键，写读不一致会让读侧永远拿到 0）。
        let expires_at = payload
            .get("expiresAt")
            .or_else(|| payload.get("expireTime"))
            .and_then(Value::as_i64)
            .filter(|value| *value > 0);

        if access_token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 accessToken（mv_ 令牌）",
                400,
            ));
        }
        if openid.is_empty() || guid.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 openid 或 guid：Marvis 需要 accessToken / openid / guid 三项\
                 （用 _recon/marvis-cred-extract.py 提取后整段粘贴）",
                400,
            ));
        }
        if access_token.chars().count() > MAX_INPUT_LENGTH
            || openid.chars().count() > MAX_INPUT_LENGTH
        {
            return Err(AccountStoreError::new("输入过长", 400));
        }
        if !access_token.starts_with(credentials::TOKEN_PREFIX) {
            return Err(AccountStoreError::new(
                format!(
                    "这不是 Marvis 的访问令牌：应以 {} 开头（本家的 UUID 形 token 是\
                     本机服务令牌，不是登录令牌）",
                    credentials::TOKEN_PREFIX
                ),
                400,
            ));
        }
        let id = account_id_for(&openid);

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != MARVIS_PROVIDER_ID {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一个\
                         openid 的 Marvis 账号（请先处理那个账号）"
                    ),
                    400,
                ));
            }
        }
        // 备注名兜底：显式传入 → 既有记录里的备注名 → 「马维斯 <尾号>」
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
            .unwrap_or_else(|| format!("马维斯 {}", token_tail_of(&access_token)));
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
            Value::String(MARVIS_PROVIDER_ID.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        record.insert("accessToken".to_string(), Value::String(access_token.clone()));
        record.insert("openid".to_string(), Value::String(openid.clone()));
        record.insert("guid".to_string(), Value::String(guid.clone()));
        if !qimei36.is_empty() {
            record.insert("qimei36".to_string(), Value::String(qimei36));
        }
        // 到期时刻：本次给了就写新的；没给但既有记录有，就沿用（手工重贴同一枚
        // 令牌时不该把已知的到期时刻抹掉 —— 抹掉等于让有效期列退回「—」）。
        if let Some(expires_at) = expires_at {
            record.insert("expiresAt".to_string(), Value::from(expires_at));
        } else if let Some(previous) = existing
            .as_ref()
            .and_then(|record| record.get("expiresAt"))
        {
            record.insert("expiresAt".to_string(), previous.clone());
        }
        record.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&access_token)),
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
                "✅ Marvis 账号已保存: {record_name}（优先级 {priority}，令牌 {}）",
                credentials::masked(&access_token)
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// Marvis 账号的**公开形态**（进 HTTP 响应）：去掉令牌本体，只留尾几位。
    ///
    /// 安全边界（SKILL 2.2）：`accessToken` 必须移除；`openid` 也一并移除 ——
    /// 它是登录身份，泄漏出去等于半个账号。
    pub fn to_marvis_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("openid");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::AccountStore;

    /// 线上形态的三件套（`mv_` 令牌 + openid + guid）。
    fn payload(extra: serde_json::Value) -> serde_json::Value {
        let mut value = json!({
            "accessToken": "mv_0123456789abcdef0123456789abcdef",
            "openid": "oAN1abcdefghijklmn",
            "guid": "guid-0001",
        });
        if let (Some(base), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            for (key, item) in extra {
                base.insert(key.clone(), item.clone());
            }
        }
        value
    }

    fn store(label: &str) -> AccountStore {
        let (db, guard) = crate::server::db::test_temp::TempDb::open(label);
        std::mem::forget(guard);
        AccountStore::with_db(Some(db))
    }

    /// 到期时刻必须落盘：账号页的有效期列与「令牌快过期了，重新提取一次」这类
    /// 提示都只能靠它。上游字段名是 `expireTime`（`LoginInfo`），会话形态叫
    /// `expiresAt` —— 两个都要认（写读不一致会让读侧永远拿到 0）。
    #[test]
    fn the_expiry_is_stored_under_either_upstream_name() {
        let store = store("marvis-expiry");
        let expires = 1_900_000_000_000i64;
        for key in ["expiresAt", "expireTime"] {
            let saved = store
                .add_marvis_account(&payload(json!({ key: expires })), None, "manual")
                .expect("应能落库");
            assert_eq!(
                saved.get("expiresAt").and_then(serde_json::Value::as_i64),
                Some(expires),
                "{key} 形态的到期时刻没有落进 expiresAt"
            );
        }
    }

    /// 重贴同一枚令牌（没给到期时刻）时，已知的值要沿用 —— 否则有效期列会从
    /// 一个真实读数退回「长期有效 / —」，用户以为信息丢了。
    #[test]
    fn re_pasting_a_token_keeps_the_known_expiry() {
        let store = store("marvis-expiry-kept");
        let expires = 1_900_000_000_000i64;
        store
            .add_marvis_account(&payload(json!({ "expireTime": expires })), None, "manual")
            .expect("首次应能落库");
        let again = store
            .add_marvis_account(&payload(json!({})), None, "manual")
            .expect("重贴应能落库");
        assert_eq!(
            again.get("expiresAt").and_then(serde_json::Value::as_i64),
            Some(expires),
            "没给新到期时刻时必须保留既有值"
        );
    }
}
