use std::time::Duration;

use super::types::WEBSITE_ORIGIN;
use crate::server::core::{egress, update};

pub async fn request(url: &str) -> Result<reqwest::Response, String> {
    if !url.starts_with(&format!("{WEBSITE_ORIGIN}/")) {
        return Err("广告请求地址无效".into());
    }
    let candidates = update::client::resolve_egress_candidates()?;
    for (_, proxy) in candidates {
        let client = egress::resolved_client(proxy.as_ref(), false, "", &[])?;
        let response = client
            .get(url)
            .timeout(Duration::from_secs(8))
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, "Agent2API-Advertisements/1")
            .send()
            .await;
        if let Ok(response) = response {
            if response.status().is_success() {
                return Ok(response);
            }
        }
    }
    Err("官网广告服务不可用".into())
}
