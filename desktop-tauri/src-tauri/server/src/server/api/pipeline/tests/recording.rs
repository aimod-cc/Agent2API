use super::{
    terminal, RecordContext, RecordingStream, TerminalFrames, LAST_CHUNK, OLD_CHAT, UNSPACED_DONE,
};
use std::sync::Arc;

use axum::body::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;

use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::db::Db;
use crate::server::request_stats::{RequestQuery, RequestStats, Retention};

fn temp_stats() -> (tempfile::TempDir, Arc<RequestStats>) {
    let dir = tempfile::tempdir().expect("应当能创建独立临时目录");
    let db = Db::open(&dir.path().join("agent2api.db")).expect("临时库应当能建起来");
    (
        dir,
        Arc::new(RequestStats::with_db(Some(db), Retention::default)),
    )
}

fn context_for(stats: Arc<RequestStats>) -> (RecordContext, Arc<RequestTelemetry>) {
    let telemetry = Arc::new(RequestTelemetry::new());
    telemetry.report_usage(&serde_json::json!({
        "prompt_tokens": 71476, "completion_tokens": 56, "total_tokens": 71532
    }));
    let context = RecordContext {
        stats,
        telemetry: telemetry.clone(),
        started_at: crate::server::logging::now_ms(),
        model: "test-model".to_string(),
        client_model: "test-model".to_string(),
        client_reasoning: String::new(),
        is_test: false,
        source: Default::default(),
        status: 200,
        raw_request: None,
        raw_response: None,
    };
    (context, telemetry)
}

fn stream_ending_at(
    frames: Vec<&'static [u8]>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin {
    futures::stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok(Bytes::from_static(frame))),
    )
    .chain(futures::stream::pending())
}

fn only_row(stats: &RequestStats) -> Value {
    let page = stats.query_requests(&RequestQuery {
        limit: Some(5),
        ..Default::default()
    });
    let entries = page["entries"].as_array().expect("查询应当返回明细数组");
    assert_eq!(page["total"], 1, "一次请求只能记录一次：{page}");
    assert_eq!(entries.len(), 1, "应当恰好落一条明细：{page}");
    entries[0].clone()
}

async fn record_after_hangup(frames: &[&'static [u8]], terminals: TerminalFrames) -> Value {
    let (_dir, stats) = temp_stats();
    let (context, _telemetry) = context_for(stats.clone());
    let mut stream = RecordingStream::with_terminals(
        Box::new(stream_ending_at(frames.to_vec())),
        context,
        terminals,
    );
    for frame in frames {
        assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), *frame);
    }
    let still_pending =
        futures::future::poll_fn(|cx| match std::pin::Pin::new(&mut stream).poll_next(cx) {
            std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
            std::task::Poll::Pending => std::task::Poll::Ready(true),
        })
        .await;
    assert!(still_pending, "客户端挂断前上游流仍应当保持连接");
    drop(stream);
    only_row(&stats)
}

fn assert_success(row: &Value) {
    assert_eq!(row["status"], 200);
    assert!(row["error"].is_null(), "正常结束不该记成中断：{row}");
    assert_eq!(row["promptTokens"], 71476);
    assert_eq!(row["completionTokens"], 56);
    assert_eq!(row["totalTokens"], 71532);
}

#[tokio::test]
async fn hangup_right_after_unspaced_done_records_success_with_usage() {
    let row = record_after_hangup(
        &[LAST_CHUNK.as_bytes(), UNSPACED_DONE.as_bytes()],
        terminal::CHAT,
    )
    .await;
    assert_success(&row);
    assert!(row["firstResponseMs"].as_i64().is_some_and(|ms| ms >= 0));
}

#[tokio::test]
async fn the_old_needle_would_have_recorded_the_same_stream_as_an_abort() {
    let row =
        record_after_hangup(&[LAST_CHUNK.as_bytes(), UNSPACED_DONE.as_bytes()], OLD_CHAT).await;
    assert!(row["error"].as_str().unwrap().contains("未完整下发"));
    assert_eq!(row["promptTokens"], 0);
}

#[tokio::test]
async fn hangup_before_the_terminal_frame_stays_an_abort() {
    let row = record_after_hangup(&[LAST_CHUNK.as_bytes()], terminal::CHAT).await;
    assert!(row["error"].as_str().unwrap().contains("未完整下发"));
    assert_eq!(row["totalTokens"], 0);
}

#[tokio::test]
async fn normal_eof_records_once_and_forwards_bytes_unchanged() {
    let (_dir, stats) = temp_stats();
    let (context, _telemetry) = context_for(stats.clone());
    let chunks = vec![Bytes::from_static(b"first"), Bytes::from_static(b"second")];
    let input = futures::stream::iter(chunks.clone().into_iter().map(Ok::<_, std::io::Error>));
    let mut stream = RecordingStream::with_terminals(Box::new(input), context, terminal::CHAT);
    for expected in chunks {
        assert_eq!(stream.next().await.unwrap().unwrap(), expected);
    }
    assert!(stream.next().await.is_none());
    assert!(stream.next().await.is_none());
    drop(stream);
    assert_success(&only_row(&stats));
}

#[tokio::test]
async fn response_and_anthropic_completion_allow_client_hangup() {
    for (terminals, frame) in [
        (
            terminal::RESPONSES,
            b"event: response.completed\n".as_slice(),
        ),
        (terminal::ANTHROPIC, b"event: message_stop\n".as_slice()),
    ] {
        assert_success(&record_after_hangup(&[frame], terminals).await);
    }
}

#[tokio::test]
async fn upstream_error_is_not_hidden_by_a_terminal_frame() {
    for (terminals, frame) in [
        (terminal::CHAT, UNSPACED_DONE.as_bytes()),
        (terminal::RESPONSES, b"event: response.failed\n".as_slice()),
        (terminal::ANTHROPIC, b"event: message_stop\n".as_slice()),
    ] {
        let (_dir, stats) = temp_stats();
        let (context, telemetry) = context_for(stats.clone());
        telemetry.note_error("上游额度不足");
        let mut stream = RecordingStream::with_terminals(
            Box::new(stream_ending_at(vec![frame])),
            context,
            terminals,
        );
        assert!(stream.next().await.unwrap().is_ok());
        drop(stream);
        let row = only_row(&stats);
        assert_eq!(row["error"], "上游额度不足");
        assert_eq!(row["totalTokens"], 0);
    }
}

#[tokio::test]
async fn dropping_before_first_frame_records_an_abort_without_first_response() {
    let (_dir, stats) = temp_stats();
    let (context, _telemetry) = context_for(stats.clone());
    drop(RecordingStream::with_terminals(
        Box::new(futures::stream::pending()),
        context,
        terminal::CHAT,
    ));
    let row = only_row(&stats);
    assert!(row["error"].as_str().unwrap().contains("未完整下发"));
    assert!(row["firstResponseMs"].is_null());
    assert_eq!(row["totalTokens"], 0);
}

#[tokio::test]
async fn io_error_is_forwarded_and_does_not_replace_the_recorded_cause() {
    let (_dir, stats) = temp_stats();
    let (context, telemetry) = context_for(stats.clone());
    telemetry.note_error("上游连接中断");
    let input = futures::stream::iter(vec![Err(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "connection reset",
    ))]);
    let mut stream = RecordingStream::with_terminals(Box::new(input), context, terminal::CHAT);
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "connection reset");
    drop(stream);
    let row = only_row(&stats);
    assert_eq!(row["error"], "上游连接中断");
    assert_eq!(row["totalTokens"], 0);
}
