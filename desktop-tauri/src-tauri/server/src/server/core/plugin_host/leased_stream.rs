use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;

use crate::server::core::providers::adapter::ProviderAdapter;
use crate::server::core::upstream::ForwardOutcome;

struct LeasedStream {
    inner: Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin>,
    _adapter: Arc<dyn ProviderAdapter>,
}

impl Stream for LeasedStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(context)
    }
}

pub(crate) fn hold_adapter(outcome: ForwardOutcome, adapter: Arc<dyn ProviderAdapter>) -> ForwardOutcome {
    match outcome {
        ForwardOutcome::Stream { status, stream } => ForwardOutcome::Stream {
            status,
            stream: Box::new(LeasedStream { inner: stream, _adapter: adapter }),
        },
        completion => completion,
    }
}
