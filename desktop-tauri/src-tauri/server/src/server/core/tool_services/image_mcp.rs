use serde_json::{json, Value};
use super::{image_config, McpRequestContext, McpSpec};

pub fn spec(editing: bool) -> McpSpec {
    let config = image_config::load().unwrap_or_default();
    let retention = config.retention_hours.clamp(1, 168);
    let mut tools = vec![generation_tool(false, config.max_images.clamp(1, 10), retention)];
    if editing { tools.push(generation_tool(true, config.max_images.clamp(1, 10), retention)); }
    tools.push(json!({
        "name": "get_image_result", "title": "Get generated images",
        "description": "Query an existing image job without generating again. Use job_id or the original request_id. Results expire after the retention period returned by the job. Never submit another generation automatically when the status is uncertain, failed or expired.",
        "inputSchema": { "type": "object", "properties": {
            "job_id": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Job ID returned by generation." },
            "request_id": { "type": "string", "minLength": 8, "maxLength": 128, "description": "Original submission identifier; use this when the submission response was lost." },
            "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 20, "description": "Wait up to this many seconds for completion, default 0." },
            "preview": { "type": "boolean", "description": "Include a small image preview, default false." }
        }, "anyOf": [{ "required": ["job_id"] }, { "required": ["request_id"] }], "additionalProperties": false },
        "annotations": { "readOnlyHint": true, "idempotentHint": true, "openWorldHint": false }
    }));
    McpSpec {
        server_info: json!({ "name": super::package_name(super::IMAGE_GENERATION_SERVICE_ID), "title": "Image Generation", "version": super::IMAGE_GENERATION_VERSION }),
        instructions: "Generate images using accounts configured in Agent2API. Generation consumes upstream credits. Use a unique request_id for each user-approved generation and reuse it for transport retries; poll get_image_result for running jobs. Never invent local file paths: HTTP results are authenticated download URLs, not files on the client machine. Model and account selection are controlled by the administrator.",
        tools,
    }
}

fn generation_tool(editing: bool, count: u32, retention: u32) -> Value {
    let mut properties = json!({
        "request_id": { "type": "string", "minLength": 8, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$",
            "description": "Unique identifier (8-128 ASCII letters, digits, hyphens or underscores). Reuse it after a timeout; use a new identifier only for a new user-approved generation." },
        "prompt": { "type": "string", "minLength": 1, "maxLength": 32000, "description": "Complete image requirements: subject, composition, style, text and changes to references." },
        "n": { "type": "integer", "minimum": 1, "maximum": count, "description": "Image count, default 1. Some providers only accept 1; the service never makes extra calls to fill a batch." },
        "size": { "type": "string", "description": "Optional auto or WIDTHxHEIGHT, subject to provider limits." },
        "quality": { "type": "string", "description": "Optional provider-supported quality. Omit unless known to be supported." },
        "output_format": { "type": "string", "enum": ["png", "jpeg", "webp"], "description": "File encoding; default png." },
        "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 20, "description": "Wait up to this many seconds, default 10; running jobs can be queried later." },
        "preview": { "type": "boolean", "description": "Include a small image preview on completion, default false." }
    });
    let mut required = vec!["request_id", "prompt"];
    if editing {
        properties["images"] = json!({ "type": "array", "minItems": 1, "maxItems": 16,
            "items": { "type": "string", "minLength": 1 },
            "description": "Ordered references: image:<id> from a previous result, public HTTP(S) URLs, or image data URIs. HTTP mode cannot read local file paths. PNG/JPEG/WebP only." });
        required.push("images");
    }
    json!({
        "name": if editing { "edit_images" } else { "generate_images" },
        "title": if editing { "Edit images" } else { "Generate images" },
        "description": format!("{} Results are retained for {retention} hours; submission identifiers are deduplicated for that retention period plus seven days. Never reuse an old request_id for a new generation.", if editing {
            "Edit reference images with the configured model. Consumes credits. Returns the image files or a running job; poll get_image_result without submitting again. No masks, exact pixel preservation or streaming output."
        } else {
            "Generate images from text with the configured model and accounts. Consumes credits. Returns image files or a running job; poll get_image_result without submitting again. Does not change the agent's chat model."
        }),
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
    })
}

pub async fn call(name: &str, args: &Value, ctx: &McpRequestContext) -> Value {
    match crate::server::api::tool_images::call(name, args, ctx).await {
        Ok(value) => value,
        Err(error) => {
            let payload = json!({ "status": "error", "code": error.code, "message": error.message,
                "request_id": args.get("request_id"), "job_id": args.get("job_id"),
                "guidance": "Do not automatically submit a new generation. Keep the original request_id; use get_image_result after an uncertain submission." });
            json!({ "resultType": "complete", "isError": true,
                "content": [{ "type": "text", "text": payload.to_string() }], "structuredContent": payload })
        }
    }
}
