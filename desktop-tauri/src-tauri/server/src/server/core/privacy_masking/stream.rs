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

pub(crate) fn wrap(body: Body, format: Format, restore: Restore) -> Body {
    let state = Decoder {
        source: body.into_data_stream(),
        format,
        restore,
        input: BytesMut::new(),
        offset: 0,
        line_start: 0,
        waiting: Vec::new(),
        waiting_bytes: 0,
        fragments: HashMap::new(),
        output: VecDeque::new(),
        eof: false,
    };
    Body::from_stream(stream::unfold(state, |mut state| async move {
        match state.next_frame().await {
            Ok(Some(frame)) => Some((Ok(frame), state)),
            Ok(None) => None,
            Err(error) => {
                state.eof = true;
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
}

impl Frame {
    fn parse(original: Bytes) -> Self {
        let json = std::str::from_utf8(&original).ok().and_then(|s| {
            let data = s
                .lines()
                .filter_map(|line| {
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
        Self {
            original,
            json,
            changed: false,
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
        for line in old.split_inclusive('\n') {
            if line.starts_with("data:") {
                if wrote {
                    continue;
                }
                wrote = true;
                out.push_str("data: ");
                out.push_str(&payload);
                if line.ends_with("\r\n") {
                    out.push_str("\r\n");
                } else if line.ends_with('\n') {
                    out.push('\n');
                }
            } else {
                out.push_str(line);
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
    waiting: Vec<Frame>,
    waiting_bytes: usize,
    fragments: HashMap<Track, Pending>,
    output: VecDeque<Bytes>,
    eof: bool,
}

impl Decoder {
    async fn next_frame(&mut self) -> io::Result<Option<Bytes>> {
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
        while let Some(n) = self.input[self.offset..].iter().position(|&b| b == b'\n') {
            let end = self.offset + n;
            self.offset = end + 1;
            if self.offset > MAX_FRAME {
                return Err(invalid());
            }
            let line = &self.input[self.line_start..end];
            if line.is_empty() || line == b"\r" {
                let frame = self.input.split_to(self.offset).freeze();
                self.offset = 0;
                self.line_start = 0;
                return Ok(Some(frame));
            }
            self.line_start = self.offset;
        }
        if self.input.len() > MAX_FRAME {
            return Err(invalid());
        }
        self.offset = self.input.len();
        Ok(None)
    }

    fn push(&mut self, bytes: Bytes) -> io::Result<()> {
        let mut frame = Frame::parse(bytes);
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
            for delta in wire::deltas(self.format, value) {
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
        if self.fragments.is_empty()
            || terminal
            || self.waiting.len() >= MAX_WAIT_FRAMES
            || self.waiting_bytes >= MAX_WAIT_BYTES
        {
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
