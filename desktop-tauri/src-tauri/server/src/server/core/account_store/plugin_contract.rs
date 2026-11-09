use std::sync::MutexGuard;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{account_identity, sql, state::StoredAccount, AccountStore};

impl AccountStore {
    pub(super) fn plugin_contract_upsert(
        &self, guard: &MutexGuard<'_, ()>, provider: &str, identity: &str, account: &StoredAccount,
    ) -> Result<(String, Option<StoredAccount>), String> {
        if identity.trim().is_empty() || identity.len() > 512 || identity.contains('\0') {
            return Err("插件必须提供非空且稳定的账号身份".into());
        }
        let records = self.with_conn(guard, |conn| {
            sql::queue_rows(conn)?.into_iter().filter(|row| row.provider == provider)
                .map(|row| sql::load_by_id(conn, &row.id)).collect::<rusqlite::Result<Vec<_>>>()
        }).map_err(|error| error.message)?;
        let mut matches = Vec::new();
        for record in records.into_iter().flatten() {
            let same = if record.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1) {
                record.get("pluginIdentity").and_then(Value::as_str) == Some(identity)
                    && account_identity::desktop(record.fields()) == account_identity::desktop(account.fields())
            } else {
                let mut legacy = account.fields().clone();
                legacy.remove("pluginIdentityContract");
                legacy.remove("pluginIdentity");
                account_identity::same(provider, record.fields(), &legacy)
                    || record.get("pluginIdentity").and_then(Value::as_str) == Some(identity)
                        && account_identity::reference(provider, record.fields()) == account_identity::reference(provider, &legacy)
                        && !account_identity::conflicts(provider, record.fields(), &legacy)
            };
            if same { matches.push(record); }
        }
        if matches.len() > 1 { return Err("多条旧账号匹配同一插件身份，未自动合并或覆盖".into()); }
        if let Some(record) = matches.pop() { return Ok((record.id().to_string(), Some(record))); }
        let mut hash = Sha256::new();
        hash.update(provider.as_bytes());
        hash.update([0, u8::from(account_identity::desktop(account.fields())), 0]);
        hash.update(identity.as_bytes());
        let id = format!("{provider}-account-{:x}", hash.finalize());
        if self.with_conn(guard, |conn| sql::load_by_id(conn, &id)).map_err(|error| error.message)?.is_some() {
            return Err("账号身份与已有主键冲突，未覆盖原账号".into());
        }
        Ok((id, None))
    }
}
