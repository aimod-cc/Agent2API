//! 原OAuth/PKCE/DPoP的宿主纯校验，不生成上下文、私钥或proof，不请求令牌端点。
//! P-256推导复用本仓已依赖的p256；比旧插件校验更严格：完整坐标、规范编码、公钥无d。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::SigningKey;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{allowed, required, EnrollmentInputError};

type Error = EnrollmentInputError;

pub(super) fn validate(value: &Value) -> Result<&Value, Error> {
    let context = object(value, Error::InvalidClient)?;
    allowed(context, &["client_id", "pkce_pair", "dpop_key_pair"])?;
    let client = text(context, "client_id", 128, Error::InvalidClient)?;
    // 必须显式，原生缺省vscode-codebot只适用旧数据兼容，不适用首次纳管。
    if !client.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(Error::InvalidClient);
    }
    pkce(context.get("pkce_pair").ok_or(Error::MissingMaterial)?)?;
    dpop(context.get("dpop_key_pair").ok_or(Error::MissingMaterial)?)?;
    Ok(value)
}

fn pkce(value: &Value) -> Result<(), Error> {
    let fields = object(value, Error::InvalidPkce)?;
    allowed(
        fields,
        &["code_verifier", "code_challenge", "code_challenge_method"],
    )?;
    let verifier = text(fields, "code_verifier", 128, Error::InvalidPkce)?;
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    {
        return Err(Error::InvalidPkce);
    }
    let method = text(fields, "code_challenge_method", 16, Error::InvalidPkce)?;
    // 原生CodeArts portal采用SHA-256字面名；标准S256是同一算法，不转换原值。
    if !matches!(method, "SHA-256" | "S256") {
        return Err(Error::InvalidPkce);
    }
    let challenge = text(fields, "code_challenge", 43, Error::InvalidPkce)?;
    let actual = decode32(challenge, Error::InvalidPkce)?;
    let expected = Zeroizing::new(Sha256::digest(verifier.as_bytes()).to_vec());
    if actual.as_slice() != expected.as_slice() {
        return Err(Error::InvalidPkce);
    }
    Ok(())
}

fn dpop(value: &Value) -> Result<(), Error> {
    let fields = object(value, Error::InvalidDpop)?;
    allowed(fields, &["private_key_jwk", "public_key_jwk"])?;
    let private = object(
        fields
            .get("private_key_jwk")
            .ok_or(Error::MissingMaterial)?,
        Error::InvalidDpop,
    )?;
    let public = object(
        fields.get("public_key_jwk").ok_or(Error::MissingMaterial)?,
        Error::InvalidDpop,
    )?;
    allowed(private, &["kty", "crv", "x", "y", "d"])?;
    allowed(public, &["kty", "crv", "x", "y"])?; // 公钥即使d为空也拒绝。
    for jwk in [private, public] {
        if text(jwk, "kty", 8, Error::InvalidDpop)? != "EC"
            || text(jwk, "crv", 16, Error::InvalidDpop)? != "P-256"
        {
            return Err(Error::InvalidDpop);
        }
    }
    let scalar = decode32(
        text(private, "d", 43, Error::InvalidDpop)?,
        Error::InvalidDpop,
    )?;
    let signing = SigningKey::from_slice(&scalar).map_err(|_| Error::InvalidDpop)?;
    let derived = signing.verifying_key().to_encoded_point(false);
    let x = derived.x().ok_or(Error::InvalidDpop)?;
    let y = derived.y().ok_or(Error::InvalidDpop)?;
    // 两份JWK都必须实际给出完整坐标；不由缺失公钥补造，再声称关联验证完成。
    for jwk in [private, public] {
        let declared_x = decode32(text(jwk, "x", 43, Error::InvalidDpop)?, Error::InvalidDpop)?;
        let declared_y = decode32(text(jwk, "y", 43, Error::InvalidDpop)?, Error::InvalidDpop)?;
        if declared_x.as_slice() != &x[..] || declared_y.as_slice() != &y[..] {
            return Err(Error::InvalidDpop);
        }
    }
    Ok(())
}

fn decode32(value: &str, error: Error) -> Result<Zeroizing<Vec<u8>>, Error> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(error);
    }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(value).map_err(|_| error)?);
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
    if bytes.len() != 32 || encoded.as_str() != value {
        return Err(error);
    }
    Ok(bytes)
}

fn object(value: &Value, error: Error) -> Result<&Map<String, Value>, Error> {
    value.as_object().ok_or(error)
}

fn text<'a>(
    fields: &'a Map<String, Value>,
    name: &str,
    maximum: usize,
    error: Error,
) -> Result<&'a str, Error> {
    required(fields, &[name], maximum).map_err(|failure| match failure {
        Error::MissingMaterial => failure,
        _ => error,
    })
}
