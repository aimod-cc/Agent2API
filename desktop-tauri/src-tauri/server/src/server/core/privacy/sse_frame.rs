//! SSE 帧解析与重组辅助：支持 CR / LF / CRLF 换行、首帧 UTF-8 BOM、多行 data 拼接、
//! 注释行及 event/id/retry 等非 data 行原样保留，带帧尺寸上限保护。

use serde_json::Value;

pub(super) const MAX_SSE_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 将文本按 CR / LF / CRLF 切分为 `(start, content_end, line_end)`。
pub(super) fn line_parts(text: &str) -> Vec<(usize, usize, usize)> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut at = 0usize;
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

/// 按 SSE 规范从字节缓冲中切分完整事件帧（以空行分隔，兼容 `\r\n` / `\n` / 单独 `\r`）。
#[derive(Default)]
pub(super) struct SseFrameSplitter {
    buf: Vec<u8>,
    scan_from: usize,
    line_start: usize,
}

impl SseFrameSplitter {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        self.drain_frames(false)
    }

    pub(super) fn finish(&mut self) -> Vec<Vec<u8>> {
        let mut frames = self.drain_frames(true);
        if !self.buf.is_empty() {
            frames.push(std::mem::take(&mut self.buf));
            self.scan_from = 0;
            self.line_start = 0;
        }
        frames
    }

    pub(super) fn take_overflow(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() > MAX_SSE_FRAME_BYTES {
            self.scan_from = 0;
            self.line_start = 0;
            Some(std::mem::take(&mut self.buf))
        } else {
            None
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn drain_frames(&mut self, eof: bool) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut start = 0usize;
        let mut line_start = self.line_start;
        let mut at = self.scan_from;

        while at < self.buf.len() {
            let b = self.buf[at];
            if b != b'\r' && b != b'\n' {
                at += 1;
                continue;
            }
            // 分片末尾的单独 '\r' 可能是 '\r\n' 的前半截，非 EOF 时等待下一片确认。
            if b == b'\r' && at + 1 == self.buf.len() && !eof {
                break;
            }
            let line_end = at;
            let next = if b == b'\r' && self.buf.get(at + 1) == Some(&b'\n') {
                at + 2
            } else {
                at + 1
            };
            if line_end == line_start {
                frames.push(self.buf[start..next].to_vec());
                start = next;
            }
            line_start = next;
            at = next;
        }

        self.scan_from = at.saturating_sub(start);
        self.line_start = line_start.saturating_sub(start);
        if start > 0 {
            self.buf.drain(..start);
        }
        frames
    }
}

pub(super) struct ParsedSseFrame {
    pub(super) event_name: String,
    pub(super) data_text: Option<String>,
    pub(super) is_done: bool,
    pub(super) leading_bom: bool,
}

pub(super) fn parse_sse_frame(raw: &[u8], first_frame: bool) -> ParsedSseFrame {
    let leading_bom = first_frame && raw.starts_with(&[0xef, 0xbb, 0xbf]);
    let Ok(text_all) = std::str::from_utf8(raw) else {
        return ParsedSseFrame {
            event_name: String::new(),
            data_text: None,
            is_done: false,
            leading_bom,
        };
    };
    let text = if leading_bom {
        text_all.strip_prefix('\u{feff}').unwrap_or(text_all)
    } else {
        text_all
    };
    let mut event_name = String::new();
    let mut data_lines: Vec<&str> = Vec::new();
    for (start, content_end, _) in line_parts(text) {
        let line = &text[start..content_end];
        if let Some(ev) = line.strip_prefix("event:") {
            event_name = ev.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }
    if data_lines.is_empty() {
        return ParsedSseFrame {
            event_name,
            data_text: None,
            is_done: false,
            leading_bom,
        };
    }
    let joined = if data_lines.len() == 1 {
        data_lines[0].to_string()
    } else {
        data_lines.join("\n")
    };
    let is_done = joined.trim() == "[DONE]";
    ParsedSseFrame {
        event_name,
        data_text: Some(joined),
        is_done,
        leading_bom,
    }
}

/// 保留原始 SSE 帧中的 event / id / retry / 注释行、BOM 与换行符，仅把 data 行替换为新 JSON。
pub(super) fn rewrite_sse_frame(raw: &[u8], leading_bom: bool, value: &Value) -> Option<Vec<u8>> {
    let encoded = serde_json::to_string(value).ok()?;
    let old = std::str::from_utf8(raw).ok()?;
    let mut out = String::with_capacity(old.len() + 32);
    let mut wrote = false;
    for (start, content_end, end) in line_parts(old) {
        let has_bom = start == 0 && leading_bom;
        let content_start = start + if has_bom { '\u{feff}'.len_utf8() } else { 0 };
        let content = &old[content_start..content_end];
        if content.starts_with("data:") {
            if wrote {
                continue;
            }
            wrote = true;
            if has_bom {
                out.push('\u{feff}');
            }
            out.push_str("data: ");
            out.push_str(&encoded);
            out.push_str(&old[content_end..end]);
        } else {
            out.push_str(&old[start..end]);
        }
    }
    wrote.then(|| out.into_bytes())
}
