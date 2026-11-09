use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::context::Context;

#[derive(Clone)]
struct Entry {
    revision: u64,
    schema: u32,
    expires: i64,
    value: Value,
    bytes: usize,
}

#[derive(Default)]
pub(super) struct Transient {
    entries: HashMap<String, Entry>,
    sequence: u64,
}

fn conversations() -> &'static Mutex<Transient> {
    static STATE: OnceLock<Mutex<Transient>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Transient::default()))
}

pub(super) fn forget_provider(provider: &str) {
    if let Ok(mut state) = conversations().lock() {
        state.entries.retain(|key, _| !key.starts_with(&format!("{provider}\0")));
    }
}

impl Transient {
    pub fn remove_namespace(&mut self, namespace: &str) {
        let prefix = format!("{namespace}\0");
        self.entries.retain(|key, _| !key.starts_with(&prefix));
    }
    fn invoke(&mut self, method: &str, namespace: &str, key: &str, params: &Value, max_ttl: i64, limit: usize) -> Result<Value, proto::WireError> {
        let now = crate::server::logging::now_ms();
        self.entries.retain(|_, entry| entry.expires > now);
        let id = format!("{namespace}\0{key}");
        let previous = self.entries.get(&id);
        let schema = params["schema"].as_u64().and_then(|value| u32::try_from(value).ok()).filter(|schema| *schema > 0)
            .ok_or_else(|| proto::WireError::with_code("状态必须声明格式版本", "bad_params"))?;
        if method == "state.get" {
            return Ok(previous.map(|entry| json!({ "revision": entry.revision, "schema": entry.schema,
                "value": entry.value, "compatible": entry.schema == schema, "expires_at": entry.expires }))
                .unwrap_or_else(|| json!({ "revision": 0, "schema": schema, "value": null, "compatible": true })));
        }
        let expected = params["revision"].as_u64().ok_or_else(|| super::error("状态写入必须携带读取版本"))?;
        let revision = previous.map(|entry| entry.revision).unwrap_or(0);
        if revision != expected { return Ok(json!({ "updated": false, "revision": revision })); }
        let ttl = params["ttl_ms"].as_i64().unwrap_or(max_ttl).clamp(1000, max_ttl);
        let value = params["value"].clone();
        let bytes = value.to_string().len();
        let total: usize = self.entries.values().map(|entry| entry.bytes).sum();
        let namespaced = self.entries.keys().filter(|entry| entry.starts_with(&format!("{namespace}\0"))).count();
        if let Some((provider, _)) = namespace.split_once('\0') {
            let prefix = format!("{provider}\0");
            let provider_entries: Vec<_> = self.entries.iter().filter(|(key, _)| key.starts_with(&prefix)).collect();
            let provider_bytes: usize = provider_entries.iter().map(|(_, entry)| entry.bytes).sum();
            if provider_entries.len() >= 1024 && previous.is_none()
                || provider_bytes.saturating_sub(previous.map(|entry| entry.bytes).unwrap_or(0)) + bytes > 16 * 1024 * 1024 {
                return Err(proto::WireError::with_code("提供商会话状态超过容量限制", "plugin_resource_limit"));
            }
        }
        if bytes > 256 * 1024 || total.saturating_sub(previous.map(|entry| entry.bytes).unwrap_or(0)) + bytes > 64 * 1024 * 1024
            || previous.is_none() && (namespaced >= limit || self.entries.len() >= 4096) {
            return Err(proto::WireError::with_code("插件临时状态超过容量限制", "plugin_resource_limit"));
        }
        self.sequence = self.sequence.checked_add(1).ok_or_else(|| super::error("状态版本已耗尽"))?;
        let revision = self.sequence;
        self.entries.insert(id, Entry { revision, schema, expires: now + ttl, value, bytes });
        Ok(json!({ "updated": true, "revision": revision }))
    }
}

impl Context {
    pub(super) async fn state(&mut self, method: &str, params: &Value) -> Result<Value, proto::WireError> {
        self.permission("state.v1")?;
        let key = params["key"].as_str().filter(|key| !key.is_empty() && key.len() <= 128 && !key.contains('\0'))
            .ok_or_else(|| super::error("插件状态名称无效"))?;
        if params.get("conversation").is_some_and(|value| value.as_str().is_none_or(|key|
            key.is_empty() || key.len() > 256 || key.contains('\0'))) { return Err(super::error("会话状态标识无效")); }
        let provider = &self.shared.options.provider;
        let schema = params["schema"].as_u64().and_then(|value| u32::try_from(value).ok()).filter(|schema| *schema > 0)
            .ok_or_else(|| super::error("插件状态格式版本无效"))?;
        match params["scope"].as_str() {
            Some("login") => {
                if self.flow.is_empty() { return Err(super::error("当前调用未绑定登录流程")); }
                let namespace = format!("{}:{}", provider, self.flow);
                self.shared.login_state.lock().map_err(|_| super::error("登录状态不可用"))?
                    .invoke(method, &namespace, key, params, 900_000, 64)
            }
            Some("conversation") => {
                let account = self.account()?;
                let session = params["conversation"].as_str().filter(|key| !key.is_empty() && key.len() <= 256 && !key.contains('\0'))
                    .map(|key| format!("plugin:{key}")).unwrap_or_else(|| self.session.clone());
                if session.is_empty() { return Err(super::error("当前请求缺少会话标识")); }
                use sha2::{Digest, Sha256};
                let identity = format!("{:x}", Sha256::digest(self.identity.as_bytes()));
                let namespace = format!("{provider}\0{account}\0{identity}\0{session}");
                conversations().lock().map_err(|_| super::error("会话状态不可用"))?
                    .invoke(method, &namespace, key, params, 86_400_000, 32)
            }
            Some("account") => {
                let account = self.account()?.to_string();
                let provider = provider.clone();
                let key = format!("plugin:{key}");
                let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(|| super::error("账号存储尚未就绪"))?;
                let params = params.clone();
                let get = method == "state.get";
                crate::server::core::blocking::run(move || {
                    if get {
                        let mut state = store.plugin_state(&provider, &account, &key)?;
                        state["compatible"] = json!(state["revision"] == 0 || state["schema"] == schema);
                        Ok(state)
                    } else {
                        let revision = params["revision"].as_u64().ok_or("状态写入缺少读取版本")?;
                        store.plugin_state_compare_exchange(&provider, &account, &key, revision, schema, params["value"].clone())
                    }
                }).await.map_err(|error| super::error(error.message))?.map_err(super::error)
            }
            _ => Err(super::error("状态范围必须是 login、conversation 或 account")),
        }
    }
}
