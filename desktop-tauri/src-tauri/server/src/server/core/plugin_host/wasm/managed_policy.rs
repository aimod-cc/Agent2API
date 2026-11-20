//! 托管登录的静态权限；浏览器来源与 API 出站来源独立审批。
//! Google code/state、CatPaw Passport 与 Kiro 双阶段回调各用固定配置，不能互借来源或输入方式。
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub methods: Vec<u64>,
    pub loopback_profiles: Vec<String>,
    pub authorization_origins: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallbackKind { GoogleCode, AutoclawCode, CatpawPassport, KiroPortal, KiroAws, TraeCode }

pub(crate) const TRAE_PROFILE: &str = "trae.pkce.v1";
pub(crate) const TRAE_AUTHORIZATION_ORIGIN: &str = "https://www.trae.cn";

pub(crate) const KIRO_PORTAL_PROFILE: &str = "kiro.portal.v1";
pub(crate) const KIRO_AWS_PROFILE: &str = "kiro.aws.v1";
pub(crate) const KIRO_PORTAL_ORIGIN: &str = "https://app.kiro.dev";
pub(crate) const KIRO_AWS_ORIGINS: &[&str] = super::aws_oidc::ORIGINS;
pub(crate) const KIRO_PORTS: &[u16] = &[
    3128, 4649, 6588, 8008, 9091, 49153, 50153, 51153, 52153, 53153,
];

pub(crate) const CATPAW_PROFILE: &str = "catpaw.passport.v1";
pub(crate) const CATPAW_ORIGIN: &str = "https://catpaw.meituan.com";
pub(crate) const CATPAW_LOGIN_ENTRY: &str = "https://catpaw.meituan.com/api/gateway/passport/login-entry";

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub id: &'static str,
    pub host: &'static str,
    pub path: &'static str,
    pub ports: &'static [u16],
    pub origins: &'static [&'static str],
    pub callback: CallbackKind,
}

fn approved(provider: &str, method: u64, id: &str) -> Option<Profile> {
    match (provider, method, id) {
        // 端口/路径：本仓原生 autoclaw/callback_server.rs 与公开 opencode-autoclaw-auth。
        // 来源：https://developers.google.com/identity/protocols/oauth2/native-app。
        ("autoclaw-intl", 0, "autoclaw-intl.zai.v1") => Some(Profile {
            id: "autoclaw-intl.zai.v1", host: "localhost", path: "/auth/callback-zai",
            ports: &[18432, 19654, 19723, 53699], origins: &[],
            callback: CallbackKind::AutoclawCode,
        }),
        ("autoclaw-intl", 1, "autoclaw-intl.google.v1") => Some(Profile {
            id: "autoclaw-intl.google.v1", host: "localhost", path: "/auth/callback-google",
            ports: &[18432, 19654, 19723, 53699], origins: &["https://accounts.google.com"],
            callback: CallbackKind::GoogleCode,
        }),
        // 本仓 CatPaw 原生 login.rs/callback_server.rs 的客户端实证；只批准 Passport。
        // 0 仅表示绑定时向操作系统申请端口，返回 URL 必须使用实际 local_addr().port()。
        ("catpaw", 0, CATPAW_PROFILE) => Some(Profile {
            id: CATPAW_PROFILE, host: "127.0.0.1", path: "/catpaw/callback", ports: &[0],
            origins: &[CATPAW_ORIGIN], callback: CallbackKind::CatpawPassport,
        }),
        // 原生 Kiro login.rs/endpoints.rs 与公开 magpie-community/plugins 的两阶段 PKCE。
        // 门户 redirect 为根地址，专用接收器另认 /signin/callback 与 /oauth/callback。
        ("kiro", 0, KIRO_PORTAL_PROFILE) => Some(Profile {
            id: KIRO_PORTAL_PROFILE, host: "localhost", path: "/signin/callback", ports: KIRO_PORTS,
            origins: &[KIRO_PORTAL_ORIGIN], callback: CallbackKind::KiroPortal,
        }),
        ("kiro", 0, KIRO_AWS_PROFILE) => Some(Profile {
            id: KIRO_AWS_PROFILE, host: "127.0.0.1", path: "/oauth/callback", ports: KIRO_PORTS,
            origins: KIRO_AWS_ORIGINS, callback: CallbackKind::KiroAws,
        }),
        // 本仓原生Trae oauth/device/callback_server：无state、IPv4随机端口、固定/authorize。
        // 这里只批准本次准备和回调归属；兑换/身份/保存仍由后续独立门关闭。
        ("trae", 0, TRAE_PROFILE) => Some(Profile {
            id: TRAE_PROFILE, host: "127.0.0.1", path: "/authorize", ports: &[0],
            origins: &[TRAE_AUTHORIZATION_ORIGIN], callback: CallbackKind::TraeCode,
        }),
        _ => None,
    }
}

impl Declaration {
    pub(crate) fn validate(&self, provider: &str) -> Result<(), String> {
        if self.methods.is_empty() || self.methods.len() > 16
            || self.methods.iter().any(|index| *index > 9_007_199_254_740_991)
            || self.methods.iter().collect::<std::collections::HashSet<_>>().len() != self.methods.len()
            || self.loopback_profiles.len() > 8
            || self.authorization_origins.len() > if provider == "kiro" { KIRO_AWS_ORIGINS.len() + 1 } else { 8 }
        { return Err("托管登录权限声明无效".into()); }
        let mut approved_origins = std::collections::HashSet::new();
        let mut profiles = std::collections::HashSet::new();
        for id in &self.loopback_profiles {
            if !profiles.insert(id) { return Err("托管登录回调配置重复".into()); }
            let profile = self.methods.iter().find_map(|index| approved(provider, *index, id))
                .ok_or_else(|| "托管登录回调配置尚未获主程序批准".to_string())?;
            approved_origins.extend(profile.origins.iter().copied());
        }
        let mut origins = std::collections::HashSet::new();
        for origin in &self.authorization_origins {
            let parsed = url::Url::parse(origin).map_err(|_| "托管授权页来源格式无效")?;
            if parsed.scheme() != "https" || parsed.port_or_known_default() != Some(443)
                || parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some()
                || !parsed.username().is_empty() || parsed.password().is_some()
                || parsed.origin().ascii_serialization() != *origin
                || !approved_origins.contains(origin.as_str()) || !origins.insert(origin)
            { return Err("托管授权页必须使用主程序批准的精确 HTTPS 来源".into()); }
        }
        Ok(())
    }

    pub(crate) fn profile(&self, provider: &str, method: u64, id: &str) -> Option<Profile> {
        if !self.methods.contains(&method) || !self.loopback_profiles.iter().any(|item| item == id) { return None; }
        let profile = approved(provider, method, id)?;
        let required = if profile.callback == CallbackKind::KiroAws { &KIRO_AWS_ORIGINS[..2] } else { profile.origins };
        required.iter().all(|origin| self.authorization_origins.iter().any(|item| item.as_str() == *origin))
            .then_some(profile)
    }

    pub(crate) fn origins(&self, provider: &str, method: u64) -> Vec<String> {
        let mut origins: Vec<String> = Vec::new();
        for id in &self.loopback_profiles {
            if let Some(profile) = self.profile(provider, method, id) {
                for origin in profile.origins {
                    if self.authorization_origins.iter().any(|item| item.as_str() == *origin)
                        && !origins.iter().any(|item| item.as_str() == *origin) { origins.push((*origin).to_string()); }
                }
            }
        }
        origins
    }
}
