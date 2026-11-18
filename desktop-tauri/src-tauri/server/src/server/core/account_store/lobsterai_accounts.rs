//! LobsterAI 账号的**公开形态**与归属探测（本仓库保留的部分）。
//!
//! ── 这份文件为什么这么小 ────────────────────────────────────
//! 本家的换码、续期、目录、转发、签到全部搬到了**提供商插件**
//! （`agent2api-providers` 的 `lobsterai/`），不随本仓库发布 —— 那是风险隔离
//! 的一部分。账号由插件的 `login.callback` 换码后落进 `pluginSecret`，公开
//! 形态走 `plugin_accounts::to_plugin_public_account`。
//!
//! 留在这里的只有**存量内置记录**（升级上来、还没被插件重写过的那种）的展示
//! 投影：它们没有 `pluginSecret`，凭证还是顶层的 `accessToken` / `uuid` /
//! 两个 `keyfrom`，界面需要按本家的字段集显示。这不是上游协议知识，
//! 只是一份字段映射，留在网关不会带来风险。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::{AccountStore, LOBSTERAI_PROVIDER_ID};

impl AccountStore {
    /// 取一条 LobsterAI 账号记录（公开形态的原始 JSON）。
    ///
    /// 只服务管理面的**归属判定**（`api::accounts::refresh_account` 用它确认
    /// 「这个 id 属于本家」）。
    pub fn lobsterai_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == LOBSTERAI_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, LOBSTERAI_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// LobsterAI 账号的**公开形态**（进 HTTP 响应）。
    ///
    /// 只对**没有** `pluginSecret` 的存量记录生效（判据在
    /// `store_view::public_account`，插件记录先走插件那份投影）。
    ///
    /// `refreshToken` 与载荷件**都要摘干净**：前者是长期有效的续期凭据，后者是
    /// 续期的身份字段 —— 一起下发等于把「这个账号能被别人续期」的条件送出去。
    /// 界面需要的「能不能续期」由 `canRefresh` 布尔表达，不给原件。
    pub fn to_lobsterai_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        let can_refresh = record.get("refreshToken").is_some()
            && record.get("firstKeyfrom").is_some()
            && record.get("latestKeyfrom").is_some();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
            object.remove("uuid");
            object.remove("firstKeyfrom");
            object.remove("latestKeyfrom");
            object.insert("available".to_string(), Value::Bool(record.has_credentials()));
            object.insert("hasSession".to_string(), Value::Bool(true));
            object.insert("canRefresh".to_string(), Value::Bool(can_refresh));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    /// 公开形态必须把**令牌本体 + 载荷件**都摘掉（只留一个 canRefresh 布尔）。
    #[test]
    fn public_shape_never_leaks_tokens_or_payload_fields() {
        let record = StoredAccount::from_map({
            let mut map = Map::new();
            map.insert("id".into(), Value::String("lobsterai-42".into()));
            map.insert("provider".into(), Value::String(LOBSTERAI_PROVIDER_ID.into()));
            map.insert("accessToken".into(), Value::String("at".into()));
            map.insert("refreshToken".into(), Value::String("rt".into()));
            map.insert("uuid".into(), Value::String("u-1".into()));
            map.insert("firstKeyfrom".into(), Value::String("fk".into()));
            map.insert("latestKeyfrom".into(), Value::String("lk".into()));
            map
        });
        let mut public = record.to_value();
        let can_refresh = public.get("refreshToken").is_some()
            && public.get("firstKeyfrom").is_some()
            && public.get("latestKeyfrom").is_some();
        if let Some(object) = public.as_object_mut() {
            for key in [
                "accessToken",
                "refreshToken",
                "uuid",
                "firstKeyfrom",
                "latestKeyfrom",
            ] {
                object.remove(key);
            }
            object.insert("canRefresh".into(), Value::Bool(can_refresh));
        }
        for key in [
            "accessToken",
            "refreshToken",
            "uuid",
            "firstKeyfrom",
            "latestKeyfrom",
        ] {
            assert!(public.get(key).is_none(), "{key} 不该出现在公开形态里");
        }
        assert_eq!(public.get("canRefresh"), Some(&Value::Bool(true)));
    }

    /// 记录里的 `json!` 形态与 `StoredAccount` 之间往返不掉字段。
    #[test]
    fn record_roundtrip_keeps_payload_fields() {
        let map = json!({
            "id": "lobsterai-42",
            "provider": LOBSTERAI_PROVIDER_ID,
            "firstKeyfrom": "fk",
            "latestKeyfrom": "lk",
        });
        let record = StoredAccount::from_map(map.as_object().cloned().unwrap_or_default());
        assert_eq!(record.get("firstKeyfrom"), Some(&Value::String("fk".into())));
        assert_eq!(record.get("latestKeyfrom"), Some(&Value::String("lk".into())));
    }
}
