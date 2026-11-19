use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct DroppingUpstream {
    url: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl DroppingUpstream {
    async fn new(failures: usize, timeout: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let count = count.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    loop {
                        let length = stream.read(&mut buffer).await.unwrap_or(0);
                        if length == 0 { return; }
                        bytes.extend_from_slice(&buffer[..length]);
                        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length: usize = headers.lines().find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok()).flatten()
                            }).unwrap_or(0);
                            if bytes.len() >= end + 4 + length { break; }
                        }
                    }
                    let hit = count.fetch_add(1, Ordering::SeqCst);
                    if hit < failures {
                        // The server accepted the complete POST, then closed before headers.
                        if timeout { tokio::time::sleep(std::time::Duration::from_millis(1300)).await; }
                        let _ = stream.shutdown().await;
                        return;
                    }
                    let body = format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({"id":"retry-ok","object":"chat.completion.chunk","model":"transport-model","choices":[{"index":0,"delta":{"content":"recovered"},"finish_reason":null}]}),
                        json!({"id":"retry-ok","object":"chat.completion.chunk","model":"transport-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}));
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { url, hits, task }
    }
}

impl Drop for DroppingUpstream {
    fn drop(&mut self) { self.task.abort(); }
}

#[test]
fn unconfirmed_inference_real_connection_retry_matrix() {
    crate::test_support::isolated(concat!(module_path!(), "::unconfirmed_inference_real_connection_retry_matrix"), || {
        crate::test_support::runtime().block_on(async {
            for (enabled, failures, timeout, expected_status, expected_hits) in [
                (false, 1, false, StatusCode::BAD_GATEWAY, 1),
                (true, 1, false, StatusCode::OK, 2),
                (true, 20, false, StatusCode::BAD_GATEWAY, 3),
                (false, 1, true, StatusCode::BAD_GATEWAY, 1),
                (true, 1, true, StatusCode::OK, 2),
            ] {
                let h = Harness::new();
                let upstream = DroppingUpstream::new(failures, timeout).await;
                config::set_timeouts(config::TimeoutPatch {
                    connect_seconds: Some(3), headers_seconds: Some(1), stream_idle_seconds: Some(3), body_seconds: Some(5),
                });
                let mock = Mock::new().await;
                let provider = format!("custom-unconfirmed-{enabled}-{failures}");
                h.provider(&provider, "chat_completions", &mock,
                    json!([{"id":"transport-model","enabled":true}]), json!([]));
                core::custom_providers::update(&provider, &json!({"baseUrl": format!("{}/v1", upstream.url)})).unwrap();
                h.key("transport-test-key", &[&provider], &[]);
                let mut settings = config::error_policy::ErrorSettings::default();
                settings.resends = 2; settings.switches = 0; settings.interval_seconds = 0;
                settings.backoff = false; settings.breaker = false;
                settings.rules.iter_mut().find(|rule| rule.kind == config::error_policy::ErrorKind::Transient).unwrap().retry_unconfirmed = enabled;
                let revision = config::error_policy::revision(&config::error_policy::snapshot());
                let saved = config::error_policy::save(settings, &revision).unwrap();
                assert_eq!(saved.settings.rules.iter().find(|rule| rule.kind == config::error_policy::ErrorKind::Transient).unwrap().retry_unconfirmed, enabled);
                let (status, _, bytes) = h.call("/v1/chat/completions", "transport-test-key",
                    json!({"model":"transport-model","stream":true,"messages":[{"role":"user","content":"test"}]})).await;
                assert_eq!(status, expected_status, "{}", String::from_utf8_lossy(&bytes));
                assert_eq!(upstream.hits.load(Ordering::SeqCst), expected_hits);
                if status == StatusCode::OK { assert!(String::from_utf8_lossy(&bytes).contains("recovered")); }
                let rows = h.rows();
                let row = rows.iter().find(|row| row["model"] == "transport-model").unwrap();
                assert_eq!(row["attempts"], 1, "{row}");
                assert_eq!(row["attemptDetails"][0]["retries"].as_array().unwrap().len(), expected_hits - 1, "{row}");
                if !enabled {
                    assert_eq!(row["attemptDetails"][0]["errorPolicy"]["configuredAction"], "retry_then_switch");
                    assert_eq!(row["attemptDetails"][0]["errorPolicy"]["action"], "return_error");
                }
            }
        });
    });
}
