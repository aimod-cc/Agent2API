use super::{terminal, RecordContext, RecordingStream, TerminalFrames, TerminalScan};

mod recording;

const UNSPACED_DONE: &str = "data:[DONE]\n\n";
const SPACED_DONE: &str = "data: [DONE]\n\n";
const OLD_CHAT: TerminalFrames = &[b"data: [DONE]\n\n"];
const LAST_CHUNK: &str = r#"data: {"id":"c1","choices":[{"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":71476}}"#;

fn seen_after(frames: TerminalFrames, chunks: &[&str]) -> bool {
    let mut scan = TerminalScan::new(frames);
    for chunk in chunks {
        scan.push(chunk.as_bytes());
    }
    scan.seen
}

fn longest(frames: TerminalFrames) -> usize {
    frames.iter().map(|frame| frame.len()).max().unwrap_or(0)
}

#[test]
fn unspaced_done_counts_as_terminal() {
    assert!(seen_after(terminal::CHAT, &[LAST_CHUNK, UNSPACED_DONE]));
    assert!(!seen_after(OLD_CHAT, &[LAST_CHUNK, UNSPACED_DONE]));
}

#[test]
fn spaced_done_still_counts() {
    assert!(seen_after(terminal::CHAT, &[LAST_CHUNK, SPACED_DONE]));
    assert!(seen_after(OLD_CHAT, &[LAST_CHUNK, SPACED_DONE]));
}

#[test]
fn crlf_line_endings_count() {
    assert!(seen_after(terminal::CHAT, &["data: [DONE]\r\n\r\n"]));
    assert!(seen_after(terminal::CHAT, &["data:[DONE]\r\n\r\n"]));
}

#[test]
fn only_a_whole_frame_counts_when_chunks_split() {
    assert!(seen_after(
        terminal::CHAT,
        &[LAST_CHUNK, "\n\ndata:[DON", "E]\n\n"]
    ));
    assert!(seen_after(terminal::CHAT, &["\n\ndata:[DON", "E]\n\n"]));
    assert!(seen_after(terminal::CHAT, &["data:[DONE]", "\n"]));
    assert!(!seen_after(terminal::CHAT, &["data:[DON", "E]"]));
}

#[test]
fn done_echoed_inside_content_is_not_a_terminal_frame() {
    let echoed = "data: {\"content\":\"示例：data: [DONE]\\n\\n 就是结束标记\"}\n\n";
    assert!(!seen_after(terminal::CHAT, &[echoed]));
    let unspaced = "data: {\"content\":\"示例：data:[DONE] 就是结束标记\"}\n\n";
    assert!(!seen_after(terminal::CHAT, &[unspaced]));
    assert!(!seen_after(
        terminal::CHAT,
        &["data:[DONE", "\", \"x\":1}", "\n"]
    ));
}

#[test]
fn scan_window_stays_bounded() {
    for frames in [terminal::CHAT, terminal::RESPONSES, terminal::ANTHROPIC] {
        let mut scan = TerminalScan::new(frames);
        for _ in 0..2000 {
            assert!(!scan.push("data: {\"content\":\"很长的一段回答内容\"}\n\n".as_bytes()));
        }
        assert!(scan.window.len() <= longest(frames).saturating_sub(1));
    }
}

#[test]
fn other_entries_keep_their_own_frames() {
    assert!(seen_after(
        terminal::RESPONSES,
        &["event: response.completed\n"]
    ));
    assert!(seen_after(
        terminal::RESPONSES,
        &["event: response.failed\n"]
    ));
    assert!(seen_after(terminal::ANTHROPIC, &["event: message_stop\n"]));
    assert!(!seen_after(
        terminal::RESPONSES,
        &[UNSPACED_DONE, SPACED_DONE]
    ));
    assert!(!seen_after(
        terminal::ANTHROPIC,
        &[UNSPACED_DONE, SPACED_DONE]
    ));
    assert!(!seen_after(
        terminal::CHAT,
        &["event: response.completed\n", "event: message_stop\n"]
    ));
    assert!(!seen_after(terminal::RESPONSES, &["event: message_stop\n"]));
    assert!(!seen_after(
        terminal::ANTHROPIC,
        &["event: response.completed\n"]
    ));
}

#[test]
fn every_terminal_survives_every_two_chunk_split() {
    for frames in [terminal::CHAT, terminal::RESPONSES, terminal::ANTHROPIC] {
        for frame in frames {
            for split in 0..=frame.len() {
                let mut scan = TerminalScan::new(frames);
                scan.push(&frame[..split]);
                scan.push(&frame[split..]);
                assert!(
                    scan.seen,
                    "收尾帧 {frame:?} 在第 {split} 字节切分后应当仍被识别"
                );
            }
        }
    }
}

#[test]
fn terminals_arriving_one_byte_at_a_time_are_recognized_once() {
    for frames in [terminal::CHAT, terminal::RESPONSES, terminal::ANTHROPIC] {
        for frame in frames {
            let mut scan = TerminalScan::new(frames);
            let hits = frame
                .iter()
                .filter(|byte| scan.push(std::slice::from_ref(*byte)))
                .count();
            assert_eq!(hits, 1, "收尾帧 {frame:?} 只能首次命中一次");
            assert!(!scan.push(frame));
            assert!(scan.seen);
            assert!(scan.window.len() <= longest(frames).saturating_sub(1));
        }
    }
}

#[test]
fn incomplete_or_quoted_terminal_events_do_not_finish_the_stream() {
    for (frames, prefix, quoted) in [
        (
            terminal::RESPONSES,
            "event: response.completed",
            "data: {\"text\":\"event: response.completed\\n\"}\n\n",
        ),
        (
            terminal::ANTHROPIC,
            "event: message_stop",
            "data: {\"text\":\"event: message_stop\\n\"}\n\n",
        ),
    ] {
        assert!(!seen_after(frames, &[prefix]));
        assert!(!seen_after(frames, &[quoted]));
    }
}

#[test]
fn empty_terminal_table_never_retains_response_bytes() {
    let mut scan = TerminalScan::new(&[]);
    assert!(!scan.push(UNSPACED_DONE.as_bytes()));
    assert!(!scan.seen);
    assert!(scan.window.is_empty());
}
