//! 用「同步密码」封装账号密钥文件（`account.key`），让它能安全地上云。
//!
//! ```text
//! seal_key(key_bytes, password)  → account.key.enc（JSON 文本）
//! open_key(envelope, password)   → key_bytes
//! ```
//!
//! ── 为什么需要这一层（而不是直接传 account.key）───────────────
//! `account.key` 是全部账号凭证与网关 Key 的对称密钥。把它原样传上云，等于
//! 「钥匙和锁放在一起」——任何拿到那个存储桶 / 网盘目录的人都能解开全部账号。
//! 所以上云的是**用同步密码派生的密钥加密后的**密钥文件；同步密码只存在用户
//! 脑子里（本机存的是它用另一把本机密钥封的信封，见 `settings` 模块头）。
//!
//! ── 为什么是 PBKDF2-HMAC-SHA256 而不是直接 sha256(密码) ────────
//! 密码的熵远低于随机密钥。直接哈希等于把「离线爆破一个密码」的成本降到
//! 「算一次 sha256」；PBKDF2 用迭代次数把每次尝试的成本抬到毫秒级，配合
//! 随机 salt，让针对某个用户的预计算表失效。
//! 迭代次数取 210000（OWASP 2023 对 PBKDF2-HMAC-SHA256 的推荐值）。
//! 该值写进信封（`iterations` 字段），将来调高时**旧信封仍能解开** ——
//! 这是「参数演进」与「向后兼容」能共存的关键，不要在代码里写死后只读不存。
//!
//! ── 不新增依赖 ──────────────────────────────────────────────
//! `hmac` / `sha2` / `aes-gcm` / `getrandom` / `base64` 都已在
//! `server/Cargo.toml` 的依赖里（分别为 ALTCHA、更新模块、账号加密、PKCE 与
//! 订阅二维码服务），这里只是把 PBKDF2 的 30 行循环写出来。
//!
//! ── 信封格式 ────────────────────────────────────────────────
//! ```json
//! {
//!   "format": "agent2api-cloud-sync-key",
//!   "version": 1,
//!   "kdf": "pbkdf2-hmac-sha256",
//!   "iterations": 210000,
//!   "salt": "<base64 32B>",
//!   "nonce": "<base64 12B>",
//!   "ciphertext": "<base64>",
//!   "keyFingerprint": "<sha256(明文密钥)[:16]>"
//! }
//! ```
//! `keyFingerprint` 是**明文密钥**的指纹（与 `crypto::key_fingerprint` 同口径），
//! 用来在解密后确认「解出来的确实是远端 manifest 说的那把钥匙」——密码正确但
//! 远端数据与 manifest 不配套时（上传中途被替换）能当场发现，而不是等到应用
//! 快照后账号全部解不开。

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::server::core::crypto;

pub(crate) const ENVELOPE_FORMAT: &str = "agent2api-cloud-sync-key";
pub(crate) const ENVELOPE_VERSION: u32 = 1;
/// 默认迭代次数（写进信封；调高后旧信封仍按自己记录的值解开）
pub(crate) const DEFAULT_ITERATIONS: u32 = 210_000;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// 账号密钥的固定长度（AES-256）
pub(crate) const KEY_LEN: usize = 32;

/// 信封（落盘 / 上云的 JSON 形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyEnvelope {
    pub format: String,
    pub version: u32,
    pub kdf: String,
    pub iterations: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    /// 明文密钥的指纹（见模块头）
    pub key_fingerprint: String,
}

/// PBKDF2-HMAC-SHA256 派生 32 字节。
///
/// 手写而不是引 `pbkdf2` crate：只有这一处用，30 行循环比多一个依赖划算
/// （与项目「不新增第三方依赖」的约定一致，见 `server/Cargo.toml`）。
fn derive_key(password: &str, salt: &[u8], iterations: u32) -> [u8; KEY_LEN] {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    // PBKDF2 的第一个块（dkLen = 32 = 一个 SHA-256 输出长度，只需一轮）
    let mut mac = HmacSha256::new_from_slice(password.as_bytes())
        .expect("HMAC 接受任意长度的密钥");
    mac.update(salt);
    mac.update(&1u32.to_be_bytes());
    let mut block = mac.finalize().into_bytes();
    let mut result = [0u8; KEY_LEN];
    result.copy_from_slice(&block);

    for _ in 1..iterations {
        let mut mac = HmacSha256::new_from_slice(password.as_bytes())
            .expect("HMAC 接受任意长度的密钥");
        mac.update(&block);
        block = mac.finalize().into_bytes();
        for (slot, byte) in result.iter_mut().zip(block.iter()) {
            *slot ^= *byte;
        }
    }
    result
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|error| format!("系统随机源不可用: {error}"))?;
    Ok(bytes)
}

/// 用同步密码封装账号密钥。
///
/// `key_bytes` 必须是 32 字节（`account.key` 的原文）；其它长度按错误上抛 ——
/// 悄悄补零 / 截断会让上云的那把钥匙与本地不是同一把，换机后账号全解不开。
pub(crate) fn seal_key(key_bytes: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if key_bytes.len() != KEY_LEN {
        return Err(format!(
            "账号密钥长度异常（应为 {KEY_LEN} 字节，实际 {} 字节）",
            key_bytes.len()
        ));
    }
    if password.is_empty() {
        return Err("同步密码不能为空".to_string());
    }
    use aes_gcm::aead::{consts::U12, Aead as _};
    use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};

    let salt: [u8; SALT_LEN] = random_bytes()?;
    let nonce_bytes: [u8; NONCE_LEN] = random_bytes()?;
    let derived = derive_key(password, &salt, DEFAULT_ITERATIONS);
    let cipher = Aes256Gcm::new_from_slice(&derived)
        .map_err(|_| "派生密钥长度不受 AES-256 支持".to_string())?;
    let nonce = Nonce::<U12>::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, key_bytes)
        .map_err(|_| "加密账号密钥失败".to_string())?;

    let envelope = KeyEnvelope {
        format: ENVELOPE_FORMAT.to_string(),
        version: ENVELOPE_VERSION,
        kdf: "pbkdf2-hmac-sha256".to_string(),
        iterations: DEFAULT_ITERATIONS,
        salt: BASE64.encode(salt),
        nonce: BASE64.encode(nonce_bytes),
        ciphertext: BASE64.encode(ciphertext),
        key_fingerprint: crypto::fingerprint_of(key_bytes),
    };
    serde_json::to_vec_pretty(&envelope).map_err(|error| format!("序列化密钥信封失败: {error}"))
}

/// 用同步密码解开账号密钥。
///
/// 密码错误 → GCM 校验失败 → 明确的「同步密码不正确」，**绝不**返回半截数据。
pub(crate) fn open_key(raw: &[u8], password: &str) -> Result<[u8; KEY_LEN], String> {
    use aes_gcm::aead::{consts::U12, Aead as _};
    use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};

    let envelope: KeyEnvelope = serde_json::from_slice(raw)
        .map_err(|_| "远端密钥文件的格式不认识（不是本功能生成的 account.key.enc）".to_string())?;
    if envelope.format != ENVELOPE_FORMAT {
        return Err(format!("远端密钥文件格式不兼容（format = {}）", envelope.format));
    }
    if envelope.version != ENVELOPE_VERSION {
        return Err(format!(
            "远端密钥文件版本不兼容（v{}，本机支持 v{ENVELOPE_VERSION}）",
            envelope.version
        ));
    }
    if envelope.kdf != "pbkdf2-hmac-sha256" {
        return Err(format!("不支持的密钥派生算法: {}", envelope.kdf));
    }
    if envelope.iterations == 0 || envelope.iterations > 10_000_000 {
        return Err("远端密钥文件的迭代次数异常".to_string());
    }
    let salt = BASE64
        .decode(&envelope.salt)
        .map_err(|_| "远端密钥文件的 salt 不是合法 base64".to_string())?;
    let nonce_bytes = BASE64
        .decode(&envelope.nonce)
        .map_err(|_| "远端密钥文件的 nonce 不是合法 base64".to_string())?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err("远端密钥文件的 nonce 长度异常".to_string());
    }
    let ciphertext = BASE64
        .decode(&envelope.ciphertext)
        .map_err(|_| "远端密钥文件的密文不是合法 base64".to_string())?;

    let derived = derive_key(password, &salt, envelope.iterations);
    let cipher = Aes256Gcm::new_from_slice(&derived)
        .map_err(|_| "派生密钥长度不受 AES-256 支持".to_string())?;
    let nonce = Nonce::<U12>::try_from(nonce_bytes.as_slice())
        .map_err(|_| "远端密钥文件的 nonce 长度异常".to_string())?;
    let plaintext = cipher.decrypt(&nonce, ciphertext.as_slice()).map_err(|_| {
        "同步密码不正确（无法解开远端保存的账号密钥）".to_string()
    })?;
    let key: [u8; KEY_LEN] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| "解密出的账号密钥长度异常".to_string())?;

    // 指纹自校验：密码对了但远端数据与 manifest 不配套时（上传中途被替换），
    // 这里能当场发现，而不是等应用快照后账号全部解不开。
    let actual = crypto::fingerprint_of(&key);
    if actual != envelope.key_fingerprint {
        return Err(
            "远端密钥文件自校验失败（指纹不一致），文件可能已损坏，请重新上传".to_string(),
        );
    }
    Ok(key)
}

/// 把解出的密钥装到本机：写 `account.key`，旧文件改名保留。
///
/// ── 为什么要先备份旧文件 ────────────────────────────────────
/// 下载可能是「用户想换回旧机器的状态」，也可能是「误操作」。旧密钥一旦被覆盖
/// 且没有副本，本机原有账号的密文就再也解不开了。改名成
/// `account.key.bak-<时间戳>` 而不是删除：出问题时用户能手工换回来。
///
/// ── 为什么必须重置 crypto 缓存 ──────────────────────────────
/// 见 `crypto::reset_cache` 的说明：不重置的话，本次运行剩余时间用的还是旧密钥。
pub(crate) fn install_key(key_bytes: &[u8]) -> Result<Option<String>, String> {
    if key_bytes.len() != KEY_LEN {
        return Err("账号密钥长度异常".to_string());
    }
    let path = crypto::key_file_path(crypto::SecretScope::Accounts);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("创建配置目录失败: {error}"))?;
    }
    let backup_name = if path.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup = path.with_file_name(format!("account.key.bak-{stamp}"));
        std::fs::rename(&path, &backup).map_err(|error| {
            format!(
                "备份原账号密钥失败（{}）：{error}",
                path.display()
            )
        })?;
        Some(
            backup
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    } else {
        None
    };
    std::fs::write(&path, key_bytes)
        .map_err(|error| format!("写入账号密钥失败（{}）：{error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    crypto::reset_cache(crypto::SecretScope::Accounts);
    Ok(backup_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open_round_trip() {
        let key = [7u8; KEY_LEN];
        let sealed = seal_key(&key, "correct horse battery").expect("seal");
        let opened = open_key(&sealed, "correct horse battery").expect("open");
        assert_eq!(opened, key);
    }

    #[test]
    fn open_rejects_wrong_password() {
        let key = [9u8; KEY_LEN];
        let sealed = seal_key(&key, "right-password").expect("seal");
        let error = open_key(&sealed, "wrong-password").expect_err("should fail");
        assert!(error.contains("同步密码不正确"), "unexpected: {error}");
    }

    #[test]
    fn envelope_records_iterations_for_forward_compat() {
        let sealed = seal_key(&[1u8; KEY_LEN], "password123").expect("seal");
        let envelope: KeyEnvelope = serde_json::from_slice(&sealed).expect("parse");
        assert_eq!(envelope.iterations, DEFAULT_ITERATIONS);
        assert_eq!(envelope.kdf, "pbkdf2-hmac-sha256");
        assert_eq!(envelope.key_fingerprint.len(), 16);
    }

    #[test]
    fn seal_rejects_wrong_key_length() {
        assert!(seal_key(&[0u8; 16], "password123").is_err());
    }
}
