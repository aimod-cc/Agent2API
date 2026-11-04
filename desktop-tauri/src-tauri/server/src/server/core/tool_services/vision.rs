//! 视觉工具服务：配置「提供商 + 上游模型 + 思考等级 + 内部调用 Key」，
//! 运行时用内部 Key 打本进程网关完成推理。
//!
//! ── 为什么走网关回环而不是直接转发 ──────────────────────────
//! 与 OmniProxy 的视觉服务同一做法：协议转换、出站脱敏、重试、故障转移与
//! 请求日志全部复用网关既有管线，视觉服务本身只负责「把图片与提示词组装成
//! 一条 OpenAI Chat 请求」。调用方传入的 http(s) 图片会先抓下来转成 data URI，
//! 保证只认 base64 的 Anthropic 上游也能收到图片。
//!
//! ── 额度与限制记在哪 ────────────────────────────────────────
//! 记在配置里选中的那把网关 Key 上（与普通客户端请求同一口径）。Key 被删除 /
//! 停用后，详情页会明确提示「已失效」，运行时返回可读错误而不是静默失败。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::core::api_keys;

/// 客户端配置 MCP 时使用的服务名（同时作为 package_name 展示）
pub const VISION_SERVICE_NAME: &str = "vision";
pub const VISION_SERVICE_VERSION: &str = "1.0.0";

/// 入参限制：与网络搜索 MCP 的批量上限保持一致
pub const VISION_MAX_IMAGES: usize = 10;
pub const VISION_MAX_PROMPT_LENGTH: usize = 16_000;
/// 单张 URL 图片抓取的大小与超时上限
const VISION_IMAGE_FETCH_LIMIT_BYTES: usize = 20 * 1024 * 1024;
const VISION_IMAGE_FETCH_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VisionConfig {
    /// 提供商 id（如 `workbuddy` / `custom-xxx`）
    pub provider: String,
    /// 提供商展示名快照
    pub provider_label: String,
    /// 上游模型 id（目录里的原始 id）
    pub upstream_model: String,
    /// 请求网关时使用的对外模型名（保存时确定，运行时不再解析）
    pub downstream_model: String,
    /// 思考等级；空 = 不传，交给映射上的思考覆盖
    pub reasoning: String,
    /// 内部调用网关使用的网关 Key id
    pub api_key_id: String,
    /// Key 名称快照：Key 被删除后详情页仍能提示是哪个 Key 失效
    pub api_key_name: String,
}

pub fn is_reasoning_level(value: &str) -> bool {
    value.is_empty() || crate::server::core::model_rules::REASONING_LEVELS.contains(&value)
}

/// 管理端 Key 下拉选项：仅列出可用的网关 Key（启用且明文可读）
pub fn vision_key_options() -> Vec<Value> {
    api_keys::list()
        .into_iter()
        .filter(api_keys::ApiKeyEntry::is_usable)
        .map(|entry| {
            json!({
                "id": entry.id,
                "name": entry.name,
                "key_prefix": api_keys::mask(&entry.key),
            })
        })
        .collect()
}

fn key_available(key_id: &str) -> bool {
    api_keys::list()
        .into_iter()
        .find(|entry| entry.id == key_id)
        .is_some_and(|entry| entry.is_usable())
}

/// 对外（管理端）展示的配置：不返回任何 Key 密文；附当前 Key 的可用性
pub fn vision_config() -> Option<Value> {
    let config = super::load().vision?;
    let available = key_available(&config.api_key_id);
    let mut value = serde_json::to_value(&config).unwrap_or(Value::Null);
    value["configured"] = json!(true);
    value["key_available"] = json!(available);
    Some(value)
}

pub fn save_vision_config(config: VisionConfig) -> Result<Value, String> {
    if config.provider.is_empty() || config.upstream_model.is_empty() {
        return Err("请选择提供商与上游模型".into());
    }
    if !is_reasoning_level(&config.reasoning) {
        return Err("思考等级无效".into());
    }
    if config.api_key_id.is_empty() {
        return Err("请选择内部调用 API Key".into());
    }
    let available = api_keys::list()
        .into_iter()
        .find(|entry| entry.id == config.api_key_id)
        .ok_or("API Key 不存在")?;
    if !available.is_usable() {
        return Err("API Key 已停用或不可用，请重新选择".into());
    }
    let mut config = config;
    config.api_key_name = available.name.clone();
    super::update(|value| value.vision = Some(config.clone()))?;
    vision_config().ok_or_else(|| "视觉配置保存后读取失败".into())
}

pub fn clear_vision_config() {
    let _ = super::update(|value| value.vision = None);
}

/* ─── 入参校验 ─────────────────────────────────────────────── */

fn normalize_image_ref(value: &Value) -> String {
    let Some(text) = value.as_str().map(str::trim) else {
        return String::new();
    };
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return text.to_string();
    }
    if lower.starts_with("data:image/") && lower.contains(";base64,") {
        return text.to_string();
    }
    String::new()
}

/// 解析并校验 MCP 工具入参
pub fn parse_vision_args(args: &Value) -> Result<(Vec<String>, String), String> {
    let images = args.get("images").and_then(Value::as_array);
    let Some(images) = images else {
        return Err(format!("images must contain between 1 and {VISION_MAX_IMAGES} image references"));
    };
    if images.is_empty() || images.len() > VISION_MAX_IMAGES {
        return Err(format!("images must contain between 1 and {VISION_MAX_IMAGES} image references"));
    }
    let normalized: Vec<String> = images.iter().map(normalize_image_ref).collect();
    if normalized.iter().any(String::is_empty) {
        return Err("each image must be an http(s) URL or an image data URI".into());
    }
    let prompt = args.get("prompt").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
    if prompt.is_empty() {
        return Err("prompt must be a non-empty string".into());
    }
    if prompt.chars().count() > VISION_MAX_PROMPT_LENGTH {
        return Err(format!("prompt must be at most {VISION_MAX_PROMPT_LENGTH} characters"));
    }
    Ok((normalized, prompt))
}

/* ─── 运行 ─────────────────────────────────────────────────── */

async fn fetch_image_as_data_uri(url: &str) -> Result<String, String> {
    let client = crate::server::core::egress::client_for(None);
    let response = client
        .get(url)
        .timeout(Duration::from_millis(VISION_IMAGE_FETCH_TIMEOUT_MS))
        .header("user-agent", "Agent2API-VisionTool/1.0")
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "Image fetch timed out (30s)".to_string()
            } else {
                format!("Image fetch failed: {error}")
            }
        })?;
    if !response.status().is_success() {
        return Err(format!("Image fetch failed with HTTP {}", response.status().as_u16()));
    }
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !mime.starts_with("image/") {
        return Err("The fetched URL does not return an image".into());
    }
    let bytes = response.bytes().await.map_err(|error| format!("Image fetch failed: {error}"))?;
    if bytes.len() > VISION_IMAGE_FETCH_LIMIT_BYTES {
        return Err("Image exceeds the 20MB limit".into());
    }
    use base64::Engine as _;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// 运行一次视觉分析：组消息 → 抓 URL 图 → 调本进程网关 → 取回复文本
pub async fn run_vision(base_url: &str, images: Vec<String>, prompt: String) -> Result<Value, String> {
    let config = super::load().vision.ok_or("Vision model is not configured; ask the administrator to configure it")?;
    if config.downstream_model.is_empty() {
        return Err("Vision model is not configured; ask the administrator to configure it".into());
    }
    let key = api_keys::list()
        .into_iter()
        .find(|entry| entry.id == config.api_key_id && entry.is_usable())
        .ok_or("The configured API Key is unavailable (deleted or disabled); re-save the vision config")?;

    let images = futures::future::join_all(images.into_iter().map(|reference| async move {
        if reference.to_ascii_lowercase().starts_with("data:") {
            Ok(reference)
        } else {
            fetch_image_as_data_uri(&reference).await
        }
    }))
    .await;
    let mut content = vec![json!({ "type": "text", "text": prompt })];
    for image in images {
        content.push(json!({ "type": "image_url", "image_url": { "url": image? } }));
    }

    let mut body = json!({
        "model": config.downstream_model,
        "messages": [{ "role": "user", "content": content }],
        "stream": false,
    });
    if !config.reasoning.is_empty() {
        body["reasoning_effort"] = json!(config.reasoning);
    }

    let timeout = super::timeout_seconds(super::VISION_SERVICE_ID);
    let client = crate::server::core::egress::client_for(None);
    let response = client
        .post(format!("{}/v1/chat/completions", base_url.trim_end_matches('/')))
        .timeout(Duration::from_secs(timeout))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", key.key))
        .header("x-agent2api-source", "vision")
        .header("user-agent", "Agent2API-VisionTool/1.0")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                format!("Vision model request timed out ({timeout}s)")
            } else {
                "Unable to reach the Agent2API gateway".to_string()
            }
        })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let message = data
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| data.get("message").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| {
                if text.is_empty() {
                    format!("gateway returned HTTP {}", status.as_u16())
                } else {
                    text.chars().take(500).collect()
                }
            });
        return Err(message);
    }
    let reply = data
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if reply.is_empty() {
        return Err("The vision model returned an empty response".into());
    }
    Ok(json!({
        "reply": reply,
        "model": data.get("model").cloned().unwrap_or(json!(config.upstream_model)),
        "usage": data.get("usage").cloned().unwrap_or(Value::Null),
    }))
}

/// 管理端「测试连接」：用当前配置跑一次最小视觉请求（1x1 PNG）
pub async fn test_vision(base_url: &str) -> Value {
    let started = crate::server::logging::now_ms();
    let result = run_vision(
        base_url,
        vec!["data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".into()],
        "Reply with the single word: ok".into(),
    )
    .await;
    let duration = crate::server::logging::now_ms() - started;
    match result {
        Ok(payload) => json!({
            "success": true,
            "duration_ms": duration,
            "reply": payload.get("reply").and_then(Value::as_str).unwrap_or("").chars().take(500).collect::<String>(),
        }),
        Err(error) => json!({
            "success": false,
            "duration_ms": duration,
            "error": error,
        }),
    }
}
