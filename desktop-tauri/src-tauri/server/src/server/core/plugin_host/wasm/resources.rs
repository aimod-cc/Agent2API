use std::collections::HashMap;
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

const MAX_BYTES: usize = 96 * 1024 * 1024;
const MAX_RESOURCES: usize = 16;

#[derive(Default)]
pub(super) struct Resources {
    entries: HashMap<String, Resource>,
    allocated: Arc<AtomicUsize>,
    memory: crate::server::core::runtime_capacity::MemoryBudget,
    // 已读取的公网下载字节不可退还；失败或删除资源都不能重置本次调用的预算。
    public_downloaded: usize,
}

struct Resource {
    mime: String,
    data: ResourceData,
}

enum ResourceData {
    Building(TrackedBuffer),
    Sealed(bytes::Bytes),
}

struct TrackedBuffer {
    bytes: Vec<u8>,
    allocated: Arc<AtomicUsize>,
    memory: crate::server::core::runtime_capacity::MemoryBudget,
}

impl TrackedBuffer {
    fn new(bytes: Vec<u8>, allocated: &Arc<AtomicUsize>, memory: &crate::server::core::runtime_capacity::MemoryBudget) -> Result<Self, proto::WireError> {
        memory.grow(bytes.len())?;
        allocated.fetch_add(bytes.len(), Ordering::AcqRel);
        Ok(Self { bytes, allocated: allocated.clone(), memory: memory.clone() })
    }
}

impl AsRef<[u8]> for TrackedBuffer {
    fn as_ref(&self) -> &[u8] { &self.bytes }
}

impl Drop for TrackedBuffer {
    fn drop(&mut self) {
        self.allocated.fetch_sub(self.bytes.len(), Ordering::AcqRel);
        self.memory.shrink(self.bytes.len());
    }
}

impl Resource {
    fn bytes(&self) -> &[u8] {
        match &self.data {
            ResourceData::Building(buffer) => &buffer.bytes,
            ResourceData::Sealed(bytes) => bytes,
        }
    }

    fn seal(&mut self) {
        if matches!(&self.data, ResourceData::Building(_)) {
            if let ResourceData::Building(buffer) = std::mem::replace(&mut self.data, ResourceData::Sealed(bytes::Bytes::new())) {
                self.data = ResourceData::Sealed(bytes::Bytes::from_owner(buffer));
            }
        }
    }
}

impl Resources {
    pub fn create(&mut self, mime: &str) -> Result<Value, proto::WireError> {
        if self.entries.len() >= MAX_RESOURCES || !matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "application/octet-stream") {
            return Err(super::error("资源数量或类型无效"));
        }
        let id = crate::server::core::protocol::random_id("blob");
        self.entries.insert(id.clone(), Resource { mime: mime.into(), data: ResourceData::Building(TrackedBuffer::new(Vec::new(), &self.allocated, &self.memory)?) });
        Ok(json!({ "id": id }))
    }

    pub async fn fetch_public(&mut self, url: &str, ordinary: super::managed_task::OrdinaryPolicy) -> Result<Value, proto::WireError> {
        if self.entries.len() >= MAX_RESOURCES {
            return Err(proto::WireError::with_code("单次调用资源数量超过 16 个", "plugin_resource_limit"));
        }
        let remaining = MAX_BYTES.saturating_sub(self.allocated.load(Ordering::Acquire));
        // 下载、检查全部完成后才登记资源；超时、取消和错误不会留下半成品。
        let image = super::resource_fetch::download(url, remaining, &mut self.public_downloaded, ordinary).await?;
        let size = image.bytes.len();
        let id = crate::server::core::protocol::random_id("blob");
        self.entries.insert(id.clone(), Resource { mime: image.mime.into(), data: ResourceData::Sealed(bytes::Bytes::from_owner(TrackedBuffer::new(image.bytes, &self.allocated, &self.memory)?)) });
        Ok(json!({ "id": id, "size": size, "mime": image.mime }))
    }

    /// 只消费本次可信入场已解码的图片；不接受 JSON ID、外部 Vec 或插件调用。
    /// 通用二进制存储 GIF 原字节，块内仍给真实图片 MIME；不开放图片输出或公网下载格式。
    pub(super) fn seal_inbound(&mut self, image: super::inbound_images::SealedImage) -> Result<Value, proto::WireError> {
        let (mime, bytes) = image.into_parts();
        let size = bytes.len();
        let stored_mime = match mime {
            "image/png" | "image/jpeg" | "image/webp" => mime,
            "image/gif" => "application/octet-stream",
            _ => return Err(super::error("宿主入场图片类型无效")),
        };
        if size == 0 || size > super::inbound_images::MAX_IMAGE_BYTES || self.entries.len() >= MAX_RESOURCES
            || self.allocated.load(Ordering::Acquire).checked_add(size).is_none_or(|total| total > MAX_BYTES) {
            return Err(proto::WireError::with_code("宿主入场图片超过资源数量或字节上限", "plugin_resource_limit"));
        }
        // 新入场不用弱唯一性的时间戳兜底，随机源失败整笔拒绝，不覆盖已有资源。
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).map_err(|_| super::error("无法生成本次图片资源标识"))?;
        let id = format!("blob_{}", random.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
        if self.entries.contains_key(&id) { return Err(super::error("本次图片资源标识重复")); }
        self.entries.insert(id.clone(), Resource { mime: stored_mime.into(), data: ResourceData::Sealed(bytes::Bytes::from_owner(TrackedBuffer::new(bytes, &self.allocated, &self.memory)?)) });
        Ok(json!({ "type": "image_resource", "resource": id, "mime": mime, "size": size }))
    }

    pub fn append(&mut self, id: &str, data: &str) -> Result<Value, proto::WireError> {
        if data.len() > 96 * 1024 { return Err(super::error("资源分片过大")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)
            .map_err(|_| super::error("资源分片编码无效"))?;
        if self.allocated.load(Ordering::Acquire).saturating_add(bytes.len()) > MAX_BYTES {
            return Err(proto::WireError::with_code("单次调用资源总量超过 96 MiB", "plugin_resource_limit"));
        }
        let resource = self.entries.get_mut(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        let ResourceData::Building(buffer) = &mut resource.data else { return Err(super::error("资源已封存")); };
        buffer.memory.grow(bytes.len())?;
        buffer.bytes.extend_from_slice(&bytes);
        buffer.allocated.fetch_add(bytes.len(), Ordering::AcqRel);
        Ok(json!({ "size": buffer.bytes.len() }))
    }

    pub fn seal(&mut self, id: &str) -> Result<Value, proto::WireError> {
        let resource = self.entries.get_mut(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        if resource.bytes().is_empty() { return Err(super::error("不能封存空资源")); }
        resource.seal();
        Ok(json!({ "id": id, "size": resource.bytes().len(), "mime": resource.mime }))
    }

    pub fn read(&self, id: &str, offset: usize, length: usize) -> Result<Value, proto::WireError> {
        let resource = self.entries.get(id).ok_or_else(|| super::error("资源不属于当前调用"))?;
        let bytes = resource.bytes();
        if offset > bytes.len() || length == 0 || length > 64 * 1024 {
            return Err(super::error("资源读取范围无效"));
        }
        let end = offset.saturating_add(length).min(bytes.len());
        Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]),
            "done": end == bytes.len(), "size": bytes.len() }))
    }

    pub fn remove(&mut self, id: &str) {
        // HTTP上传仍引用原字节时不退还内存预算，最后一份引用释放后才归还。
        self.entries.remove(id);
    }

    pub fn raw(&self, id: &str) -> Result<bytes::Bytes, proto::WireError> {
        let resource = self.entries.get(id).ok_or_else(|| super::error("资源未完成或不属于当前调用"))?;
        match &resource.data {
            ResourceData::Sealed(bytes) => Ok(bytes.clone()),
            ResourceData::Building(_) => Err(super::error("资源未完成或不属于当前调用")),
        }
    }

    pub fn image(&self, id: &str) -> Result<String, proto::WireError> {
        let resource = self.entries.get(id).filter(|resource| matches!(&resource.data, ResourceData::Sealed(_)) && resource.mime.starts_with("image/"))
            .ok_or_else(|| super::error("图片资源未完成或不属于当前调用"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(resource.bytes()))
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
