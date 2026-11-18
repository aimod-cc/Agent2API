#[derive(Clone, Copy, Debug)]
pub(crate) enum FailureKind {
    ConnectTimeout,
    ConnectionFailed,
    Interrupted,
    ResponseTimeout,
    RequestTimeout,
}

impl FailureKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ConnectTimeout => "connect_timeout",
            Self::ConnectionFailed => "connection_failed",
            Self::Interrupted => "connection_interrupted",
            Self::ResponseTimeout => "response_timeout",
            Self::RequestTimeout => "request_timeout",
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ConnectTimeout => "usage_network_connect_timeout",
            Self::ConnectionFailed => "usage_network_connection_failed",
            Self::Interrupted => "usage_network_interrupted",
            Self::ResponseTimeout => "usage_network_response_timeout",
            Self::RequestTimeout => "usage_network_request_timeout",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ConnectTimeout => "连接超时",
            Self::ConnectionFailed => "无法建立连接",
            Self::Interrupted => "网络连接中断",
            Self::ResponseTimeout => "等待上游响应超时",
            Self::RequestTimeout => "请求超时",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::ConnectTimeout => "连接上游超时，请检查此账号的网络或代理",
            Self::ConnectionFailed => "无法建立网络连接，请检查此账号的网络或代理",
            Self::Interrupted => "网络连接中断，请检查此账号的网络或代理后重试",
            Self::ResponseTimeout => "等待上游响应超时，请稍后重试",
            Self::RequestTimeout => "余额请求超时（连接或响应未完成），请检查网络或稍后重试",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "connect_timeout" => Some(Self::ConnectTimeout),
            "connection_failed" => Some(Self::ConnectionFailed),
            "connection_interrupted" => Some(Self::Interrupted),
            "response_timeout" => Some(Self::ResponseTimeout),
            "request_timeout" => Some(Self::RequestTimeout),
            _ => None,
        }
    }

    pub(crate) fn from_code(code: Option<&str>) -> Option<Self> {
        match code {
            Some("usage_network_connect_timeout") => Some(Self::ConnectTimeout),
            Some("usage_network_connection_failed") => Some(Self::ConnectionFailed),
            Some("usage_network_interrupted") => Some(Self::Interrupted),
            Some("usage_network_response_timeout") => Some(Self::ResponseTimeout),
            Some("usage_network_request_timeout") => Some(Self::RequestTimeout),
            _ => None,
        }
    }
}

pub(crate) fn classify(error: &reqwest::Error, response_started: bool) -> Option<FailureKind> {
    if error.is_timeout() {
        return Some(if error.is_connect() { FailureKind::ConnectTimeout }
            else if response_started { FailureKind::ResponseTimeout }
            else { FailureKind::RequestTimeout });
    }
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause.downcast_ref::<std::io::Error>().is_some_and(|error| matches!(error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof)) {
            return Some(FailureKind::Interrupted);
        }
        source = cause.source();
    }
    if error.is_connect() { Some(FailureKind::ConnectionFailed) }
    else if error.is_body() || error.is_request() && !error.is_builder() { Some(FailureKind::Interrupted) }
    else { None }
}
