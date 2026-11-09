use agent2api_provider_protocol::LoginPoll;
use serde::Deserialize;

use crate::server::errors::GatewayError;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginCallbackResult {
    #[serde(flatten)]
    pub poll: LoginPoll,
    #[serde(default)]
    pub next_url: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

impl From<LoginPoll> for LoginCallbackResult {
    fn from(poll: LoginPoll) -> Self {
        Self {
            poll,
            next_url: None,
            retryable: false,
        }
    }
}

pub(super) fn validate(result: &LoginCallbackResult, hosts: &[String]) -> Result<(), GatewayError> {
    if (result.retryable || result.next_url.is_some())
        && !matches!(result.poll, LoginPoll::Pending { .. })
        || (result.retryable && result.next_url.is_some())
    {
        return Err(GatewayError::with_status(
            502,
            "插件返回了不一致的登录回调状态",
        ));
    }
    let Some(next) = &result.next_url else {
        return Ok(());
    };
    let invalid =
        || GatewayError::with_status(502, "下一步授权地址不在本次登录允许的 HTTPS 站点中");
    if next.len() > 16 * 1024 || next.bytes().any(|ch| ch <= b' ' || ch == 0x7f) {
        return Err(invalid());
    }
    let url = url::Url::parse(next).map_err(|_| invalid())?;
    let host = url.host_str().ok_or_else(invalid)?.to_ascii_lowercase();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || (!hosts.is_empty()
            && !hosts.iter().any(|allowed| {
                let allowed = allowed
                    .trim()
                    .trim_start_matches("*.")
                    .trim_end_matches('.')
                    .to_ascii_lowercase();
                !allowed.is_empty() && (host == allowed || host.ends_with(&format!(".{allowed}")))
            }))
    {
        return Err(invalid());
    }
    Ok(())
}
