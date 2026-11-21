//! Bounded SSE framing and per-choice/per-call restoration. The event envelope is kept
//! (event name, id, retry, comments and [DONE]); only whitelisted JSON string fields change.
use std::collections::{HashMap, VecDeque};
use std::io;

use axum::body::{Body, BodyDataStream};
use bytes::{Bytes, BytesMut};
use futures::{stream, StreamExt};
use serde_json::Value;

use super::{
    wire::{self, Format, Track},
    Restore,
};

const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_WAIT_BYTES: usize = 2 * 1024 * 1024;
const MAX_WAIT_FRAMES: usize = 64;
fn line_parts(text: &str) -> Vec<(usize, usize, usize)> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\r' || bytes[at] == b'\n' {
            let content_end = at;
            if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                at += 2;
            } else {
                at += 1;
            }
            parts.push((start, content_end, at));
            start = at;
        } else {
            at += 1;
        }
    }
    if start < bytes.len() {
        parts.push((start, bytes.len(), bytes.len()));
    }
    parts
}

pub(crate) fn wrap(body: Body, format: Format, restore: Restore) -> Body {
    let state = Decoder {
        source: body.into_data_stream(),
        format,
        restore,
        input: BytesMut::new(),
        offset: 0,
        line_start: 0,
        first_frame: true,
        waiting: Vec::new(),
        waiting_bytes: 0,
        fragments: HashMap::new(),
        output: VecDeque::new(),
        eof: false,
        failed: false,
    };
    Body::from_stream(stream::unfold(state, |mut state| async move {
        match state.next_frame().await {
            Ok(Some(frame)) => Some((Ok(frame), state)),
            Ok(None) => None,
            Err(error) => {
                state.failed = true;
                state.offset = 0;
                state.line_start = 0;
                state.output.clear();
                state.input.clear();
                state.waiting.clear();
                state.fragments.clear();
                Some((Err(error), state))
            }
        }
    }))
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "privacy masking: SSE output limit or invalid frame",
    )
}

struct Frame {
    original: Bytes,
    json: Option<Value>,
    changed: bool,
    leading_bom: bool,
}

impl Frame {
    fn parse(original: Bytes, first: bool) -> Self {
        let json = std::str::from_utf8(&original).ok().and_then(|raw| {
            let text = if first {
                raw.strip_prefix('\u{feff}').unwrap_or(raw)
            } else {
                raw
            };
            let data = line_parts(text)
                .into_iter()
                .filter_map(|(start, end, _)| {
                    let line = &text[start..end];
                    line.strip_prefix("data:")
                        .map(|part| part.strip_prefix(' ').unwrap_or(part))
                })
                .collect::<Vec<_>>();
            if data.len() == 1 {
                serde_json::from_str(data[0]).ok()
            } else if !data.is_empty() {
                serde_json::from_str(&data.join("\n")).ok()
            } else {
                None
            }
        });
        let leading_bom = first && original.starts_with(&[0xef, 0xbb, 0xbf]);
        Self {
            original,
            json,
            changed: false,
            leading_bom,
        }
    }

    fn encode(self) -> io::Result<Bytes> {
        if !self.changed {
            return Ok(self.original);
        }
        let payload = serde_json::to_string(self.json.as_ref().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let old = std::str::from_utf8(&self.original).map_err(|_| invalid())?;
        let mut out = String::with_capacity(old.len());
        let mut wrote = false;
        for (start, content_end, end) in line_parts(old) {
            let bom = start == 0 && self.leading_bom;
            let content = &old[start + if bom { '\u{feff}'.len_utf8() } else { 0 }..content_end];
            if content.starts_with("data:") {
                if wrote {
                    continue;
                }
                wrote = true;
                if bom {
                    out.push('\u{feff}');
                }
                out.push_str("data: ");
                out.push_str(&payload);
                out.push_str(&old[content_end..end]);
            } else {
                out.push_str(&old[start..end]);
            }
        }
        Ok(Bytes::from(out))
    }
}

struct Pending {
    frame: usize,
    path: String,
    suffix: String,
}

struct Decoder {
    source: BodyDataStream,
    format: Format,
    restore: Restore,
    input: BytesMut,
    offset: usize,
    line_start: usize,
    first_frame: bool,
    waiting: Vec<Frame>,
    waiting_bytes: usize,
    fragments: HashMap<Track, Pending>,
    output: VecDeque<Bytes>,
    eof: bool,
    failed: bool,
}

impl Decoder {
    async fn next_frame(&mut self) -> io::Result<Option<Bytes>> {
        if self.failed {
            return Ok(None);
        }
        loop {
            if let Some(frame) = self.output.pop_front() {
                return Ok(Some(frame));
            }
            if let Some(frame) = self.take_frame()? {
                self.push(frame)?;
                continue;
            }
            if self.eof {
                if !self.input.is_empty() {
                    let tail = self.input.split().freeze();
                    self.offset = 0;
                    self.line_start = 0;
                    self.push(tail)?;
                }
                self.flush()?;
                return Ok(self.output.pop_front());
            }
            match self.source.next().await {
                Some(Ok(bytes)) => self.input.extend_from_slice(&bytes),
                Some(Err(error)) => return Err(io::Error::other(error)),
                None => self.eof = true,
            }
        }
    }

    fn take_frame(&mut self) -> io::Result<Option<Bytes>> {
        let mut at = self.offset;
        while at < self.input.len() {
            if self.input[at] != b'\r' && self.input[at] != b'\n' {
                at += 1;
                continue;
            }
            if self.input[at] == b'\r' && at + 1 == self.input.len() && !self.eof {
                self.offset = at;
                return Ok(None);
            }
            let line_end = at;
            let next = if self.input[at] == b'\r' && self.input.get(at + 1) == Some(&b'\n') {
                at + 2
            } else {
                at + 1
            };
            if next > MAX_FRAME {
                return Err(invalid());
            }
            if line_end == self.line_start {
                let frame = self.input.split_to(next).freeze();
                self.offset = 0;
                self.line_start = 0;
                return Ok(Some(frame));
            }
            self.line_start = next;
            self.offset = next;
            at = next;
        }
        if self.input.len() > MAX_FRAME {
            return Err(invalid());
        }
        self.offset = self.input.len();
        Ok(None)
    }

    fn push(&mut self, bytes: Bytes) -> io::Result<()> {
        let mut frame = Frame::parse(bytes, self.first_frame);
        self.first_frame = false;
        let seq = self.waiting.len();
        let mut terminal = false;
        if let Some(value) = frame.json.as_mut() {
            let original = value.clone();
            wire::response(self.format, value, &mut self.restore).map_err(|_| invalid())?;
            terminal = matches!(
                value.get("type").and_then(Value::as_str),
                Some(
                    "message_stop"
                        | "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "error"
                )
            );
            for delta in wire::deltas(self.format, value, self.restore.json_output()) {
                let Some(Value::String(field)) = value.pointer_mut(&delta.path) else {
                    continue;
                };
                if field.is_empty() {
                    continue;
                }
                let mut joined = self
                    .fragments
                    .remove(&delta.track)
                    .map_or_else(String::new, |previous| previous.suffix);
                joined.push_str(field);
                let n = self.restore.partial_len(&joined);
                let tail = joined.split_off(joined.len() - n);
                self.restore
                    .apply(&mut joined, delta.json)
                    .map_err(|_| invalid())?;
                *field = joined;
                if !tail.is_empty() {
                    self.fragments.insert(
                        delta.track,
                        Pending {
                            frame: seq,
                            path: delta.path,
                            suffix: tail,
                        },
                    );
                }
            }
            frame.changed = *value != original;
        } else if let Ok(frame_text) = std::str::from_utf8(&frame.original) {
            terminal = frame_text.lines().any(|line| {
                line.strip_prefix("data:")
                    .is_some_and(|s| s.trim() == "[DONE]")
            });
        }
        self.waiting_bytes += frame.original.len();
        self.waiting.push(frame);
        let limit_reached =
            self.waiting.len() >= MAX_WAIT_FRAMES || self.waiting_bytes >= MAX_WAIT_BYTES;
        if limit_reached && !self.fragments.is_empty() && !terminal {
            return Err(invalid());
        }
        if self.fragments.is_empty() || terminal || limit_reached {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        // A naturally unfinished prefix is ordinary text: never drop it at EOF or at the cap.
        for pending in self.fragments.drain().map(|(_, pending)| pending) {
            let frame = self.waiting.get_mut(pending.frame).ok_or_else(invalid)?;
            let field = frame
                .json
                .as_mut()
                .and_then(|v| v.pointer_mut(&pending.path));
            let Some(Value::String(field)) = field else {
                return Err(invalid());
            };
            field.push_str(&pending.suffix);
            frame.changed = true;
        }
        for frame in self.waiting.drain(..) {
            self.output.push_back(frame.encode()?);
        }
        self.waiting_bytes = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::privacy_masking::{RequestMask, Rules};

    fn mapped_token() -> (super::super::RequestMask, String) {
        let rules = crate::server::core::privacy_masking::Rules::compile(
            crate::server::core::privacy_masking::Settings {
                terms: vec!["stream-fixture".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let mask = super::super::RequestMask::new(&rules);
        let mut value =
            serde_json::json!({"messages":[{"role":"user","content":"stream-fixture"}]});
        mask.apply(Format::Chat, &mut value).unwrap();
        let token = value
            .pointer("/messages/0/content")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        (mask, token)
    }

    async fn collect(
        source: impl futures::Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
        restore: Restore,
    ) -> io::Result<String> {
        let mut output = Vec::new();
        let mut wrapped = wrap(Body::from_stream(source), Format::Chat, restore).into_data_stream();
        while let Some(part) = wrapped.next().await {
            output.extend_from_slice(&part.map_err(io::Error::other)?);
        }
        Ok(String::from_utf8(output).unwrap())
    }

    #[tokio::test]
    async fn stream_start_bom_and_split_crlf_restore_first_frame() {
        let (mask, token) = mapped_token();
        let json = serde_json::json!({"choices":[{"message":{"content":token}}]}).to_string();
        let body = format!("\u{feff}data: {json}\r\n\r\n");
        let bytes = body.as_bytes();
        let split = bytes.iter().rposition(|&byte| byte == b'\r').unwrap() + 1;
        let pieces = vec![
            Ok(Bytes::copy_from_slice(&bytes[..split])),
            Ok(Bytes::copy_from_slice(&bytes[split..])),
        ];
        let result = collect(stream::iter(pieces), mask.restore()).await.unwrap();
        assert!(result.starts_with('\u{feff}'));
        assert!(result.contains("stream-fixture"));
        assert!(result.ends_with("\r\n\r\n"));
    }

    #[tokio::test]
    async fn later_bom_line_is_preserved_when_another_data_line_changes() {
        let (mask, token) = mapped_token();
        let json = serde_json::json!({"choices":[{"message":{"content":token}}]}).to_string();
        let ignored = format!("{}data: ignored", char::from_u32(0xfeff).unwrap());
        let body = format!(": first\r\n\r\n{ignored}\r\ndata: {json}\r\n\r\n");
        let result = collect(stream::iter([Ok(Bytes::from(body))]), mask.restore())
            .await
            .unwrap();
        assert!(result.contains(&ignored));
        assert!(result.contains("stream-fixture"));
    }

    #[tokio::test]
    async fn lone_cr_is_a_valid_sse_line_and_frame_terminator() {
        let (mask, token) = mapped_token();
        let json = serde_json::json!({"choices":[{"message":{"content":token}}]}).to_string();
        let body = format!("data: {json}\r\r");
        let result = collect(stream::iter([Ok(Bytes::from(body))]), mask.restore())
            .await
            .unwrap();
        assert!(result.contains("stream-fixture"));
        assert!(result.ends_with("\r\r"));
    }

    #[tokio::test]
    async fn waiting_cap_errors_instead_of_flushing_partial_token() {
        let (mask, token) = mapped_token();
        let split = token.len() / 2;
        let event = serde_json::json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":&token[..split]}).to_string();
        let mut pieces: Vec<Result<Bytes, io::Error>> =
            vec![Ok(Bytes::from(format!("data: {event}\r\n\r\n")))];
        pieces.extend(
            (0..(MAX_WAIT_FRAMES - 1)).map(|_| Ok(Bytes::from_static(b": keepalive\r\n\r\n"))),
        );
        let mut wrapped = wrap(
            Body::from_stream(stream::iter(pieces)),
            Format::Responses,
            mask.restore(),
        )
        .into_data_stream();
        assert!(wrapped.next().await.is_some_and(|event| event.is_err()));
        assert!(wrapped.next().await.is_none());
    }

    #[tokio::test]
    async fn errored_stream_fuses_after_partial_frame() {
        let source = Body::from_stream(stream::iter([
            Ok(Bytes::from_static(b"data: {\"incomplete\":")),
            Err(io::Error::other("synthetic source error")),
        ]));
        let mut wrapped = wrap(
            source,
            Format::Chat,
            RequestMask::new(&Rules::default()).restore(),
        )
        .into_data_stream();
        assert!(wrapped.next().await.is_some_and(|event| event.is_err()));
        assert!(wrapped.next().await.is_none());
    }
}
