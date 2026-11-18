use aes_gcm::{aead::consts::U12, AeadInOut, Aes256Gcm, KeyInit, Nonce};
use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use windows::core::Owned;
use windows::Win32::Foundation::HLOCAL;
use windows::Win32::Security::Cryptography::{
    CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::{Zeroize, Zeroizing};

use super::super::invalid;

struct Output(CRYPT_INTEGER_BLOB);
impl Drop for Output {
    fn drop(&mut self) {
        if self.0.pbData.is_null() {
            return;
        }
        unsafe {
            std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
            drop(Owned::new(HLOCAL(self.0.pbData.cast())));
        }
    }
}

fn key(local_state: &[u8]) -> Result<Output, proto::WireError> {
    let state: Value = serde_json::from_slice(local_state).map_err(|_| invalid())?;
    let encoded = state["os_crypt"]["encrypted_key"]
        .as_str()
        .filter(|value| value.len() <= 16384)
        .ok_or_else(invalid)?;
    let blob = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| invalid())?);
    let blob = blob
        .strip_prefix(b"DPAPI")
        .filter(|blob| !blob.is_empty())
        .ok_or_else(invalid)?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = Output(CRYPT_INTEGER_BLOB::default());
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.0,
        )
    }
    .map_err(|_| invalid())?;
    if output.0.pbData.is_null() || output.0.cbData != 32 {
        return Err(invalid());
    }
    Ok(output)
}

pub(super) fn encrypt(token: &str, local_state: &[u8]) -> Result<String, proto::WireError> {
    let output = key(local_state)?;
    let key = unsafe { std::slice::from_raw_parts(output.0.pbData, 32) };
    let mut random = [0u8; 12];
    getrandom::getrandom(&mut random).map_err(|_| invalid())?;
    let nonce = Nonce::<U12>::from(random);
    let mut encrypted = token.as_bytes().to_vec();
    Aes256Gcm::new_from_slice(key).map_err(|_| invalid())?
        .encrypt_in_place(&nonce, &[], &mut encrypted).map_err(|_| invalid())?;
    let mut bytes = b"v10".to_vec();
    bytes.extend_from_slice(&random);
    bytes.extend_from_slice(&encrypted);
    Ok(format!("enc:{}", STANDARD.encode(bytes)))
}

pub(super) fn decrypt(raw: &str, local_state: &[u8]) -> Result<String, proto::WireError> {
    let output = key(local_state)?;
    let key = unsafe { std::slice::from_raw_parts(output.0.pbData, 32) };
    let encrypted = Zeroizing::new(
        STANDARD
            .decode(raw.strip_prefix("enc:").ok_or_else(invalid)?)
            .map_err(|_| invalid())?,
    );
    if encrypted.len() <= 31 || encrypted.get(..3) != Some(b"v10") {
        return Err(invalid());
    }
    let nonce = Nonce::<U12>::try_from(&encrypted[3..15]).map_err(|_| invalid())?;
    let mut plain = Zeroizing::new(encrypted[15..].to_vec());
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| invalid())?
        .decrypt_in_place(&nonce, &[], &mut *plain)
        .map_err(|_| invalid())?;
    let token = std::str::from_utf8(&plain).map_err(|_| invalid())?.trim();
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    if token.is_empty()
        || token.len() > 32768
        || token
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(invalid());
    }
    Ok(token.to_string())
}
