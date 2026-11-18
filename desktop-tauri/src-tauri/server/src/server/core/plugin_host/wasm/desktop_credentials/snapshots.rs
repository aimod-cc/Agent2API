use agent2api_provider_protocol as proto;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{absolute_env, app_data, content, database, home, invalid, object, project};

pub(super) fn workbuddy(provider: &str) -> Result<Value, proto::WireError> {
    #[cfg(windows)]
    let base = absolute_env("LOCALAPPDATA").ok_or_else(invalid)?;
    #[cfg(target_os = "macos")]
    let base = home()?.join("Library/Application Support");
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = home()?.join(".local/share");
    let root = base.join("CodeBuddyExtension/Data/Public/auth");
    let prefix = if provider == "workbuddy-intl" {
        "workbuddy-desktop-ai"
    } else {
        "workbuddy-desktop"
    };
    let current = root.join(format!("{prefix}.info"));
    let path = if current.exists() {
        current
    } else {
        let entries = std::fs::read_dir(&root)
            .map_err(|_| invalid())?
            .take(2049)
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|_| invalid())?;
        if entries.len() > 2048 {
            return Err(invalid());
        }
        let mut files = entries
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&format!("{prefix}.")) && name.ends_with(".info")
                    })
            })
            .collect::<Vec<_>>();
        files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        files.into_iter().next().ok_or_else(invalid)?
    };
    let value = object(&path, 256 * 1024)?;
    let account_keys = [
        "uid",
        "nickname",
        "type",
        "enterpriseId",
        "enterpriseName",
        "departmentFullName",
        "oneidAccountId",
        "idp",
    ];
    let auth_keys = [
        "accessToken",
        "access_token",
        "token",
        "refreshToken",
        "refresh_token",
        "expiresAt",
        "expires_at",
        "refreshExpiresAt",
        "refresh_expires_at",
        "domain",
        "tokenType",
    ];
    let mut result = project(&value, &["edition", "endpoint", "prefixPath", "platform"]);
    for (group, keys) in [("account", &account_keys[..]), ("auth", &auth_keys[..])] {
        let mut fields = project(&value[group], keys);
        // 官方嵌套字段优先；混合导出只补缺值，不把其他账号或地区字段拼入。
        for key in keys {
            let aliases: &[&str] = match (group, *key) {
                ("auth", "accessToken" | "access_token" | "token") => &["accessToken", "access_token", "token"],
                ("auth", "refreshToken" | "refresh_token") => &["refreshToken", "refresh_token"],
                ("auth", "expiresAt" | "expires_at") => &["expiresAt", "expires_at"],
                ("auth", "refreshExpiresAt" | "refresh_expires_at") => &["refreshExpiresAt", "refresh_expires_at"],
                _ => std::slice::from_ref(key),
            };
            if aliases.iter().any(|alias| value[group].get(*alias).is_some_and(|field| {
                !field.is_null() && !field.as_str().is_some_and(|text| text.trim().is_empty())
            })) {
                continue;
            }
            if let Some(field) = value.get(*key) {
                fields[*key] = field.clone();
            }
        }
        result[group] = fields;
    }
    content(&path, result)
}

pub(super) fn orcaterm() -> Result<Value, proto::WireError> {
    let path = match absolute_env("ORCATERM_DATA_FILE") {
        Some(path) => path,
        None => {
            #[cfg(any(windows, target_os = "macos"))]
            let root = app_data()?;
            #[cfg(not(any(windows, target_os = "macos")))]
            let root = match absolute_env("XDG_DATA_HOME") {
                Some(path) => path,
                None => home()?.join(".local/share"),
            };
            root.join("com.orcaterm-desktop.app/data.bin")
        }
    };
    content(
        &path,
        project(
            &object(&path, 256 * 1024)?,
            &["oauth_access_token", "oauth_expires_at", "auth_identity"],
        ),
    )
}

pub(super) fn cline() -> Result<Value, proto::WireError> {
    let path = home()?.join(".cline/data/settings/providers.json");
    let value = object(&path, 256 * 1024)?;
    let auth = &value["providers"]["cline"]["settings"]["auth"];
    if !auth.is_object() {
        return Err(invalid());
    }
    let mut auth = project(
        auth,
        &[
            "accessToken",
            "refreshToken",
            "expiresAt",
            "accountId",
            "metadata",
        ],
    );
    if auth["metadata"].is_object() {
        auth["metadata"] = json!({"userInfo":project(&auth["metadata"]["userInfo"], &["subject", "clineUserId", "email", "name", "firstName", "lastName"])});
    }
    let mut response = content(
        &path,
        json!({"providers":{"cline":{"settings":{"auth":auth}}}}),
    )?;
    response
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("sourceId");
    Ok(response)
}

pub(super) fn quwork() -> Result<Value, proto::WireError> {
    let path = app_data()?.join("qurwork-desktop/qurwork-desktop.sqlite");
    if path.exists() {
        let db = database(&path)?;
        let row: Option<String> = db.query_row("SELECT session_json FROM auth_session WHERE id = 1 AND schema_version = 2 AND length(session_json) <= 262144", [], |row| row.get(0)).optional().map_err(|_| invalid())?;
        let row = row.ok_or_else(invalid)?;
        let value: Value = super::super::managed_task::declaration::parse_object(row.as_bytes(), 256 * 1024)
            .map_err(|_| invalid())?;
        return content(&path, quwork_projection(&value, 0)?);
    }
    let path = home()?.join(".qurwork-desktop-dsh/.dsh/.credentials.yaml");
    let raw = super::file(&path, 256 * 1024)?;
    let raw = std::str::from_utf8(&raw).map_err(|_| invalid())?;
    let raw = raw
        .lines()
        .find_map(|line| line.trim().strip_prefix("QURIVS_LLM_API_KEY:"))
        .ok_or_else(invalid)?
        .trim();
    let token = if raw.starts_with('"') {
        serde_json::from_str::<String>(raw).map_err(|_| invalid())?
    } else {
        raw.trim_matches('\'').to_string()
    };
    content(&path, json!({"llmToken":token}))
}

// 保留原客户端的凭据容器语义，只投影本家字段，不返回未筛选的嵌套 JSON。
fn quwork_projection(value: &Value, depth: usize) -> Result<Value, proto::WireError> {
    if depth > 6 || !value.is_object() || value.to_string().len() > 256 * 1024 {
        return Err(invalid());
    }
    let mut input = value;
    for key in ["pluginSecret", "secret", "session", "data"] {
        if input[key].is_object() {
            input = &input[key];
        }
    }
    let nested = input["llmToken"].as_str().filter(|raw| raw.trim().starts_with('{'));
    let mut result = json!({});
    if let Some(raw) = input["credentials"].as_str().or(nested).filter(|raw| !raw.trim().is_empty()) {
        let raw = raw.trim().trim_start_matches('\u{feff}').trim();
        let projected = if raw.starts_with('{') {
            let value: Value = super::super::managed_task::declaration::parse_object(raw.as_bytes(), 256 * 1024)
                .map_err(|_| invalid())?;
            quwork_projection(&value, depth + 1)?
        } else {
            let raw = raw.lines().find_map(|line| line.trim().strip_prefix("QURIVS_LLM_API_KEY:"))
                .map(str::trim).unwrap_or(raw);
            let token = if raw.starts_with('"') {
                serde_json::from_str::<String>(raw).map_err(|_| invalid())?
            } else {
                raw.trim_matches('\'').to_string()
            };
            json!({"llmToken": token})
        };
        // 保留内外层的令牌和到期优先规则，由插件原解析器归一。
        result["credentials"] = json!(serde_json::to_string(&projected).map_err(|_| invalid())?);
    }
    for (target, aliases) in [
        ("accessToken", &["accessToken", "access_token", "token"][..]),
        ("llmToken", &["llmToken", "llm_token", "apiKey", "key"][..]),
    ] {
        if target == "llmToken" && nested.is_some() { continue; }
        if let Some(raw) = aliases.iter().find_map(|key| input[*key].as_str().filter(|raw| !raw.trim().is_empty())) {
            result[target] = json!(raw);
        }
    }
    for key in ["expiresAt", "expire", "updatedAt", "name", "deviceIdentifier", "deviceUuid"] {
        if let Some(value) = input.get(key).filter(|value| !value.is_null()
            && !value.as_str().is_some_and(|text| text.trim().is_empty())) {
            result[key] = value.clone();
        }
    }
    let user = if input["user"].is_object() { Some(&input["user"]) }
        else if input.get("uid").is_some() { Some(input) } else { None };
    if let Some(user) = user {
        result["user"] = project(user, &["id", "uid", "userId", "username", "display_name", "displayName", "nickname", "phone"]);
    }
    Ok(result)
}
