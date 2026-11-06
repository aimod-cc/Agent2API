use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{extract::{Path, State}, http::{header, HeaderMap}, response::Response};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::{core::{blocking, key_scope::KeyScope, providers::images::ImageOperation,
    tool_services::{self, image_config, image_jobs::{self, ImageFile, ImageJob}, McpRequestContext},
    upstream::usage::RequestTelemetry}, db::Db, errors::GatewayError, logging, ServerState};
use super::images::{self, ImageExecution};

pub(super) fn database(state: &ServerState) -> Result<Db, GatewayError> {
    state.db().cloned().ok_or_else(|| GatewayError::with_status(503, "图片任务数据库不可用，未发送生图请求"))
}

pub(super) fn owner(headers: &HeaderMap) -> String {
    let key = headers.get("x-api-key").and_then(|value| value.to_str().ok()).or_else(|| {
        headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).map(crate::server::http::strip_bearer_prefix)
    }).unwrap_or("");
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

pub(super) fn validate_origin(headers: &HeaderMap) -> Result<(), GatewayError> {
    let Some(origin) = headers.get(header::ORIGIN) else { return Ok(()) };
    let parsed = origin.to_str().ok().and_then(|value| url::Url::parse(value).ok());
    let host = headers.get(header::HOST).and_then(|value| value.to_str().ok());
    let valid = parsed.as_ref().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none()
            && url.path() == "/" && url.query().is_none() && url.fragment().is_none()
            && host.is_some_and(|host| url::Url::parse(&format!("{}://{host}", url.scheme())).is_ok_and(|expected| expected.origin() == url.origin()))
    });
    if valid { Ok(()) } else { Err(GatewayError::with_status(403, "图片 MCP 拒绝不受信任的网页来源")) }
}

pub(super) fn authorize(job: &ImageJob, owner: &str, scope: Option<&KeyScope>) -> Result<(), GatewayError> {
    if job.owner != owner || !crate::server::core::key_scope::allows_provider(scope, &job.provider)
        || !crate::server::core::key_scope::allows_model(scope, &job.model) {
        return Err(GatewayError::with_status(404, "图片任务不存在或无权访问").with_code("image_job_not_found"));
    }
    Ok(())
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn submission_id(args: &Value) -> Result<String, GatewayError> {
    let id = text(args, "request_id");
    if !(8..=128).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Err(GatewayError::bad_request("request_id 必须是 8–128 位英文字母、数字、连字符或下划线；重试应使用原编号"));
    }
    Ok(id.to_string())
}

fn options(args: &Value, query: bool) -> Result<(u64, bool), GatewayError> {
    let Some(object) = args.as_object() else { return Err(GatewayError::bad_request("工具参数必须是对象")) };
    let allowed: &[&str] = if query { &["job_id", "request_id", "wait_seconds", "preview"] }
        else { &["request_id", "prompt", "n", "size", "quality", "output_format", "images", "wait_seconds", "preview"] };
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(GatewayError::bad_request(format!("图片工具不支持参数 {key}")));
    }
    let wait = match args.get("wait_seconds") {
        None => if query { 0 } else { 10 },
        Some(value) => value.as_u64().filter(|value| *value <= 20).ok_or_else(|| GatewayError::bad_request("wait_seconds 必须为 0–20 的整数"))?,
    };
    let preview = match args.get("preview") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| GatewayError::bad_request("preview 必须为布尔值"))?,
    };
    Ok((wait, preview))
}

pub(crate) async fn call(name: &str, args: &Value, ctx: &McpRequestContext) -> Result<Value, GatewayError> {
    let query = name == "get_image_result";
    let (wait, preview) = options(args, query)?;
    if name == "get_image_result" {
        for key in ["job_id", "request_id"] {
            if args.get(key).is_some_and(|value| value.as_str().is_none_or(|text| text.is_empty() || text.len() > 128)) {
                return Err(GatewayError::bad_request("任务编号必须是 1–128 个字符的字符串"));
            }
        }
    }
    let db = database(&ctx.state)?;
    let mut job = if query {
        let id = text(args, "job_id").to_string();
        let request_id = text(args, "request_id").to_string();
        if id.is_empty() && request_id.is_empty() {
            return Err(GatewayError::bad_request("请提供 job_id 或原 request_id"));
        }
        let db_copy = db.clone();
        let owner = ctx.owner.clone();
        blocking::run(move || {
            if !id.is_empty() { image_jobs::get(&db_copy, &id) } else {
                image_jobs::find_request(&db_copy, &owner, &request_id)?.ok_or_else(|| GatewayError::with_status(404, "未找到该提交编号，未重新生图"))
            }
        }).await??
    } else {
        submit(name, args, ctx, &db).await?
    };
    authorize(&job, &ctx.owner, ctx.scope.as_ref())?;
    if !text(args, "request_id").is_empty() && text(args, "request_id") != job.request_id {
        return Err(GatewayError::bad_request("job_id 与 request_id 不属于同一任务"));
    }
    let until = Instant::now() + Duration::from_secs(wait);
    while job.state == "running" && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let db_copy = db.clone();
        let id = job.id.clone();
        job = blocking::run(move || image_jobs::get(&db_copy, &id)).await??;
    }
    super::tool_image_results::mcp_result(&db, &job, &ctx.base_url, preview).await
}

async fn submit(name: &str, args: &Value, ctx: &McpRequestContext, db: &Db) -> Result<ImageJob, GatewayError> {
    let operation = match name {
        "generate_images" => ImageOperation::Generate,
        "edit_images" => ImageOperation::Edit,
        _ => return Err(GatewayError::bad_request("未知的图片工具")),
    };
    let request_id = submission_id(args)?;
    let mut normalized_args = args.clone();
    if normalized_args.get("n").is_none() { normalized_args["n"] = json!(1); }
    if normalized_args.get("output_format").is_none() { normalized_args["output_format"] = json!("png"); }
    let args = &normalized_args;
    let mut hash_args = args.clone();
    if let Some(object) = hash_args.as_object_mut() { object.remove("wait_seconds"); object.remove("preview"); }
    let hash = format!("{:x}", Sha256::digest(format!("{name}\n{hash_args}").as_bytes()));
    let db_copy = db.clone();
    let owner = ctx.owner.clone();
    let request_copy = request_id.clone();
    if let Some(job) = blocking::run(move || image_jobs::find_request(&db_copy, &owner, &request_copy)).await?? {
        authorize(&job, &ctx.owner, ctx.scope.as_ref())?;
        return same_request(job, &hash);
    }
    let config = image_config::load().ok_or_else(|| GatewayError::with_status(503, "尚未配置图片生成服务"))?;
    let target = image_config::target(&config, operation)?.clone();
    let scope = image_config::restrict_scope(ctx.scope.as_ref(), &target)?;
    let mut payload = json!({ "model": target.model, "response_format": "b64_json", "output_format": "png" });
    for key in ["prompt", "n", "size", "quality", "output_format"] {
        if let Some(value) = args.get(key) { payload[key] = value.clone(); }
    }
    if let Some(references) = args.get("images") {
        if operation != ImageOperation::Edit { return Err(GatewayError::bad_request("参考图只能用于编辑工具")); }
        let references = references.as_array().filter(|items| !items.is_empty() && items.len() <= 16)
            .ok_or_else(|| GatewayError::bad_request("编辑需要 1–16 张参考图"))?;
        let mut images = Vec::new();
        let mut inline_bytes = 0usize;
        for reference in references {
            let reference = reference.as_str().ok_or_else(|| GatewayError::bad_request("参考图必须是图片 ID、URL 或 data URI"))?;
            if let Some(id) = reference.strip_prefix("image:") {
                let id = id.to_string();
                let db_copy = db.clone();
                let (job, file) = blocking::run(move || image_jobs::read_file(&db_copy, &id)).await??;
                authorize(&job, &ctx.owner, ctx.scope.as_ref())?;
                if file.bytes.len() > 10 * 1024 * 1024 { return Err(GatewayError::bad_request("参考图片超过 10 MiB，请先缩小图片")); }
                images.push(json!({ "image_url": format!("data:{};base64,{}", file.mime, base64::engine::general_purpose::STANDARD.encode(file.bytes)) }));
            } else {
                if !(reference.starts_with("https://") || reference.starts_with("http://") || reference.starts_with("data:image/")) {
                    return Err(GatewayError::bad_request("HTTP MCP 不能读取本机文件路径，请使用本地桥接"));
                }
                images.push(json!({ "image_url": reference }));
            }
            inline_bytes = inline_bytes.saturating_add(images.last().and_then(|image| image["image_url"].as_str()).map_or(0, str::len));
            if inline_bytes > super::image_input::MAX_INLINE_BYTES {
                return Err(GatewayError::with_status(413, "参考图内联数据总大小不得超过 14 MiB"));
            }
        }
        payload["images"] = json!(images);
    }
    images::validate(&mut payload, operation)?;
    let count = payload["n"].as_u64().unwrap_or(1) as u32;
    if count > config.max_images.clamp(1, 10) {
        return Err(GatewayError::bad_request("生成数量超过管理员设置的单次上限"));
    }
    let store = ctx.store.clone();
    let target_copy = target.clone();
    blocking::run(move || image_config::validate_target(&store, &target_copy, operation)).await??;
    let timeout = tool_services::timeout_seconds(tool_services::IMAGE_GENERATION_SERVICE_ID);
    let db = db.clone();
    let db_copy = db.clone();
    let owner = ctx.owner.clone();
    let hash_copy = hash.clone();
    let state = ctx.state.clone();
    let source = ctx.source.clone();
    // 受理落库与启动独立于 HTTP 请求生命周期，断线不能只留下占位记录。
    let acceptance = tokio::spawn(async move {
        let (job, created) = blocking::run(move || image_jobs::create(&db_copy, &config, image_jobs::NewJob {
            owner: &owner, request_id: &request_id, request_hash: &hash_copy, operation: name_for(operation), target: &target, count, timeout,
        })).await??;
        let job = same_request(job, &hash)?;
        if created {
            let id = job.id.clone();
            tokio::spawn(async move {
                run_job(state, db, id, payload, ImageExecution { operation, headers: HeaderMap::new(),
                    scope: Some(scope), source, started_at: logging::now_ms(), is_assistant: false }, timeout).await;
            });
        }
        Ok::<_, GatewayError>(job)
    });
    acceptance.await.map_err(|_| GatewayError::with_status(503,
        "图片任务受理结果未知，请使用原 request_id 查询，勿重新提交").with_code("image_result_uncertain"))?
}

fn same_request(job: ImageJob, hash: &str) -> Result<ImageJob, GatewayError> {
    if job.request_hash != hash {
        return Err(GatewayError::with_status(409, "同一 request_id 已用于不同的图片请求；未再次生图").with_code("image_request_conflict"));
    }
    Ok(job)
}

fn name_for(operation: ImageOperation) -> &'static str {
    match operation { ImageOperation::Generate => "generate", ImageOperation::Edit => "edit" }
}

async fn run_job(state: ServerState, db: Db, id: String, mut payload: Value, input: ImageExecution, timeout: u64) {
    let telemetry = Arc::new(RequestTelemetry::new());
    telemetry.ensure_id(&id);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let preparation = async {
        if payload.get("images").is_some() { super::image_input::prepare(&mut payload).await?; }
        Ok::<_, GatewayError>(())
    };
    match tokio::time::timeout(Duration::from_secs(timeout), preparation).await {
        Ok(Ok(())) => {}
        result => {
            let message = match result {
                Ok(Err(error)) => error.message,
                _ => "读取参考图超时，尚未发起生图请求".to_string(),
            };
            let _ = blocking::run(move || image_jobs::fail(&db, &id, false, &message)).await;
            return;
        }
    }
    let budget = deadline.saturating_duration_since(Instant::now());
    let result = images::execute_with_telemetry(&state, payload, input, telemetry.clone(), Some(budget)).await;
    let outcome = match result {
        Ok(response) => {
            let snapshot = telemetry.snapshot();
            let db_copy = db.clone();
            let id_copy = id.clone();
            match blocking::run(move || {
                let (mut metadata, files) = prepare_files(response)?;
                metadata["actual_account_id"] = json!(snapshot.account_id);
                metadata["actual_account_name"] = json!(snapshot.account_name);
                metadata["actual_model"] = json!(snapshot.upstream_model);
                metadata["credit"] = json!(snapshot.credit);
                image_jobs::finish(&db_copy, &id_copy, metadata, files)
            }).await {
                Ok(result) => result.map_err(|error| (true, format!("图片已生成，但结果保存失败：{}；请勿重新提交", error.message))),
                Err(error) => Err((true, format!("图片已生成，但结果处理失败：{}；请勿重新提交", error.message))),
            }
        }
        Err(error) => {
            let uncertain = telemetry.snapshot().attempts > 0 && !matches!(error.status_code, 400 | 401 | 403 | 404 | 422 | 429);
            Err((uncertain, error.message))
        }
    };
    if let Err((uncertain, message)) = outcome {
        let summary = message.clone();
        let id_copy = id.clone();
        if let Err(error) = blocking::run(move || image_jobs::fail(&db, &id_copy, uncertain, &message)).await.and_then(|result| result) {
            logging::log("[Tools]", &format!("图片任务 {id} 状态保存失败：{}", error.message));
        }
        logging::log("[Tools]", &format!("图片任务 {id}：{summary}"));
    }
}

fn prepare_files(response: Value) -> Result<(Value, Vec<ImageFile>), GatewayError> {
    let items = response["data"].as_array().filter(|items| !items.is_empty())
        .ok_or_else(|| GatewayError::new("上游未返回图片"))?;
    let mut files = Vec::new();
    for item in items {
        let encoded = item["b64_json"].as_str().ok_or_else(|| GatewayError::new("生图结果缺少图片字节"))?;
        if encoded.len() > (image_jobs::MAX_IMAGE_BYTES as usize).div_ceil(3) * 4 { return Err(GatewayError::new("生成图片过大")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| GatewayError::new("图片编码无效"))?;
        let mime = super::image_input::mime_of(&bytes)?.to_string();
        let format = image::guess_format(&bytes).map_err(|_| GatewayError::new("图片格式无效"))?;
        let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format).into_dimensions()
            .map_err(|_| GatewayError::new("图片尺寸无效"))?;
        files.push(ImageFile { id: image_jobs::random_id()?, mime, width, height, bytes });
    }
    let mut metadata = json!({});
    for key in ["created", "usage", "points_consumed", "size", "quality", "output_format"] {
        if let Some(value) = response.get(key) { metadata[key] = value.clone(); }
    }
    Ok((metadata, files))
}

pub async fn download(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    async fn run(state: ServerState, id: String, headers: HeaderMap) -> Result<Response, GatewayError> {
        validate_origin(&headers)?;
        let access = super::model_access::authenticate(&headers, "image-generation")?;
        let db = database(&state)?;
        let (job, file) = blocking::run(move || image_jobs::read_file(&db, &id)).await??;
        authorize(&job, &owner(&headers), access.scope.as_ref())?;
        Ok(super::tool_image_results::file_response(file))
    }
    match run(state, id, headers).await { Ok(response) => response, Err(error) => error.payload_response() }
}
