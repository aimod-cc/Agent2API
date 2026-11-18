use serde_json::{json, Value};
use super::{plugin_accounts, sql, AccountStore};
use crate::server::core::plugin_host::wasm::desktop_launch::Secret;
use crate::server::db::{CredentialSource, Db};

pub(crate) struct LaunchOrigin {
    store: AccountStore,
    db: Db,
    source: CredentialSource,
    id: String,
    provider: String,
    row: i64,
    added: i64,
    initial: Value,
    accepted: std::sync::Mutex<Value>,
}

fn read(conn: &rusqlite::Connection, id: &str, provider: &str) -> Result<(i64, i64, Value), String> {
    let record = sql::load_by_id(conn, id).map_err(|_| "账号凭证不可读")?.ok_or("账号不存在")?;
    if record.provider() != provider { return Err("账号不属于当前提供商".into()); }
    let row = conn.query_row("SELECT rowid FROM accounts WHERE id=?1 AND provider=?2", [id, provider], |row| row.get(0))
        .map_err(|_| "账号原始记录不可确认")?;
    Ok((row, record.added_at(), json!({ "identity": record.get("pluginIdentity").cloned().unwrap_or_else(|| json!(record.id())),
        "desktop": record.get("desktop").and_then(Value::as_bool).unwrap_or(false), "secret": plugin_accounts::secret_of(&record) })))
}

impl AccountStore {
    pub(crate) fn desktop_launch_origin(&self, provider: &str, id: &str) -> Result<LaunchOrigin, String> {
        let db = self.database().cloned().ok_or("账号数据库不可用")?;
        let _guard = self.guard();
        let ((row, added, initial), source) = db.prepare_credential_source(|conn| read(conn, id, provider))
            .map_err(|_| "账号存储来源不可确认")??;
        Ok(LaunchOrigin { store: self.clone(), db, source, id: id.into(), provider: provider.into(), row, added,
            accepted: std::sync::Mutex::new(initial.clone()), initial })
    }
}

impl LaunchOrigin {
    pub(crate) fn initial(&self) -> &Value { &self.initial }

    pub(crate) fn check(&self, expect: &Value) -> Result<(), String> {
        let _guard = self.store.guard();
        self.db.commit_credential_source(&self.source, |conn| {
            let (row, added, current) = read(conn, &self.id, &self.provider)?;
            let current = Secret(current);
            if row != self.row || added != self.added || &current.0 != expect {
                return Err("账号在启动准备期间已变化，请重新发起操作".into());
            }
            Ok(())
        }).map_err(|_| "账号存储已恢复或切换，本次启动确认已失效")?
    }

    pub(crate) fn redact(&self, message: &str) -> String {
        fn replace(value: &Value, message: &mut String) {
            match value {
                Value::Object(fields) => for (key, value) in fields {
                    let key = key.to_ascii_lowercase();
                    if key.contains("token") || key.contains("secret") || key.contains("password") || key.contains("cookie") || key.contains("authorization") || key == "key" {
                        if let Some(value) = value.as_str().filter(|value| value.len() >= 6) { *message = message.replace(value, "[已隐藏]"); }
                    }
                    replace(value, message);
                },
                Value::Array(values) => values.iter().for_each(|value| replace(value, message)),
                _ => {},
            }
        }
        let mut message = message.to_string();
        replace(&self.initial["secret"], &mut message);
        if let Ok(accepted) = self.accepted.lock() { replace(&accepted["secret"], &mut message); }
        message
    }

    pub(crate) fn validate_record(&self, record: &Value) -> Result<(), String> {
        if record["id"].as_str() != Some(self.id.as_str()) || record["provider"].as_str() != Some(self.provider.as_str()) {
            return Err("账号读取来源与本次启动不符".into());
        }
        let expected = self.accepted.lock().map_err(|_| "账号启动凭证来源不可用")?;
        if record["secret"] != expected["secret"] { return Err("账号在启动准备期间已变化，请重新发起操作".into()); }
        self.check(&expected)
    }

    pub(crate) fn update(&self, manager: &crate::server::access::local_session::Source,
        expect: &Value, secret: &Value, public: &Value) -> Result<Value, String> {
        let mut accepted = self.accepted.lock().map_err(|_| "账号启动凭证来源不可用")?;
        if &accepted["secret"] != expect { return Err("账号写回不是本次启动读取的凭证".into()); }
        let _guard = self.store.guard();
        let updated = self.db.commit_credential_source(&self.source, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_| "账号凭证写回不可用")?;
            if !manager.is_current() { return Err("本机管理会话已变化，未写回账号凭证".into()); }
            let (row, added, current) = read(&tx, &self.id, &self.provider)?;
            let current = Secret(current);
            if row != self.row || added != self.added || current.0 != *accepted {
                return Err("账号在启动准备期间已变化，请重新发起操作".into());
            }
            let mut record = sql::load_by_id(&tx, &self.id).map_err(|_| "账号凭证不可读")?.ok_or("账号不存在")?;
            record.set("pluginSecret", secret.clone());
            record.set("pluginPublic", public.clone());
            for key in ["accessToken", "refreshToken", "expiresAt"] {
                if record.get(key).is_some() {
                    if let Some(value) = secret.get(key) { record.set(key, value.clone()); }
                }
            }
            record.set_updated_at(crate::server::logging::now_ms());
            sql::update_in_place(&tx, &record).map_err(|_| "账号凭证写回失败")?;
            tx.commit().map_err(|_| "账号凭证写回结果未确认")?;
            Ok::<(), String>(())
        }).map_err(|_| "账号存储已恢复或切换，本次启动确认已失效")?;
        updated?;
        let next = json!({ "identity": self.initial["identity"], "desktop": self.initial["desktop"], "secret": secret });
        crate::server::core::plugin_host::wasm::desktop_launch::wipe(&mut accepted);
        *accepted = next;
        Ok(json!({ "updated": true }))
    }

    pub(crate) fn accept(&self, secret: &Value) -> Result<Secret, String> {
        let expected = self.accepted.lock().map_err(|_| "账号启动凭证来源不可用")?;
        if &expected["secret"] != secret { return Err("插件准备结果不是本次账号凭证".into()); }
        self.check(&expected)?;
        Ok(Secret(expected.clone()))
    }
}

impl Drop for LaunchOrigin {
    fn drop(&mut self) {
        crate::server::core::plugin_host::wasm::desktop_launch::wipe(&mut self.initial);
        if let Ok(value) = self.accepted.get_mut() { crate::server::core::plugin_host::wasm::desktop_launch::wipe(value); }
    }
}
