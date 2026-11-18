//! 登录结果的签名与身份验证。Magpie 只解码 claims，此处用现有 ring 验 RSA 签名。
//! 公钥只从固定 OpenAI issuer 的发现文档取得，绝不采信 JWT 自带的 jku / jwk。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{http, ISSUER};

const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const MAX_JWT_BYTES: usize = 128 * 1024;
const KEY_TTL: Duration = Duration::from_secs(3600);

struct CachedKeys {
    keys: Vec<Value>,
    at: Instant,
}

fn keys_cache() -> &'static Mutex<Option<CachedKeys>> {
    static KEYS: OnceLock<Mutex<Option<CachedKeys>>> = OnceLock::new();
    KEYS.get_or_init(|| Mutex::new(None))
}

fn invalid() -> GatewayError {
    GatewayError::with_status(400, "ChatGPT 登录结果的签名或身份校验未通过，请重新登录")
}

async fn signing_key(client: &reqwest::Client, kid: &str) -> Result<Value, GatewayError> {
    let mut cache = keys_cache().lock().await;
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.at.elapsed() < KEY_TTL)
    {
        if let Some(key) = cached
            .keys
            .iter()
            .find(|key| key.get("kid").and_then(Value::as_str) == Some(kid))
        {
            return Ok(key.clone());
        }
        // 未知 kid 可触发轮换，但不能为每个伪造回调反复打发现接口。
        if cached.at.elapsed() < Duration::from_secs(30) {
            return Err(invalid());
        }
    }
    let discovery = http::get_json(client, DISCOVERY).await?;
    if discovery.get("issuer").and_then(Value::as_str) != Some(ISSUER) {
        return Err(invalid());
    }
    let jwks_uri = discovery
        .get("jwks_uri")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let url = url::Url::parse(jwks_uri).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let data = http::get_json(client, jwks_uri).await?;
    let keys = data
        .get("keys")
        .and_then(Value::as_array)
        .filter(|keys| !keys.is_empty() && keys.len() <= 64)
        .ok_or_else(invalid)?
        .clone();
    let mut ids = std::collections::HashSet::new();
    for key in &keys {
        let id = key
            .get("kid")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        if !ids.insert(id) {
            return Err(invalid());
        }
    }
    let key = keys
        .iter()
        .find(|key| key.get("kid").and_then(Value::as_str) == Some(kid))
        .cloned();
    *cache = Some(CachedKeys {
        keys,
        at: Instant::now(),
    });
    key.ok_or_else(invalid)
}

pub(super) async fn verify(
    client: &reqwest::Client,
    token: &str,
    client_id: &str,
    expected_nonce: Option<&str>,
    expected_subject: Option<&str>,
) -> Result<Value, GatewayError> {
    if token.len() > MAX_JWT_BYTES || client_id.is_empty() {
        return Err(invalid());
    }
    let mut parts = token.split('.');
    let header = parts.next().ok_or_else(invalid)?;
    let payload = parts.next().ok_or_else(invalid)?;
    let signature = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() {
        return Err(invalid());
    }
    let header_value: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if header_value.get("alg").and_then(Value::as_str) != Some("RS256")
        || header_value.get("crit").is_some()
        || header_value.get("b64").is_some()
    {
        return Err(invalid());
    }
    let kid = header_value
        .get("kid")
        .and_then(Value::as_str)
        .filter(|kid| !kid.is_empty() && kid.len() <= 256)
        .ok_or_else(invalid)?;
    let key = signing_key(client, kid).await?;
    if key.get("kty").and_then(Value::as_str) != Some("RSA")
        || key
            .get("alg")
            .is_some_and(|alg| alg.as_str() != Some("RS256"))
        || key
            .get("use")
            .is_some_and(|usage| usage.as_str() != Some("sig"))
        || key.get("key_ops").is_some_and(|ops| {
            !ops.as_array()
                .is_some_and(|ops| ops.iter().any(|op| op.as_str() == Some("verify")))
        })
    {
        return Err(invalid());
    }
    let component = |name: &str| -> Result<Vec<u8>, GatewayError> {
        URL_SAFE_NO_PAD
            .decode(key.get(name).and_then(Value::as_str).ok_or_else(invalid)?)
            .map_err(|_| invalid())
    };
    let n = component("n")?;
    let e = component("e")?;
    let signed = format!("{header}.{payload}");
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
    RsaPublicKeyComponents { n: &n, e: &e }
        .verify(&RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &signature)
        .map_err(|_| invalid())?;

    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if claims.get("iss").and_then(Value::as_str) != Some(ISSUER) {
        return Err(invalid());
    }
    let audience = claims.get("aud").ok_or_else(invalid)?;
    let matches_audience = audience.as_str() == Some(client_id)
        || audience
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(client_id)));
    if !matches_audience {
        return Err(invalid());
    }
    let azp = claims.get("azp").and_then(Value::as_str);
    if azp.is_some_and(|value| value != client_id)
        || audience.as_array().is_some_and(|values| values.len() > 1) && azp != Some(client_id)
    {
        return Err(invalid());
    }
    let now = logging::now_ms() / 1000;
    let expiry = claims
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or_else(invalid)?;
    let issued = claims
        .get("iat")
        .and_then(Value::as_i64)
        .ok_or_else(invalid)?;
    if expiry <= now - 60
        || issued > now + 60
        || issued <= 0
        || expiry < issued
        || claims
            .get("nbf")
            .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now + 60))
    {
        return Err(invalid());
    }
    if expected_nonce
        .is_some_and(|nonce| claims.get("nonce").and_then(Value::as_str) != Some(nonce))
    {
        return Err(invalid());
    }
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty() && sub.len() <= 512)
        .ok_or_else(invalid)?;
    if expected_subject.is_some_and(|expected| expected != subject) {
        return Err(invalid());
    }
    Ok(claims)
}
