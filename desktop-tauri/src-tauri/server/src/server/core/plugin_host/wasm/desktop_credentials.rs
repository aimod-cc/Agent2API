//! 固定客户端凭据读取只用于本次显式导入或已保存的本机账号，不提供路径、SQL和进程接口。
use std::{
    io::Read,
    path::{Path, PathBuf},
};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::context::Context;

mod astudio;
mod autoclaw;
mod codar;
pub(super) mod cookie_file;
mod commandcode;
mod doubao;
mod dumate;
mod kiro;
mod kuku;
mod loomy;
mod raccoon;
pub(super) mod refresh;
mod snapshots;

pub(super) const HELPER_CATPAW: &str = "desktop.credentials.catpaw.v1";
pub(super) const HELPER_DOUBAO: &str = "desktop.credentials.doubaowork.v1";
pub(super) const HELPER_CODAR: &str = "desktop.credentials.codar.v1";
pub(super) const HELPER_RACCOON_REFRESH: &str = "desktop.refresh.raccoon.v1";
pub(super) const HOST: &str = "agent2api-host";
pub(super) const VERSION: &str = "desktop-read-v1";

tokio::task_local! {
    static IMPORT_PROVIDER: String;
    static IMPORT_USER_CREDENTIALS: bool;
}

pub(crate) async fn import_scope<F: std::future::Future>(
    provider: &str,
    selected: bool,
    work: F,
) -> F::Output {
    IMPORT_PROVIDER
        .scope(
            if selected {
                provider.to_string()
            } else {
                String::new()
            },
            work,
        )
        .await
}

pub(crate) async fn supplied_scope<F: std::future::Future>(selected: bool, work: F) -> F::Output {
    IMPORT_USER_CREDENTIALS.scope(selected, work).await
}

pub(crate) fn supplied_import() -> bool {
    IMPORT_USER_CREDENTIALS
        .try_with(|selected| *selected)
        .unwrap_or(false)
}

pub(crate) fn selected_import(provider: &str) -> bool {
    IMPORT_PROVIDER
        .try_with(|selected| selected == provider)
        .unwrap_or(false)
}

pub(crate) fn helper(provider: &str) -> Option<&'static str> {
    match provider {
        "workbuddy" => Some("desktop.credentials.workbuddy.v1"),
        "workbuddy-intl" => Some("desktop.credentials.workbuddy-intl.v1"),
        "raccoon" => Some("desktop.credentials.raccoon.v1"),
        "autoclaw" => Some("desktop.credentials.autoclaw.v1"),
        "autoclaw-intl" => Some("desktop.credentials.autoclaw-intl.v1"),
        "catpaw" => Some(HELPER_CATPAW),
        "doubaowork" => Some(HELPER_DOUBAO),
        "codar" => Some(HELPER_CODAR),
        "commandcode" => Some("desktop.credentials.commandcode.v1"),
        "dumate" => Some("desktop.credentials.dumate.v1"),
        "quwork" => Some("desktop.credentials.quwork.v1"),
        "orcaterm" => Some("desktop.credentials.orcaterm.v1"),
        "astudio" => Some("desktop.credentials.astudio.v1"),
        "kiro" => Some("desktop.credentials.kiro.v1"),
        "cline-free" | "cline-pass" => Some("desktop.credentials.cline.v1"),
        "kuku" => Some("desktop.credentials.kuku.v1"),
        "loomy" => Some("desktop.credentials.loomy.v1"),
        _ => None,
    }
}

pub(super) fn candidate(provider: &str, id: &str) -> Option<Value> {
    if provider == "raccoon" && id == HELPER_RACCOON_REFRESH {
        return Some(json!({"pack":HOST,"version":"raccoon-refresh-v1","provider":provider,"id":id,
            "operations":["status", "refresh"],"digest":"raccoon-refresh-v1"}));
    }
    if let Some(candidate) = super::qoder_umid::candidate(provider, id) { return Some(candidate); }
    if let Some(candidate) = super::client_device::candidate(provider, id) { return Some(candidate); }
    if let Some(candidate) = super::quwork_device::candidate(provider, id) { return Some(candidate); }
    if let Some(candidate) = refresh::candidate(provider, id) { return Some(candidate); }
    if helper(provider) != Some(id) {
        return None;
    }
    Some(
        json!({"pack":HOST,"version":VERSION,"provider":provider,"id":id,"operations":["read"],"digest":VERSION}),
    )
}

pub(super) fn authorize(context: &Context) -> Result<(), proto::WireError> {
    context.permission("desktop.credentials.v1")?;
    let provider = &context.shared.options.provider;
    if helper(provider).is_none_or(|id| {
        !context
            .shared
            .options
            .manifest
            .helpers
            .iter()
            .any(|value| value == id)
    }) {
        return Err(invalid());
    }
    if context.desktop_selected {
        return Ok(());
    }
    context.account()?;
    let store =
        crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let record = store.plugin_account(&context.account).ok_or_else(invalid)?;
    if record["public"]["enabled"] == false && !context.account_maintenance
        || record["provider"] != *provider
        || record["desktop"] != true
    {
        return Err(proto::WireError::with_code(
            "本机读取仅用于本次导入或已保存的本机账号",
            "forbidden",
        ));
    }
    Ok(())
}

pub(in crate::server::core::plugin_host::wasm) fn account_projection(record: &Value) -> Value {
    json!({"provider":record["provider"],"identity":record["identity"],"desktop":record["desktop"],
        "secret":super::legacy_secret::decode(&record["secret"]),"originalSecret":record["secret"]})
}

fn account_snapshot(context: &Context) -> Result<Option<Value>, proto::WireError> {
    authorize(context)?;
    if context.desktop_selected {
        return Ok(None);
    }
    let store =
        crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let record = store.plugin_account(&context.account).ok_or_else(invalid)?;
    Ok(Some(account_projection(&record)))
}

pub(super) async fn raccoon_refresh(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    raccoon::invoke(context, params).await
}

pub(super) async fn invoke(context: &Context, params: &Value) -> Result<Value, proto::WireError> {
    let before = account_snapshot(context)?;
    if params.as_object().is_none_or(|fields| !fields.is_empty()) {
        return Err(invalid());
    }
    let provider = context.shared.options.provider.clone();
    // Kiro 已保存的只读关联固定到宿主原账号来源，不能被后来出现的另一客户端挤掉。
    // 来源不收插件参数；显式导入仍按官方固定来源顺序读取。
    let kiro_source = if provider == "kiro" {
        before.as_ref().map(|record| {
            record["secret"]["source"].as_str().unwrap_or("").to_string()
        })
    } else {
        None
    };
    let result = crate::server::core::blocking::run(move || {
        if provider == "kiro" {
            kiro::read_source(kiro_source.as_deref().unwrap_or("kiro"))
        } else {
            read(&provider)
        }
    })
        .await
        .map_err(|_| invalid())?;
    if account_snapshot(context)? != before {
        return Err(proto::WireError::with_code(
            "本机账号在读取期间已变化，请重新发起操作",
            "forbidden",
        ));
    }
    result
}

pub(super) fn read(provider: &str) -> Result<Value, proto::WireError> {
    match provider {
        "workbuddy" | "workbuddy-intl" => snapshots::workbuddy(provider),
        "raccoon" => raccoon::read(),
        "autoclaw" | "autoclaw-intl" => autoclaw::read(provider),
        "catpaw" => catpaw(),
        "doubaowork" => doubao::read(),
        "codar" => codar::read(),
        "commandcode" => commandcode::read(),
        "dumate" => dumate::read(),
        "quwork" => snapshots::quwork(),
        "orcaterm" => snapshots::orcaterm(),
        "astudio" => astudio::read(),
        "kiro" => kiro::read(),
        "cline-free" | "cline-pass" => snapshots::cline(),
        "kuku" => kuku::read(),
        "loomy" => loomy::read(),
        _ => Err(invalid()),
    }
}

pub(super) fn supplied_tokens(params: &Value) -> Result<Value, proto::WireError> {
    let source = &params["source"];
    let mut value = match source["kind"].as_str() {
        Some("manual") => source["payload"]["inputs"]
            .as_object()
            .map(|fields| Value::Object(fields.clone()))
            .unwrap_or_else(|| source["payload"].clone()),
        Some("file_text") => {
            serde_json::from_str(source["content"].as_str().unwrap_or("")).map_err(|_| invalid())?
        }
        _ => return Err(invalid()),
    };
    if let Some(raw) = value["credentialJson"]
        .as_str()
        .filter(|raw| !raw.trim().is_empty())
    {
        value = serde_json::from_str(raw).map_err(|_| invalid())?;
    }
    if let Some(raw) = value.as_str() {
        value = serde_json::from_str(raw).map_err(|_| invalid())?;
    }
    if !value.is_object() {
        return Err(invalid());
    }
    let fields = value.as_object_mut().ok_or_else(invalid)?;
    if let Some(auth) = fields.get("auth").and_then(Value::as_object).cloned() {
        for (key, value) in auth {
            if fields
                .get(&key)
                .is_none_or(|value| value.is_null() || value == "")
            {
                fields.insert(key, value);
            }
        }
    }
    let token = |keys: &[&str]| {
        let raw = keys
            .iter()
            .filter_map(|key| value[*key].as_str())
            .map(str::trim)
            .find(|value| !value.is_empty())
            .unwrap_or("");
        // 与两版 AutoClaw 插件的 strip_bearer 同形；授权仍只限本次确切凭据对。
        let mut token = raw;
        loop {
            if !token.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer")) {
                return token.to_string();
            }
            let rest = &token[6..];
            if rest.is_empty() { return String::new(); }
            if !rest.starts_with(char::is_whitespace) { return token.to_string(); }
            token = rest.trim_start();
        }
    };
    Ok(
        json!({"token":token(&["token", "accessToken", "access_token"]), "refreshToken":token(&["refreshToken", "refresh_token"])}),
    )
}

pub(super) fn authorize_decrypt(
    context: &Context,
    payload: &Value,
) -> Result<(), proto::WireError> {
    context.permission("desktop.credentials.v1")?;
    if helper(&context.shared.options.provider).is_none_or(|id| {
        !context
            .shared
            .options
            .manifest
            .helpers
            .iter()
            .any(|value| value == id)
    }) {
        return Err(invalid());
    }
    let expected = context.supplied_credentials.as_ref().ok_or_else(|| {
        proto::WireError::with_code(
            "加密凭据只能来自本次用户明确粘贴或选择的 AutoClaw 登录文件",
            "forbidden",
        )
    })?;
    if payload != expected {
        return Err(proto::WireError::with_code(
            "不能解密本次导入之外的凭据",
            "forbidden",
        ));
    }
    Ok(())
}

pub(super) fn decrypt(provider: &str, payload: &Value) -> Result<Value, proto::WireError> {
    if !matches!(provider, "autoclaw" | "autoclaw-intl") {
        return Err(invalid());
    }
    autoclaw::decrypt(provider, payload)
}

pub(super) fn catpaw() -> Result<Value, proto::WireError> {
    let path = home()?.join(".meituan-catpaw/auth.json");
    let value = object(&path, 64 * 1024)?;
    let auth = value["auth"].as_object().ok_or_else(invalid)?;
    let account = value["account"].as_object().ok_or_else(invalid)?;
    if let Some(kind) = auth.get("loginType").filter(|value| !value.is_null()) {
        let kind = kind.as_str().ok_or_else(invalid)?.trim();
        if !kind.is_empty() && !kind.eq_ignore_ascii_case("passport") { return Err(invalid()); }
    }
    let token = catpaw_field(auth.get("accessToken"), 8192)?;
    let uid = catpaw_field(account.get("uid"), 256)?;
    let login = catpaw_field(account.get("loginName"), 256)?;
    let user = if uid.is_empty() { &login } else { &uid };
    if token.is_empty() || user.is_empty() || user == "desktop-auth"
        || user.len() > 512 || [&uid, &login].iter().any(|value| value.contains(&token)) {
        return Err(invalid());
    }
    Ok(json!({"accessToken": token, "uid": user, "loginName": login, "loginType": "passport"}))
}

fn catpaw_field(value: Option<&Value>, maximum: usize) -> Result<String, proto::WireError> {
    let raw = match value {
        None | Some(Value::Null) => return Ok(String::new()),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if number.is_i64() || number.is_u64() => number.to_string(),
        _ => return Err(invalid()),
    };
    let text = raw.trim();
    if text.chars().count() > maximum || text.contains(';') || text.chars().any(char::is_control)
        || reqwest::header::HeaderValue::from_str(text).is_err() {
        return Err(invalid());
    }
    Ok(text.to_string())
}

pub(in crate::server::core::plugin_host::wasm) fn home() -> Result<PathBuf, proto::WireError> {
    #[cfg(windows)]
    let name = "USERPROFILE";
    #[cfg(not(windows))]
    let name = "HOME";
    absolute_env(name).ok_or_else(invalid)
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn app_data() -> Result<PathBuf, proto::WireError> {
    #[cfg(windows)]
    return absolute_env("APPDATA")
        .or_else(|| home().ok().map(|path| path.join("AppData/Roaming")))
        .ok_or_else(invalid);
    #[cfg(target_os = "macos")]
    return Ok(home()?.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    return absolute_env("XDG_CONFIG_HOME")
        .or_else(|| home().ok().map(|path| path.join(".config")))
        .ok_or_else(invalid);
}

pub(super) fn regular(path: &Path) -> Result<std::fs::Metadata, proto::WireError> {
    let mut parent = Some(path);
    while let Some(current) = parent {
        let meta = std::fs::symlink_metadata(current).map_err(|_| invalid())?;
        if meta.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
        parent = current.parent();
        if parent.is_some_and(|parent| parent.as_os_str().is_empty()) {
            break;
        }
    }
    let meta = std::fs::metadata(path).map_err(|_| invalid())?;
    if !meta.is_file() {
        return Err(invalid());
    }
    Ok(meta)
}

pub(super) fn file(path: &Path, maximum: u64) -> Result<Vec<u8>, proto::WireError> {
    regular(path)?;
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    let meta = file.metadata().map_err(|_| invalid())?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > maximum {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > maximum {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(in crate::server::core::plugin_host::wasm) fn object(path: &Path, maximum: u64) -> Result<Value, proto::WireError> {
    let bytes = zeroize::Zeroizing::new(file(path, maximum)?);
    super::managed_task::declaration::parse_object(&bytes, maximum as usize).map_err(|_| invalid())
}

fn project(value: &Value, keys: &[&str]) -> Value {
    let mut result = json!({});
    for key in keys {
        if let Some(value) = value.get(*key) {
            result[*key] = value.clone();
        }
    }
    result
}

fn content(path: &Path, value: Value) -> Result<Value, proto::WireError> {
    let content = serde_json::to_string(&value).map_err(|_| invalid())?;
    if content.len() > 256 * 1024 {
        return Err(invalid());
    }
    Ok(
        json!({"sourceId":digest(path.to_string_lossy().as_bytes()),"sourceRevision":digest(content.as_bytes()),"content":content}),
    )
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn database(path: &Path) -> Result<rusqlite::Connection, proto::WireError> {
    if regular(path)?.len() > 64 * 1024 * 1024 {
        return Err(invalid());
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        if sidecar.exists() {
            regular(&sidecar)?;
        }
    }
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| invalid())?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| invalid())?;
    Ok(db)
}

pub(super) fn text<'a>(
    value: &'a Value,
    name: &str,
    maximum: usize,
) -> Result<&'a str, proto::WireError> {
    value[name]
        .as_str()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= maximum
                && !value.contains(';')
                && !value.chars().any(char::is_control)
        })
        .ok_or_else(invalid)
}

pub(super) fn invalid() -> proto::WireError {
    proto::WireError::with_code("本机客户端登录态不可读、未登录或格式不支持，请先在官方客户端登录，或改用网页登录和手工凭据", "desktop_credentials_unavailable")
}
