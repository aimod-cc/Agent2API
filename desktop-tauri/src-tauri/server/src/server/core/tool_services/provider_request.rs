use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use super::provider_keys::{self, MAX_PROVIDER_KEYS};
use super::search::{SearchError, SearchErrorCode};

pub(super) struct ProviderRequest<'a> {
    provider: &'a str,
    operation: &'static str,
    deadline: Instant,
    test_key_id: Option<&'a str>,
    allow_disabled: bool,
}

impl<'a> ProviderRequest<'a> {
    pub fn new(provider: &'a str, operation: &'static str, timeout_seconds: u64) -> Self {
        Self {
            provider,
            operation,
            deadline: Instant::now() + Duration::from_secs(timeout_seconds.max(1)),
            test_key_id: None,
            allow_disabled: false,
        }
    }

    pub fn for_test(
        provider: &'a str,
        operation: &'static str,
        timeout_seconds: u64,
        key_id: Option<&'a str>,
    ) -> Self {
        Self {
            test_key_id: key_id,
            allow_disabled: true,
            ..Self::new(provider, operation, timeout_seconds)
        }
    }

    pub async fn post_json(&self, url: &str, payload: Value) -> Result<Value, SearchError> {
        let mut attempted = BTreeSet::new();
        let mut last_error = None;
        let client = crate::server::core::egress::client_for(None);
        while attempted.len() < MAX_PROVIDER_KEYS {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(last_error.unwrap_or_else(|| {
                    SearchError::new(SearchErrorCode::TimedOut, "The upstream provider timed out")
                }));
            }
            let attempt = match provider_keys::next_key(
                self.provider,
                self.operation,
                &attempted,
                self.test_key_id,
                self.allow_disabled,
            ) {
                Ok(attempt) => attempt,
                Err(error) => return Err(last_error.unwrap_or(error)),
            };
            attempted.insert(attempt.key.id.clone());
            let result = match attempt.decrypt() {
                Ok(api_key) => {
                    let response = client
                        .post(url)
                        .timeout(remaining)
                        .header("authorization", format!("Bearer {}", api_key.as_str()))
                        .header("content-type", "application/json")
                        .header("accept", "application/json")
                        .json(&payload)
                        .send()
                        .await
                        .map_err(transport_error)?;
                    let status = response.status().as_u16();
                    let headers = response.headers().clone();
                    let raw = response.text().await.map_err(transport_error)?;
                    let body: Option<Value> = serde_json::from_str(&raw).ok();
                    match super::provider_errors::response_error(
                        self.provider,
                        status,
                        &headers,
                        body.as_ref(),
                        &raw,
                        &api_key,
                    ) {
                        Some(error) => Err(error),
                        None => body.ok_or_else(|| {
                            SearchError::new(
                                SearchErrorCode::ProviderUnavailable,
                                "The upstream provider returned an invalid JSON response",
                            )
                        }),
                    }
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(body) => {
                    provider_keys::record_success(self.provider, self.operation, &attempt)?;
                    return Ok(body);
                }
                Err(error) => {
                    if !matches!(
                        error.code,
                        SearchErrorCode::QuotaExhausted
                            | SearchErrorCode::RateLimited
                            | SearchErrorCode::InvalidProviderKey
                    ) {
                        return Err(error);
                    }
                    provider_keys::record_failure(self.provider, self.operation, &attempt, &error)?;
                    if self.test_key_id.is_some() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            SearchError::new(
                SearchErrorCode::ProviderUnavailable,
                "No provider API key is available for this request",
            )
        }))
    }
}

fn transport_error(error: reqwest::Error) -> SearchError {
    if error.is_timeout() {
        SearchError::new(SearchErrorCode::TimedOut, "The upstream provider timed out")
    } else {
        SearchError::new(
            SearchErrorCode::ProviderUnavailable,
            "Unable to connect to the upstream provider",
        )
    }
}
