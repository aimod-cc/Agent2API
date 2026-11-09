use std::collections::HashMap;
use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

const MAX_BYTES: usize = 96 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Resources {
    entries: HashMap<String, Resource>,
    bytes: usize,
}

struct Resource {
    mime: String,
    bytes: Vec<u8>,
    sealed: bool,
}

impl Resources {
    pub fn create(&mut self, mime: &str) -> Result<Value, proto::WireError> {
        if self.entries.len() >= 16 || !matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "application/octet-stream") {
            return Err(super::error("资源数量或类型无效"));
        }
        let id = crate::server::core::protocol::random_id("blob");
        self.entries.insert(id.clone(), Resource { mime: mime.into(), bytes: Vec::new(), sealed: false });
        Ok(json!({ "id": id }))
    }

    pub fn append(&mut self, id: &str, data: &str) -> Result<Value, proto::WireError> {
        if data.len() > 96 * 1024 { return Err(super::error("资源分片过大")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)
            .map_err(|_| super::error("资源分片编码无效"))?;
        if self.bytes.saturating_add(bytes.len()) > MAX_BYTES {
            return Err(proto::WireError::with_code("单次调用资源总量超过 96 MiB", "plugin_resource_limit"));
        }
        let resource = self.entries.get_mut(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        if resource.sealed { return Err(super::error("资源已封存")); }
        self.bytes += bytes.len();
        resource.bytes.extend_from_slice(&bytes);
        Ok(json!({ "size": resource.bytes.len() }))
    }

    pub fn seal(&mut self, id: &str) -> Result<Value, proto::WireError> {
        let resource = self.entries.get_mut(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        if resource.bytes.is_empty() { return Err(super::error("不能封存空资源")); }
        resource.sealed = true;
        Ok(json!({ "id": id, "size": resource.bytes.len(), "mime": resource.mime }))
    }

    pub fn read(&self, id: &str, offset: usize, length: usize) -> Result<Value, proto::WireError> {
        let resource = self.entries.get(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        if offset > resource.bytes.len() || length == 0 || length > 64 * 1024 {
            return Err(super::error("资源读取范围无效"));
        }
        let end = offset.saturating_add(length).min(resource.bytes.len());
        Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&resource.bytes[offset..end]),
            "done": end == resource.bytes.len(), "size": resource.bytes.len() }))
    }

    pub fn remove(&mut self, id: &str) {
        if let Some(resource) = self.entries.remove(id) { self.bytes = self.bytes.saturating_sub(resource.bytes.len()); }
    }

    pub fn raw(&self, id: &str) -> Result<bytes::Bytes, proto::WireError> {
        let resource = self.entries.get(id).filter(|resource| resource.sealed)
            .ok_or_else(|| super::error("资源未完成或不属于当前调用"))?;
        Ok(bytes::Bytes::copy_from_slice(&resource.bytes))
    }

    pub fn image(&self, id: &str) -> Result<String, proto::WireError> {
        let resource = self.entries.get(id).filter(|resource| resource.sealed && resource.mime.starts_with("image/"))
            .ok_or_else(|| super::error("图片资源未完成或不属于当前调用"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&resource.bytes))
    }
}

pub(super) fn materialize(value: &mut Value, resources: &Resources, options: &super::manifest::Options) -> Result<(), proto::WireError> {
    let Some(result) = value.get_mut("result") else { return Ok(()) };
    let data = result.get_mut("data").and_then(Value::as_array_mut)
        .filter(|data| !data.is_empty() && data.len() <= 16)
        .ok_or_else(|| super::errors::protocol())?;
    let mut used = std::collections::HashSet::new();
    let mut total = 0usize;
    for image in data {
        let map = image.as_object_mut().ok_or_else(super::errors::protocol)?;
        let count = ["resource", "b64_json", "url"].iter().filter(|key| map.contains_key(**key)).count();
        if count != 1 { return Err(super::errors::protocol()); }
        if let Some(id) = map.remove("resource") {
            let id = id.as_str().ok_or_else(super::errors::protocol)?;
            if !used.insert(id.to_string()) { return Err(super::errors::protocol()); }
            let data = resources.image(id)?;
            total = total.saturating_add(data.len());
            if total > MAX_BYTES.div_ceil(3) * 4 { return Err(super::error("图片结果总量超过限制")); }
            map.insert("b64_json".into(), json!(data));
        } else if let Some(url) = map.get("url") {
            options.check_url(url.as_str().ok_or_else(super::errors::protocol)?, false)?;
        } else if !map.get("b64_json").and_then(Value::as_str).is_some_and(|data| !data.is_empty()
            && base64::engine::general_purpose::STANDARD.decode(data).is_ok()) {
            return Err(super::errors::protocol());
        }
    }
    Ok(())
}
