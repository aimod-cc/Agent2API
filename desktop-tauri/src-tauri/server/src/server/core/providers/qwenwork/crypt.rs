//! 千问办公登录态的解密原语：DPAPI + AES-256-GCM + base64。
//!
//! ── 为什么单独一个文件（而不是塞进 `auth.rs`）───────────────────
//! 这三件事都是**通用密码学原语**，与「千问办公的账号长什么样」无关：
//! 将来接任何把登录态存在 Electron `os_crypt` 里的客户端（同一个上游家族
//! 的其它换皮），这层都能原样复用。`auth.rs` 只负责「读出哪两个文件、
//! 字段怎么映射」。
//!
//! ── Chromium os_crypt 的真实布局（照抄不会有错）────────────────
//! ```text
//!   Local State → os_crypt.encrypted_key
//!       = base64( "DPAPI" + DPAPI(32 字节 AES-256 主密钥) )
//!
//!   auth-v2.dat
//!       = "v10" + nonce(12B) + ciphertext(N B) + tag(16B)
//!         用主密钥做 AES-256-GCM，AAD 为空
//! ```
//! 注意 `v10` 之后的 12 字节是 **nonce**、最后 16 字节是 **GCM tag**，
//! 中间才是密文 —— `aes-gcm` crate 的 `decrypt` 接口要的是「nonce + 密文||tag」
//! 这一段整体，所以这里**不**手工切 tag，只把 tag 长度校验一遍。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 全部返回 `Result`，任何一步失败都给出可读原因（用户在「导入本机登录态」
//! 时会看到这句话，含糊的「解密失败」帮不上忙）。

use crate::server::errors::GatewayError;

/// base64 解码（标准字母表，容忍缺失的 padding）。
///
/// Chromium 存 `encrypted_key` 时用的是标准 base64，但不同版本对 `=` 的处理
/// 不一致，所以这里两种都收。
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let trimmed = text.trim();
    if let Ok(bytes) = engine.decode(trimmed) {
        return Some(bytes);
    }
    // 补齐 padding 再试一次
    let pad = (4 - trimmed.len() % 4) % 4;
    let mut padded = String::with_capacity(trimmed.len() + pad);
    padded.push_str(trimmed);
    for _ in 0..pad {
        padded.push('=');
    }
    engine.decode(padded).ok()
}

/// 用 DPAPI 解出 Chromium 的 AES-256 主密钥。
///
/// `CryptUnprotectData` 以**当前进程用户**身份解密，不需要密码 ——
/// 因此 Agent2API 与千问办公必须跑在同一个 Windows 用户下（正常情况下都是）。
#[cfg(windows)]
pub fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, GatewayError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    if blob.is_empty() {
        return Err(GatewayError::with_status(400, "DPAPI 密文为空"));
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: 输入指针指向 `blob` 的只读数据，`cbData` 与切片长度一致；
    // 输出结构由 API 填充，成功后必须用 `LocalFree` 归还。
    let ok = unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut output) };
    if ok.is_err() {
        return Err(GatewayError::with_status(
            400,
            "DPAPI 解密失败：Agent2API 与千问办公可能不在同一 Windows 用户下",
        ));
    }
    // SAFETY: 成功后 `output.pbData` 指向 API 分配的缓冲区，长度为 `cbData`。
    let bytes = if output.pbData.is_null() || output.cbData == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    // SAFETY: 配对 `CryptUnprotectData` 的分配，归还所有权；即使 `bytes` 已在
    // 上面拷贝出来，也必须释放原始缓冲区，否则每次调用泄漏一块。
    unsafe {
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
    }
    if bytes.is_empty() {
        return Err(GatewayError::with_status(400, "DPAPI 解密结果为空"));
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn dpapi_unprotect(_blob: &[u8]) -> Result<Vec<u8>, GatewayError> {
    Err(GatewayError::with_status(400, "非 Windows 系统没有 DPAPI"))
}

/// 用 AES-256-GCM 解 `v10` 载荷（`nonce` 12 字节 + `cipher||tag`）。
pub fn aes256_gcm_open(key: &[u8], nonce: &[u8], cipher_with_tag: &[u8]) -> Result<Vec<u8>, GatewayError> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    if key.len() != 32 {
        return Err(GatewayError::with_status(
            400,
            format!("AES-256 主密钥长度应为 32，实际 {}", key.len()),
        ));
    }
    if nonce.len() != 12 {
        return Err(GatewayError::with_status(
            400,
            format!("GCM nonce 长度应为 12，实际 {}", nonce.len()),
        ));
    }
    if cipher_with_tag.len() < 16 {
        return Err(GatewayError::with_status(400, "GCM 密文长度不足（缺 tag）"));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| GatewayError::with_status(400, "AES-256 主密钥初始化失败"))?;
    // `Nonce::from_slice` 已废弃（新版本改用 `TryFrom`），但它返回的是引用，
    // 而 `decrypt` 收 `&Nonce` —— 这里用 `Nonce::from` 拷进数组再借用，
    // 效果相同且不触碰废弃 API。
    let nonce_ref = &Nonce::from(<[u8; 12]>::try_from(nonce).map_err(|_| {
        GatewayError::with_status(400, "GCM nonce 长度应为 12")
    })?);
    cipher.decrypt(nonce_ref, cipher_with_tag).map_err(|_| {
        GatewayError::with_status(
            400,
            "AES-GCM 解密失败：登录态文件已损坏，或主密钥与文件不匹配",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_accepts_padded_and_unpadded() {
        // "hello" 的 base64 是 aGVsbG8=，去 padding 后 aGVsbG8
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert!(base64_decode("!!!not base64!!!").is_none());
    }

    #[test]
    fn aes_gcm_roundtrip() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};

        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let sealed = cipher
            .encrypt(Nonce::from_slice(&nonce), &b"{\"schemaVersion\":2}"[..])
            .unwrap();
        let opened = aes256_gcm_open(&key, &nonce, &sealed).expect("open");
        assert_eq!(opened, b"{\"schemaVersion\":2}");
    }

    #[test]
    fn aes_gcm_rejects_bad_key_length() {
        assert!(aes256_gcm_open(&[0u8; 16], &[0u8; 12], &[0u8; 32]).is_err());
    }

    #[test]
    fn aes_gcm_rejects_short_cipher() {
        assert!(aes256_gcm_open(&[0u8; 32], &[0u8; 12], &[0u8; 4]).is_err());
    }
}
