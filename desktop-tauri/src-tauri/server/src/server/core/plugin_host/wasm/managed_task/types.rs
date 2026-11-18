//! 仅定义约束和内部标量；不预占名额、不延长期限、不授予账号或网络权限。

use std::time::Duration;

pub(super) const MAX_PROFILES: usize = 1;
pub(super) const MAX_DECLARATION_BYTES: usize = 1024;
pub(super) const MAX_CREATE_BYTES: usize = 256 * 1024;
pub(super) const MAX_CONTENT_BYTES: usize = MAX_CREATE_BYTES;
pub(super) const MAX_SYSTEM_PROMPT_BYTES: usize = MAX_CREATE_BYTES;
pub(super) const MAX_CREATION_RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_STOP_RESPONSE_BYTES: usize = 64 * 1024;
pub(super) const MAX_COOKIE_BYTES: usize = 8 * 1024;
pub(super) const MAX_JSON_DEPTH: usize = 32;

pub(super) const CREATE_SUBMISSIONS_PER_CALL: usize = 1;
pub(super) const UNSETTLED_PER_ACCOUNT: usize = 1;
pub(super) const UNSETTLED_PER_PACKAGE: usize = crate::server::config::runtime_capacity::MAX_EXECUTIONS;
pub(super) const UNSETTLED_GLOBAL: usize = crate::server::config::runtime_capacity::MAX_EXECUTIONS;
pub(super) const STOP_ATTEMPTS_PER_CREATION: usize = 1;
pub(super) const PREPARE_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const CREATE_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const STOP_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const MAX_LIFETIME: Duration = Duration::from_secs(60 * 60);

pub(super) fn bounded_lifetime(remaining_call: Duration) -> Duration {
    remaining_call.min(MAX_LIFETIME)
}

/// 内部已校验 UUID，不是任务所有权。严格连字符格式，不接受 URN/括号/空白/零值。
/// 不实现 Serialize/Deserialize/Debug，避免成为可反序列化的权限替身。
pub(super) struct NonZeroUuid(String);

impl NonZeroUuid {
    pub(super) fn parse(raw: &str) -> Option<Self> {
        if raw.len() != 36 {
            return None;
        }
        let mut nonzero = false;
        for (index, byte) in raw.bytes().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                if byte != b'-' {
                    return None;
                }
            } else if byte.is_ascii_hexdigit() {
                nonzero |= byte != b'0';
            } else {
                return None;
            }
        }
        nonzero.then(|| Self(raw.to_ascii_lowercase()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// 校验错误只有宿主固定文案，不夹带输入内容、凭据、响应正文或 serde 原始错误。
pub(super) struct ValidationError(&'static str);

impl ValidationError {
    pub(super) fn new(message: &'static str) -> Self {
        Self(message)
    }
    pub(super) fn message(&self) -> &'static str {
        self.0
    }
}
