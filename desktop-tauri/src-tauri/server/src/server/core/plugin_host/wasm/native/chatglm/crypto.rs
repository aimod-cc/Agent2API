//! 必要算法移植自同许可 Agent2WebProviders/crates/providers/src/crypto.rs。
//! Local State 不缓存；仅 v10 + 12 字节 nonce + 密文 + 16 字节 tag，DPAPI 禁止 UI。
//! Chromium schema 24 的域绑定不是填充，必须校验后才可去掉：
//! https://chromium.googlesource.com/chromium/src/+/master/net/extras/sqlite/sqlite_persistent_cookie_store.cc
//! https://learn.microsoft.com/windows/win32/api/dpapi/nf-dpapi-cryptunprotectdata

use std::cell::Cell;

use aes_gcm::{aead::consts::U12, AeadInOut, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use sha2::{Digest, Sha256};
use ::windows::core::Owned;
use ::windows::Win32::Foundation::HLOCAL;
use ::windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN};
use zeroize::{Zeroize, Zeroizing};

use super::{folders, validate_token, ReadError, COOKIE_HOST, MAX_TOKEN};

const MAX_KEY_TEXT: usize = 16 * 1024;
const MAX_KEY_BLOB: usize = 12 * 1024;

pub(super) fn key(local_state: &[u8]) -> Result<Zeroizing<Vec<u8>>, ReadError> {
    let failure = Cell::new(ReadError::Malformed);
    let mut deserializer = serde_json::Deserializer::from_slice(local_state);
    let encoded = Wanted { key: "os_crypt", inner: Some("encrypted_key"), failure: &failure }
        .deserialize(&mut deserializer).map_err(|_| failure.get())?;
    deserializer.end().map_err(|_| ReadError::Malformed)?;
    let mut bytes = Zeroizing::new(Vec::new());
    bytes.try_reserve_exact(MAX_KEY_BLOB).map_err(|_| ReadError::TooLarge)?;
    STANDARD.decode_vec(encoded.as_bytes(), &mut bytes).map_err(|_| ReadError::Malformed)?;
    if bytes.len() > MAX_KEY_BLOB { return Err(ReadError::TooLarge); }
    let blob = bytes.strip_prefix(b"DPAPI").filter(|blob| !blob.is_empty())
        .ok_or(ReadError::UnsupportedLayout)?;
    folders::ensure_process_user()?;
    unprotect(blob)
}

pub(super) fn token(plain: &[u8], encrypted: &[u8], key: &[u8], version: u32) -> Result<Zeroizing<String>, ReadError> {
    if !plain.is_empty() {
        if !encrypted.is_empty() { return Err(ReadError::Conflict); }
        return token_text(plain);
    }
    if encrypted.len() > MAX_TOKEN + 3 + 12 + 16 + 32 { return Err(ReadError::TooLarge); }
    if encrypted.len() <= 3 + 12 + 16 || encrypted.get(..3) != Some(b"v10".as_slice()) {
        return Err(ReadError::UnsupportedLayout);
    }
    if key.len() != 32 { return Err(ReadError::Decryption); }
    let nonce = Nonce::<U12>::try_from(&encrypted[3..15]).map_err(|_| ReadError::Malformed)?;
    let mut plaintext = Zeroizing::new(encrypted[15..].to_vec());
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| ReadError::Decryption)?;
    cipher.decrypt_in_place(&nonce, &[], &mut *plaintext).map_err(|_| ReadError::Decryption)?;
    match version {
        23 => token_text(&plaintext),
        24 => {
            let domain_hash = Sha256::digest(COOKIE_HOST.as_bytes());
            if plaintext.len() <= domain_hash.len() || plaintext[..32] != domain_hash[..] {
                return Err(ReadError::Decryption);
            }
            token_text(&plaintext[32..])
        }
        _ => Err(ReadError::UnsupportedLayout),
    }
}

fn token_text(bytes: &[u8]) -> Result<Zeroizing<String>, ReadError> {
    if bytes.len() > MAX_TOKEN { return Err(ReadError::TooLarge); }
    let value = std::str::from_utf8(bytes).map_err(|_| ReadError::Malformed)?;
    validate_token(value)?;
    Ok(Zeroizing::new(value.to_owned()))
}

struct DpapiOutput(CRYPT_INTEGER_BLOB);
impl Drop for DpapiOutput {
    fn drop(&mut self) {
        if self.0.pbData.is_null() { return; }
        // SAFETY: 仅持有 CryptUnprotectData 返回的分配；先清零再由 Owned<HLOCAL> 唯一释放。
        // 输出长度由 Windows API 给出，不从输入文件构造。
        unsafe {
            std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
            let owned = Owned::new(HLOCAL(self.0.pbData.cast()));
            drop(owned);
        }
    }
}

fn unprotect(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, ReadError> {
    if blob.is_empty() || blob.len() > MAX_KEY_BLOB { return Err(ReadError::TooLarge); }
    let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
    let mut output = DpapiOutput(CRYPT_INTEGER_BLOB::default());
    // SAFETY: 输入缓冲存活；input 为 API 约定的只读 blob；所有可选参数为空。
    // 仅当前用户 DPAPI，不加载进程、不弹解密 UI、不尝试其它账户或密钥。
    unsafe { CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output.0) }
        .map_err(|_| ReadError::Decryption)?;
    if output.0.pbData.is_null() || output.0.cbData != 32 { return Err(ReadError::Decryption); }
    // SAFETY: API 成功且长度已核对为 AES-256 的 32 字节；output 仍存活。
    let key = unsafe { std::slice::from_raw_parts(output.0.pbData, 32) };
    Ok(Zeroizing::new(key.to_vec()))
}

// 不把整份 Local State 解析成 Value：只投影两层所需字段，拒绝重复键，未知值由 IgnoredAny 跳过。
// Local State 外层字节上限由 files 保证，serde_json 默认递归限制保留；取出的敏感文本立即 Zeroizing。
struct Wanted<'a> {
    key: &'static str,
    inner: Option<&'static str>,
    failure: &'a Cell<ReadError>,
}
impl<'de> DeserializeSeed<'de> for Wanted<'_> {
    type Value = Zeroizing<String>;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Wanted<'_> {
    type Value = Zeroizing<String>;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result { formatter.write_str("bounded credential object") }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut value = None;
        let mut count = 0usize;
        while let Some(key) = map.next_key_seed(Text(self.failure))? {
            count += 1;
            if count > 512 {
                self.failure.set(ReadError::TooLarge);
                return Err(de::Error::custom("credential object limit"));
            }
            if key.as_str() == self.key {
                if value.is_some() {
                    self.failure.set(ReadError::Conflict);
                    return Err(de::Error::custom("duplicate credential field"));
                }
                value = Some(match self.inner {
                    Some(key) => map.next_value_seed(Wanted { key, inner: None, failure: self.failure })?,
                    None => map.next_value_seed(Text(self.failure))?,
                });
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        value.ok_or_else(|| de::Error::custom("missing credential field"))
    }
}
struct Text<'a>(&'a Cell<ReadError>);
impl<'de> DeserializeSeed<'de> for Text<'_> {
    type Value = Zeroizing<String>;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_string(self)
    }
}
impl<'de> Visitor<'de> for Text<'_> {
    type Value = Zeroizing<String>;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result { formatter.write_str("bounded credential string") }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        if value.len() > MAX_KEY_TEXT {
            self.0.set(ReadError::TooLarge);
            return Err(de::Error::custom("credential string limit"));
        }
        Ok(Zeroizing::new(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        let value = Zeroizing::new(value);
        if value.len() > MAX_KEY_TEXT {
            self.0.set(ReadError::TooLarge);
            return Err(de::Error::custom("credential string limit"));
        }
        Ok(value)
    }
}
