//! 仅关联用户主动选择的官方本机登录态；不读取 .credentials.json 或系统钥匙串。
//! Magpie claude_signin 的 auth status + logins 的 oauthAccount 身份判据。
//! 删除账号只删除网关记录；原 CLI 目录、登录态和自动续期仍归官方程序所有。
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use agent2api_provider_protocol::{LoginPoll, LoginStart, NormalizeInput, NormalizeSource, NormalizedAccount};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{errors, process, ID};
use crate::server::core::{
    account_store::{self, AccountStore},
    proxies::{ProxyResolution, ResolvedProxy},
};
use crate::server::errors::GatewayError;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Link {
    pub kind: String,
    pub executable: PathBuf,
    pub config_dir: Option<PathBuf>,
    pub profile_path: PathBuf,
    pub account_key: String,
    pub email: String,
    pub organization: String,
    pub plan: String,
    pub cli_version: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Profile {
    #[serde(default, deserialize_with = "optional_default")]
    pub oauth_account: ProfileAccount,
    #[serde(default, deserialize_with = "optional_default")]
    pub additional_model_options_cache: Vec<ModelOption>,
    #[serde(default)]
    pub additional_model_options_answered_at: Option<i64>,
    #[serde(default, deserialize_with = "optional_default")]
    pub model_access_cache: Vec<ModelAccess>,
}

fn optional_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProfileAccount {
    #[serde(default)]
    pub account_uuid: String,
    #[serde(default)]
    pub organization_uuid: String,
    #[serde(default)]
    pub email_address: String,
    #[serde(default)]
    pub organization_name: String,
}

#[derive(Deserialize)]
pub(super) struct ModelOption {
    pub value: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelAccess {
    pub api_name: String,
    pub entitled: Option<bool>,
}

impl ProfileAccount {
    fn key(&self) -> String {
        let who = if self.account_uuid.is_empty() {
            self.email_address.trim().to_ascii_lowercase()
        } else {
            self.account_uuid.clone()
        };
        if who.is_empty() {
            return String::new();
        }
        process::digest(format!("{who}\0{}", self.organization_uuid).as_bytes())
    }
}

impl Link {
    pub fn from_secret(secret: &Value) -> Result<Self, GatewayError> {
        let link: Self = serde_json::from_value(secret.clone())
            .map_err(|_| errors::local("claude_link_invalid", "Claude 账号不是有效的本机引用，请重新关联官方登录态"))?;
        if link.kind != "claude-code-link-v1"
            || link.account_key.is_empty()
            || !link.executable.is_absolute()
            || !link.profile_path.is_absolute()
            || link.config_dir.as_ref().is_some_and(|p| !p.is_absolute())
        {
            return Err(errors::local("claude_link_invalid", "Claude 本机引用不完整，请重新关联"));
        }
        // 路径必须符合官方位置关系，导入记录不能令网关读取任意文件。
        let expected = match &link.config_dir {
            Some(dir) => dir.join(".claude.json"),
            None => process::home()?.join(".claude.json"),
        };
        if expected != link.profile_path {
            return Err(errors::local("claude_link_invalid", "Claude 本机引用不属于当前机器的官方配置位置"));
        }
        Ok(link)
    }

    pub fn profile(&self) -> Result<Profile, GatewayError> {
        read_profile(&self.profile_path)
    }

    pub fn check_profile(&self) -> Result<(), GatewayError> {
        let profile = self.profile()?;
        if profile.oauth_account.key() != self.account_key {
            return Err(errors::local(
                "claude_account_changed",
                "官方 Claude Code 已退出或切换账号；为防止借用其他登录态，请重新关联",
            ));
        }
        if !self.executable.is_file() {
            return Err(errors::local("claude_cli_missing", "已关联的 Claude Code 程序不存在，请重新选择官方原生程序"));
        }
        Ok(())
    }

    pub fn public(&self) -> Value {
        json!({ "name": if self.email.is_empty() { "Claude Code" } else { &self.email },
            "uid": self.account_key, "email": self.email, "enterpriseName": self.organization,
            "plan": self.plan, "cliVersion": self.cli_version, "credentialSource": "official-cli-reference", "desktop": true,
            "hasAccessToken": false, "hasRefreshToken": false, "canRefresh": false,
            "tokenTail": Value::Null, "cliManagedRefresh": true })
    }
}

fn read_profile(path: &Path) -> Result<Profile, GatewayError> {
    let file = std::fs::File::open(path).map_err(|_| {
        errors::local("claude_profile_missing", "所选 Claude 配置目录没有可读取的官方账号资料，请先在官方 CLI 登录")
    })?;
    if !file.metadata().is_ok_and(|m| m.is_file() && m.len() <= 8 * 1024 * 1024) {
        return Err(errors::local("claude_profile_invalid", "Claude 账号资料文件无效或过大"));
    }
    // Deserialize 只保留明确需要的公开字段；API keys、OAuth token 等未知字段不入内存结构。
    serde_json::from_reader(file.take(8 * 1024 * 1024 + 1))
        .map_err(|_| errors::local("claude_profile_invalid", "Claude 账号资料格式不正确，未回显文件内容"))
}

pub(super) fn methods() -> Value {
    json!({ "instructions": "只关联网关所在机器上已登录的官方 Claude Code；请先自行运行 claude auth login。网关不接收订阅令牌，不替代官方登录。此路线使用 -p，额度计费可能不同于交互终端。",
        "methods": [{ "methodIndex": 0, "type": "oauth", "label": "关联官方 Claude Code 登录态",
            "localOnly": true, "browserMode": "none", "prompts": [
                { "key": "accountSource", "type": "select", "label": "官方登录态位置", "default": "current",
                    "options": [{"value":"current","label":"当前系统用户的官方登录态"}, {"value":"directory","label":"我指定的官方配置目录"}] },
                { "key": "configDir", "type": "text", "label": "已登录的 CLAUDE_CONFIG_DIR 绝对路径", "optional": false,
                    "when": {"key":"accountSource","op":"eq","value":"directory"} },
                { "key": "cliPath", "type": "text", "label": "官方原生 CLI 路径（可选，默认从安装位置查找）", "optional": true },
                { "key": "consent", "type": "select", "label": "授权范围", "options": [
                    {"value":"own-subscription","label":"允许网关使用我选择的官方登录态；客户端工具仅回传执行"}] }
            ] }] })
}

fn options_link(options: &Value) -> Result<Link, GatewayError> {
    if options.get("methodIndex").and_then(Value::as_u64) != Some(0) {
        return Err(GatewayError::bad_request("请选择已声明的 Claude 登录方式"));
    }
    let inputs = options
        .get("inputs")
        .and_then(Value::as_object)
        .ok_or_else(|| GatewayError::bad_request("Claude 登录缺少 inputs"))?;
    if inputs.keys().any(|key| !["accountSource", "configDir", "cliPath", "consent"].contains(&key.as_str())) {
        return Err(GatewayError::bad_request("Claude 登录包含未声明的字段；不能提交令牌或脚本参数"));
    }
    let text = |key: &str| inputs.get(key).and_then(Value::as_str).unwrap_or("").trim();
    if text("consent") != "own-subscription" {
        return Err(GatewayError::bad_request("请明确授权关联自己的官方 Claude 登录态"));
    }
    let config_dir = match text("accountSource") {
        "current" => None,
        "directory" => {
            let dir = PathBuf::from(text("configDir"));
            if !dir.is_absolute() || !dir.is_dir() {
                return Err(GatewayError::bad_request("Claude 配置目录必须是已存在的绝对路径"));
            }
            Some(std::fs::canonicalize(dir).map_err(|_| GatewayError::bad_request("无法读取所选 Claude 配置目录"))?)
        }
        _ => return Err(GatewayError::bad_request("Claude 登录态位置无效")),
    };
    let profile_path = match &config_dir {
        Some(dir) => dir.join(".claude.json"),
        None => process::home()?.join(".claude.json"),
    };
    let profile = read_profile(&profile_path)?;
    let account_key = profile.oauth_account.key();
    if account_key.is_empty() {
        return Err(errors::local("claude_auth_required", "所选目录没有官方 Claude 订阅账号，请先在官方 CLI 完成登录"));
    }
    Ok(Link {
        kind: "claude-code-link-v1".into(),
        executable: process::executable(text("cliPath"))?,
        config_dir,
        profile_path,
        account_key,
        email: profile.oauth_account.email_address,
        organization: profile.oauth_account.organization_name,
        plan: String::new(),
        cli_version: String::new(),
    })
}

pub(super) async fn verify(link: &Link, proxy: Option<&ResolvedProxy>) -> Result<String, GatewayError> {
    link.check_profile()?;
    process::verify_version(link, proxy).await?;
    verify_status(link, proxy).await
}

async fn verify_status(link: &Link, proxy: Option<&ResolvedProxy>) -> Result<String, GatewayError> {
    let output = process::capture(link, &["auth".into(), "status".into(), "--json".into()], proxy).await?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Status {
        logged_in: bool,
        #[serde(default)]
        email: String,
        #[serde(default)]
        subscription_type: String,
        #[serde(default)]
        auth_method: String,
        #[serde(default)]
        api_provider: String,
    }
    let status: Status =
        serde_json::from_slice(&output).map_err(|_| errors::protocol("Claude Code 没有返回可识别的登录状态"))?;
    let method = status.auth_method.to_ascii_lowercase();
    if !status.logged_in
        || status.subscription_type.is_empty()
        || !matches!(method.as_str(), "claude.ai" | "oauth_token" | "oauth" | "claudeai")
        || !matches!(status.api_provider.as_str(), "" | "firstParty")
    {
        return Err(GatewayError::with_status(
            401,
            "所选 CLI 未使用官方 Claude 订阅登录；API Key 或云厂商凭证不能作为订阅账号添加",
        )
        .with_code("claude_auth_required"));
    }
    if !status.email.is_empty() && !status.email.eq_ignore_ascii_case(&link.email) {
        return Err(errors::local("claude_account_changed", "Claude CLI 报告的账号与所选资料不一致，未关联"));
    }
    link.check_profile()?;
    Ok(status.subscription_type)
}

pub(super) async fn normalize(input: NormalizeInput) -> Result<NormalizedAccount, GatewayError> {
    if input.provider != ID {
        return Err(GatewayError::bad_request("Claude 账号提供商不匹配"));
    }
    let options = match input.source {
        NormalizeSource::Manual { payload } => payload,
        _ => {
            return Err(GatewayError::bad_request(
                "请通过 Claude 动态登录表单明确选择本机账号；不接受令牌文件或隐式导入",
            ))
        }
    };
    let proxy = input.proxy.map(|p| ResolvedProxy {
        source: "custom".into(),
        protocol: p.protocol,
        host: p.host,
        port: p.port,
        username: p.username,
        password: p.password,
        label: String::new(),
        non_direct: false,
    });
    let mut link = options_link(&options)?;
    let version = process::verify_version(&link, proxy.as_ref()).await?;
    link.plan = verify_status(&link, proxy.as_ref()).await?;
    link.cli_version = version;
    let mut public = link.public();
    if let Some(name) = input.name.filter(|name| !name.trim().is_empty()) {
        public["name"] = json!(name);
    }
    Ok(NormalizedAccount {
        identity: link.account_key.clone(),
        secret: serde_json::to_value(&link).map_err(|_| errors::protocol("Claude 本机引用无法序列化"))?,
        public,
        desktop: true,
    })
}

struct Pending {
    account: NormalizedAccount,
    expires: Instant,
}
fn flows() -> &'static Mutex<HashMap<String, Pending>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    FLOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn start(options: Value) -> Result<LoginStart, GatewayError> {
    let wire = options
        .get("proxy")
        .filter(|v| !v.is_null())
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| GatewayError::bad_request("Claude 登录代理形态无效，未回退直连"))?;
    let account = normalize(NormalizeInput {
        provider: ID.into(),
        source: NormalizeSource::Manual { payload: options },
        name: None,
        proxy: wire,
    })
    .await?;
    let flow = process::token()?;
    let mut slots = flows().lock().map_err(|_| errors::local("claude_login_unavailable", "Claude 登录流程暂不可用"))?;
    slots.retain(|_, pending| pending.expires > Instant::now());
    if slots.len() >= 16 {
        return Err(errors::local("claude_login_busy", "待保存的 Claude 账号过多，请稍后重试"));
    }
    slots.insert(flow.clone(), Pending { account, expires: Instant::now() + Duration::from_secs(120) });
    Ok(LoginStart {
        flow,
        url: String::new(),
        instructions: "官方 Claude 登录态已核对，正在保存本机引用；没有复制令牌".into(),
        poll_interval_ms: 500,
        external_browser: false,
        allowed_hosts: Vec::new(),
    })
}

pub(super) fn poll(flow: &str) -> Result<LoginPoll, GatewayError> {
    let pending =
        flows().lock().map_err(|_| errors::local("claude_login_unavailable", "Claude 登录流程暂不可用"))?.remove(flow);
    match pending {
        Some(pending) if pending.expires > Instant::now() => Ok(LoginPoll::Done { account: pending.account }),
        _ => Ok(LoginPoll::Failed { message: "Claude 关联流程已过期或取消，请重新关联".into() }),
    }
}
pub(super) fn cancel(flow: &str) {
    if let Ok(mut pending) = flows().lock() {
        pending.remove(flow);
    }
}

pub(super) fn persist(account: &NormalizedAccount, proxy: Option<&Value>) -> Result<String, GatewayError> {
    let link = Link::from_secret(&account.secret)?;
    link.check_profile()?;
    if account.identity != link.account_key {
        return Err(errors::local("claude_link_invalid", "Claude 账号身份与本机引用不一致"));
    }
    let store = account_store::plugin_accounts::shared()
        .ok_or_else(|| errors::local("claude_store_unavailable", "账号库未初始化"))?;
    let mut public = link.public();
    if let Some(name) =
        account.public.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty() && name.len() <= 512)
    {
        public["name"] = json!(name);
    }
    let (saved, _) = store
        .plugin_upsert_account_with_source_and_proxy(
            &account.identity,
            ID,
            public.get("name").and_then(Value::as_str).unwrap_or("Claude Code"),
            &account.secret,
            &public,
            true,
            "builtin",
            proxy,
        )
        .map_err(|_| errors::local("claude_store_failed", "Claude 账号与代理保存失败，未报告关联成功"))?;
    saved
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| errors::local("claude_store_failed", "已保存的 Claude 账号缺少身份标识"))
}

pub(super) fn load(store: &AccountStore, account: &str) -> Result<Link, GatewayError> {
    if account.is_empty() {
        return Err(errors::local("claude_account_required", "Claude 订阅必须显式关联并选择本机账号"));
    }
    let value = store
        .plugin_account(account)
        .filter(|a| a.get("provider").and_then(Value::as_str) == Some(ID))
        .ok_or_else(|| errors::local("claude_account_missing", "所选 Claude 账号不存在"))?;
    Link::from_secret(value.get("secret").unwrap_or(&Value::Null))
}

pub(super) fn public_from_secret(secret: &Value) -> Result<Value, GatewayError> {
    Ok(Link::from_secret(secret)?.public())
}

pub(super) fn account_proxy(store: &AccountStore, account: &str) -> Result<Option<ResolvedProxy>, GatewayError> {
    let row = store
        .plugin_account(account)
        .ok_or_else(|| errors::local("claude_account_missing", "所选 Claude 账号不存在"))?;
    let proxy = row.get("proxy").unwrap_or(&Value::Null);
    match proxy.get("state").and_then(Value::as_str).unwrap_or("unset") {
        "resolved" => ResolvedProxy::from_json(proxy.get("proxy").unwrap_or(&Value::Null))
            .map_err(|_| errors::local("claude_proxy_invalid", "Claude 账号代理出口无效，未回退直连")),
        "unset" => Ok(None),
        "static" => match crate::server::core::proxies::resolve_account_proxy(proxy.get("config")) {
            Some(ProxyResolution::Resolved(proxy)) => Ok(Some(proxy)),
            Some(ProxyResolution::Failed(_)) => {
                Err(errors::local("claude_proxy_invalid", "Claude 账号代理不可用，未回退直连"))
            }
            None => Ok(None),
        },
        _ => Err(errors::local("claude_proxy_invalid", "Claude 账号代理不可用，未回退直连")),
    }
}
