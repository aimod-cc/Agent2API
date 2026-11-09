use serde_json::{json, Value};
use super::{sql, AccountStore};

impl AccountStore {
    pub(crate) fn plugin_state_revision(&self) -> u64 {
        self.database().map(|db| db.account_revision()).unwrap_or(0)
    }
    pub(crate) fn plugin_state(&self, provider: &str, id: &str, key: &str) -> Result<Value, String> {
        let record = self.read_record(id).ok_or("账号不存在")?;
        if record.provider() != provider { return Err("账号不属于当前提供商".into()); }
        Ok(record.get("pluginState").and_then(|state| state.get(key)).cloned()
            .unwrap_or_else(|| json!({ "revision": 0, "schema": 1, "value": null })))
    }

    pub(crate) fn plugin_state_compare_exchange(
        &self, provider: &str, id: &str, key: &str, expected: u64, schema: u32, value: Value,
    ) -> Result<Value, String> {
        if key.is_empty() || key.len() > 160 || schema == 0 || value.to_string().len() > 256 * 1024 {
            return Err("插件状态名称、版本或大小无效".into());
        }
        let guard = self.guard();
        self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let Some(mut record) = sql::load_by_id(&tx, id)? else { return Ok(Err("账号不存在".to_string())) };
            if record.provider() != provider { return Ok(Err("账号归属已变化".to_string())); }
            let mut state = record.get("pluginState").and_then(Value::as_object).cloned().unwrap_or_default();
            let revision = state.get(key).and_then(|entry| entry["revision"].as_u64()).unwrap_or(0);
            if revision != expected { return Ok(Ok(json!({ "updated": false, "revision": revision }))); }
            if !state.contains_key(key) && state.len() >= 128 { return Ok(Err("账号插件状态条数超过上限".into())); }
            let revision = revision.checked_add(1).ok_or(rusqlite::Error::InvalidQuery)?;
            state.insert(key.into(), json!({ "revision": revision, "schema": schema, "value": value }));
            if serde_json::to_vec(&state).map(|bytes| bytes.len()).unwrap_or(usize::MAX) > 2 * 1024 * 1024 {
                return Ok(Err("账号插件状态总量超过 2 MiB".into()));
            }
            record.set("pluginState", Value::Object(state));
            sql::update_in_place(&tx, &record)?;
            tx.commit()?;
            Ok(Ok(json!({ "updated": true, "revision": revision })))
        }).map_err(|error| error.message)?
    }

    pub(crate) fn plugin_states_for_provider(&self, provider: &str, key: &str) -> Vec<(String, Value)> {
        self.read_provider(provider).iter().filter(|record| record.enabled() && record.has_credentials())
            .filter_map(|record| record.get("pluginState").and_then(|state| state.get(key))
                .map(|value| (record.id().to_string(), value.clone())))
            .collect()
    }
}
