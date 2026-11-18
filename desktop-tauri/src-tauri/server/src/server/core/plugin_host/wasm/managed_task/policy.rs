//! 固定 profile 安全约束。声明不是授权；provider/entry 必须来自宿主真实调用，
//! 后续仍需对包ID、provider、模块摘要、声明摘要核对显式批准。
//! 首版只接受文本任务所需封闭子集，不支持 URL 模板、附件或任意 JSON 表达式。

use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use super::{
    strict_json::{self, Object},
    types::{self, NonZeroUuid, ValidationError},
};

pub(super) const PROFILE: &str = "monkeycode.cn.task.v1";
pub(super) const INTL_PROFILE: &str = "monkeycode.intl.task.v1";
pub(super) const PROVIDER: &str = "monkeycode";
pub(super) const INTL_PROVIDER: &str = "monkeycode-intl";
pub(super) const ENTRY: &str = "conversation";
pub(super) const ORIGIN: &str = "https://monkeycode-ai.com";
pub(super) const HOST: &str = "monkeycode-ai.com";

pub(super) fn profile(provider: &str) -> Option<&'static str> {
    match provider { PROVIDER => Some(PROFILE), INTL_PROVIDER => Some(INTL_PROFILE), _ => None }
}
pub(super) fn origin(provider: &str) -> Option<&'static str> {
    match provider { PROVIDER => Some(ORIGIN), INTL_PROVIDER => Some("https://monkeycode-ai.net"), _ => None }
}
pub(super) fn hostname(provider: &str) -> Option<&'static str> {
    match provider { PROVIDER => Some(HOST), INTL_PROVIDER => Some("monkeycode-ai.net"), _ => None }
}
pub(super) const TASKS_PATH: &str = "/api/v1/users/tasks";
pub(super) const STOP_PATH: &str = "/api/v1/users/tasks/stop";
pub(super) const STREAM_PATH: &str = "/api/v1/users/tasks/stream";
pub(super) const COOKIE_NAME: &str = "monkeycode_ai_session";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCreate {
    content: String,
    #[serde(default, deserialize_with = "present_text")]
    system_prompt: Option<String>,
    model_id: String,
    image_id: String,
    host_id: String,
    cli_name: String,
    resource: Object<Resource>,
    repo: Object<Repo>,
}

fn present_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    // 可缺省，但显式 null 不是文本；不使用 Option 的宽松反序列化。
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    core: u32,
    memory: u64,
    life: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Repo {
    repo_url: String,
    branch: String,
    repo_filename: String,
    zip_url: String,
}

/// 重建后的正文；无任意 JSON/URL/头注入，也不是已获准发送的网络请求。
pub(super) struct CreateBody {
    bytes: Vec<u8>,
}

impl CreateBody {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, ValidationError> {
        strict_json::validate(bytes, types::MAX_CREATE_BYTES).map_err(|_| body_error())?;
        let raw = serde_json::from_slice::<Object<RawCreate>>(bytes)
            .map_err(|_| body_error())?
            .0;
        if raw.content.trim().is_empty()
            || raw.content.len() > types::MAX_CONTENT_BYTES
            || raw
                .system_prompt
                .as_ref()
                .is_some_and(|text| text.len() > types::MAX_SYSTEM_PROMPT_BYTES)
            || raw.host_id != "public_host"
            || raw.cli_name != "opencode"
            || raw.resource.0.core != 1
            || raw.resource.0.memory != 1_073_741_824
            || raw.resource.0.life != 3600
            || !raw.repo.0.repo_url.is_empty()
            || raw.repo.0.branch != "master"
            || !raw.repo.0.repo_filename.is_empty()
            || !raw.repo.0.zip_url.is_empty()
        {
            return Err(body_error());
        }
        let model = NonZeroUuid::parse(&raw.model_id).ok_or_else(body_error)?;
        let image = NonZeroUuid::parse(&raw.image_id).ok_or_else(body_error)?;
        // 正常换行/制表等合法文本逐字保留；绝不把提示词文本当 JSON 再解析。
        let mut body = json!({
            "content": raw.content,
            "model_id": model.as_str(), "image_id": image.as_str(),
            "host_id": "public_host", "cli_name": "opencode",
            "resource": { "core": 1, "memory": 1_073_741_824u64, "life": 3600 },
            "repo": { "repo_url": "", "branch": "master", "repo_filename": "", "zip_url": "" }
        });
        if let Some(system) = raw.system_prompt {
            body["system_prompt"] = Value::String(system);
        }
        let bytes = serde_json::to_vec(&body).map_err(|_| body_error())?;
        if bytes.len() > types::MAX_CREATE_BYTES {
            return Err(body_error());
        }
        Ok(Self { bytes })
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 执行器取得唯一正文所有权；完成创建后不在终态保存提示词。
    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

fn body_error() -> ValidationError {
    ValidationError::new("任务正文无效：必须是大小受限的固定文本任务结构，UUID及资源/仓库字段必须准确，不接受额外字段")
}

/// 只保存当前可信快照 accessToken 的固定 Cookie 头，不提供原值展示或 JSON 导入。
pub(super) struct ProjectedCookie {
    header: HeaderValue,
}

impl ProjectedCookie {
    /// 调用方负责从可信账号快照取 accessToken；此处只校验投影，不证明账号归属。
    pub(super) fn from_trusted_access_token(access_token: &str) -> Result<Self, ValidationError> {
        if access_token.is_empty()
            || access_token.len() > types::MAX_COOKIE_BYTES
            || !access_token.bytes().all(|byte| {
                matches!(byte,
                0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
            })
        {
            return Err(ValidationError::new(
                "可信账号的 Session 投影无效，未使用请求正文或其他 Cookie 替代",
            ));
        }
        let mut header = HeaderValue::from_str(&format!("{COOKIE_NAME}={access_token}"))
            .map_err(|_| ValidationError::new("可信账号的 Cookie 头无法构造"))?;
        header.set_sensitive(true);
        Ok(Self { header })
    }

    /// 仅供将来执行器构造其固定请求头；替换而非追加调用方 Cookie。
    pub(super) fn insert_into(&self, headers: &mut HeaderMap) {
        headers.insert(COOKIE, self.header.clone());
    }
}

/// 固定地址描述，不是网络授权；没有自定义地址构造方法。
pub(super) struct Route {
    method: &'static str,
    url: String,
}

impl Route {
    pub(super) fn method(&self) -> &'static str {
        self.method
    }
    pub(super) fn url(&self) -> &str {
        &self.url
    }
}

pub(super) fn create_route(provider: &str) -> Result<Route, ValidationError> {
    Ok(Route { method: "POST", url: format!("{}{TASKS_PATH}", origin(provider).ok_or_else(body_error)?) })
}

pub(super) fn stop_route(provider: &str) -> Result<Route, ValidationError> {
    Ok(Route { method: "PUT", url: format!("{}{STOP_PATH}", origin(provider).ok_or_else(body_error)?) })
}

pub(super) fn detail_route(provider: &str, id: &NonZeroUuid) -> Result<Route, ValidationError> {
    Ok(Route { method: "GET", url: format!("{}{TASKS_PATH}/{}", origin(provider).ok_or_else(body_error)?, id.as_str()) })
}

pub(super) fn stream_route(provider: &str, id: &NonZeroUuid) -> Result<Route, ValidationError> {
    Ok(Route { method: "GET", url: format!("wss://{}{STREAM_PATH}?id={}&mode=attach", hostname(provider).ok_or_else(body_error)?, id.as_str()) })
}

/// 仅对宿主确认选择此 profile 的包使用；封锁普通任务通道，不代替已有来源授权。
pub(super) fn guard_ordinary_request(
    provider: &str, raw_url: &str,
    headers: &HeaderMap,
) -> Result<(), ValidationError> {
    let host = hostname(provider).ok_or_else(body_error)?;
    if headers
        .keys()
        .any(|name| method_override_header(name.as_str()) || routing_header(name.as_str()))
    {
        return Err(ValidationError::new(
            "受控任务 profile 不允许来源、方法或路径覆盖头",
        ));
    }
    if raw_url.len() > 8192 || raw_url.contains('\\') || raw_url.chars().any(char::is_control) {
        return Err(ValidationError::new(
            "普通请求地址不规范，无法排除任务通道绕过",
        ));
    }
    let url = url::Url::parse(raw_url)
        .map_err(|_| ValidationError::new("普通请求地址无效，未授予任务访问"))?;
    if url
        .host_str()
        .is_none_or(|candidate| !candidate.trim_end_matches('.').eq_ignore_ascii_case(host))
    {
        // 其他来源仍必须由普通网络权限检查，绝不因此授权国际站或第三方站点。
        return Ok(());
    }
    if !matches!(url.scheme(), "https" | "wss")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ValidationError::new(
            "MonkeyCode 受控来源必须是默认443的HTTPS/WSS且不含登录信息或片段",
        ));
    }
    let prefix = raw_url
        .get(..8)
        .is_some_and(|value| value.eq_ignore_ascii_case("https://"))
        || raw_url
            .get(..6)
            .is_some_and(|value| value.eq_ignore_ascii_case("wss://"));
    let authority = raw_url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if !prefix
        || !(authority.eq_ignore_ascii_case(host)
            || authority.eq_ignore_ascii_case(&format!("{host}:443")))
    {
        return Err(ValidationError::new(
            "MonkeyCode 来源必须使用固定域名与规范HTTPS/WSS地址",
        ));
    }
    // Url 会先折叠点段；同时检查原始路径，不能让编码或后端二次规范化绕过任务边界。
    let raw_path = raw_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.find(['/', '?', '#']).map(|index| &rest[index..]))
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    guard_ordinary_path(raw_path)?;
    let path = url.path().to_ascii_lowercase();
    if path == TASKS_PATH || path.starts_with(&format!("{TASKS_PATH}/")) {
        return Err(ValidationError::new(
            "任务创建、停止、详情和流必须经本次可信收据通道，不能使用普通网络入口",
        ));
    }
    Ok(())
}

/// 同时用于绝对 URL 与相对 Location；必须在 Url 解析或 join 折叠路径之前保留原文。
pub(super) fn guard_ordinary_path(raw_path: &str) -> Result<(), ValidationError> {
    if raw_path.len() > 8192
        || raw_path.contains(['%', ';', '\\'])
        || raw_path.contains("//")
        || raw_path.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        || raw_path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ValidationError::new(
            "MonkeyCode 普通请求路径存在编码或规范化歧义，未授予任务访问",
        ));
    }
    Ok(())
}

fn routing_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "host" | "forwarded" | "x-original-host" | "x-host" | "x-original-uri")
        || name.starts_with("x-forwarded-")
}

pub(super) fn method_override_header(name: &str) -> bool {
    [
        "x-http-method-override",
        "x-http-method",
        "x-method-override",
        "http-method-override",
        "x-original-method",
        "x-original-url",
        "x-rewrite-url",
    ]
    .iter()
    .any(|blocked| name.eq_ignore_ascii_case(blocked))
}
