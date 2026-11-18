//! 原 P-256 密钥的纯校验和固定 STS DPoP；不生成新密钥，不读取时钟或系统随机源。
//! 核心沿用仓内 codearts/dpop.rs；完整公私坐标和规范编码与首次登记校验一致。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::hazmat::PrehashSigner, Signature, SigningKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{allowed, clock, object, required, ProtocolError, Result, TOKEN_URI};

/// 无 Clone/Debug/serde；保存原私钥标量，不接受按字符串重建任何调用或发送许可。
/// scalar由Zeroizing持有；p256临时SigningKey由库自己的析构负责尽力清理。
pub(super) struct ProofKey {
    scalar: Zeroizing<Vec<u8>>,
    x: Zeroizing<String>,
    y: Zeroizing<String>,
}

impl ProofKey {
    pub(super) fn from_pair(value: &Value) -> Result<Self> {
        let failure = ProtocolError::InvalidDpop;
        let pair = object(value, failure)?;
        allowed(pair, &["private_key_jwk", "public_key_jwk"], failure)?;
        let private = object(pair.get("private_key_jwk").ok_or(failure)?, failure)?;
        let public = object(pair.get("public_key_jwk").ok_or(failure)?, failure)?;
        allowed(private, &["kty", "crv", "x", "y", "d"], failure)?;
        allowed(public, &["kty", "crv", "x", "y"], failure)?;
        for fields in [private, public] {
            if required(fields, &["kty"], 8).map_err(|_| failure)? != "EC"
                || required(fields, &["crv"], 16).map_err(|_| failure)? != "P-256"
            {
                return Err(failure);
            }
        }
        let scalar = decode32(required(private, &["d"], 43).map_err(|_| failure)?, failure)?;
        let signing = SigningKey::from_slice(scalar.as_slice()).map_err(|_| failure)?;
        let point = signing.verifying_key().to_encoded_point(false);
        let actual_x = point.x().ok_or(failure)?;
        let actual_y = point.y().ok_or(failure)?;
        for fields in [private, public] {
            let x = decode32(required(fields, &["x"], 43).map_err(|_| failure)?, failure)?;
            let y = decode32(required(fields, &["y"], 43).map_err(|_| failure)?, failure)?;
            if x.as_slice() != &actual_x[..] || y.as_slice() != &actual_y[..] {
                return Err(failure);
            }
        }
        // 两份完整声明均已真实比对；这不是为缺少的原公钥填入默认值。
        let x = Zeroizing::new(URL_SAFE_NO_PAD.encode(actual_x));
        let y = Zeroizing::new(URL_SAFE_NO_PAD.encode(actual_y));
        Ok(Self { scalar, x, y })
    }

    pub(super) fn token_proof(&self, now_ms: i64, jti: &[u8; 32]) -> Result<Zeroizing<String>> {
        clock(now_ms)?;
        if jti.iter().all(|byte| *byte == 0) {
            return Err(ProtocolError::InvalidRandom);
        }
        let mut id = Zeroizing::new(String::with_capacity(64));
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &byte in jti {
            id.push(HEX[(byte >> 4) as usize] as char);
            id.push(HEX[(byte & 15) as usize] as char);
        }
        let header = Zeroizing::new(
            serde_json::to_vec(&Header {
                alg: "ES256",
                typ: "dpop+jwt",
                jwk: PublicJwk {
                    kty: "EC",
                    crv: "P-256",
                    x: &self.x,
                    y: &self.y,
                },
            })
            .map_err(|_| ProtocolError::SigningFailed)?,
        );
        let payload = Zeroizing::new(
            serde_json::to_vec(&Payload {
                htm: "POST",
                htu: TOKEN_URI,
                iat: now_ms / 1000,
                jti: &id,
            })
            .map_err(|_| ProtocolError::SigningFailed)?,
        );
        let mut proof = Zeroizing::new(String::with_capacity(2048));
        URL_SAFE_NO_PAD.encode_string(header.as_slice(), &mut proof);
        proof.push('.');
        URL_SAFE_NO_PAD.encode_string(payload.as_slice(), &mut proof);
        let key = SigningKey::from_slice(self.scalar.as_slice())
            .map_err(|_| ProtocolError::InvalidDpop)?;
        let digest = Zeroizing::new(Sha256::digest(proof.as_bytes()).to_vec());
        let signature: Signature = key
            .sign_prehash(digest.as_slice())
            .map_err(|_| ProtocolError::SigningFailed)?;
        proof.push('.');
        // JOSE固定64字节r||s，不转DER，不加nonce/ath或替换算法，不作low-S改写。
        URL_SAFE_NO_PAD.encode_string(signature.to_bytes(), &mut proof);
        Ok(proof)
    }
}

#[derive(Serialize)]
struct PublicJwk<'a> {
    kty: &'static str,
    crv: &'static str,
    x: &'a str,
    y: &'a str,
}
#[derive(Serialize)]
struct Header<'a> {
    alg: &'static str,
    typ: &'static str,
    jwk: PublicJwk<'a>,
}
#[derive(Serialize)]
struct Payload<'a> {
    htm: &'static str,
    htu: &'static str,
    iat: i64,
    jti: &'a str,
}

pub(super) fn decode32(value: &str, failure: ProtocolError) -> Result<Zeroizing<Vec<u8>>> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(failure);
    }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(value).map_err(|_| failure)?);
    let canonical = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
    if bytes.len() != 32 || canonical.as_str() != value {
        return Err(failure);
    }
    Ok(bytes)
}
