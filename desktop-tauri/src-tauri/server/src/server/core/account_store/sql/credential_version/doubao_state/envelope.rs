//! 固定enc1信封的原地有界加解密；沿用AES-256-GCM/12字节nonce/标准Base64，不碰密钥加载。
//! 明文与密文工作缓冲均留在外持有Envelope，解密/加密失败不在Db锁内drop部分大缓冲。

use aes_gcm::{aead::consts::U12, AeadInOut, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use zeroize::Zeroizing;

use super::super::{bounded, Error, StorageFailure};
use super::StateError;

const PREFIX: &str = "enc1:";
const NONCE: usize = 12;
const TAG: usize = 16;

pub(super) struct Envelope {
    pub(super) plaintext: Zeroizing<Vec<u8>>,
    blob: Zeroizing<Vec<u8>>,
    pub(super) encoded: String,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            plaintext: Zeroizing::new(Vec::with_capacity(bounded::MAX_ROW_BYTES)),
            blob: Zeroizing::new(Vec::with_capacity(bounded::MAX_ROW_BYTES)), encoded: String::new(),
        }
    }
}

impl Envelope {
    pub(super) fn reserve_write(&mut self) {
        self.encoded.reserve(bounded::MAX_ROW_BYTES);
    }
}

pub(super) fn open_into(encoded: &str, key: &[u8; 32], out: &mut Envelope) -> Result<(), StateError> {
    if !out.plaintext.is_empty() || !out.blob.is_empty() || !out.encoded.is_empty() {
        return Err(Error::InvalidRecord.into());
    }
    let encoded = encoded.trim().strip_prefix(PREFIX).ok_or(Error::InvalidRecord)?;
    if encoded.len() > bounded::MAX_ROW_BYTES { return Err(Error::LimitExceeded.into()); }
    STANDARD.decode_vec(encoded, &mut out.blob).map_err(|_| Error::InvalidRecord)?;
    if out.blob.len() <= NONCE + TAG { return Err(Error::InvalidRecord.into()); }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::InvalidRecord)?;
    let nonce = Nonce::<U12>::try_from(&out.blob[..NONCE]).map_err(|_| Error::InvalidRecord)?;
    out.plaintext.extend_from_slice(&out.blob[NONCE..]);
    cipher.decrypt_in_place(&nonce, b"", &mut *out.plaintext).map_err(|_| Error::InvalidRecord)?;
    std::str::from_utf8(&out.plaintext).map_err(|_| Error::InvalidRecord)?;
    Ok(())
}

pub(super) fn seal_into(key: &[u8; 32], out: &mut Envelope) -> Result<(), StateError> {
    if !out.blob.is_empty() || !out.encoded.is_empty() || out.plaintext.len() > bounded::MAX_PLAIN_BYTES {
        return Err(Error::LimitExceeded.into());
    }
    let mut nonce = [0u8; NONCE];
    getrandom::getrandom(&mut nonce).map_err(|_| Error::Storage(StorageFailure::RandomUnavailable))?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    out.blob.extend_from_slice(&out.plaintext);
    let nonce_value = Nonce::<U12>::try_from(nonce.as_slice()).map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    cipher.encrypt_in_place(&nonce_value, b"", &mut *out.blob)
        .map_err(|_| Error::Storage(StorageFailure::Encryption))?;
    // encode_string直接追加至外持有输出；在原blob前置nonce，不format或另造大串。
    out.encoded.push_str(PREFIX);
    out.blob.splice(0..0, nonce);
    STANDARD.encode_string(&out.blob, &mut out.encoded);
    Ok(())
}
