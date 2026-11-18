//! 原Response唯一读者；大块余量留在同一对象，未交付完不再读网络。
use std::sync::Arc;

use agent2api_provider_protocol as proto;
use bytes::Bytes;

use super::{error, protocol::Protocol, read::MAX_CHUNK};

pub(super) struct Reader {
    response: Option<reqwest::Response>,
    pending: Bytes,
    offset: usize,
    wire_bytes: usize,
    transport_eof: bool,
    failure: bool,
    protocol: Protocol,
    capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
}

pub(super) struct Chunk { pub(super) bytes: Bytes, pub(super) eof: bool }

impl Reader {
    pub(super) fn new(response: reqwest::Response, conversation: &str,
        capture: Option<Arc<crate::server::core::debug_traffic::TrafficCapture>>,
    ) -> Self {
        Self { response: Some(response), pending: Bytes::new(), offset: 0, wire_bytes: 0, transport_eof: false, failure: false, protocol: Protocol::new(conversation), capture }
    }
    pub(super) fn protocol(&self) -> &Protocol { &self.protocol }
    pub(super) fn transport_eof(&self) -> bool { self.transport_eof }
    pub(super) fn settlement_ready(&self) -> bool {
        self.transport_eof && !self.failure && self.response.is_none() && self.pending.is_empty() && self.protocol.complete()
    }
    /// 只有原Response正常EOF且无未交付原字节，才借原Protocol核工具声明与主张。
    pub(super) fn checked_tool_evidence(
        &self,
        material: &crate::server::core::plugin_host::wasm::conversation::CatPawMaterial,
        claims: &serde_json::Value,
    ) -> Result<Option<&super::tool_evidence::ToolEvidence>, proto::WireError> {
        if !self.settlement_ready() { return Err(super::settlement::invalid_evidence()); }
        self.protocol.checked_tool_evidence(material, claims)
    }
    /// 仅原Owner永久预约后移动原证据；原Response/尾块已结束，不复制或重造证据。
    pub(super) fn take_tool_evidence(&mut self) -> Result<super::tool_evidence::ToolEvidence, proto::WireError> {
        if !self.settlement_ready() { return Err(super::settlement::invalid_evidence()); }
        self.protocol.take_tool_evidence()
    }
    pub(super) fn discard_transport(&mut self) {
        self.failure = true;
        self.protocol.interrupted();
        self.response = None;
        self.pending = Bytes::new();
        self.offset = 0;
    }
    pub(super) async fn next(&mut self) -> Result<Chunk, proto::WireError> {
        if self.failure { return Err(error::protocol()); }
        let result = self.next_inner().await;
        if result.is_err() { self.failure = true; self.protocol.interrupted(); }
        result
    }
    async fn next_inner(&mut self) -> Result<Chunk, proto::WireError> {
        if self.transport_eof { return Err(error::read_conflict()); }
        loop {
            if self.offset < self.pending.len() {
                let end = self.offset.saturating_add(MAX_CHUNK).min(self.pending.len());
                let bytes = self.pending.slice(self.offset..end);
                // 原字节增量检查不调用插件；失败后永久封闭，不交付本块部分产物。
                self.protocol.push(&bytes)?;
                self.offset = end;
                if end == self.pending.len() { self.pending = Bytes::new(); self.offset = 0; }
                return Ok(Chunk { bytes, eof: false });
            }
            let response = self.response.as_mut().ok_or_else(error::protocol)?;
            let chunk = response.chunk().await.map_err(|_| error::protocol())?;
            match chunk {
                Some(bytes) => {
                    if let Some(capture) = &self.capture { capture.push(&bytes); }
                    self.wire_bytes = self.wire_bytes.checked_add(bytes.len()).filter(|total| *total <= 64 * 1024 * 1024).ok_or_else(error::protocol)?;
                    if bytes.is_empty() { continue; }
                    self.pending = bytes;
                    self.offset = 0;
                }
                None => {
                    // 唯一传输EOF依据是原Response正常Ok(None)，不是DONE/finished/取消。
                    self.transport_eof = true;
                    self.response = None;
                    self.protocol.finish()?;
                    return Ok(Chunk { bytes: Bytes::new(), eof: true });
                }
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(capture) = &self.capture { capture.finish(); }
    }
}
