use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use serde::Deserialize;

pub const TARGET: &str = "wasm32-unknown-unknown";
pub const ABI: u32 = 1;
pub const CAPABILITIES: &[&str] = &[
    "http.v1", "websocket.v1", "accounts.cas.v1", "settings.v1", "sessions.v1",
    "clock.v1", "random.v1", "locks.v1", "catalog.v1", "chat.stream.v1",
    "login.forms.v1", "checkin.rolling.v1", "errors.v1", "accounts.identity.v1",
    "catalog.accounts.v1", "state.v1", "resources.v1", "images.v1", "tasks.v1",
    "login.interaction.v1", "login.callback.v1", "login.captcha.geetest.v1", "native.helpers.v1",
    "settings.schema.v1",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub abi: u32,
    pub requires: Vec<String>,
    pub origins: Vec<String>,
    #[serde(default)]
    pub helpers: Vec<String>,
    #[serde(default = "memory_default")]
    pub memory_mib: usize,
    #[serde(default = "concurrency_default")]
    pub concurrency: usize,
    #[serde(default)]
    pub verified: bool,
}

fn memory_default() -> usize { 64 }
fn concurrency_default() -> usize { 8 }

#[derive(Clone, Debug)]
pub struct Options {
    pub provider: String,
    pub manifest: Manifest,
}

pub fn is_wasm(manifest: &proto::PackManifest) -> bool {
    manifest.entry.contains_key(TARGET)
}

pub fn read(dir: &Path, manifest: &proto::PackManifest) -> Result<Option<Options>, String> {
    if !is_wasm(manifest) {
        if dir.join("wasm.json").exists() { return Err("非 Wasm 插件不能声明 Wasm 权限".into()); }
        return Ok(None);
    }
    if manifest.entry.len() != 1 || manifest.providers.len() != 1 {
        return Err("Wasm 插件包必须只声明一个提供商和一个跨平台入口".into());
    }
    if !manifest.ui.is_empty() || dir.join("ui").exists() {
        return Err("Wasm 插件只能使用主程序的声明式界面，不能携带可执行网页".into());
    }
    let text = std::fs::read(dir.join("wasm.json")).map_err(|_| "Wasm 插件缺少 wasm.json")?;
    if text.len() > 32 * 1024 { return Err("Wasm 权限清单过大".into()); }
    let spec: Manifest = serde_json::from_slice(&text).map_err(|_| "Wasm 权限清单格式无效")?;
    if spec.abi != ABI { return Err(format!("插件需要 Wasm 接口 {}，当前支持 {}", spec.abi, ABI)); }
    if spec.requires.len() > CAPABILITIES.len() { return Err("Wasm 能力声明过多".into()); }
    for capability in &spec.requires {
        if !CAPABILITIES.contains(&capability.as_str()) {
            return Err(format!("当前网关不支持插件所需能力：{capability}"));
        }
    }
    let requires = |name: &str| spec.requires.iter().any(|item| item == name);
    for (capability, dependencies) in [
        ("settings.schema.v1", &["settings.v1"][..]),
        ("catalog.accounts.v1", &["catalog.v1", "accounts.cas.v1"][..]),
        ("accounts.identity.v1", &["accounts.cas.v1"][..]),
        ("login.interaction.v1", &["login.forms.v1", "state.v1"][..]),
        ("login.captcha.geetest.v1", &["login.interaction.v1"][..]),
    ] {
        if requires(capability) && dependencies.iter().any(|dependency| !requires(dependency)) {
            return Err(format!("Wasm 能力 {capability} 缺少依赖声明"));
        }
    }
    if !(8..=128).contains(&spec.memory_mib) || !(1..=16).contains(&spec.concurrency) {
        return Err("Wasm 资源声明超出允许范围（内存 8–128 MiB，并发 1–16）".into());
    }
    if spec.helpers.len() > 8 || spec.helpers.iter().any(|helper| helper.is_empty() || helper.len() > 80
        || !helper.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
        || !spec.helpers.is_empty() && !spec.requires.iter().any(|capability| capability == "native.helpers.v1") {
        return Err("Wasm 原生辅助能力声明无效".into());
    }
    if spec.origins.len() > 32 { return Err("Wasm 网络地址声明过多".into()); }
    for origin in &spec.origins {
        let url = url::Url::parse(origin).map_err(|_| "Wasm 网络地址格式无效")?;
        if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
            || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
            || url.path() != "/" || url.origin().ascii_serialization() != *origin
        {
            return Err("Wasm 网络权限必须是明确的 HTTPS 来源，不允许通配符或路径".into());
        }
    }
    let entry = manifest.entry.get(TARGET).ok_or("Wasm 入口不存在")?;
    if !entry.ends_with(".wasm") { return Err("Wasm 入口必须是 .wasm 文件".into()); }
    checked_entry(dir, entry)?;
    Ok(Some(Options { provider: manifest.providers[0].id.0.clone(), manifest: spec }))
}

pub fn checked_entry(dir: &Path, entry: &str) -> Result<PathBuf, String> {
    let root = dir.canonicalize().map_err(|_| "插件目录不可读")?;
    let path = dir.join(entry).canonicalize().map_err(|_| "插件入口不存在")?;
    if !path.starts_with(root) || !path.is_file() { return Err("插件入口必须是包内的普通文件".into()); }
    Ok(path)
}

impl Options {
    pub fn allows(&self, capability: &str) -> bool {
        self.manifest.requires.iter().any(|item| item == capability)
    }

    pub fn check_url(&self, value: &str, websocket: bool) -> Result<(), proto::WireError> {
        let mut url = url::Url::parse(value).map_err(|_| super::error("插件请求地址无效"))?;
        if websocket && url.scheme() == "wss" {
            url.set_scheme("https").map_err(|_| super::error("插件长连接地址无效"))?;
        }
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
            || url.fragment().is_some() || !self.manifest.origins.contains(&url.origin().ascii_serialization())
        {
            return Err(proto::WireError::with_code("插件请求地址未获网络授权", "forbidden"));
        }
        Ok(())
    }
}
