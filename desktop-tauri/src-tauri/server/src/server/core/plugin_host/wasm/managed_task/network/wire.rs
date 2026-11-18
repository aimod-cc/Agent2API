//! 统计reqwest::Upgraded输出的实际升级流字节（含帧头/控制帧/未知内容），不是文本长度。
//! 计的是TLS解密后的升级字节，不含TLS/TCP开销或HTTP握手；到限直接报错，不伪装EOF。

use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
    time::Instant,
};

use super::super::{lifecycle::Record, status::EndReason};
use super::observation::{TextChunk, MAX_UPGRADED_BYTES};

pub(super) struct CountedUpgrade {
    inner: reqwest::Upgraded,
    record: Arc<Record>,
    output: mpsc::Sender<TextChunk>,
    deadline: Instant,
    remaining: usize,
    exhausted: bool,
    yield_read: bool,
}

impl CountedUpgrade {
    pub(super) fn new(
        inner: reqwest::Upgraded,
        record: &Arc<Record>,
        output: &mpsc::Sender<TextChunk>,
    ) -> Self {
        Self {
            inner,
            record: record.clone(),
            output: output.clone(),
            deadline: record.call_deadline(),
            remaining: MAX_UPGRADED_BYTES,
            exhausted: false,
            yield_read: false,
        }
    }

    pub(super) fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }
    pub(super) fn exhausted(&self) -> bool {
        self.exhausted
    }

    fn check(&self) -> io::Result<()> {
        if self.output.is_closed() {
            self.record.request_stop(EndReason::ReceiverDropped);
        }
        if self.record.end_reason().is_some() {
            Err(io::Error::other("managed observation closed"))
        } else if Instant::now() >= self.deadline {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "managed observation deadline",
            ))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for CountedUpgrade {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check() {
            return Poll::Ready(Err(error));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.remaining == 0 {
            this.exhausted = true;
            return Poll::Ready(Err(io::Error::other(
                "managed observation byte budget exhausted",
            )));
        }
        if std::mem::take(&mut this.yield_read) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        // 用有界子ReadBuf约束底层实际交付长度，包括hyper在101后预读的升级字节。
        // 每16KiB让出一次poll，碎片帧洪流也会回到外层select检查停止。
        let limit = this.remaining.min(buf.remaining()).min(16 * 1024);
        let mut limited = ReadBuf::new(&mut buf.initialize_unfilled()[..limit]);
        let result = Pin::new(&mut this.inner).poll_read(cx, &mut limited);
        let received = limited.filled().len();
        buf.advance(received);
        this.remaining -= received;
        this.yield_read = received > 0;
        result
    }
}

impl AsyncWrite for CountedUpgrade {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Err(error) = this.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
