use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use futures::{stream, StreamExt};
use tokio::sync::oneshot;

pub(super) struct PendingUpstream {
    pub url: String,
    pub disconnected: oneshot::Receiver<()>,
    task: tokio::task::JoinHandle<()>,
}

struct OnDrop(Option<oneshot::Sender<()>>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

impl PendingUpstream {
    pub async fn start() -> Self {
        let (sender, disconnected) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |_body: Bytes| {
                let guard = OnDrop(sender.lock().unwrap().take());
                async move {
                    let first = stream::once(async {
                        Ok::<_, std::io::Error>(Bytes::from_static(
                            b"data: {\"choices\":[{\"delta\":{\"content\":\"started\"}}]}\n\n",
                        ))
                    });
                    let pending = stream::unfold(guard, |guard| async move {
                        std::future::pending::<()>().await;
                        Some((Ok::<_, std::io::Error>(Bytes::new()), guard))
                    });
                    axum::response::Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(first.chain(pending)))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            url,
            disconnected,
            task,
        }
    }
}

impl Drop for PendingUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
