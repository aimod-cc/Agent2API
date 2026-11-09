//! 账号设置只覆盖 HTTP 请求头，不改凭证或请求体里的客户端身份字段。

use std::future::Future;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};

tokio::task_local! {
    static CURRENT: String;
}

pub(crate) async fn scope<F: Future>(value: &str, work: F) -> F::Output {
    CURRENT.scope(value.trim().to_string(), work).await
}

pub(crate) fn apply(headers: &mut Vec<(String, String)>, value: &str) {
    let value = value.trim();
    if value.is_empty() { return; }
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("user-agent"));
    headers.push(("User-Agent".to_string(), value.to_string()));
}

pub(crate) trait AccountRequest: Sized {
    fn with_user_agent(self, value: &str) -> Self;
    fn with_account_user_agent(self) -> Self {
        let value = CURRENT.try_with(Clone::clone).unwrap_or_default();
        self.with_user_agent(&value)
    }
}

impl AccountRequest for reqwest::RequestBuilder {
    fn with_user_agent(self, value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() { return self; }
        match HeaderValue::from_str(value) {
            Ok(value) => {
                let mut headers = HeaderMap::new();
                headers.insert(USER_AGENT, value);
                self.headers(headers)
            }
            Err(_) => self.header(USER_AGENT, value),
        }
    }
}
