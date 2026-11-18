use std::pin::Pin;
use std::task::{Context, Poll};

use agent2api_provider_protocol as proto;
use tokio::sync::mpsc::UnboundedReceiver;

use super::StreamEvent;
use crate::server::core::runtime_capacity::Allocation;

pub(crate) struct StreamEvents {
    inner: UnboundedReceiver<StreamEvent>,
    _memory: Option<Allocation>,
}

impl StreamEvents {
    pub(super) fn wasm(inner: UnboundedReceiver<StreamEvent>) -> Result<Self, proto::WireError> {
        Ok(Self { inner, _memory: Some(Allocation::new(512 * 1024)?) })
    }

    pub(crate) async fn recv(&mut self) -> Option<StreamEvent> { self.inner.recv().await }

    pub(crate) fn poll_recv(&mut self, context: &mut Context<'_>) -> Poll<Option<StreamEvent>> {
        Pin::new(&mut self.inner).poll_recv(context)
    }
}

impl From<UnboundedReceiver<StreamEvent>> for StreamEvents {
    fn from(inner: UnboundedReceiver<StreamEvent>) -> Self { Self { inner, _memory: None } }
}
