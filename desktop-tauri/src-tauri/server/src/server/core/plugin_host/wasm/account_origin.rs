//! 账号来源授权只绑定已安装清单、包摘要和本次账号，不接受插件自行登记网络地址。

pub(super) mod network;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::context::Context;
use super::super::{self as host, install::{InstalledPack, PackRegistry}};
use crate::server::config;

pub(crate) const CAPABILITY: &str = "http.account-origin.v1";
const GRANTS_KEY: &str = "pluginAccountOriginGrants";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub secret_field: String,
    pub base_path: String,
    pub host_suffix: String,
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub method: String,
    pub path: String,
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !matches!(value, "__proto__" | "prototype" | "constructor")
}

pub(super) fn path(value: &str) -> bool {
    value.starts_with('/') && value.len() <= 512 && (value == "/" || !value.ends_with('/'))
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte))
        && (value == "/" || value[1..].split('/').all(|part| !matches!(part, "" | "." | "..")))
}

fn digest(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| "账号来源声明无法编码".to_string())
}

fn digest_text(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Declaration {
    pub(super) fn validate(&self) -> Result<(), String> {
        let suffix = self.host_suffix.strip_prefix('.').ok_or("账号来源必须声明明确的域名后缀")?;
        let labels: Vec<_> = suffix.split('.').collect();
        // 不用宽泛 TLD/组织域当委托。首版仅接受至少四级的明确 ASCII 服务域，仍须用户确认。
        if !identifier(&self.secret_field) || !path(&self.base_path) || suffix.len() > 252
            || labels.len() < 4 || labels.iter().any(|label| label.is_empty() || label.len() > 63
                || label.starts_with('-') || label.ends_with('-')
                || !label.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
            || labels.last().is_none_or(|label| label.len() < 2 || !label.bytes().all(|byte| byte.is_ascii_lowercase()))
            || matches!(labels.last().copied(), Some("local" | "localhost" | "internal" | "invalid" | "test" | "example" | "onion"))
            || suffix.ends_with(".home.arpa") || suffix.parse::<std::net::IpAddr>().is_ok()
            || self.routes.is_empty() || self.routes.len() > 16 {
            return Err("账号来源字段、基础路径或委托域名范围无效，不能使用通配符或宽泛域名".into());
        }
        let mut routes = HashSet::new();
        if self.routes.iter().any(|route| !matches!(route.method.as_str(), "GET" | "POST") || !path(&route.path)
            || !routes.insert((&route.method, &route.path))) {
            return Err("账号来源只允许不重复的 GET/POST 精确路径，不允许查询串、编码路径或点段".into());
        }
        Ok(())
    }

    pub(super) fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.routes.sort();
        normalized
    }

    pub(super) fn contains_host(&self, host: &str) -> bool {
        host.ends_with(&self.host_suffix) && host.len() > self.host_suffix.len()
    }

    fn source(&self, secret: &Value) -> Result<url::Url, proto::WireError> {
        let raw = secret.get(&self.secret_field).and_then(Value::as_str)
            .filter(|value| value.len() <= 2048 && !value.contains('\\') && !value.chars().any(|ch| ch.is_control() || ch.is_whitespace()))
            .ok_or_else(|| denied("当前账号没有规范的服务器来源，请重新登录或导入账号"))?;
        if !raw.starts_with("https://") || raw[8..].split(['/', '?', '#']).next().is_none_or(|part| part.is_empty() || part.contains('@')) {
            return Err(denied("账号服务器来源必须是规范 HTTPS 地址"));
        }
        // OfficeAce 存量凭据允许官方裸主机或尾斜线；仅恢复它原有固定 /v2 规范化。
        // Binding 仍保存原账号字段，规范化不允许任意路径，也不能改变域名、端口或调用来源。
        let compatible = if self.secret_field == "baseUrl" && self.base_path == "/v2"
            && self.host_suffix == ".officeace.cn-southwest-2.huaweicloud-agentarts.com" {
            let base = raw.trim_end_matches('/');
            Some(if base.ends_with("/v2") { base.to_string() } else { format!("{base}/v2") })
        } else { None };
        let normalized = compatible.as_deref().unwrap_or(raw);
        let url = url::Url::parse(normalized).map_err(|_| denied("账号服务器来源格式无效"))?;
        if url.scheme() != "https" || url.port_or_known_default() != Some(443) || !url.username().is_empty()
            || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
            || url.path() != self.base_path || !normalized.ends_with(&self.base_path)
            || normalized != url.as_str() || url.host_str().is_none_or(|host| !self.contains_host(host)) {
            return Err(denied("当前账号服务器来源不符合已授权的域名、端口或基础路径"));
        }
        Ok(url)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Grant {
    pack: String,
    provider: String,
    caller_digest: String,
    declaration_digest: String,
    declaration: Declaration,
}

type Grants = BTreeMap<String, BTreeMap<String, Grant>>;

pub(crate) fn grant_configuration(provider: &str) -> Value {
    let Some(pack) = host::provider_pack(provider) else { return Value::Null; };
    config::with_raw(|raw| raw.get(GRANTS_KEY).and_then(|grants| grants.get(&pack))
        .and_then(|items| items.get(provider)).cloned()).flatten().unwrap_or(Value::Null)
}

fn grants() -> Result<Grants, String> {
    let value = config::with_raw(|raw| raw.get(GRANTS_KEY).cloned()).flatten().unwrap_or_else(|| json!({}));
    serde_json::from_value(value).map_err(|_| "账号来源授权记录格式无效".to_string())
}

fn installed(pack: &InstalledPack, verify: bool) -> Option<super::manifest::Options> {
    if !identifier(&pack.id) || !digest_text(&pack.sha256) || pack.version.is_empty()
        || !pack.version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+')) {
        return None;
    }
    let dir = host::install::plugins_dir().join(&pack.id).join(&pack.version);
    let manifest = host::install::read_manifest(&dir).ok()?;
    if manifest.id != pack.id || manifest.version != pack.version
        || manifest.entry.get(super::manifest::TARGET) != Some(&pack.entry) { return None; }
    host::install::validate_manifest(&manifest, proto::PROTOCOL_VERSION).ok()?;
    let options = super::manifest::read(&dir, &manifest).ok()??;
    if !pack.manifest_providers.iter().any(|provider| provider.id.as_str() == options.provider) { return None; }
    if verify {
        let (signature, _) = host::install::verify_signature(&dir, &manifest, super::manifest::TARGET);
        if signature == host::install::SignatureState::Invalid || pack.signed && !signature.is_signed() { return None; }
    }
    Some(options)
}

fn available(pack: &InstalledPack, options: &super::manifest::Options, owners: &BTreeMap<String, String>) -> bool {
    if !pack.enabled || !pack.providers.get(&options.provider).copied().unwrap_or(true)
        || owners.get(&options.provider).map(String::as_str) != Some(pack.id.as_str())
        || !host::serves_provider(&options.provider) || host::registry::is_switching(&options.provider) { return false; }
    let Some(runtime) = host::host_for_pack(&pack.id).filter(|host| host.ready()) else { return false; };
    host::host_for_provider(&options.provider).is_some_and(|owner| Arc::ptr_eq(&owner, &runtime))
        && runtime.options.wasm.as_ref().is_some_and(|loaded| loaded.provider == options.provider
            && loaded.allows(CAPABILITY) && loaded.manifest.account_origin == options.manifest.account_origin)
}

fn matching(grant: &Grant, pack: &InstalledPack, provider: &str, declaration_digest: &str) -> bool {
    grant.pack == pack.id && grant.provider == provider && grant.caller_digest == pack.sha256
        && grant.declaration_digest == declaration_digest && digest(&grant.declaration).is_ok_and(|value| value == declaration_digest)
}

fn row(pack: &InstalledPack, provider: &str, declaration: &Declaration, declaration_digest: &str,
    available: bool, grant: Option<&Grant>) -> Value {
    json!({ "pack": pack.id, "provider": provider, "available": available,
        "authorized": available && grant.is_some_and(|grant| matching(grant, pack, provider, declaration_digest)),
        "grantPresent": grant.is_some(), "callerDigest": pack.sha256, "declarationDigest": declaration_digest,
        "secretField": declaration.secret_field, "basePath": declaration.base_path, "hostSuffix": declaration.host_suffix,
        "routes": declaration.routes, "proxySupported": true })
}

pub(crate) async fn snapshot(pack_id: &str) -> Result<Value, String> {
    let _mutation = host::operations::mutation_guard().await;
    snapshot_current(pack_id)
}

fn snapshot_current(pack_id: &str) -> Result<Value, String> {
    if !identifier(pack_id) { return Err("插件标识无效".into()); }
    let registry = PackRegistry::load();
    let pack = registry.packs.iter().find(|pack| pack.id == pack_id).ok_or("插件未安装")?;
    let owners = host::ownership::owners(&registry);
    let all = grants()?;
    let saved = all.get(pack_id);
    let mut rows = Vec::new();
    let mut current_provider = None;
    if let Some(options) = installed(pack, true) {
        if let Some(declaration) = &options.manifest.account_origin {
            let declaration_digest = digest(declaration)?;
            let grant = saved.and_then(|items| items.get(&options.provider));
            rows.push(row(pack, &options.provider, declaration, &declaration_digest, available(pack, &options, &owners), grant));
            current_provider = Some(options.provider);
        }
    }
    for grant in saved.into_iter().flat_map(|items| items.values()) {
        if Some(grant.provider.as_str()) == current_provider.as_deref() { continue; }
        if grant.pack == pack.id && identifier(&grant.provider) && grant.declaration.validate().is_ok() {
            // 声明已删除/文件不可用仍能看到并撤销旧授权；空摘要不能被用来重新授权。
            rows.push(row(pack, &grant.provider, &grant.declaration, "", false, Some(grant)));
        }
    }
    Ok(json!({ "accountOrigins": rows }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Change {
    pub pack: String,
    pub provider: String,
    pub caller_digest: String,
    pub declaration_digest: String,
    pub authorized: bool,
    #[serde(default)]
    pub confirmed: bool,
}

pub(crate) async fn configure(change: Change) -> Result<Value, String> {
    let _mutation = host::operations::mutation_guard().await;
    if !identifier(&change.pack) || !identifier(&change.provider)
        || change.caller_digest.len() > 128 || change.declaration_digest.len() > 128 { return Err("账号来源授权参数无效".into()); }
    let registry = PackRegistry::load();
    let pack = registry.packs.iter().find(|pack| pack.id == change.pack).ok_or("插件未安装")?;
    let mut all = grants()?;
    if change.authorized {
        if !change.confirmed { return Err("请先明确确认账号来源权限".into()); }
        let options = installed(pack, true).ok_or("插件清单无效或完整性校验未通过")?;
        let declaration = options.manifest.account_origin.as_ref().ok_or("插件未声明账号来源权限")?;
        let declaration_digest = digest(declaration)?;
        if options.provider != change.provider || !available(pack, &options, &host::ownership::owners(&registry)) {
            return Err("插件未启用、尚未就绪或不再拥有该提供商".into());
        }
        if change.caller_digest != pack.sha256 || change.declaration_digest != declaration_digest {
            return Err("插件包或账号来源声明已变化，请重新查看后确认".into());
        }
        if all.values().map(BTreeMap::len).sum::<usize>() >= 256
            && !all.get(&pack.id).is_some_and(|items| items.contains_key(&options.provider)) {
            return Err("账号来源授权数量已达上限".into());
        }
        let provider = options.provider.clone();
        all.entry(pack.id.clone()).or_default().insert(provider.clone(), Grant {
            pack: pack.id.clone(), provider, caller_digest: pack.sha256.clone(),
            declaration_digest, declaration: declaration.clone(),
        });
    } else if let Some(items) = all.get_mut(&change.pack) {
        if items.get(&change.provider).is_some_and(|grant| grant.pack != change.pack || grant.provider != change.provider) {
            return Err("不能撤销其他插件或提供商的授权".into());
        }
        items.remove(&change.provider);
        if items.is_empty() { all.remove(&change.pack); }
    }
    let saved = config::update_raw_field(GRANTS_KEY, serde_json::to_value(all).map_err(|_| "授权记录无法编码")?);
    let mut result = snapshot_current(&change.pack)?;
    result["saved"] = json!(saved);
    Ok(result)
}

pub(super) fn denied(message: &str) -> proto::WireError { proto::WireError::with_code(message, "forbidden") }

fn authorized(context: &Context, declaration: &Declaration) -> Result<(), proto::WireError> {
    let provider = &context.shared.options.provider;
    let pack_id = host::provider_pack(provider).ok_or_else(|| denied("账号来源提供商归属不可用"))?;
    let registry = PackRegistry::load();
    let pack = registry.packs.iter().find(|pack| pack.id == pack_id).ok_or_else(|| denied("账号来源插件未安装"))?;
    let options = installed(pack, false).ok_or_else(|| denied("账号来源插件清单已变化或不可用"))?;
    let current_host = host::host_for_provider(provider).ok_or_else(|| denied("账号来源执行体不可用"))?;
    if !current_host.embedded.runtime.try_read().is_ok_and(|runtime|
        runtime.as_ref().is_some_and(|runtime| Arc::ptr_eq(&runtime.shared, &context.shared))) {
        return Err(denied("账号来源执行体已经切换，不能沿用旧调用授权"));
    }
    if options.provider != *provider || options.manifest.account_origin.as_ref() != Some(declaration)
        || !available(pack, &options, &host::ownership::owners(&registry)) {
        return Err(denied("账号来源插件已停用、切换或声明已变化"));
    }
    digest(declaration).map_err(|_| denied("账号来源声明无效"))?;
    Ok(())
}

pub(super) struct Binding {
    account: String,
    identity: String,
    source: Option<Value>,
    credential_digest: String,
}

impl Context {
    pub(super) fn observe_origin_account(&mut self, record: &Value) -> Result<(), proto::WireError> {
        let Some(declaration) = self.shared.options.manifest.account_origin.as_ref() else { return Ok(()) };
        let source = record["secret"].get(&declaration.secret_field).cloned();
        if self.origin_binding.as_ref().is_some_and(|old| old.account != self.account
            || old.identity != self.identity || old.source != source) {
            return Err(denied("账号身份或服务器来源已变化，请重新发起操作"));
        }
        self.origin_binding = Some(Binding { account: self.account.clone(), identity: self.identity.clone(), source,
            credential_digest: digest(&super::legacy_secret::original(self, &self.account, &record["secret"]))
                .map_err(|_| denied("账号凭据快照不可用"))? });
        Ok(())
    }

    pub(super) fn protect_origin_update(&self, expect: &Value, secret: &Value) -> Result<(), proto::WireError> {
        let Some(declaration) = self.shared.options.manifest.account_origin.as_ref() else { return Ok(()) };
        let current = self.origin_binding.as_ref().ok_or_else(|| denied("账号来源缺少可信凭据快照"))?;
        if expect.get(&declaration.secret_field) != current.source.as_ref()
            || secret.get(&declaration.secret_field) != current.source.as_ref() {
            return Err(denied("插件不能通过凭据刷新修改或删除服务器来源，请重新登录或导入账号"));
        }
        Ok(())
    }

    pub(super) fn origin_updated(&mut self, secret: &Value) -> Result<(), proto::WireError> {
        if let Some(binding) = &mut self.origin_binding {
            binding.credential_digest = digest(secret).map_err(|_| denied("账号凭据快照不可用"))?;
        }
        Ok(())
    }
}
