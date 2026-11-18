use std::time::Duration;

use reqwest::Request;
use serde_json::{json, Value};
use tokio::time::Instant;
use zeroize::{Zeroize, Zeroizing};

use super::kiro_response::{ProfileResponse, TokenResponse};
use crate::server::{
    core::{egress::managed::FrozenHttpClient, login::managed::KiroExchangeInput},
    errors::GatewayError,
};

const TOKEN_LIMIT: usize = 128 * 1024;
const PROFILE_LIMIT: usize = 2 * 1024 * 1024;
const US: &str = "https://management.us-east-1.kiro.dev";
const EU: &str = "https://management.eu-central-1.kiro.dev";
const BUILDER_PROFILE: &str = "arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX";

pub(super) struct Account(Value);
impl Account {
    pub(super) fn value(&self) -> &Value {
        &self.0
    }
    pub(super) fn into_value(mut self) -> Value {
        std::mem::take(&mut self.0)
    }
}
impl Drop for Account {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}
fn wipe(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(wipe),
        Value::Object(fields) => fields.values_mut().for_each(wipe),
        _ => {}
    }
}

pub(super) fn exchange_request(
    client: &FrozenHttpClient,
    input: &KiroExchangeInput,
) -> Result<Request, GatewayError> {
    if input.method() != "POST"
        || input.endpoint() != "https://prod.us-east-1.auth.desktop.kiro.dev/oauth/token"
            && super::super::aws_oidc::token_region(input.endpoint()).is_none()
        || input.body().is_empty()
        || input.body().len() > TOKEN_LIMIT
    {
        return Err(unconfirmed());
    }
    let fingerprint =
        super::super::runtime_info::client_fingerprint().map_err(|_| unconfirmed())?;
    let fingerprint = fingerprint
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(unconfirmed)?;
    let request = client
        .client()
        .map_err(|_| unconfirmed())?
        .post(input.endpoint())
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", format!("KiroIDE-1.1.70-{fingerprint}"))
        .body(input.body().to_vec())
        .build()
        .map_err(|_| unconfirmed())?;
    if request.method() != reqwest::Method::POST
        || request.url().as_str() != input.endpoint()
        || request.body().and_then(reqwest::Body::as_bytes) != Some(input.body())
    {
        return Err(unconfirmed());
    }
    Ok(request)
}

pub(super) async fn exchange_response(
    client: &FrozenHttpClient,
    input: &KiroExchangeInput,
    request: Request,
) -> Result<TokenResponse, GatewayError> {
    let deadline = input
        .deadline()
        .min(Instant::now() + Duration::from_secs(25));
    let (status, bytes) = observe(client, request, input.endpoint(), TOKEN_LIMIT, deadline).await?;
    TokenResponse::parse(status, &bytes, crate::server::logging::now_ms())
}

pub(super) fn profile_request(
    client: &FrozenHttpClient,
    input: &KiroExchangeInput,
    token: &TokenResponse,
) -> Result<(Request, String, String, String), GatewayError> {
    let (region, profile) = profile_source(input, token)?;
    let base = if region == "eu-central-1" { EU } else { US };
    let mut url =
        format!("{base}/Get-Usage-Limits?origin=KIRO_CLI&resourceType=CREDIT&isEmailRequired=true");
    if !profile.is_empty() {
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("profileArn", &profile)
            .finish();
        url.push('&');
        url.push_str(&encoded);
    }
    if contains_secret(&url, input, token) {
        return Err(unconfirmed());
    }
    let request = client
        .client()
        .map_err(|_| unconfirmed())?
        .get(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .bearer_auth(token.access())
        .build()
        .map_err(|_| unconfirmed())?;
    if request.method() != reqwest::Method::GET
        || request.url().as_str() != url
        || request.body().is_some()
    {
        return Err(unconfirmed());
    }
    Ok((request, url, region, profile))
}

pub(super) async fn profile_response(
    client: &FrozenHttpClient,
    input: &KiroExchangeInput,
    token: &TokenResponse,
    request: Request,
    url: &str,
) -> Result<ProfileResponse, GatewayError> {
    let deadline = input
        .deadline()
        .min(Instant::now() + Duration::from_secs(15));
    let (status, bytes) = observe(client, request, url, PROFILE_LIMIT, deadline).await?;
    ProfileResponse::parse(status, &bytes, token)
}

fn profile_source(
    input: &KiroExchangeInput,
    token: &TokenResponse,
) -> Result<(String, String), GatewayError> {
    let mut region = if input.endpoint() == "https://prod.us-east-1.auth.desktop.kiro.dev/oauth/token" {
        "us-east-1"
    } else {
        let region = super::super::aws_oidc::token_region(input.endpoint()).ok_or_else(unconfirmed)?;
        if region.starts_with("eu-") { "eu-central-1" } else { "us-east-1" }
    }.to_string();
    let mut profile = token.profile().unwrap_or("").to_string();
    if contains_secret(&profile, input, token) {
        return Err(unconfirmed());
    }
    if !profile.is_empty() {
        let parts = profile.split(':').collect::<Vec<_>>();
        if parts.len() != 6
            || parts[0] != "arn"
            || parts[1] != "aws"
            || parts[2] != "codewhisperer"
            || !matches!(parts[3], "us-east-1" | "eu-central-1")
            || parts[4].is_empty()
            || !parts[5].starts_with("profile/")
            || parts[5].len() <= 8
        {
            return Err(unconfirmed());
        }
        region = parts[3].to_string();
    } else if input.login_provider() == "builderid" {
        profile = BUILDER_PROFILE.to_string();
        region = "us-east-1".to_string();
    }
    Ok((region, profile))
}

pub(super) fn account(
    input: &KiroExchangeInput,
    token: &TokenResponse,
    profile: &ProfileResponse,
    region: &str,
    profile_arn: &str,
) -> Result<Account, GatewayError> {
    let channel = input.login_provider();
    if !matches!(
        channel,
        "google" | "github" | "builderid" | "enterprise" | "internal"
    ) || !matches!(region, "us-east-1" | "eu-central-1")
        || token
            .expires()
            .is_some_and(|at| at <= crate::server::logging::now_ms())
    {
        return Err(unconfirmed());
    }
    let method = if input.client_id().is_some() {
        "idc"
    } else {
        "social"
    };
    // IDC 客户端注册绑定认证目录；profile 的业务区不能改写下次 OIDC 续期来源。
    let auth_region = if method == "idc" {
        super::super::aws_oidc::token_region(input.endpoint()).ok_or_else(unconfirmed)?
    } else {
        region
    };
    if (method == "social") != matches!(channel, "google" | "github") {
        return Err(unconfirmed());
    }
    let identity = format!("kiro:{}:{channel}", profile.email().to_lowercase());
    if identity.len() > 512 {
        return Err(unconfirmed());
    }
    let name = format!("{} ({channel})", profile.email())
        .chars()
        .take(100)
        .collect::<String>();
    let tail = token
        .access()
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let mut account = Account(json!({
        "identity": identity, "desktop": false,
        "secret": { "method": method, "access": token.access(), "refresh": token.refresh().unwrap_or(""),
            "expires": token.expires(), "region": auth_region, "profileArn": profile_arn,
            "clientId": input.client_id().unwrap_or(""), "clientSecret": input.client_secret_candidate().unwrap_or(""),
            "tokenURL": "", "loginProvider": channel, "uid": channel, "accountId": profile.email(),
            "plan": profile.plan().unwrap_or(""), "source": "", "name": name, "identitySeed": "" },
        "public": { "name": name, "accountId": profile.email(), "uid": channel,
            "hasCredentials": true, "hasRefreshToken": token.refresh().is_some(), "desktop": false,
            "available": true, "tokenTail": tail }
    }));
    if let Some(expires) = token.expires() {
        account.0["public"]["expiresAt"] = json!(expires);
    }
    if let Some(plan) = profile.plan() {
        account.0["public"]["plan"] = json!(plan);
    }
    let public = Zeroizing::new(account.0["public"].to_string());
    if contains_secret(&public, input, token)
        || contains_secret(&identity, input, token)
        || contains_secret(&name, input, token)
        || contains_secret(profile.email(), input, token)
        || profile
            .plan()
            .is_some_and(|plan| contains_secret(plan, input, token))
    {
        return Err(unconfirmed());
    }
    super::super::public_view::validate(&account.0["public"], &account.0["secret"])
        .map_err(|_| unconfirmed())?;
    Ok(account)
}

fn contains_secret(text: &str, input: &KiroExchangeInput, token: &TokenResponse) -> bool {
    let lower = Zeroizing::new(text.to_lowercase());
    for secret in [
        Some(token.access()),
        token.refresh(),
        input.client_secret_candidate(),
        Some(input.code()),
        Some(input.verifier()),
        Some(input.challenge()),
    ]
    .into_iter()
    .flatten()
    {
        if secret.is_empty() {
            continue;
        }
        let clear = Zeroizing::new(secret.to_lowercase());
        let encoded = Zeroizing::new(
            url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>(),
        );
        let encoded_lower = Zeroizing::new(encoded.to_lowercase());
        if lower.contains(clear.as_str()) || lower.contains(encoded_lower.as_str()) {
            return true;
        }
    }
    false
}

async fn observe(
    client: &FrozenHttpClient,
    request: Request,
    expected: &str,
    limit: usize,
    deadline: Instant,
) -> Result<(u16, Zeroizing<Vec<u8>>), GatewayError> {
    tokio::time::timeout_at(deadline, async {
        let mut response = client
            .client()
            .map_err(|_| unconfirmed())?
            .execute(request)
            .await
            .map_err(|_| unconfirmed())?;
        if response.url().as_str() != expected
            || response.headers().contains_key("location")
            || response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            || response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value.contains("text/html") || value.contains("application/xhtml+xml")
                })
        {
            return Err(unconfirmed());
        }
        let status = response.status().as_u16();
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unconfirmed())? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > limit)
            {
                return Err(unconfirmed());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    })
    .await
    .map_err(|_| unconfirmed())?
}

pub(super) fn unconfirmed() -> GatewayError {
    GatewayError::with_status(
        409,
        "Kiro本次授权兑换或真实用户信息未确认，未重发授权码或保存候选账号",
    )
    .with_code("kiro_exchange_unconfirmed")
}
