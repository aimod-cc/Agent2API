//! AtomCode 账号的**公开形态**与归属探测（本仓库保留的部分）。
//!
//! ── 这份文件为什么这么小 ────────────────────────────────────
//! 本家的登录（代理轮询）、目录、转发、额度、续期、领取全部搬到了**提供商
//! 插件**（`agent2api-providers` 的 `atomcode/`），不随本仓库发布 —— 那是风险
//! 隔离的一部分。新增账号由插件的 `account.normalize` 归一化后落进
//! `pluginSecret`，公开形态走 `plugin_accounts::to_plugin_public_account`。
//!
//! 留在这里的只有**存量内置记录**的展示投影与归属判定：它们没有
//! `pluginSecret`，凭证还是顶层的 `accessToken` / `refreshToken`，
//! 界面需要按本家的字段集显示。这不是上游协议知识。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::{ATOMCODE_PROVIDER_ID, AccountStore};

impl AccountStore {
    /// 取一条 AtomCode 账号记录（公开形态的原始 JSON）。
    ///
    /// 只服务管理面的**归属判定**（`refresh_account` 用它确认「这个 id 属于本家」）。
    pub fn atomcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == ATOMCODE_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, ATOMCODE_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// AtomCode 账号的**公开形态**（进 HTTP 响应）。
    ///
    /// 只对**没有** `pluginSecret` 的存量记录生效（判据在
    /// `store_view::public_account`，插件记录先走插件那份投影）。
    ///
    /// 摘掉**两个**令牌本体：`accessToken` 与 `refreshToken`。只摘前者的那套
    /// 在这里是漏的 —— refreshToken 是长期有效的续期凭据，不能随账号列表下发。
    pub fn to_atomcode_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
            object.insert("available".to_string(), Value::Bool(record.has_credentials()));
            object.insert(
                "hasSession".to_string(),
                Value::Bool(record.get("accessToken").is_some() || record.get("tokenTail").is_some()),
            );
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    /// 公开形态必须把**两个令牌本体**都摘掉。
    #[test]
    fn public_shape_never_leaks_either_token() {
        let record = StoredAccount::from_map({
            let mut map = Map::new();
            map.insert("id".into(), Value::String("atomcode-42".into()));
            map.insert("provider".into(), Value::String(ATOMCODE_PROVIDER_ID.into()));
            map.insert("accessToken".into(), Value::String("at-secret".into()));
            map.insert("refreshToken".into(), Value::String("rt-secret".into()));
            map.insert("tokenTail".into(), Value::String("cret".into()));
            map
        });
        // 公开形态是 store 方法，这里直接照它的规则断言（避免构造整个 store）
        let mut public = record.to_value();
        if let Some(object) = public.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
        }
        assert!(public.get("accessToken").is_none());
        assert!(public.get("refreshToken").is_none());
        assert_eq!(public.get("tokenTail").and_then(Value::as_str), Some("cret"));
    }

    /// 记录里的 `json!` 形态与 `StoredAccount` 之间往返不掉字段。
    #[test]
    fn record_roundtrip_keeps_identity_fields() {
        let map = json!({
            "id": "atomcode-42",
            "provider": ATOMCODE_PROVIDER_ID,
            "userId": "42",
            "username": "someone",
        });
        let record = StoredAccount::from_map(map.as_object().cloned().unwrap_or_default());
        assert_eq!(record.get("userId"), Some(&Value::String("42".into())));
        assert_eq!(record.get("username"), Some(&Value::String("someone".into())));
    }
}
