use agent2api_provider_protocol as proto;
use base64::Engine;

use super::error;

pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Pending {
    data: Option<Box<[u8]>>,
    cursor: usize,
    memory: crate::server::core::runtime_capacity::MemoryBudget,
}

impl Pending {
    pub(super) fn store(&mut self, bytes: &[u8]) -> Result<(), proto::WireError> {
        if self.data.is_some() { return Err(error("网络分片尚未读完，不能覆盖")); }
        if bytes.len() > MAX_BYTES { return Err(error("网络分片超过缓存限制")); }
        if bytes.is_empty() { return Ok(()); }
        self.memory.resize(bytes.len())?;
        // 只复制余量，不保留上游 Bytes 的共享原始分配；定长数据容量不等于整个读取峰值。
        self.data = Some(Box::from(bytes));
        self.cursor = 0;
        Ok(())
    }

    pub(super) fn read_encoded(&mut self, limit: usize) -> Result<Option<String>, proto::WireError> {
        let Some(bytes) = self.data.as_ref() else { return Ok(None); };
        if limit == 0 { return Err(error("网络分片长度无效")); }
        let remaining = bytes.get(self.cursor..).ok_or_else(|| error("网络分片游标无效"))?;
        let size = remaining.len().min(limit);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&remaining[..size]);
        self.cursor += size;
        if self.cursor == bytes.len() { self.clear(); }
        Ok(Some(encoded))
    }

    pub(super) fn clear(&mut self) {
        self.data = None;
        self.memory.shrink(self.memory.bytes());
        self.cursor = 0;
    }
}
