use agent2api_provider_protocol as proto;
use serde_json::Value;
use super::{context::Context, resources::Resources};
use super::super::script::{HttpDeadline, HttpHandle, RequestBody};

pub(super) fn guard_ordinary(policy: super::managed_task::OrdinaryPolicy, params: &Value) -> Result<(), proto::WireError> {
    if !policy.applies() { return Ok(()); }
    let invalid = || proto::WireError::with_code("普通网络请求地址或请求头格式无效", "bad_params");
    let url = params["url"].as_str().ok_or_else(invalid)?;
    let mut headers = reqwest::header::HeaderMap::new();
    match params.get("headers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(fields)) if fields.len() <= 128 => {
            for (name, value) in fields {
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
                let value = reqwest::header::HeaderValue::from_str(value.as_str().ok_or_else(invalid)?)
                    .map_err(|_| invalid())?;
                if headers.contains_key(&name) {
                    return Err(proto::WireError::with_code("受控任务 profile 不允许大小写别名重复请求头", "bad_params"));
                }
                headers.insert(name, value);
            }
        }
        _ => return Err(invalid()),
    }
    policy.check_request(url, &headers)
}

pub(super) fn guard_token_exchange(context: &Context, params: &Value) -> Result<(), proto::WireError> {
    let provider = context.shared.options.provider.as_str();
    if !matches!(provider, "kiro" | "codearts" | "doubaowork") { return Ok(()); }
    let raw = params["url"].as_str().ok_or_else(|| super::error("换证请求地址无效"))?;
    let parsed = url::Url::parse(raw).map_err(|_| super::error("换证请求地址无效"))?;
    let host = parsed.host_str().unwrap_or("");
    let mut path = parsed.path().to_string();
    for _ in 0..4 {
        let decoded = url::form_urlencoded::parse(format!("path={path}").as_bytes()).next()
            .map(|(_, value)| value.into_owned()).ok_or_else(|| super::error("换证请求地址无效"))?;
        if decoded == path { break; }
        path = decoded;
    }
    if path.contains('%') { return Err(super::error("换证请求地址无效")); }
    let path = format!("/{}", path.split('/').filter(|part| !part.is_empty())
        .map(|part| part.split(';').next().unwrap_or("")).collect::<Vec<_>>().join("/"));
    let reserved = if provider == "kiro" {
        (host.ends_with(".auth.desktop.kiro.dev") && path.trim_end_matches('/').eq_ignore_ascii_case("/oauth/token"))
            || (host.starts_with("oidc.") && host.ends_with(".amazonaws.com")
                && path.trim_end_matches('/').eq_ignore_ascii_case("/token"))
    } else if provider == "doubaowork" {
        host == "www.doubao.com" && ["/alice/ug/campaign/landing/checkin", "/alice/ug/campaign/item/reward/claim"]
            .iter().any(|reserved| path.trim_end_matches('/').eq_ignore_ascii_case(reserved))
    } else {
        host == "sts.cn-north-4.myhuaweicloud.com"
            && path.trim_end_matches('/').eq_ignore_ascii_case("/v1/oauth2/tokens")
    };
    if reserved {
        if provider == "kiro" && path == "/token" && context.managed.is_none()
            && !context.account.is_empty() && params["method"] == "POST"
            && valid_kiro_refresh_body(params)?
        {
            return Ok(());
        }
        if provider == "codearts" && context.managed.is_none()
            && !context.account.is_empty() && params["method"] == "POST"
            && parsed.query().is_none()
            && params.get("body_resource").is_none_or(Value::is_null)
            && params.get("multipart").is_none_or(Value::is_null)
            && valid_codearts_refresh_body(params)?
        {
            return Ok(());
        }
        return Err(proto::WireError::with_code("一次性授权兑换必须由本次原调用的固定宿主入口执行", "forbidden"));
    }
    Ok(())
}

fn decode_refresh_body(params: &Value, provider_label: &str) -> Result<zeroize::Zeroizing<Vec<u8>>, proto::WireError> {
    use base64::Engine;
    let message = format!("{provider_label}续期正文无效");
    let body = params["body"].as_str().filter(|body| body.len() <= 176 * 1024)
        .ok_or_else(|| super::error(&message))?;
    let raw = zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(body)
        .map_err(|_| super::error(&message))?);
    if raw.len() > 128 * 1024 { return Err(super::error(&message)); }
    Ok(raw)
}

fn valid_refresh_scalar(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8192 && !value.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn valid_kiro_refresh_body(params: &Value) -> Result<bool, proto::WireError> {
    let raw = decode_refresh_body(params, "Kiro")?;
    let fields: serde_json::Map<String, Value> = super::managed_task::declaration::parse_object(&raw, 128 * 1024)
        .map_err(|_| super::error("Kiro续期正文无效"))?;
    Ok(fields.len() == 4
        && fields.get("grantType").and_then(Value::as_str) == Some("refresh_token")
        && fields.keys().all(|key| matches!(key.as_str(), "clientId" | "clientSecret" | "refreshToken" | "grantType"))
        && fields.values().all(|value| value.as_str().is_some_and(valid_refresh_scalar)))
}

fn valid_codearts_refresh_body(params: &Value) -> Result<bool, proto::WireError> {
    let raw = decode_refresh_body(params, "CodeArts")?;
    if std::str::from_utf8(&raw).is_err() {
        return Err(super::error("CodeArts续期正文无效"));
    }
    let mut fields = std::collections::HashMap::new();
    for (key, value) in url::form_urlencoded::parse(&raw) {
        if fields.len() >= 4
            || !matches!(key.as_ref(), "client_id" | "code_verifier" | "grant_type" | "refresh_token")
            || !valid_refresh_scalar(&value)
            || fields.insert(key.into_owned(), value.into_owned()).is_some()
        {
            return Ok(false);
        }
    }
    Ok(fields.len() == 4 && fields.get("grant_type").map(String::as_str) == Some("refresh_token"))
}

pub(super) fn deadline(context: &Context, params: &Value, started: tokio::time::Instant) -> Result<HttpDeadline, proto::WireError> {
    let Some(value) = params.get("timeout_ms") else { return Ok(HttpDeadline::default()); };
    // 只接受 JSON 无符号整数，不把负数、浮点、文本、bool 或 null 转成时长。
    let millis = match value {
        Value::Number(number) if number.is_u64() => number.as_u64(),
        _ => None,
    }.ok_or_else(|| proto::WireError::with_code("timeout_ms 必须是无符号整数", "bad_params"))?;
    let deadline = HttpDeadline::at((started + std::time::Duration::from_millis(millis.clamp(1000, 600_000)))
        .min(context.call_deadline));
    deadline.check()?;
    context.permission("http.timeout.v1")?;
    Ok(deadline)
}

impl Context {
    pub(super) fn keep_http(&mut self, result: Value, handle: HttpHandle, deadline: HttpDeadline) -> Result<Value, proto::WireError> {
        deadline.check()?;
        let id = handle.id().to_string();
        if self.resources.len() >= 8 || self.resources.contains_key(&id) || self.http_handles.contains_key(&id) {
            return Err(super::error("单次插件调用连接数超限或请求标识重复"));
        }
        let sse = result["headers"].as_object().into_iter().flatten().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-type") && value.as_str().is_some_and(|value|
                value.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/event-stream"))
        });
        self.progress.http_open(&id, sse);
        self.resources.insert(id.clone(), "http");
        self.http_handles.insert(id.clone(), handle);
        if let Err(error) = deadline.check() { self.forget_http(&id); return Err(error); }
        Ok(result)
    }

    pub(super) fn forget_http(&mut self, id: &str) {
        // 先关闭具体下载并回收全局槽，再移除当前调用归属；包括超时终态。
        self.http_handles.remove(id);
        self.resources.remove(id);
        self.progress.http_close(id);
    }
}

fn name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-[]".contains(&byte))
}

pub(super) fn body(context: &Context, params: &Value, deadline: HttpDeadline) -> Result<Option<RequestBody>, proto::WireError> {
    deadline.check()?;
    let resource = params.get("body_resource").filter(|value| !value.is_null());
    let multipart = params.get("multipart").filter(|value| !value.is_null());
    if resource.is_some() as u8 + multipart.is_some() as u8 + params.get("body").is_some_and(|value| !value.is_null()) as u8 > 1 {
        return Err(super::error("请求正文不能同时使用多种传输形式"));
    }
    if let Some(resource) = resource {
        context.permission("resources.v1")?;
        let bytes = context.blobs.raw(resource.as_str().ok_or_else(super::errors::protocol)?)?;
        deadline.check()?;
        return Ok(Some(RequestBody::Bytes(bytes)));
    }
    let Some(multipart) = multipart else { return Ok(None) };
    context.permission("resources.v1")?;
    Ok(Some(RequestBody::Multipart(form(&context.blobs, multipart, deadline)?)))
}

fn form(resources: &Resources, multipart: &Value, deadline: HttpDeadline) -> Result<reqwest::multipart::Form, proto::WireError> {
    let fields = multipart["fields"].as_object().filter(|fields| fields.len() <= 64).ok_or_else(super::errors::protocol)?;
    let files = multipart["files"].as_array().filter(|files| files.len() <= 16).ok_or_else(super::errors::protocol)?;
    let mut total = 0;
    let mut form = reqwest::multipart::Form::new();
    for (key, value) in fields {
        deadline.check()?;
        if !name(key) || !value.is_string() { return Err(super::errors::protocol()); }
        let text = value.as_str().unwrap_or("");
        total += text.len();
        if total > 1024 * 1024 { return Err(super::error("上传文本字段过大")); }
        form = form.text(key.clone(), text.to_string());
    }
    for file in files {
        deadline.check()?;
        let field = file["field"].as_str().unwrap_or("");
        let filename = file["filename"].as_str().unwrap_or("");
        if !name(field) || !name(filename) { return Err(super::errors::protocol()); }
        let bytes = resources.raw(file["resource"].as_str().ok_or_else(super::errors::protocol)?)?;
        deadline.check()?;
        total += bytes.len();
        if total > 96 * 1024 * 1024 { return Err(super::error("上传总量超过资源限制")); }
        let part = reqwest::multipart::Part::stream(reqwest::Body::from(bytes))
            .file_name(filename.to_string()).mime_str(file["content_type"].as_str().unwrap_or("application/octet-stream"))
            .map_err(|_| super::error("上传文件类型无效"))?;
        form = form.part(field.to_string(), part);
    }
    deadline.check()?;
    Ok(form)
}
