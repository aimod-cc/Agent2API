//! 只做协议转换，不发网络请求；句柄和输入缓冲均归当前 Wasm 调用所有。

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::context::Context;
use super::super::script;

pub(super) const MAX_CONVERTERS: usize = 8;
pub(super) const MAX_CHUNK_BYTES: usize = 64 * 1024;
pub(super) const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 1024 * 1024;

fn bad_params(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "bad_params")
}

fn limited(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "plugin_resource_limit")
}

fn fields(params: &Value, required: &[&str], optional: &[&str]) -> Result<(), proto::WireError> {
    let object = params.as_object().ok_or_else(|| bad_params("协议转换参数必须是对象"))?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object.keys().any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str())) {
        return Err(bad_params("协议转换缺少参数或包含未声明字段"));
    }
    Ok(())
}

fn text<'a>(params: &'a Value, key: &str, max: usize) -> Result<&'a str, proto::WireError> {
    params[key].as_str().filter(|value| value.len() <= max && !value.chars().any(char::is_control))
        .ok_or_else(|| bad_params("协议转换文本字段无效或超过长度限制"))
}

fn protocol(params: &Value, streaming: bool) -> Result<&str, proto::WireError> {
    let value = text(params, "protocol", 32)?;
    if !matches!(value, "chat" | "anthropic" | "responses") && !(streaming && value == "gemini") {
        return Err(proto::WireError::with_code("不支持指定的协议转换", proto::WireError::UNSUPPORTED));
    }
    Ok(value)
}

fn json_size(value: &Value) -> Result<(), proto::WireError> {
    let bytes = serde_json::to_vec(value).map_err(|_| bad_params("协议转换内容无法序列化"))?;
    if bytes.len() > MAX_OUTPUT_BYTES { return Err(limited("协议转换请求或结果超过 4 MiB 限制")); }
    Ok(())
}

pub(super) struct Conversion {
    id: String,
    line: Vec<u8>,
    event_bytes: usize,
    total_bytes: usize,
}

impl Conversion {
    fn check_line(&self) -> Result<(), proto::WireError> {
        let line = std::str::from_utf8(&self.line).map_err(|_| bad_params("协议转换输入不是有效 UTF-8"))?;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            if !data.is_empty() && data != "[DONE]" {
                let value: Value = serde_json::from_str(data).map_err(|_| bad_params("协议转换收到无效的 JSON 事件"))?;
                if !value.is_object() { return Err(bad_params("协议转换事件必须是 JSON 对象")); }
            }
        }
        Ok(())
    }

    fn observe(&mut self, bytes: &[u8], finish: bool) -> Result<(), proto::WireError> {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        if self.total_bytes > MAX_STREAM_BYTES { return Err(limited("单个协议转换流输入超过 64 MiB 限制")); }
        for &byte in bytes {
            self.event_bytes = self.event_bytes.saturating_add(1);
            if self.event_bytes > MAX_EVENT_BYTES { return Err(limited("协议转换单个事件超过 1 MiB 限制")); }
            if byte == b'\n' {
                self.check_line()?;
                if self.line.is_empty() || self.line == b"\r" { self.event_bytes = 0; }
                self.line.clear();
            } else {
                self.line.push(byte);
            }
        }
        if finish && !self.line.is_empty() { self.check_line()?; self.line.clear(); }
        Ok(())
    }
}

impl Drop for Conversion {
    fn drop(&mut self) { script::close_stream(&json!({ "id": self.id })); }
}

impl Context {
    pub(super) fn protocol(&mut self, method: &str, params: &Value) -> Result<Value, proto::WireError> {
        self.permission("protocol.convert.v1")?;
        match method {
            "protocol.request" => {
                fields(params, &["protocol", "body", "model"], &[])?;
                let target_protocol = protocol(params, false)?.to_string();
                text(params, "model", 512)?;
                let body = &params["body"];
                if !body.is_object() || ["messages", "tools"].iter().any(|key|
                    body.get(*key).is_some_and(|value| value.as_array().is_none_or(|items| items.iter().any(|item| !item.is_object())))) {
                    return Err(bad_params("协议转换请求体必须是对象，消息和工具必须是对象数组"));
                }
                json_size(body)?;
                let result = script::convert_request(params)
                    .map_err(|_| bad_params("请求协议转换失败，请检查消息与工具格式"))?;
                if !result.is_object() { return Err(super::error("请求协议转换未返回对象")); }
                json_size(&result)?;
                self.note_upstream_protocol(&target_protocol, true);
                Ok(result)
            }
            "protocol.open" => {
                fields(params, &["protocol", "model"], &["session"])?;
                let protocol = protocol(params, true)?.to_string();
                let model = text(params, "model", 512)?;
                let session = if params.get("session").is_some() { text(params, "session", 1024)? } else { self.session.as_str() };
                if self.converters.len() >= MAX_CONVERTERS {
                    return Err(limited("单次调用协议转换器超过 8 个"));
                }
                // Gemini 核心转换器会记工具签名；不能把插件提供的会话串当跨账号全局键。
                let session = if protocol == "gemini" && !session.is_empty() {
                    let mut digest = Sha256::new();
                    for part in [self.shared.options.provider.as_str(), self.account.as_str(), self.identity.as_str(), session] {
                        digest.update((part.len() as u64).to_le_bytes());
                        digest.update(part.as_bytes());
                    }
                    format!("wasm:{:x}", digest.finalize())
                } else { String::new() };
                let id = format!("wasm-{}", crate::server::core::protocol::random_id("convert"));
                let value = script::open_stream(&json!({ "id": id, "protocol": protocol, "model": model, "session": session }))?;
                let converter = Conversion { id: id.clone(), line: Vec::new(), event_bytes: 0, total_bytes: 0 };
                if value["id"].as_str() != Some(id.as_str()) { return Err(super::error("协议转换句柄无效")); }
                self.converters.insert(id.clone(), converter);
                self.note_upstream_protocol(&protocol, true);
                Ok(json!({ "id": id }))
            }
            "protocol.push" | "protocol.close" => {
                let id = text(params, "id", 80)?;
                // 先移出本地登记；后续任何校验/转换错误由 Drop 释放，仅成功且未 finish 才归还。
                let mut converter = self.converters.remove(id).ok_or_else(||
                    proto::WireError::with_code("协议转换器不属于当前调用或已结束", "forbidden"))?;
                if method == "protocol.close" {
                    fields(params, &["id"], &[])?;
                    return Ok(Value::Null);
                }
                fields(params, &["id", "data", "finish"], &[])?;
                let finish = params["finish"].as_bool().ok_or_else(|| bad_params("协议转换 finish 必须是布尔值"))?;
                let encoded = params["data"].as_str().ok_or_else(|| bad_params("协议转换 data 必须是 base64 文本"))?;
                if encoded.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 { return Err(limited("协议转换输入分片超过 64 KiB")); }
                let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
                    .map_err(|_| bad_params("协议转换输入不是有效 base64"))?;
                if bytes.len() > MAX_CHUNK_BYTES { return Err(limited("协议转换输入分片超过 64 KiB")); }
                if finish && !bytes.is_empty() { return Err(bad_params("协议转换收尾必须传空数据，请先提交最后一片内容")); }
                converter.observe(&bytes, finish)?;
                let result = script::push_stream(params)?;
                let data = result["data"].as_str().ok_or_else(|| super::error("协议转换结果缺少字节内容"))?;
                if data.len() > MAX_OUTPUT_BYTES.div_ceil(3) * 4 { return Err(limited("协议转换结果超过 4 MiB 限制")); }
                let decoded = base64::engine::general_purpose::STANDARD.decode(data)
                    .map_err(|_| super::error("协议转换结果编码无效"))?;
                if decoded.len() > MAX_OUTPUT_BYTES { return Err(limited("协议转换结果超过 4 MiB 限制")); }
                if !finish { self.converters.insert(id.to_string(), converter); }
                Ok(result)
            }
            _ => Err(proto::WireError::with_code("不支持指定的协议转换操作", proto::WireError::NO_METHOD)),
        }
    }
}
