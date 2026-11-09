//! 账号的业务身份：添加、重新登录与备份导入共用；provider 作用域由调用方隔离。
//! 插件凭证内的字段优先于顶层旧镜像，缺少身份时只比较完整凭证，不拿昵称或尾号判重。

use serde_json::{Map, Value};

use crate::server::core::providers::{kind_from_id, ProviderKind};

const USER_KEYS: &[&str] = &["userId", "user_id", "userid", "uid"];
const ACCESS_KEYS: &[&str] = &["accessToken", "access_token", "token", "session", "oauth_access_token"];
const REFRESH_KEYS: &[&str] = &["refreshToken", "refresh_token"];
const API_KEYS: &[&str] = &["apiKey", "api_key", "accessToken", "access_token"];

fn field_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub(crate) fn text(fields: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    let pick = |source: &Map<String, Value>| keys.iter().find_map(|key| source.get(*key).and_then(field_text));
    match fields.get("pluginSecret").and_then(Value::as_object) {
        Some(secret) if keys.iter().any(|key| secret.contains_key(*key)) => pick(secret),
        _ => pick(fields),
    }
}

pub(crate) fn desktop(fields: &Map<String, Value>) -> bool {
    fields.get("desktop").and_then(Value::as_bool) == Some(true)
        || ["pluginSecret", "pluginPublic"].into_iter().any(|key| {
            fields.get(key).and_then(|value| value.get("desktop")).and_then(Value::as_bool) == Some(true)
        })
}

pub(crate) fn reference(provider: &str, fields: &Map<String, Value>) -> bool {
    if fields.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1) { return desktop(fields); }
    // KukuAI 和 OrcaTerm 的桌面端导入保存凭证快照，来源徽章不构成另一条身份。
    desktop(fields) && !matches!(kind_from_id(provider), Some(ProviderKind::Kuku | ProviderKind::OrcaTerm))
}

pub(crate) fn guest(provider: &str, fields: &Map<String, Value>) -> bool {
    if fields.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1) { return false; }
    if kind_from_id(provider) != Some(ProviderKind::Chatglm) { return false; }
    let flag = fields.get("pluginSecret").and_then(|value| value.get("isGuest")).and_then(Value::as_bool)
        .or_else(|| fields.get("isGuest").and_then(Value::as_bool))
        .or_else(|| fields.get("pluginPublic").and_then(|value| value.get("isGuest")).and_then(Value::as_bool));
    flag.unwrap_or_else(|| {
        let record = super::state::StoredAccount::from_map(fields.clone());
        super::chatglm_accounts::guest_of_record(&record)
    })
}

fn variant(fields: &Map<String, Value>) -> String {
    text(fields, &["variant"]).unwrap_or_else(|| "solo".to_string())
}

fn office_base(fields: &Map<String, Value>) -> String {
    let base = text(fields, &["baseUrl", "base_url", "api_base", "apiBase"]).unwrap_or_default();
    let base = base.trim_end_matches('/');
    if base.is_empty() || base.ends_with("/v2") { base.to_string() } else { format!("{base}/v2") }
}

pub(crate) fn stable(provider: &str, fields: &Map<String, Value>) -> Option<String> {
    if fields.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1) {
        return fields.get("pluginIdentity").and_then(field_text);
    }
    if guest(provider, fields) { return None; }
    match kind_from_id(provider) {
        Some(ProviderKind::WorkBuddy | ProviderKind::WorkBuddyIntl) => text(fields, &["uid"]),
        Some(ProviderKind::CatPaw) => text(fields, &["uid", "loginName", "login_name"]),
        Some(ProviderKind::Kuku) => text(fields, &["uid"]),
        Some(ProviderKind::CodeArts) => {
            let domain = text(fields, &["domain_id", "domainId"])?;
            let user = text(fields, &["user_id", "userId"])?;
            Some(format!("{domain}\u{0}{user}"))
        }
        Some(ProviderKind::Qoder | ProviderKind::QoderIntl) => {
            let region = crate::server::core::providers::qoder::endpoints::Region::from_provider_id(provider)?;
            Some(format!("{}:{}", region.id(), text(fields, USER_KEYS)?))
        }
        Some(ProviderKind::Trae) => Some(format!("{}:{}", variant(fields), text(fields, &["uid"])?)),
        Some(ProviderKind::ClineFree | ProviderKind::ClinePass) => {
            text(fields, &["account", "accountId", "account_id", "userId", "user_id", "uid"])
                .or_else(|| text(fields, &["pluginIdentity"]))
        }
        Some(ProviderKind::OfficeAce) => {
            let key = text(fields, &["modelAppKey", "appKey", "model_app_key"])?;
            let base = office_base(fields);
            Some(if base.is_empty() { key } else { format!("{base}\u{0}{key}") })
        }
        Some(ProviderKind::Antigravity) => text(fields, &["email", "userId", "user_id"]).map(|value| value.to_lowercase()),
        Some(ProviderKind::Raccoon | ProviderKind::AutoClaw | ProviderKind::AutoClawIntl
            | ProviderKind::Zcode | ProviderKind::ZcodeIntl | ProviderKind::MonkeyCode
            | ProviderKind::MonkeyCodeIntl | ProviderKind::Chatglm | ProviderKind::LobsterAi) => text(fields, USER_KEYS),
        Some(ProviderKind::OrcaTerm) => text(fields, &["uid", "userId", "user_id"]),
        Some(ProviderKind::Loomy) => text(fields, &["userId", "userid", "user_id", "uid", "phone"]),
        Some(ProviderKind::CommandCode) => text(fields, API_KEYS),
        Some(ProviderKind::Modelscope | ProviderKind::ModelscopeIntl) => {
            text(fields, &["userId", "user_id", "havanaId", "HavanaId", "uid"])
        }
        Some(ProviderKind::Atomcode) => text(fields, &["userId", "user_id", "uid", "username", "userName"]),
        Some(ProviderKind::Accio | ProviderKind::AccioCn) => text(fields, USER_KEYS),
        None => text(fields, &["userId", "user_id", "userid", "uid", "loginName", "login_name", "account", "email", "phone", "username"]),
    }
}

fn credential_keys(provider: &str) -> Vec<&'static [&'static str]> {
    match kind_from_id(provider) {
        Some(ProviderKind::CommandCode) => vec![API_KEYS],
        Some(ProviderKind::CodeArts) => vec![&["access_key_id", "accessKeyId", "accessToken"], REFRESH_KEYS],
        Some(ProviderKind::Antigravity) => vec![REFRESH_KEYS, ACCESS_KEYS],
        Some(ProviderKind::OfficeAce) => vec![&["modelAppKey", "appKey", "model_app_key"]],
        _ => vec![ACCESS_KEYS, REFRESH_KEYS, &["apiKey", "api_key"], &["jwt"]],
    }
}

pub(crate) fn credential(provider: &str, fields: &Map<String, Value>) -> Option<String> {
    credential_keys(provider).into_iter().find_map(|keys| text(fields, keys))
        .or_else(|| fields.get("pluginSecret").filter(|value| !value.is_null()).map(Value::to_string))
}

pub(crate) fn conflicts(provider: &str, left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    if [left, right].iter().any(|fields| fields.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1)) {
        return matches!((left.get("pluginIdentity"), right.get("pluginIdentity")), (Some(a), Some(b)) if a != b);
    }
    let differs = |keys: &[&str]| match (text(left, keys), text(right, keys)) {
        (Some(left), Some(right)) => left != right,
        _ => false,
    };
    let scope_conflict = match kind_from_id(provider) {
        Some(ProviderKind::CodeArts) => differs(&["domain_id", "domainId"]) || differs(&["user_id", "userId"]),
        Some(ProviderKind::Trae) => variant(left) != variant(right),
        Some(ProviderKind::OfficeAce) => office_base(left) != office_base(right),
        _ => false,
    };
    scope_conflict || matches!((stable(provider, left), stable(provider, right)), (Some(left), Some(right)) if left != right)
}

pub(crate) fn same(provider: &str, left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    if reference(provider, left) != reference(provider, right) || guest(provider, left) || guest(provider, right) || conflicts(provider, left, right) {
        return false;
    }
    if matches!((stable(provider, left), stable(provider, right)), (Some(left), Some(right)) if left == right) {
        return true;
    }
    credential_keys(provider).into_iter().any(|keys| {
        matches!((text(left, keys), text(right, keys)), (Some(left), Some(right)) if left == right)
    }) || matches!((left.get("pluginSecret"), right.get("pluginSecret")), (Some(left), Some(right)) if !left.is_null() && left == right)
}

pub(crate) fn for_import(provider: &str, fields: &Map<String, Value>) -> Result<String, String> {
    if guest(provider, fields) {
        return text(fields, &["id"]).ok_or_else(|| "访客账号缺少独立 id".to_string());
    }
    if let Some(identity) = stable(provider, fields) { return Ok(identity); }
    if kind_from_id(provider).is_some_and(|kind| matches!(kind, ProviderKind::WorkBuddy | ProviderKind::WorkBuddyIntl)) {
        return Err("缺少 uid（无法标识 WorkBuddy 账号）".to_string());
    }
    credential(provider, fields).or_else(|| text(fields, &["id"]))
        .ok_or_else(|| format!("缺少可用的身份字段（无法标识 {provider} 账号）"))
}
