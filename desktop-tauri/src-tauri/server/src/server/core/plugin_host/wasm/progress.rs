mod sse;

use serde_json::Value;

use sse::Payload;

#[derive(Default)]
pub(super) struct Progress {
    upstream_bytes: u64,
    pending_bytes: u64,
    output_bytes: u64,
    http: std::collections::HashMap<String, Payload>,
    catpaw: Option<Payload>,
}

impl Progress {
    pub(super) fn http_open(&mut self, id: &str, sse: bool) {
        self.http.insert(id.to_string(), Payload::new(sse));
    }

    pub(super) fn http_read(&mut self, id: &str, result: &Value) {
        let Some(payload) = self.http.get_mut(id) else { return; };
        if result["done"] == true {
            let bytes = payload.finish();
            self.http.remove(id);
            self.credit(bytes);
        } else if let Some(bytes) = decode(result.get("data")) {
            let bytes = payload.push(&bytes);
            self.credit(bytes);
        }
    }

    pub(super) fn http_close(&mut self, id: &str) { self.http.remove(id); }

    pub(super) fn catpaw_read(&mut self, result: &Value) {
        let payload = self.catpaw.get_or_insert_with(|| Payload::new(true));
        let bytes = if result["eof"] == true { payload.finish() }
            else { decode(result.get("data")).map_or(0, |bytes| payload.push(&bytes)) };
        self.credit(bytes);
    }

    pub(super) fn websocket_read(&mut self, result: &Value) {
        let bytes = match result["type"].as_str() {
            Some("text") => result["data"].as_str().filter(|text| sse::meaningful(text.as_bytes()))
                .map_or(0, |text| text.len() as u64),
            Some("binary") => decode(result.get("data")).map_or(0, |bytes| bytes.len() as u64),
            _ => 0,
        };
        self.credit(bytes);
    }

    pub(super) fn task_read(&mut self, result: &Value) {
        if result["done"] == true { return; }
        let bytes = match result["status"].as_str() {
            Some("text") => result["text"].as_str().map_or(0, |text| text.len() as u64),
            Some("tool") => result["tool"].to_string().len() as u64,
            _ => 0,
        };
        self.credit(bytes);
    }

    fn credit(&mut self, bytes: u64) {
        self.upstream_bytes = self.upstream_bytes.saturating_add(bytes);
        self.pending_bytes = self.pending_bytes.saturating_add(bytes);
    }

    pub(super) fn take(&mut self) -> u64 { std::mem::take(&mut self.pending_bytes) }
    pub(super) fn upstream_bytes(&self) -> u64 { self.upstream_bytes }
    pub(super) fn output(&mut self, bytes: usize) { self.output_bytes = self.output_bytes.saturating_add(bytes as u64); }
    pub(super) fn output_bytes(&self) -> u64 { self.output_bytes }
}

fn decode(value: Option<&Value>) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(value?.as_str()?).ok()
}
