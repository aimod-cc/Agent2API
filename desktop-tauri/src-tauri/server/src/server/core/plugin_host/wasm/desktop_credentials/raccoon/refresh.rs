//! 本机续期仅消费已保存引用的当前凭据；单独授权、固定接口，并在返回前写回官方文件。

use std::{io::Write, path::PathBuf, sync::Arc};

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::OwnedMutexGuard;
use zeroize::Zeroizing;

use super::{content, invalid, object, project, selected};
use super::super::{account_snapshot, HELPER_RACCOON_REFRESH};
use crate::server::core::plugin_host::{activity::UsageLease, script, wasm::{context::{Context, Shared}, native}};

const URL: &str = "https://xiaohuanxiong.com/api/web/auth/v1/refresh";
const PROFILE_URL: &str = "https://xiaohuanxiong.com/api/web/auth/v1/user_info";
const CONFIRM_URL: &str = "https://xiaohuanxiong.com/api/web/auth/v1/iam/confirm";
const MAXIMUM: usize = 256 * 1024;
const STATE: &str = "host.raccoon-refresh.v1";

struct Source {
    path: PathBuf,
    summary: Value,
    value: Value,
    bytes: Zeroizing<Vec<u8>>,
}

impl Source {
    fn read() -> Result<Self, proto::WireError> {
        let path = selected()?;
        let bytes = Zeroizing::new(super::super::file(&path, MAXIMUM as u64)?);
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if !value.is_object() { return Err(invalid()); }
        let summary = content(&path, project(&value)?)?;
        Ok(Self { path, summary, value, bytes })
    }
}

fn error(status: u16, message: &str, code: &str) -> proto::WireError {
    proto::WireError::with_code(message, format!("provider_http:{}", json!({"status":status,"code":code})))
}

fn changed() -> proto::WireError {
    error(409, "小浣熊本机账号或登录文件已变化，未覆盖当前登录态，请刷新后重试", "desktop_identity_changed")
}

fn token(value: &Value, key: &str) -> String {
    strip(value[key].as_str().unwrap_or(""))
}

fn strip(raw: &str) -> String {
    let mut text = raw.trim();
    loop {
        if !text.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer")) { break; }
        let rest = &text[6..];
        if !rest.starts_with(char::is_whitespace) { break; }
        text = rest.trim_start();
    }
    text.to_string()
}

fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok().filter(Value::is_object)
}

fn iam(token: &str) -> bool {
    claims(token).is_some_and(|claims| claims["iss"].as_str().is_some_and(|issuer| issuer.ends_with("/iam")))
}

fn text(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn user(claims: &Value) -> String {
    for key in ["sid", "iss"] {
        let raw = text(claims, key);
        let raw = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")).unwrap_or(&raw);
        if !raw.is_empty() && raw.len() <= 256 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            if let Some(number) = num_bigint::BigUint::parse_bytes(raw.as_bytes(), 16) { return number.to_str_radix(10); }
        }
    }
    ["sub", "id", "user_id"].iter().map(|key| text(claims, key)).find(|user| !user.is_empty()).unwrap_or_default()
}

fn expiry(access: &str) -> Option<i64> {
    let decoded = claims(access)?;
    let value = &decoded["exp"];
    let at = value.as_f64().or_else(|| value.as_str()?.parse::<f64>().ok())? * 1000.0;
    (at.is_finite() && at > 0.0 && at < i64::MAX as f64).then_some(at as i64)
}

fn renewable(source: &Source) -> bool {
    let Ok(value) = project(&source.value) else { return false; };
    !token(&value, "refreshToken").is_empty() && !iam(&token(&value, "accessToken"))
}

fn bound(reference: &Value, source: &Source) -> Result<(), proto::WireError> {
    if reference["provider"] != "raccoon" || reference["desktop"] != true
        || reference["secret"]["desktop"] != true
        || reference["secret"]["sourceId"] != source.summary["sourceId"] {
        return Err(changed());
    }
    let value = project(&source.value)?;
    let access = token(&value, "accessToken");
    if !iam(&access) {
        if user(&claims(&access).ok_or_else(invalid)?) != text(&reference["secret"], "userId") { return Err(changed()); }
        let org = text(&value, "officeIdentity");
        let expected = text(&reference["secret"], "officeIdentity");
        let org = if org == "personal" { "" } else { &org };
        let expected = if expected == "personal" { "" } else { &expected };
        if org != expected { return Err(changed()); }
    }
    Ok(())
}

pub(in crate::server::core::plugin_host::wasm::desktop_credentials) async fn invoke(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("desktop.credentials.v1")?;
    if !context.shared.options.manifest.helpers.iter().any(|id| id == HELPER_RACCOON_REFRESH)
        || params.as_object().is_none_or(|fields| fields.len() != 3
            || fields.keys().any(|key| !matches!(key.as_str(), "helper" | "operation" | "payload"))) {
        return Err(invalid());
    }
    let operation = params["operation"].as_str().ok_or_else(invalid)?;
    let reference = account_snapshot(context)?.ok_or_else(invalid)?;
    let source = crate::server::core::blocking::run(Source::read).await.map_err(|_| invalid())??;
    if account_snapshot(context)?.as_ref() != Some(&reference) { return Err(changed()); }
    bound(&reference, &source)?;
    if operation == "status" && params["payload"].as_object().is_some_and(|fields| fields.is_empty()) {
        let authorized = native::authorized_builtin("raccoon", HELPER_RACCOON_REFRESH).is_ok();
        return Ok(json!({"authorized":authorized,"refreshable":renewable(&source)}));
    }
    let payload = &params["payload"];
    if operation != "refresh" || payload.as_object().is_none_or(|fields| fields.len() != 2
        || !fields.contains_key("sourceId") || !fields.contains_key("sourceRevision"))
        || payload["sourceId"] != source.summary["sourceId"]
        || payload["sourceRevision"] != source.summary["sourceRevision"] {
        return Err(changed());
    }
    native::authorized_builtin("raccoon", HELPER_RACCOON_REFRESH)?;
    if !renewable(&source) {
        return Err(error(401, "小浣熊当前本机登录态没有可用续期凭据，请在官方客户端重新登录", "raccoon_relogin"));
    }
    // 插件已有账号锁只属于本次实例；后台写回另持宿主来源锁和运行占用，取消不能丢轮换结果。
    let lock_key = format!("raccoon:desktop-refresh:{}", source.summary["sourceId"].as_str().ok_or_else(invalid)?);
    let lock = context.shared.account_lock(&lock_key);
    let guard = tokio::time::timeout_at(context.call_deadline, lock.lock_owned()).await
        .map_err(|_| error(409, "小浣熊本机登录态正在续期，请稍后重试", "plugin_busy"))?;
    if account_snapshot(context)?.as_ref() != Some(&reference) { return Err(changed()); }
    let latest = crate::server::core::blocking::run(Source::read).await.map_err(|_| invalid())??;
    bound(&reference, &latest)?;
    if latest.summary != source.summary { return Ok(latest.summary.clone()); }
    let host = crate::server::core::plugin_host::host_for_provider("raccoon").ok_or_else(invalid)?;
    let lease = host.activity.acquire().ok_or_else(invalid)?;
    let job = Job { shared: context.shared.clone(), account: context.account.clone(), reference, source: latest,
        proxy: context.proxy.clone(), user_agent: context.user_agent.clone().filter(|value| !value.is_empty())
            .unwrap_or_else(|| "raccoon-work".into()),
        deadline: script::HttpDeadline::at(context.call_deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(30))),
        _guard: guard, _lease: lease };
    crate::spawn_task(async move { run(job).await }).await.map_err(|_| invalid())?
}

struct Job {
    shared: Arc<Shared>,
    account: String,
    reference: Value,
    source: Source,
    proxy: Value,
    user_agent: String,
    deadline: script::HttpDeadline,
    _guard: OwnedMutexGuard<()>,
    _lease: Arc<UsageLease>,
}

impl Job {
    fn current(&self) -> Result<(), proto::WireError> {
        if self.shared.cancel.is_cancelled() { return Err(changed()); }
        native::authorized_builtin("raccoon", HELPER_RACCOON_REFRESH)?;
        let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
        let record = store.plugin_account(&self.account).ok_or_else(invalid)?;
        if super::super::account_projection(&record) != self.reference { return Err(changed()); }
        Ok(())
    }
}

fn reserve(job: &Job) -> Result<(), proto::WireError> {
    job.current()?;
    let source = Source::read()?;
    if source.path != job.source.path || source.bytes.as_slice() != job.source.bytes.as_slice() {
        return Err(changed());
    }
    bound(&job.reference, &source)?;
    let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let previous = store.plugin_state_strict("raccoon", &job.account, STATE).map_err(|_| invalid())?;
    if previous["schema"] != 1 {
        return Err(invalid());
    }
    let expected = format!("{:x}", sha2::Sha256::digest(token(&project(&source.value)?, "refreshToken")));
    if previous["value"]["refreshDigest"] == expected
        || previous["value"]["sourceId"] == source.summary["sourceId"]
            && previous["value"]["sourceRevision"] == source.summary["sourceRevision"] {
        return Err(error(409, "小浣熊上次本机续期结果尚未确认，未重复使用旧刷新令牌；请重新登录官方客户端", "raccoon_refresh_unconfirmed"));
    }
    let revision = previous["revision"].as_u64().ok_or_else(invalid)?;
    let result = store.plugin_state_compare_exchange_strict("raccoon", &job.account, STATE, revision, 1,
        json!({ "sourceId": source.summary["sourceId"], "sourceRevision": source.summary["sourceRevision"], "refreshDigest": expected }))
        .map_err(|_| invalid())?;
    if result["updated"] != true { return Err(changed()); }
    job.current()
}

async fn request(job: &Job, url: &str, method: &str, access: &str, body: Option<&Value>) -> Result<(u16, Value), proto::WireError> {
    job.current()?;
    let params = json!({"id":crate::server::core::protocol::random_id("raccoon-refresh"),"method":method,"url":url,
        "headers":{"Authorization":format!("Bearer {access}"),"Content-Type":"application/json","Accept":"application/json","User-Agent":job.user_agent,
            "X-Client-Name":"raccoon-work"},
        "body":body.map(|body| STANDARD.encode(body.to_string().as_bytes())),"provider":"raccoon","proxy":job.proxy});
    let (head, handle) = script::open_body_with_policy(&params, None, false, job.deadline).await?;
    if head["url"] != url || head["headers"]["location"].is_string() { return Err(invalid()); }
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let chunk = handle.read().await?;
        if let Some(data) = chunk["data"].as_str() {
            let data = Zeroizing::new(STANDARD.decode(data).map_err(|_| invalid())?);
            if bytes.len().checked_add(data.len()).is_none_or(|length| length > MAXIMUM) { return Err(invalid()); }
            bytes.extend_from_slice(&data);
        }
        if chunk["done"] == true { break; }
    }
    let reply: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let status = head["status"].as_u64().and_then(|status| u16::try_from(status).ok()).ok_or_else(invalid)?;
    Ok((status, reply))
}

fn code(reply: &Value) -> Option<i64> {
    reply["code"].as_i64().or_else(|| reply["code"].as_str()?.parse().ok())
}

fn profile_data(status: u16, reply: Value) -> Result<Value, proto::WireError> {
    if !(200..300).contains(&status) || code(&reply) != Some(0) || !reply["data"].is_object() {
        return Err(error(502, "小浣熊续期后未能确认原账号身份，未写回不同或未确认的登录态", "raccoon_refresh_identity_unconfirmed"));
    }
    Ok(reply["data"].clone())
}

async fn confirm_identity(job: &Job, access: &str) -> Result<(), proto::WireError> {
    let identity = if iam(access) {
        let claims = claims(access).ok_or_else(invalid)?;
        if claims["iss"] != "xiaohuanxiong.com/iam" || text(&claims, "sub").is_empty()
            || expiry(access).is_none_or(|at| at <= crate::server::logging::now_ms()) { return Err(invalid()); }
        let (status, reply) = request(job, PROFILE_URL, "GET", access, None).await?;
        let profile = if code(&reply) == Some(200_901) {
            let (status, reply) = request(job, CONFIRM_URL, "POST", access, Some(&json!({}))).await?;
            if !(200..300).contains(&status) || code(&reply) != Some(0) { return Err(invalid()); }
            let (status, reply) = request(job, PROFILE_URL, "GET", access, None).await?;
            profile_data(status, reply)?
        } else { profile_data(status, reply)? };
        text(&profile, "id")
    } else {
        user(&claims(access).ok_or_else(invalid)?)
    };
    if identity.is_empty() || identity != text(&job.reference["secret"], "userId") { return Err(changed()); }
    Ok(())
}

async fn run(job: Job) -> Result<Value, proto::WireError> {
    job.current()?;
    let previous = project(&job.source.value)?;
    let access = Zeroizing::new(token(&previous, "accessToken"));
    let refresh = Zeroizing::new(token(&previous, "refreshToken"));
    let body = json!({"refresh_token":refresh.as_str()});
    reserve(&job)?;
    let (status, reply) = request(&job, URL, "POST", &access, Some(&body)).await?;
    let code = code(&reply);
    if !(200..300).contains(&status) || code.is_some_and(|code| code != 0) {
        let status = if status == 401 || matches!(code, Some(200_033 | 200_034 | 200_035)) { 401 } else { 502 };
        return Err(error(status, "小浣熊本机续期被上游拒绝，请同步或重新登录官方客户端", "raccoon_refresh_failed"));
    }
    let data = reply.get("data").filter(|value| value.is_object()).unwrap_or(&reply);
    let next_access = token(data, "access_token");
    if next_access.is_empty() || claims(&next_access).is_none() { return Err(invalid()); }
    confirm_identity(&job, &next_access).await?;
    let next_refresh = if iam(&next_access) { String::new() } else {
        let next = token(data, "refresh_token");
        if next.is_empty() { refresh.to_string() } else { next }
    };
    job.current()?;
    crate::server::core::blocking::run(move || persist(&job, &next_access, &next_refresh)).await
        .map_err(|_| error(502, "小浣熊续期结果未能写回，请重新登录官方客户端", "raccoon_refresh_persist_failed"))?
}

fn persist(job: &Job, access: &str, refresh: &str) -> Result<Value, proto::WireError> {
    job.current()?;
    let latest = Source::read()?;
    if latest.path != job.source.path || latest.bytes.as_slice() != job.source.bytes.as_slice() { return Ok(latest.summary.clone()); }
    let mut value = latest.value.clone();
    replace(&mut value, &["accessToken", "access_token", "token", "auth_token"], "access_token", access);
    replace(&mut value, &["refreshToken", "refresh_token"], "refresh_token", refresh);
    let expiry = expiry(access);
    if let Some(expiry) = expiry {
        if let Some(fields) = value.as_object_mut() {
            for key in ["expiresAt", "tokenExpiresAt"] { if fields.contains_key(key) { fields.insert(key.into(), json!(expiry)); } }
        }
        if let Some(fields) = value.get_mut("auth").and_then(Value::as_object_mut) {
            for key in ["expiresAt", "tokenExpiresAt"] { if fields.contains_key(key) { fields.insert(key.into(), json!(expiry)); } }
        }
    }
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(&value).map_err(|_| invalid())?);
    if bytes.len() > MAXIMUM { return Err(invalid()); }
    let parent = latest.path.parent().ok_or_else(invalid)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| invalid())?;
    file.as_file().set_permissions(super::super::regular(&latest.path)?.permissions()).map_err(|_| invalid())?;
    file.write_all(&bytes).and_then(|_| file.write_all(b"\n")).and_then(|_| file.as_file().sync_all()).map_err(|_| invalid())?;
    let seen = Source::read()?;
    if seen.path != latest.path || seen.bytes.as_slice() != latest.bytes.as_slice() { return Ok(seen.summary.clone()); }
    job.current()?;
    file.persist(&latest.path).map_err(|_| error(502, "小浣熊已续期但写回官方登录文件失败，请重新登录官方客户端", "raccoon_refresh_persist_failed"))?;
    let stored = object(&latest.path, MAXIMUM as u64)?;
    let projection = project(&stored)?;
    if token(&projection, "accessToken") != access || token(&projection, "refreshToken") != refresh { return Err(changed()); }
    let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let reserved = store.plugin_state_strict("raccoon", &job.account, STATE).map_err(|_| invalid())?;
    let digest = format!("{:x}", sha2::Sha256::digest(token(&project(&job.source.value)?, "refreshToken")));
    if reserved["value"]["refreshDigest"] != digest
        || store.plugin_state_compare_exchange_strict("raccoon", &job.account, STATE,
            reserved["revision"].as_u64().ok_or_else(invalid)?, 1, Value::Null).map_err(|_| invalid())?["updated"] != true {
        return Err(invalid());
    }
    content(&latest.path, projection)
}

fn replace(value: &mut Value, aliases: &[&str], fallback: &str, next: &str) {
    let nested = value["auth"].is_object();
    let mut found = false;
    if let Some(fields) = value.as_object_mut() {
        for key in aliases {
            if fields.contains_key(*key) {
                found = true;
                if next.is_empty() { fields.remove(*key); } else { fields.insert((*key).into(), json!(next)); }
            }
        }
    }
    if let Some(fields) = value.get_mut("auth").and_then(Value::as_object_mut) {
        for key in aliases {
            if fields.contains_key(*key) {
                found = true;
                if next.is_empty() { fields.remove(*key); } else { fields.insert((*key).into(), json!(next)); }
            }
        }
    }
    if !found && !next.is_empty() {
        if nested { value["auth"][fallback] = json!(next); } else { value[fallback] = json!(next); }
    }
}
