//! Tabbit 的固定 runs 投递先冻结本次真实对话；没有回执不能恢复重放机会。
//! WorkBuddy 国内固定对话使用独立封闭发送票，不借用CatPaw来源或JSON代理。
//! 这只记录已进入提交准备，不证明上游已收到或执行，也不授予网络或任务权限。

#[path = "workbuddy_http.rs"]
mod workbuddy_http;
pub(in crate::server::core::plugin_host) use workbuddy_http::{WorkBuddyExecution, MAX_BODY as WORKBUDDY_MAX_BODY};

use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{unavailable, ConversationCall};
use crate::server::core::plugin_host::wasm::context::{Context, Shared};

impl Context {
    pub(in crate::server::core::plugin_host::wasm) fn guard_http_submission(
        &self,
        params: &Value,
    ) -> Result<(), proto::WireError> {
        let origin = match self.shared.options.provider.as_str() {
            "tabbit" => "https://web.tabbit.com",
            "tabbit-intl" => "https://web.tabbit.ai",
            "codearts" if url_path(params)? == "/api/v2/chat/completions" => {
                self.conversation.as_ref().ok_or_else(unavailable)?
                    .freeze_http_submission(&self.shared, &self.account)?;
                return Ok(());
            }
            "doubaowork" if url_path(params)? == "/chat/completion" => {
                self.conversation.as_ref().ok_or_else(unavailable)?
                    .freeze_http_submission(&self.shared, &self.account)?;
                return Ok(());
            }
            _ => return Ok(()),
        };
        let method =
            reqwest::Method::from_bytes(params["method"].as_str().unwrap_or("GET").as_bytes())
                .map_err(|_| invalid())?;
        if method != reqwest::Method::POST {
            return Ok(());
        }
        let url = params["url"].as_str().ok_or_else(invalid)?;
        let url = url::Url::parse(url).map_err(|_| invalid())?;
        if url.origin().ascii_serialization() != origin {
            return Ok(());
        }
        let path = decoded_path(url.path())?;
        let Some(room) = path
            .strip_prefix("/api/v3/chat/rooms/")
            .and_then(|path| path.strip_suffix("/runs"))
        else {
            return Ok(());
        };
        if room.is_empty() {
            return Err(invalid());
        }
        self.conversation
            .as_ref()
            .ok_or_else(unavailable)?
            .freeze_http_submission(&self.shared, &self.account)
    }
}

impl ConversationCall {
    fn freeze_http_submission(
        &self,
        shared: &Arc<Shared>,
        account: &str,
    ) -> Result<(), proto::WireError> {
        let mut state = self.signal.state.lock().map_err(|_| unavailable())?;
        if !state.guard_issued
            || state.consumer
            || state.execution.is_some()
            || state.task_issued
            || state.source_issued
            || state.record.is_some()
            || account.is_empty()
            || account != self.account
            || !matches!(self.provider.as_str(), "tabbit" | "tabbit-intl" | "codearts" | "doubaowork")
            || shared.options.provider != self.provider
            || !std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(shared))
        {
            return Err(unavailable());
        }
        if state.http_submission {
            return Err(proto::WireError::with_code(
                "本次 Tabbit 对话已进入消息提交，未再次提交；上游结果仍以原请求为准",
                "request_replay_blocked",
            ));
        }
        self.with_current(|| self.preparation.freeze())
            .map_err(|_| unavailable())?;
        state.http_submission = true;
        Ok(())
    }
}

fn url_path(params: &Value) -> Result<String, proto::WireError> {
    let raw = params["url"].as_str().ok_or_else(invalid)?;
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    decoded_path(url.path())
}

fn decoded_path(path: &str) -> Result<String, proto::WireError> {
    if path.len() > 8192 {
        return Err(invalid());
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = if bytes[index] == b'%' {
            let high = *bytes.get(index + 1).ok_or_else(invalid)?;
            let low = *bytes.get(index + 2).ok_or_else(invalid)?;
            index += 3;
            hex(high).ok_or_else(invalid)? * 16 + hex(low).ok_or_else(invalid)?
        } else {
            let byte = bytes[index];
            index += 1;
            byte
        };
        if byte.is_ascii_control() || byte == b'\\' {
            return Err(invalid());
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded).map_err(|_| invalid())
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid() -> proto::WireError {
    proto::WireError::with_code("Tabbit 消息提交地址或方法格式无效", "bad_params")
}
