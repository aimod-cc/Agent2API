//! Codex 官方图片生成与编辑请求整理，对照 openai/codex `codex-api/src/images.rs`。
//! 发送至 `{BASE}/images/generations` 与 `{BASE}/images/edits`，返回标准 OpenAI JSON。

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use crate::server::core::providers::adapter::ChatRequestPlan;
use crate::server::core::providers::images::{ImageOperation, ImageResponseKind};
use crate::server::errors::GatewayError;

use super::{account, http, models, request};

const MAX_EDIT_IMAGES: usize = 5;

/// 构造 Codex 订阅的图片生成或编辑请求计划。
pub(super) fn build(
    account: &Value,
    body: &Value,
    client_headers: &HeaderMap,
    operation: ImageOperation,
) -> Result<ChatRequestPlan, GatewayError> {
    let tokens = request::authorized_tokens(account)?;
    http::observe_client(client_headers);
    let fields = body
        .as_object()
        .ok_or_else(|| GatewayError::bad_request("Codex 图片请求必须是 JSON 对象"))?;
    let prompt = fields
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayError::bad_request("Codex 图片请求缺少有效的 prompt"))?;
    if prompt.chars().count() > 32_000 {
        return Err(GatewayError::bad_request("Codex prompt 最多 32000 个字符"));
    }
    reject_unsupported(fields)?;

    let model = {
        let raw = account::text(body, &["model"]);
        if raw.is_empty() {
            models::IMAGE_MODEL
        } else {
            raw
        }
    };
    let mut out = Map::new();
    out.insert("model".into(), json!(model));
    out.insert("prompt".into(), json!(prompt));
    if let Some(size) = fields
        .get("size")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        out.insert("size".into(), json!(size));
    }
    if let Some(background) = fields
        .get("background")
        .filter(|value| !value.is_null())
    {
        let value = background
            .as_str()
            .map(str::trim)
            .filter(|value| matches!(*value, "auto" | "opaque" | "transparent"))
            .ok_or_else(|| {
                GatewayError::bad_request("Codex background 仅支持 auto、opaque 或 transparent")
            })?;
        out.insert("background".into(), json!(value));
    }
    if let Some(quality) = fields.get("quality").filter(|value| !value.is_null()) {
        let mapped = map_quality(quality)?;
        out.insert("quality".into(), json!(mapped));
    }
    if let Some(n) = fields.get("n").filter(|value| !value.is_null()) {
        let n = n
            .as_u64()
            .filter(|value| (1..=10).contains(value))
            .ok_or_else(|| GatewayError::bad_request("Codex n 必须是 1 到 10 之间的整数"))?;
        if n > 1 {
            out.insert("n".into(), json!(n));
        }
    }
    let path = match operation {
        ImageOperation::Generate => "/images/generations",
        ImageOperation::Edit => {
            let images = collect_edit_images(fields)?;
            out.insert("images".into(), Value::Array(images));
            "/images/edits"
        }
    };
    let headers =
        request::build_headers(&tokens, client_headers, "application/json", false, None)?;
    let mut plan = ChatRequestPlan::images(
        format!("{}{path}", http::BASE),
        headers,
        Value::Object(out),
        ImageResponseKind::Openai,
    );
    plan.policy.respect_model_cooldown = true;
    Ok(plan)
}

fn reject_unsupported(fields: &Map<String, Value>) -> Result<(), GatewayError> {
    if fields.get("style").is_some_and(|value| !value.is_null()) {
        return Err(GatewayError::bad_request(
            "Codex 图片接口不支持 style 参数，请将风格要求写入 prompt",
        ));
    }
    if let Some(moderation) = fields.get("moderation").filter(|value| !value.is_null()) {
        if moderation.as_str() != Some("auto") {
            return Err(GatewayError::bad_request(
                "Codex 图片接口不支持调整 moderation 审核强度",
            ));
        }
    }
    for key in ["mask", "input_fidelity"] {
        if fields.get(key).is_some_and(|value| !value.is_null()) {
            return Err(GatewayError::bad_request(
                "Codex 参考图编辑不支持 mask 或 input_fidelity，请在 prompt 中描述修改范围",
            ));
        }
    }
    Ok(())
}

fn map_quality(value: &Value) -> Result<&'static str, GatewayError> {
    match value.as_str().map(str::trim) {
        Some("auto") => Ok("auto"),
        Some("low") => Ok("low"),
        Some("medium" | "standard") => Ok("medium"),
        Some("high" | "hd") => Ok("high"),
        _ => Err(GatewayError::bad_request(
            "Codex 图片 quality 仅支持 auto、low、medium、high（或 standard/hd）",
        )),
    }
}

fn collect_edit_images(fields: &Map<String, Value>) -> Result<Vec<Value>, GatewayError> {
    let images = fields
        .get("images")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| GatewayError::bad_request("Codex 图片编辑需要至少 1 张参考图"))?;
    if images.len() > MAX_EDIT_IMAGES {
        return Err(GatewayError::bad_request(format!(
            "Codex 参考图编辑最多支持 {MAX_EDIT_IMAGES} 张参考图"
        )));
    }
    images
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| {
                GatewayError::bad_request("Codex 参考图条目必须是 JSON 对象")
            })?;
            if let Some(url) = object
                .get("image_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Ok(json!({ "image_url": url }));
            }
            if let Some(file_id) = object
                .get("file_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Ok(json!({ "file_id": file_id }));
            }
            Err(GatewayError::bad_request("Codex 参考图缺少有效的 image_url"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::providers::adapter::UpstreamResponse;

    fn sample_account() -> Value {
        json!({
            "secret": {
                "authMode": "chatgpt",
                "accessToken": "eyJhbGciOiJub25lIn0.e30.sig",
                "accountId": "acct_123456",
            }
        })
    }

    #[test]
    fn builds_generation_plan_and_strips_gateway_only_fields() {
        let body = json!({
            "model": "gpt-image-2",
            "prompt": "画一只戴墨镜的橘猫",
            "n": 1,
            "size": "1024x1536",
            "quality": "hd",
            "background": "transparent",
            "response_format": "b64_json",
            "output_format": "webp",
            "output_compression": 85,
            "moderation": "auto",
            "user": "client-user"
        });
        let plan = build(
            &sample_account(),
            &body,
            &HeaderMap::new(),
            ImageOperation::Generate,
        )
        .expect("生成图片请求计划应构造成功");
        assert_eq!(plan.url, "https://chatgpt.com/backend-api/codex/images/generations");
        assert_eq!(
            plan.response,
            UpstreamResponse::Images(ImageResponseKind::Openai)
        );
        assert!(plan.policy.respect_model_cooldown);
        assert_eq!(
            plan.body,
            json!({
                "model": "gpt-image-2",
                "prompt": "画一只戴墨镜的橘猫",
                "size": "1024x1536",
                "quality": "high",
                "background": "transparent",
            })
        );
        assert!(plan
            .headers
            .iter()
            .any(|(k, v)| k == "Accept" && v == "application/json"));
        assert!(!plan.headers.iter().any(|(k, _)| k == "OpenAI-Beta"));
    }

    #[test]
    fn builds_edit_plan_and_enforces_five_image_limit() {
        let ok_body = json!({
            "model": "gpt-image-2",
            "prompt": "改成夜景",
            "n": 2,
            "quality": "standard",
            "images": [{ "image_url": "data:image/png;base64,Zm9v" }]
        });
        let plan = build(
            &sample_account(),
            &ok_body,
            &HeaderMap::new(),
            ImageOperation::Edit,
        )
        .expect("编辑图片请求计划应构造成功");
        assert_eq!(plan.url, "https://chatgpt.com/backend-api/codex/images/edits");
        assert_eq!(
            plan.body,
            json!({
                "model": "gpt-image-2",
                "prompt": "改成夜景",
                "n": 2,
                "quality": "medium",
                "images": [{ "image_url": "data:image/png;base64,Zm9v" }]
            })
        );

        let too_many = json!({
            "model": "gpt-image-2",
            "prompt": "合并参考图",
            "images": vec![json!({ "image_url": "data:image/png;base64,Zm9v" }); 6]
        });
        let err = build(
            &sample_account(),
            &too_many,
            &HeaderMap::new(),
            ImageOperation::Edit,
        )
        .err()
        .expect("超过 5 张参考图应拒绝");
        assert_eq!(err.status_code, 400);
    }

    #[test]
    fn rejects_unsupported_image_parameters() {
        for bad in [
            json!({ "model": "gpt-image-2", "prompt": "cat", "style": "vivid" }),
            json!({ "model": "gpt-image-2", "prompt": "cat", "moderation": "low" }),
            json!({ "model": "gpt-image-2", "prompt": "cat", "quality": "xhigh" }),
        ] {
            let err = build(
                &sample_account(),
                &bad,
                &HeaderMap::new(),
                ImageOperation::Generate,
            )
            .err()
            .expect("不支持的参数应返回 400");
            assert_eq!(err.status_code, 400);
        }
    }
}
