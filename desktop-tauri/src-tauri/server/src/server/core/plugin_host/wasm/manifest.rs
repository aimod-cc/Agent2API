use std::io::Read;
use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const TARGET: &str = "wasm32-unknown-unknown";
pub const ABI: u32 = 1;
pub const CAPABILITIES: &[&str] = &[
    "http.v1", "http.timeout.v1", "websocket.v1", "accounts.cas.v1", "settings.v1", "sessions.v1",
    "clock.v1", "random.v1", "locks.v1", "catalog.v1", "chat.stream.v1",
    "login.forms.v1", "checkin.rolling.v1", "errors.v1", "accounts.identity.v1",
    "catalog.accounts.v1", "state.v1", "resources.v1", "images.v1", "tasks.v1",
    "login.interaction.v1", "login.callback.v1", "login.captcha.geetest.v1", "login.captcha.alicom4.v1", "native.helpers.v1",
    "settings.schema.v1", "login.sms.v1", "login.device.v1", "resources.fetch-public.v1", "protocol.convert.v1",
    "http.account-origin.v1", "runtime.platform.v1", "verification.account.v1", "verification.claim.v1",
    "runtime.architecture.v1", "runtime.client-fingerprint.v1", "request.context.v1",
    "login.managed.v1", "login.loopback.v1", "login.captcha.aliyun.v1",
    "resources.inbound-images.v1", "login.desktop.chatglm.v1", "credential.codearts-refresh.v1", "http.catpaw-turn.v1",
    "claim.doubao-tasks.v1", "http.managed-task.v1",
    "http.codearts-session.v1", "login.codearts.v1", "desktop.credentials.v1", "http.doubao-turn.v1",
    "crypto.monkeycode-cap.v1", "desktop.launch.v1", "resources.cookie-file.v1", "credential.kiro-external-refresh.v1", "credentials.legacy-import.v1",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub abi: u32,
    pub requires: Vec<String>,
    pub origins: Vec<String>,
    #[serde(default)]
    pub account_origin: Option<super::account_origin::Declaration>,
    #[serde(default)]
    pub desktop_launch: Option<super::desktop_launch::declaration::Declaration>,
    #[serde(default)]
    pub managed_login: Option<super::managed_policy::Declaration>,
    #[serde(default, deserialize_with = "super::managed_task::declaration::optional")]
    pub(crate) managed_task: Option<super::managed_task::declaration::Declaration>,
    #[serde(default)]
    pub helpers: Vec<String>,
    #[serde(default = "memory_default")]
    pub memory_mib: usize,
    #[serde(default = "concurrency_default")]
    pub concurrency: usize,
    #[serde(default)]
    pub concurrency_hard_limit: Option<usize>,
    #[serde(default)]
    pub verified: bool,
}

fn memory_default() -> usize { 64 }
fn concurrency_default() -> usize { 8 }

#[derive(Clone, Debug)]
pub struct Options {
    pub provider: String,
    pub manifest: Manifest,
    /// 实际解析的完整wasm.json字节摘要；不代表ZIP归属、安装见证或用户批准。
    source_digest: [u8; 32],
}

pub fn is_wasm(manifest: &proto::PackManifest) -> bool {
    manifest.entry.contains_key(TARGET)
}

pub fn read(dir: &Path, manifest: &proto::PackManifest) -> Result<Option<Options>, String> {
    if !is_wasm(manifest) {
        if dir.join("wasm.json").exists() { return Err("非 Wasm 插件不能声明 Wasm 权限".into()); }
        return Ok(None);
    }
    let file = std::fs::File::open(checked_entry(dir, "wasm.json")?).map_err(|_| "Wasm 插件缺少 wasm.json")?;
    if file.metadata().map_err(|_| "Wasm 权限清单不可读")?.len() > 32 * 1024 {
        return Err("Wasm 权限清单过大".into());
    }
    let mut text = Vec::new();
    file.take(32 * 1024 + 1).read_to_end(&mut text).map_err(|_| "Wasm 权限清单不可读")?;
    if text.len() > 32 * 1024 { return Err("Wasm 权限清单过大".into()); }
    read_bytes(dir, manifest, &text).map(Some)
}

pub(crate) fn read_bytes(dir: &Path, manifest: &proto::PackManifest, text: &[u8]) -> Result<Options, String> {
    if !is_wasm(manifest) || manifest.entry.len() != 1 || manifest.providers.len() != 1 {
        return Err("Wasm 插件包必须只声明一个提供商和一个跨平台入口".into());
    }
    if !manifest.ui.is_empty() || dir.join("ui").exists() {
        return Err("Wasm 插件只能使用主程序的声明式界面，不能携带可执行网页".into());
    }
    let mut spec: Manifest = super::managed_task::declaration::parse_object(text, 32 * 1024)
        .map_err(|_| "Wasm 权限清单格式无效")?;
    if spec.abi != ABI { return Err(format!("插件需要 Wasm 接口 {}，当前支持 {}", spec.abi, ABI)); }
    if spec.requires.len() > CAPABILITIES.len() { return Err("Wasm 能力声明过多".into()); }
    for capability in &spec.requires {
        if !CAPABILITIES.contains(&capability.as_str()) {
            return Err(format!("当前网关不支持插件所需能力：{capability}"));
        }
    }
    let requires = |name: &str| spec.requires.iter().any(|item| item == name);
    for (capability, dependencies) in [
        ("request.context.v1", &["chat.stream.v1"][..]),
        ("http.timeout.v1", &["http.v1"][..]),
        ("http.doubao-turn.v1", &["http.v1", "http.timeout.v1", "accounts.cas.v1", "chat.stream.v1", "request.context.v1"][..]),
        ("http.codearts-session.v1", &["http.v1", "http.timeout.v1", "accounts.cas.v1", "chat.stream.v1"][..]),
        ("login.codearts.v1", &["login.forms.v1", "sessions.v1"][..]),
        ("desktop.launch.v1", &["accounts.cas.v1", "accounts.identity.v1"][..]),
        ("desktop.credentials.v1", &["accounts.cas.v1", "login.forms.v1", "native.helpers.v1"][..]),
        ("http.managed-task.v1", &["http.v1", "accounts.cas.v1", "chat.stream.v1", "request.context.v1"][..]),
        ("crypto.monkeycode-cap.v1", &["http.v1", "http.timeout.v1", "accounts.cas.v1", "clock.v1"][..]),
        ("http.catpaw-turn.v1", &["http.v1", "http.timeout.v1", "accounts.cas.v1", "chat.stream.v1", "request.context.v1"][..]),
        ("http.account-origin.v1", &["http.v1", "accounts.cas.v1"][..]),
        ("settings.schema.v1", &["settings.v1"][..]),
        ("resources.fetch-public.v1", &["resources.v1"][..]),
        ("credential.kiro-external-refresh.v1", &["accounts.cas.v1", "accounts.identity.v1", "locks.v1"][..]),
        ("credentials.legacy-import.v1", &["accounts.cas.v1", "login.forms.v1"][..]),
        ("resources.cookie-file.v1", &["accounts.cas.v1", "login.forms.v1"][..]),
        ("resources.inbound-images.v1", &["resources.v1", "chat.stream.v1"][..]),
        ("login.desktop.chatglm.v1", &["login.forms.v1", "accounts.cas.v1"][..]),
        ("catalog.accounts.v1", &["catalog.v1", "accounts.cas.v1"][..]),
        ("accounts.identity.v1", &["accounts.cas.v1"][..]),
        ("credential.codearts-refresh.v1", &["accounts.cas.v1", "accounts.identity.v1"][..]),
        ("claim.doubao-tasks.v1", &["accounts.cas.v1", "accounts.identity.v1", "http.v1", "http.timeout.v1"][..]),
        ("verification.account.v1", &["accounts.cas.v1", "state.v1"][..]),
        ("verification.claim.v1", &["verification.account.v1"][..]),
        ("login.interaction.v1", &["login.forms.v1", "state.v1"][..]),
        ("login.sms.v1", &["login.forms.v1", "http.v1"][..]),
        ("login.device.v1", &["login.sms.v1"][..]),
        ("login.captcha.geetest.v1", &["login.interaction.v1"][..]),
        ("login.captcha.alicom4.v1", &["login.interaction.v1"][..]),
        ("login.managed.v1", &["login.forms.v1", "state.v1"][..]),
        ("login.loopback.v1", &["login.managed.v1"][..]),
        ("login.captcha.aliyun.v1", &["login.managed.v1"][..]),
    ] {
        if requires(capability) && dependencies.iter().any(|dependency| !requires(dependency)) {
            return Err(format!("Wasm 能力 {capability} 缺少依赖声明"));
        }
    }
    if (requires("http.codearts-session.v1") || requires("login.codearts.v1")) && manifest.providers[0].id.as_str() != "codearts" {
        return Err("CodeArts 会话和登录能力不能跨提供商使用".into());
    }
    if requires("crypto.monkeycode-cap.v1") && !matches!(manifest.providers[0].id.as_str(), "monkeycode" | "monkeycode-intl") {
        return Err("MonkeyCode 官方计算验证不能跨提供商使用".into());
    }
    if requires("desktop.credentials.v1") && super::desktop_credentials::helper(manifest.providers[0].id.as_str())
        .is_none_or(|id| !spec.helpers.iter().any(|helper| helper == id)) {
        return Err("固定本机来源尚未接通或缺少对应读取声明".into());
    }
    if spec.helpers.iter().any(|id| id == super::desktop_credentials::HELPER_RACCOON_REFRESH)
        && (manifest.providers[0].id.as_str() != "raccoon" || !requires("desktop.credentials.v1")) {
        return Err("小浣熊本机续期只能由对应插件通过固定本机凭据入口使用".into());
    }
    for provider in ["kiro", "autoclaw", "autoclaw-intl", "cline-free", "cline-pass"] {
        if super::desktop_credentials::refresh::helper(provider).is_some_and(|helper| spec.helpers.iter().any(|id| id == helper))
            && (manifest.providers[0].id.as_str() != provider || !requires("desktop.credentials.v1")) {
            return Err("本机续期辅助只能由对应提供商通过固定本机凭据入口使用".into());
        }
    }
    if spec.helpers.iter().any(|id| id == super::qoder_umid::HELPER)
        && manifest.providers[0].id.as_str() != "qoder-intl" {
        return Err("Qoder设备验证只能由国际版活动入口使用".into());
    }
    for provider in ["qwenwork", "qwenwork-intl", "qoder", "qoder-intl"] {
        if super::client_device::helper(provider).is_some_and(|helper| spec.helpers.iter().any(|id| id == helper))
            && manifest.providers[0].id.as_str() != provider {
            return Err("本机设备编号只能由对应提供商读取".into());
        }
    }
    if spec.helpers.iter().any(|id| id == super::quwork_device::HELPER)
        && manifest.providers[0].id.as_str() != "quwork" {
        return Err("QuWork设备资料只能由对应提供商读取".into());
    }
    if requires("credential.kiro-external-refresh.v1") && manifest.providers[0].id.as_str() != "kiro" {
        return Err("Kiro外部授权续期只能由对应插件使用".into());
    }
    if requires("credentials.legacy-import.v1") && !matches!(manifest.providers[0].id.as_str(),
        "raccoon" | "autoclaw" | "autoclaw-intl" | "catpaw" | "workbuddy" | "workbuddy-intl") {
        return Err("旧环境凭据导入仅允许已迁移的固定提供商".into());
    }
    if requires("resources.cookie-file.v1") && manifest.providers[0].id.as_str() != "kuku" {
        return Err("Cookie数据库文件解析只能由库库插件使用".into());
    }
    if requires("resources.inbound-images.v1") && manifest.providers[0].id.as_str() != "deepseek-web" {
        return Err("入站图片资源能力仅适用于已接通的 DeepSeek 网页提供商".into());
    }
    if requires("login.desktop.chatglm.v1") && manifest.providers[0].id.as_str() != "chatglm" {
        return Err("本机 ChatGLM 凭据导入能力不能跨提供商使用".into());
    }
    if requires("http.doubao-turn.v1") && manifest.providers[0].id.as_str() != "doubaowork" {
        return Err("豆包工作固定对话能力不能跨提供商使用".into());
    }
    if requires("http.catpaw-turn.v1") && manifest.providers[0].id.as_str() != "catpaw" {
        return Err("CatPaw固定轮次能力不能跨提供商使用".into());
    }
    if requires("credential.codearts-refresh.v1") && manifest.providers[0].id.as_str() != "codearts" {
        return Err("CodeArts固定换证能力不能跨提供商使用".into());
    }
    if requires("claim.doubao-tasks.v1") && manifest.providers[0].id.as_str() != "doubaowork" {
        return Err("豆包任务领奖能力不能跨提供商使用".into());
    }
    if requires("http.managed-task.v1") && (spec.managed_task.as_ref().is_none_or(|declaration| declaration.validate_provider(manifest.providers[0].id.as_str()).is_err())
        || !matches!(manifest.providers[0].id.as_str(), "monkeycode" | "monkeycode-intl")) {
        return Err("固定任务能力必须声明对应地区的 MonkeyCode 任务配置".into());
    }
    if let Some(declaration) = &spec.managed_task {
        declaration.validate_provider(manifest.providers[0].id.as_str())?;
    }
    if requires("desktop.launch.v1") != spec.desktop_launch.is_some() {
        return Err("本机启动必须同时声明能力与客户端范围".into());
    }
    if let Some(declaration) = &spec.desktop_launch { declaration.validate()?; }
    if requires("login.managed.v1") != spec.managed_login.is_some() {
        return Err("托管登录必须同时声明能力和受控登录方式".into());
    }
    if let Some(declaration) = &spec.managed_login {
        declaration.validate(manifest.providers[0].id.as_str())?;
        if !declaration.loopback_profiles.is_empty() && !requires("login.loopback.v1") {
            return Err("托管回调配置缺少 loopback 能力".into());
        }
    }
    if requires(super::account_origin::CAPABILITY) != spec.account_origin.is_some() {
        return Err("账号来源权限必须同时声明能力与受控来源规则".into());
    }
    if let Some(declaration) = &mut spec.account_origin {
        declaration.validate()?;
        *declaration = declaration.normalized();
    }
    if requires("http.managed-task.v1") && spec.managed_task.as_ref().is_some_and(|declaration| !declaration.has_profile()) {
        return Err("Wasm 受控任务能力必须声明一个对应地区的固定 profile".into());
    }
    if !(8..=128).contains(&spec.memory_mib) || !(1..=4096).contains(&spec.concurrency)
        || spec.concurrency_hard_limit.is_some_and(|limit| !(1..=4096).contains(&limit)) {
        return Err("Wasm 资源声明超出允许范围（内存 8–128 MiB，并发 1–4096）".into());
    }
    if spec.helpers.len() > 8 || spec.helpers.iter().any(|helper| helper.is_empty() || helper.len() > 80
        || !helper.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
        || !spec.helpers.is_empty() && !spec.requires.iter().any(|capability| capability == "native.helpers.v1") {
        return Err("Wasm 原生辅助能力声明无效".into());
    }
    let origin_limit = if manifest.providers[0].id.as_str() == "kiro" { super::aws_oidc::ORIGINS.len() + 8 } else { 32 };
    if spec.origins.len() > origin_limit { return Err("Wasm 网络地址声明过多".into()); }
    for origin in &spec.origins {
        let url = url::Url::parse(origin).map_err(|_| "Wasm 网络地址格式无效")?;
        if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
            || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
            || url.path() != "/" || url.origin().ascii_serialization() != *origin
        {
            return Err("Wasm 网络权限必须是明确的 HTTPS 来源，不允许通配符或路径".into());
        }
        if spec.account_origin.as_ref().is_some_and(|declaration| url.host_str().is_some_and(|host| declaration.contains_host(host))) {
            return Err("账号委托域名不能同时声明为静态网络来源，请使用受控账号来源入口".into());
        }
    }
    let entry = manifest.entry.get(TARGET).ok_or("Wasm 入口不存在")?;
    if !entry.ends_with(".wasm") { return Err("Wasm 入口必须是 .wasm 文件".into()); }
    checked_entry(dir, entry)?;
    Ok(Options { provider: manifest.providers[0].id.0.clone(), manifest: spec,
        source_digest: Sha256::digest(text).into() })
}

pub fn checked_entry(dir: &Path, entry: &str) -> Result<PathBuf, String> {
    let root = dir.canonicalize().map_err(|_| "插件目录不可读")?;
    let path = dir.join(entry).canonicalize().map_err(|_| "插件入口不存在")?;
    if !path.starts_with(root) || !path.is_file() { return Err("插件入口必须是包内的普通文件".into()); }
    Ok(path)
}

impl Options {
    pub(crate) fn source_digest(&self) -> &[u8; 32] { &self.source_digest }

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
