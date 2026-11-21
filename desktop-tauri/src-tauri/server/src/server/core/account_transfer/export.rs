use serde::Serialize;
use serde_json::Value;

use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::{AccountStore, AccountStoreError};

use super::identity::CREDENTIAL_KEYS;

pub const ACCOUNT_FILE_FORMAT: &str = "agent2api-account";

pub enum ExportSelection {
    Account(String),
    Provider(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFile {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub content: Value,
}

#[derive(Serialize)]
pub struct ExportFailure {
    pub id: String,
    pub name: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct AccountFiles {
    pub total: usize,
    pub files: Vec<AccountFile>,
    pub failed: Vec<ExportFailure>,
}

pub fn export_account_files(
    store: &AccountStore,
    selection: ExportSelection,
) -> Result<AccountFiles, AccountStoreError> {
    let definitions = crate::server::core::custom_providers::list();
    let stamp = crate::server::logging::now_ms();
    store.with_lock(|guard| {
        let ids = match selection {
            ExportSelection::Account(id) => vec![id],
            ExportSelection::Provider(provider) => {
                let mut rows = store.with_conn(guard, sql::queue_rows)?;
                rows.retain(|row| row.provider == provider);
                rows.sort_by_key(|row| (row.priority, row.added_at, row.id.clone()));
                rows.into_iter().map(|row| row.id).collect()
            }
        };
        let mut result = AccountFiles {
            total: ids.len(),
            files: Vec::new(),
            failed: Vec::new(),
        };
        for (index, id) in ids.into_iter().enumerate() {
            let record = match store.with_conn(guard, |conn| sql::load_by_id(conn, &id)) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    result.failed.push(ExportFailure {
                        id,
                        name: String::new(),
                        error: "账号不存在".to_string(),
                    });
                    continue;
                }
                Err(error) => {
                    result.failed.push(ExportFailure {
                        id,
                        name: String::new(),
                        error: error.message,
                    });
                    continue;
                }
            };
            match account_file(&record, &definitions, stamp, index + 1) {
                Ok(file) => result.files.push(file),
                Err(error) => result.failed.push(ExportFailure {
                    id,
                    name: record.name(),
                    error,
                }),
            }
        }
        Ok(result)
    })
}

fn account_file(
    record: &StoredAccount,
    definitions: &[Value],
    stamp: i64,
    index: usize,
) -> Result<AccountFile, String> {
    let provider = record.provider();
    let mut content = super::to_export_record(record)
        .as_object()
        .cloned()
        .unwrap_or_default();
    content.retain(|key, _| !key.starts_with("pluginLedger."));
    let plugin_secret = content.get("pluginSecret").cloned();
    // 插件凭证优先于顶层镜像；摊平后仍保留原始 secret，兼容凭证导入与备份回流。
    if let Some(Value::Object(secret)) = plugin_secret.as_ref() {
        // 旧记录的镜像可能已失效；插件 secret 是凭证的权威副本。
        for key in CREDENTIAL_KEYS {
            content.remove(*key);
        }
        for (key, value) in secret {
            if key.starts_with('_')
                || key.starts_with("pluginLedger.")
                || matches!(
                    key.as_str(),
                    "id" | "provider"
                        | "name"
                        | "priority"
                        | "enabled"
                        | "addedAt"
                        | "updatedAt"
                        | "rateLimits"
                        | "pluginSecret"
                        | "pluginPublic"
                        | "customProviders"
                )
            {
                continue;
            }
            content.insert(key.clone(), value.clone());
        }
    }
    let desktop =
        record.is_desktop() || content.get("desktop").and_then(Value::as_bool) == Some(true);
    if desktop && super::identity::is_reserved_desktop_id(record.id()) {
        return Err(
            "桌面端实时登录态的凭证不随账号文件迁移，请在目标机器重新导入桌面端登录态".to_string(),
        );
    }
    if desktop {
        let credentials = record
            .get("pluginSecret")
            .and_then(Value::as_object)
            .unwrap_or(record.fields());
        let has_token = CREDENTIAL_KEYS.iter().any(|key| {
            credentials
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        });
        if !has_token {
            return Err("桌面端实时登录态的凭证未保存在账号库中，无法导出；请在目标机器重新导入桌面端登录态".to_string());
        }
        content.remove("desktop");
        if let Some(Value::Object(secret)) = content.get_mut("pluginSecret") {
            secret.remove("desktop");
        }
        if let Some(Value::Object(public)) = content.get_mut("pluginPublic") {
            public.insert("desktop".to_string(), Value::Bool(false));
        }
    }
    if !record.has_credentials() {
        return Err("账号未配置凭证，无法导出".to_string());
    }
    let name = record.name();
    content.insert("id".to_string(), Value::String(record.id().to_string()));
    content.insert("provider".to_string(), Value::String(provider.clone()));
    content.insert("name".to_string(), Value::String(name.clone()));
    content.insert(
        "format".to_string(),
        Value::String(ACCOUNT_FILE_FORMAT.to_string()),
    );
    if let Some(Value::Object(secret)) = plugin_secret.as_ref() {
        let has_access = content
            .get("accessToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty());
        if !has_access {
            let token = crate::server::core::account_store::raccoon_accounts::first_non_empty(
                secret,
                &["token", "access_token", "session"],
            );
            if !token.is_empty() {
                content.insert("accessToken".to_string(), Value::String(token));
            }
        }
    }
    if provider.starts_with(crate::server::core::custom_providers::ID_PREFIX) {
        let definition = definitions
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(provider.as_str()))
            .ok_or_else(|| "自定义提供商定义不存在，无法导出可迁移的账号".to_string())?;
        content.insert(
            "customProviders".to_string(),
            Value::Array(vec![definition.clone()]),
        );
    }
    let label = if name.trim().is_empty() {
        record.id()
    } else {
        name.trim()
    };
    let stem = safe_file_stem(&format!("{provider}-{label}"));
    Ok(AccountFile {
        id: record.id().to_string(),
        name,
        file_name: format!("{stem}-{stamp}-{index:03}.json"),
        content: Value::Object(content),
    })
}

fn safe_file_stem(value: &str) -> String {
    let stem: String = value
        .chars()
        .take(90)
        .map(|char| {
            if char.is_control()
                || matches!(char, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                char
            }
        })
        .collect();
    let stem = stem.trim().trim_end_matches(['.', ' ']);
    if stem.is_empty() {
        "account".to_string()
    } else {
        stem.to_string()
    }
}
