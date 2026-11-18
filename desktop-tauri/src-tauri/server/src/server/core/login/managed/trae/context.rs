//! Trae 当轮私有 PKCE/设备上下文；协议依据原生 trae/oauth.rs 与 device.rs。
//! verifier只进原宿主固定换码，私钥只进本轮有限secret；不读取客户端文件或环境凭据。

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use p256::{
    ecdsa::SigningKey,
    pkcs8::{EncodePrivateKey, EncodePublicKey},
};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::server::{
    core::plugin_host::wasm::managed_policy::TRAE_AUTHORIZATION_ORIGIN, errors::GatewayError,
};

pub(super) const IDE_VERSION: &str = "0.1.69";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Variant {
    Cn,
    Solo,
}

impl Variant {
    pub(super) fn parse(value: &str) -> Result<Self, GatewayError> {
        match value {
            "cn" => Ok(Self::Cn),
            "solo" => Ok(Self::Solo),
            _ => Err(GatewayError::bad_request(
                "Trae 本次登录只接受明确的 cn 或 solo，未猜测归属",
            )),
        }
    }
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Solo => "solo",
        }
    }
    pub(super) fn platform_code(self) -> &'static str {
        match self {
            Self::Cn => "IDE_PC",
            Self::Solo => "SOLO_PC",
        }
    }
    pub(super) fn client_id(self) -> &'static str {
        match self {
            Self::Cn => "ono9krqynydwx5",
            Self::Solo => "en1oxy7wnw8j9n",
        }
    }
}

/// 字段仅同一 Trae 宿主模块的下一片消费者可见，没有 JSON/Clone/Debug 入口。
pub(super) struct ProtocolContext {
    pub(super) variant: Variant,
    pub(super) verifier: Zeroizing<String>,
    pub(super) device_public: String,
    pub(super) device_private: Zeroizing<String>,
    pub(super) device_id: String,
    pub(super) machine_id: String,
    pub(super) trace: String,
    pub(super) platform: &'static str,
    pub(super) callback: String,
    pub(super) authorization: String,
}

impl ProtocolContext {
    pub(super) fn generate(variant: Variant, port: u16) -> Result<Self, GatewayError> {
        if port == 0 {
            return Err(invalid());
        }
        let mut random = Zeroizing::new([0u8; 48]);
        getrandom::getrandom(&mut *random).map_err(|_| invalid())?;
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*random));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let (public, private) = device_keys()?;
        let device_id = digits()?;
        let machine_id = uuid()?;
        let trace = uuid()?;
        let platform = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "mac",
            "linux" => "linux",
            _ => "other",
        };
        let callback = format!("http://127.0.0.1:{port}/authorize");
        let mut params: Vec<(&str, String, bool)> = vec![
            ("login_version", "1".into(), false),
            (
                "auth_from",
                if variant == Variant::Solo {
                    "solo"
                } else {
                    "trae"
                }
                .into(),
                false,
            ),
            ("login_channel", "native_ide".into(), false),
            ("plugin_version", "1.0.0".into(), true),
            ("auth_type", "local".into(), false),
            ("client_id", variant.client_id().into(), false),
            ("redirect", "0".into(), false),
            ("login_trace_id", trace.clone(), true),
            // 原生官方页按精确127.0.0.1:/authorize形态匹配，此字段不URL编码。
            ("auth_callback_url", callback.clone(), false),
            ("machine_id", machine_id.clone(), true),
            ("device_id", device_id.clone(), true),
            ("x_device_id", device_id.clone(), true),
            ("x_machine_id", machine_id.clone(), true),
            // 未读取硬件品牌或系统发行版，未知值留空，不能冒充原生示例设备。
            ("x_device_brand", String::new(), true),
            ("x_device_type", platform.into(), true),
            ("x_os_version", String::new(), true),
            ("x_env", "prod".into(), true),
            ("x_app_version", IDE_VERSION.into(), true),
            ("x_app_type", "trae".into(), true),
            ("code_challenge", challenge, true),
            ("code_challenge_method", "S256".into(), false),
        ];
        if variant == Variant::Solo {
            params.push(("hide_saas_login", "true".into(), false));
        }
        let query = params
            .into_iter()
            .map(|(key, value, escaped)| {
                format!("{key}={}", if escaped { encode(&value) } else { value })
            })
            .collect::<Vec<_>>()
            .join("&");
        let authorization = format!("{TRAE_AUTHORIZATION_ORIGIN}/authorization?{query}");
        Ok(Self {
            variant,
            verifier,
            device_public: public,
            device_private: private,
            device_id,
            machine_id,
            trace,
            platform,
            callback,
            authorization,
        })
    }
}

fn device_keys() -> Result<(String, Zeroizing<String>), GatewayError> {
    for _ in 0..16 {
        let mut scalar = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(&mut *scalar).map_err(|_| invalid())?;
        let key = SigningKey::from_slice(&*scalar);
        scalar.zeroize();
        let Ok(key) = key else {
            continue;
        };
        let public = key
            .verifying_key()
            .to_public_key_der()
            .map_err(|_| invalid())?;
        let private = key.to_pkcs8_der().map_err(|_| invalid())?;
        return Ok((
            pem("PUBLIC KEY", public.as_bytes()),
            Zeroizing::new(pem("PRIVATE KEY", private.as_bytes())),
        ));
    }
    Err(invalid())
}

fn pem(label: &str, bytes: &[u8]) -> String {
    let encoded = Zeroizing::new(STANDARD.encode(bytes));
    let mut result = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        result.extend(line.iter().map(|byte| char::from(*byte)));
        result.push('\n');
    }
    result.push_str(&format!("-----END {label}-----\n"));
    result
}

fn digits() -> Result<String, GatewayError> {
    let mut result = String::new();
    for _ in 0..16 {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(&mut *bytes).map_err(|_| invalid())?;
        for &byte in bytes.iter().filter(|byte| **byte < 250) {
            result.push(char::from(b'0' + byte % 10));
            if result.len() == 16 {
                return Ok(result);
            }
        }
    }
    Err(invalid())
}

fn uuid() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| invalid())?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(char::from(byte))
            }
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}
fn invalid() -> GatewayError {
    GatewayError::with_status(503, "Trae 本轮安全登录材料生成失败，未发起授权")
}
