use super::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tokio::sync::Notify;

#[tokio::test]
async fn cancellation_at_or_during_send_stops_real_http_for_both_paths() {
    for path in ["/v1/chat/completions", "/v1/responses"] {
        let hits = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Notify::new());
        let count = hits.clone(); let signal = received.clone();
        let app = axum::Router::new().route(path, axum::routing::post(move || {
            let count = count.clone(); let signal = signal.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst); signal.notify_one();
                std::future::pending::<&'static str>().await
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = TransportRequest { url: format!("http://{}{path}", listener.local_addr().unwrap()),
            headers: vec![("Content-Type".into(), "application/json".into())], payload: "{}".into(),
            proxy: None, system_proxy_when_unset: false };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let telemetry = Arc::new(RequestTelemetry::new());
        let id = crate::server::core::upstream::request::new_request_id();
        let cancel = cancellation::register(&id).unwrap(); telemetry.set_cancel_token(cancel.clone());
        // The pending-send case first proves this HTTP fixture really receives POSTs.
        let sending_telemetry = telemetry.clone(); let sending_transport = transport.clone();
        let sending = tokio::spawn(async move { send_or_cancel(&sending_transport, &sending_telemetry, true).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), received.notified()).await.unwrap();
        cancel.cancel();
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), sending).await.unwrap().unwrap().unwrap_err();
        assert_eq!(error.message, cancellation::MANUAL_TERMINATED);
        assert_eq!(error.status_code, cancellation::MANUAL_TERMINATED_STATUS as i32);
        assert!(!error.unconfirmed_inference);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Simulates cancellation after credential/request preparation but before sending.
        let error = send_or_cancel(&transport, &telemetry, true).await.unwrap_err();
        assert_eq!(error.message, cancellation::MANUAL_TERMINATED);
        assert!(!error.unconfirmed_inference);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1, "cancelled {path} was sent again");
        cancellation::unregister(&id); server.abort();
    }
}
