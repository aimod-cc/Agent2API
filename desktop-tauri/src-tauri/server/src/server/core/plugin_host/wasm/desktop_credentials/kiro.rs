//! Kiro 固定只读来源；保存的 CLI/IDE 引用不因另一客户端出现而切换。
//! 只取官方 token、注册与 profile 的确切字段，不接受路径、SQL 或执行命令。
use agent2api_provider_protocol as proto;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

#[cfg(any(windows, target_os = "macos"))]
use super::app_data;
use super::{database, home, invalid, object};

fn copy_alias(
    value: &mut Value,
    target: &str,
    input: &Value,
    aliases: &[&str],
) -> Result<(), proto::WireError> {
    let mut found = None;
    for key in aliases {
        if let Some(item) = input
            .get(*key)
            .filter(|item| !item.is_null() && *item != "")
        {
            if found.is_some_and(|old| old != item) {
                return Err(invalid());
            }
            found = Some(item);
        }
    }
    if let Some(item) = found {
        value[target] = item.clone();
    }
    Ok(())
}

fn registration(
    path: &std::path::Path,
    value: &mut Value,
    hash: &str,
) -> Result<(), proto::WireError> {
    if hash.is_empty()
        || hash.len() > 128
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid());
    }
    // 即使上游文件名来自 JSON，也只允许同目录内的一个普通文件。
    let item = object(&path.with_file_name(format!("{hash}.json")), 64 * 1024)?;
    copy_alias(value, "clientId", &item, &["clientId", "client_id"])?;
    copy_alias(
        value,
        "clientSecret",
        &item,
        &["clientSecret", "client_secret"],
    )?;
    Ok(())
}

fn auth_row(db: &rusqlite::Connection, key: &str) -> Result<Option<Value>, proto::WireError> {
    let row: Option<String> = db
        .query_row(
            "SELECT value FROM auth_kv WHERE key = ?1 AND length(CAST(value AS BLOB)) <= 131072",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    row.map(|text| serde_json::from_str(&text).map_err(|_| invalid()))
        .transpose()
}

fn state_field(db: &rusqlite::Connection, key: &str) -> Result<Option<Value>, proto::WireError> {
    if !matches!(key, "auth.idc.region" | "api.codewhisperer.profile") {
        return Err(invalid());
    }
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'state')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| invalid())?;
    if !exists {
        return Ok(None);
    }
    let row: Option<String> = db
        .query_row(
            "SELECT value FROM state WHERE key = ?1 AND length(CAST(value AS BLOB)) <= 4096",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    Ok(row.map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text))))
}

fn login_provider(input: &Value, method: &str, start: &str) -> String {
    let provider = input["provider"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["google", "github", "builderid", "enterprise", "internal"].contains(&provider.as_str()) {
        return provider;
    }
    if method == "external-idp" {
        return "external-idp".into();
    }
    if method == "idc" {
        return if start.is_empty() || start == "https://view.awsapps.com/start" {
            "builderid"
        } else {
            "enterprise"
        }
        .into();
    }
    String::new()
}

fn read_cli() -> Result<Option<Value>, proto::WireError> {
    #[cfg(any(windows, target_os = "macos"))]
    let path = app_data()?.join("kiro-cli/data.sqlite3");
    #[cfg(not(any(windows, target_os = "macos")))]
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or(home()?.join(".local/share"))
        .join("kiro-cli/data.sqlite3");
    if !path.exists() {
        return Ok(None);
    }
    let db = database(&path)?;
    // 同一只读事务里取 token/注册/profile，避免客户端切号时拼出两份材料。
    let transaction = db.unchecked_transaction().map_err(|_| invalid())?;
    for kind in ["social", "odic", "external-idp"] {
        let key = format!("kirocli:{kind}:token");
        let Some(token) = auth_row(&transaction, &key)? else {
            continue;
        };
        if !token.is_object() {
            return Err(invalid());
        }
        let method = if kind == "odic" { "idc" } else { kind };
        let mut value = json!({"method":method,"source":"kiro-cli"});
        for (target, aliases) in [
            ("access", &["access_token", "accessToken"][..]),
            ("refresh", &["refresh_token", "refreshToken"][..]),
            ("expires", &["expires_at", "expiresAt"][..]),
            ("region", &["region"][..]),
            ("profileArn", &["profile_arn", "profileArn"][..]),
            ("clientId", &["client_id", "clientId"][..]),
            ("tokenURL", &["token_endpoint", "tokenEndpoint"][..]),
            ("accountId", &["email", "accountId"][..]),
        ] {
            copy_alias(&mut value, target, &token, aliases)?;
        }
        if value["access"].as_str().is_none_or(str::is_empty) {
            continue;
        }
        if method == "idc" {
            if let Some(item) = auth_row(&transaction, "kirocli:odic:device-registration")? {
                copy_alias(&mut value, "clientId", &item, &["client_id", "clientId"])?;
                copy_alias(
                    &mut value,
                    "clientSecret",
                    &item,
                    &["client_secret", "clientSecret"],
                )?;
            }
            if value["region"].as_str().is_none_or(str::is_empty) {
                if let Some(region) = state_field(&transaction, "auth.idc.region")? {
                    value["region"] = region;
                }
            }
        }
        if value["profileArn"].as_str().is_none_or(str::is_empty) {
            if let Some(profile) = state_field(&transaction, "api.codewhisperer.profile")? {
                value["profileArn"] = if profile.is_object() {
                    profile["arn"].clone()
                } else {
                    profile
                };
            }
        }
        let start = token["start_url"]
            .as_str()
            .or_else(|| token["startUrl"].as_str())
            .unwrap_or("");
        let provider = login_provider(&token, method, start);
        value["loginProvider"] = json!(provider);
        value["uid"] = value["loginProvider"].clone();
        return Ok(Some(value));
    }
    Ok(None)
}

fn read_ide() -> Result<Value, proto::WireError> {
    let path = home()?.join(".aws/sso/cache/kiro-auth-token.json");
    let token = object(&path, 128 * 1024)?;
    let mut value = json!({"source":"kiro-ide"});
    for (target, aliases) in [
        ("accessToken", &["accessToken", "access_token"][..]),
        ("refreshToken", &["refreshToken", "refresh_token"][..]),
        ("expiresAt", &["expiresAt", "expires_at"][..]),
        ("region", &["region"][..]),
        ("profileArn", &["profileArn", "profile_arn"][..]),
        ("clientId", &["clientId", "client_id"][..]),
        ("clientSecret", &["clientSecret", "client_secret"][..]),
        (
            "tokenURL",
            &["tokenEndpoint", "token_endpoint", "tokenURL", "tokenUrl"][..],
        ),
        ("accountId", &["accountId", "email"][..]),
    ] {
        copy_alias(&mut value, target, &token, aliases)?;
    }
    let method = token["authMethod"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let provider = token["provider"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let explicit = match method.as_str() {
        "social" => Some("social"),
        "idc" | "oidc" | "odic" => Some("idc"),
        "external-idp" | "external_idp" | "externalidp" => Some("external-idp"),
        "" if matches!(provider.as_str(), "google" | "github") => Some("social"),
        "" if matches!(
            provider.as_str(),
            "externalidp" | "external_idp" | "external-idp"
        ) =>
        {
            Some("external-idp")
        }
        "" => None,
        _ => return Err(invalid()),
    };
    // 社交与外部 IdP 不使用 AWS 注册材料；缺少无关 companion 不能让有效本机导入失败。
    if explicit.is_none() || explicit == Some("idc") {
        if let Some(hash) = token["clientIdHash"].as_str() {
            registration(&path, &mut value, hash)?;
        }
    }
    let method = explicit.unwrap_or_else(|| {
        if value["clientId"].as_str().is_none_or(str::is_empty) {
            "social"
        } else {
            "idc"
        }
    });
    value["method"] = json!(method);
    value["loginProvider"] = json!(login_provider(&token, method, ""));
    value["uid"] = value["loginProvider"].clone();
    Ok(value)
}

pub(super) fn read() -> Result<Value, proto::WireError> {
    read_source("kiro")
}

pub(super) fn read_source(source: &str) -> Result<Value, proto::WireError> {
    match source {
        "kiro-cli" => read_cli()?.ok_or_else(invalid),
        "kiro-ide" => read_ide(),
        "kiro" => match read_cli()? {
            Some(value) => Ok(value),
            None => read_ide(),
        },
        _ => Err(invalid()),
    }
}
