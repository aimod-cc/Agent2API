use super::*;
use futures::{stream, StreamExt};

#[tokio::test]
async fn cancelling_before_waiting_does_not_lose_the_signal() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    token.cancel();
    assert!(token.is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(1), token.cancelled())
        .await
        .expect("已取消的令牌不能继续等待");
}

#[tokio::test]
async fn cancellation_wakes_all_registered_waiters() {
    let token = CancelToken::new();
    let first = token.cancelled();
    let second = token.cancelled();
    tokio::pin!(first, second);
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        tokio::join!(&mut first, &mut second);
    })
    .await
    .expect("所有等待者都应该收到取消信号");
}

#[tokio::test]
async fn normal_stream_end_does_not_wait_for_cancellation() {
    let token = Arc::new(CancelToken::new());
    let input = stream::iter(vec![Ok(Bytes::from_static(b"answer"))]).boxed();
    let mut output = cancellable(input, Some(token));
    assert_eq!(
        output.next().await.unwrap().unwrap(),
        Bytes::from_static(b"answer")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), output.next())
            .await
            .expect("上游结束后必须立即结束")
            .is_none()
    );
    assert!(output.next().await.is_none());
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_stream_and_emits_one_error() {
    let token = Arc::new(CancelToken::new());
    let mut output = cancellable(stream::pending().boxed(), Some(token.clone()));
    assert!(futures::poll!(output.next()).is_pending());
    token.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), output.next())
        .await
        .expect("取消不能等待上游发出下一帧")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), MANUAL_TERMINATED);
    assert!(output.next().await.is_none());
    assert!(output.next().await.is_none());
}

#[tokio::test]
async fn cancellation_takes_priority_over_an_already_ready_chunk() {
    let token = Arc::new(CancelToken::new());
    let input = stream::iter(vec![Ok(Bytes::from_static(b"must not be sent"))]).boxed();
    token.cancel();
    let mut output = cancellable(input, Some(token));
    assert_eq!(
        output.next().await.unwrap().unwrap_err().to_string(),
        MANUAL_TERMINATED
    );
    assert!(output.next().await.is_none());
}

#[tokio::test]
async fn no_cancel_token_preserves_bytes_and_original_errors() {
    let input = stream::iter(vec![
        Ok(Bytes::from_static(b"answer")),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "original failure",
        )),
    ])
    .boxed();
    let mut output = cancellable(input, None);
    assert_eq!(
        output.next().await.unwrap().unwrap(),
        Bytes::from_static(b"answer")
    );
    let error = output.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "original failure");
    assert!(output.next().await.is_none());
}

#[test]
fn registry_rejects_empty_ids_and_unregisters_completed_requests() {
    assert!(register("  ").is_none());
    assert!(!cancel(""));
    let id = crate::server::core::upstream::request::new_request_id();
    let token = register(&format!(" {id} ")).unwrap();
    assert!(cancel(&id));
    assert!(token.is_cancelled());
    unregister(&id);
    unregister(&id);
    assert!(!cancel(&id));
}

#[test]
fn replacing_a_registration_does_not_cancel_the_previous_request() {
    let id = crate::server::core::upstream::request::new_request_id();
    let first = register(&id).unwrap();
    let second = register(&id).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(cancel(&id));
    assert!(!first.is_cancelled());
    assert!(second.is_cancelled());
    unregister(&id);
}
