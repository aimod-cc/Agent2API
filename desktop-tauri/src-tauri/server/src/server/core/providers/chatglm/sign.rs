//! 智谱清言（`chatglm.cn`）请求签名与设备号 —— 从客户端 2.0.7 的
//! `auth-headers.js` 逆向（唯一事实来源是壳侧那份实现，本文件逐步骤对照）。
//!
//! ── 三件套（每次请求都要现算）────────────────────────────────
//! ```text
//!   X-Timestamp  智谱自创的校验位算法（**不是普通时间戳**）：
//!                取毫秒串，"各位数字之和 − 倒数第二位" 再 % 10，
//!                把这一位写回倒数第二位。照抄才能过 —— 直接发真实毫秒串
//!                会被服务端判 40013（时间戳不合法）。
//!   X-Nonce      UUID 去掉横线（32 位十六进制，客户端用 crypto.randomUUID）
//!   X-Sign       md5(`{timestamp}-{nonce}-{secret}`) 的十六进制
//! ```
//!
//! ── 密钥随环境不同（两把，都是客户端内置的）────────────────────
//! 生产（`chatglm.cn` / `agentmore.chatglm.cn`）与测试域各一把；
//! `pick_secret` 按 base 域名判定，与客户端 `pickSignSecret` 同口径。
//!
//! ── 实测的校验边界（2026-10，决定各处怎么用）──────────────────
//! | 情况 | 结果 |
//! |---|---|
//! | 密钥写错 | `40014` 拒绝 |
//! | 时间戳过期 1 小时 | `40013` 拒绝 |
//! | 缺 `X-Device-Id` | `40001` / `40004` 拒绝 |
//! | 缺 `Authorization` | `40003` 拒绝 |
//! | `X-Device-Id` 非 UUID（32 位 hex / 短串 / 纯时间戳） | `40004` 拒绝 |
//!
//! 两条结论写进实现里：**签名与设备号都是硬校验，不能省**；
//! 设备号必须 UUID 形态（`getrandom` 生成 16 字节后按 UUID v4 排版）。
//!
//! ── 设备号是身份的一部分（不是可丢弃的装饰）────────────────────
//! 服务端把 `X-Device-Id` 写进签发的 JWT（`device_id` 声明），且访客身份
//! 按设备号分配 —— **换设备号 = 换一个访客身份**（实测两个不同 UUID 各自
//! 换到了不同的 `user_id`）。因此账号里的设备号必须**持久化**：
//! 每次请求现随机一个，等于每次都在换身份、丢掉会话连续性。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use md5::{Digest, Md5};

/// 生产环境签名密钥（客户端 `auth-headers.js` 内置）
pub const SIGN_SECRET_PROD: &str = "8a1317a7468aa3ad86e997d08f3f31cb";

/// 测试环境签名密钥（同文件另一常量）
pub const SIGN_SECRET_TEST: &str = "befbc3fe57555ffa1daaf52b39be0fbe";

/// 智谱的时间戳算法（客户端 `getXTimestamp` 的逐字复刻）。
///
/// ```text
///   ms = Date.now().toString()
///   sum = Σ(每个数字位) − 倒数第二位
///   result = ms[0..len-2] + (sum % 10) + ms[len-1]
/// ```
/// 倒数第二位被**替换**掉（不是插入），因此结果与输入**等长**。
/// `ms` 长度不足 2 位时原样返回 —— 真实时钟不会有这种值，只是不 panic。
pub fn x_timestamp_from(millis: u128) -> String {
    let ms = millis.to_string();
    let len = ms.len();
    if len < 2 {
        return ms;
    }
    let mut sum: u32 = 0;
    for byte in ms.bytes() {
        // 到这一步每个字节都必然是 ASCII 数字（来自 u128 的 Display）
        sum += u32::from(byte.saturating_sub(b'0'));
    }
    // 倒数第二位的下标（= len - 2）；越界不可能，稳妥起见用 get 取值
    let Some(penultimate) = ms.as_bytes().get(len - 2).map(|byte| u32::from(*byte - b'0'))
    else {
        return ms;
    };
    let checksum = (sum.saturating_sub(penultimate)) % 10;
    let head = ms.get(..len - 2).unwrap_or("");
    let tail = ms.get(len - 1..).unwrap_or("");
    format!("{head}{checksum}{tail}")
}

/// 当前时刻的 `X-Timestamp`
pub fn x_timestamp() -> String {
    x_timestamp_from(crate::server::logging::now_ms().max(0) as u128)
}

/// 生成一个 UUID v4 字符串（`8-4-4-4-12` 带横线）。
///
/// 设备号必须是这个形态（实测非 UUID 会被 40004 拒绝），因此不能拿
/// 「32 位 hex」或时间戳凑数。随机源是 `getrandom`（本项目已是直接依赖）。
/// 随机源失败（几乎不可能）时退回时间戳派生的形态：可读性差但**仍是 UUID 形态**，
/// 比发一个注定 40004 的值好。
pub fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let millis = crate::server::logging::now_ms().max(0) as u128;
        bytes[..16].copy_from_slice(&millis.to_le_bytes()[..16]);
    }
    // 版本位（第 7 字节高 4 位 = 0100）与变体位（第 9 字节高 2 位 = 10）
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let part = |start: usize, end: usize| hex.get(start..end).unwrap_or("").to_string();
    format!(
        "{}-{}-{}-{}-{}",
        part(0, 8),
        part(8, 12),
        part(12, 16),
        part(16, 20),
        part(20, 32)
    )
}

/// `X-Nonce`：UUID 去掉横线（客户端 `getXNonce` 的形态）
pub fn x_nonce() -> String {
    uuid_v4().replace('-', "")
}

/// 按 base URL 选签名密钥（与客户端 `pickSignSecret` 同口径）。
///
/// 判定顺序刻意与客户端一致：先看测试/预发特征，再看生产域，兜底给测试密钥。
pub fn pick_secret(base_url: &str) -> &'static str {
    let value = base_url.to_ascii_lowercase();
    if value.contains("test.chatglm.cn") || value.contains("staging") || value.contains("dev") {
        return SIGN_SECRET_TEST;
    }
    if value.contains("chatglm.cn") {
        return SIGN_SECRET_PROD;
    }
    SIGN_SECRET_TEST
}

/// 一次签名的三个头（同一组值必须一起用：sign 由这对 timestamp/nonce 算出）
#[derive(Clone, Debug)]
pub struct SignHeaders {
    pub timestamp: String,
    pub nonce: String,
    pub sign: String,
}

/// 生成一组签名头（`X-Timestamp` / `X-Nonce` / `X-Sign`）。
pub fn sign_headers(base_url: &str) -> SignHeaders {
    sign_headers_with(base_url, x_timestamp(), x_nonce())
}

/// 用指定的 timestamp / nonce 算签名（测试与「同一组值复用」用）
pub fn sign_headers_with(base_url: &str, timestamp: String, nonce: String) -> SignHeaders {
    let secret = pick_secret(base_url);
    let mut hasher = Md5::new();
    hasher.update(format!("{timestamp}-{nonce}-{secret}").as_bytes());
    let sign = hasher.finalize();
    let sign = sign.iter().map(|byte| format!("{byte:02x}")).collect();
    SignHeaders {
        timestamp,
        nonce,
        sign,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 时间戳算法与客户端实现逐字一致（用固定的毫秒值锁定，防止将来改公式）
    #[test]
    fn timestamp_matches_client_algorithm() {
        // 每一步的算式（与客户端 getXTimestamp 逐字对照）：
        //   1792175060134 → 各位和 1+7+9+2+1+7+5+0+6+0+1+3+4 = 46，减倒数第二位 3 = 43，43 % 10 = 3
        //   → 倒数第二位写回 3（原值就是 3，故与输入相同）
        assert_eq!(x_timestamp_from(1792175060134), "1792175060134");
        //   1792175060145 → 和 48，减倒数第二位 4 = 44，% 10 = 4 → 写回 4（与原值相同）
        assert_eq!(x_timestamp_from(1792175060145), "1792175060145");
        //   1792175060178 → 和 54，减倒数第二位 7 = 47，% 10 = 7 → 写回 7（与原值相同）
        assert_eq!(x_timestamp_from(1792175060178), "1792175060178");
        // 一个**必然改变**倒数第二位的例子（这条才真正验到"替换"这一步）：
        //   1792175060149 → 和 55，减倒数第二位 4 = 51，% 10 = 1 → 倒数第二位由 4 变 1
        assert_eq!(x_timestamp_from(1792175060149), "1792175060189");
        // 结果与输入等长（倒数第二位是"替换"而不是"插入"）
        assert_eq!(x_timestamp_from(1792175060149).len(), 13);
    }

    /// 签名算法：md5(`{ts}-{nonce}-{secret}`)
    #[test]
    fn sign_is_md5_of_three_parts() {
        let headers = sign_headers_with("https://chatglm.cn", "1792175060134".into(), "abc".into());
        // 手工算一次作为向量
        let mut hasher = Md5::new();
        hasher.update(format!("1792175060134-abc-{SIGN_SECRET_PROD}").as_bytes());
        let expected: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(headers.sign, expected);
        assert_eq!(headers.timestamp, "1792175060134");
        assert_eq!(headers.nonce, "abc");
    }

    /// 生产/测试域选不同的密钥
    #[test]
    fn picks_secret_per_environment() {
        assert_eq!(pick_secret("https://chatglm.cn"), SIGN_SECRET_PROD);
        assert_eq!(pick_secret("https://agentmore.chatglm.cn/"), SIGN_SECRET_PROD);
        assert_eq!(pick_secret("https://test.chatglm.cn"), SIGN_SECRET_TEST);
        assert_eq!(pick_secret("https://agent-test.chatglm.cn/"), SIGN_SECRET_TEST);
        // 未知域兜底测试密钥（与客户端一致）
        assert_eq!(pick_secret("https://example.com"), SIGN_SECRET_TEST);
    }

    /// 设备号必须是 UUID 形态（非 UUID 会被上游 40004 拒绝）
    #[test]
    fn generates_uuid_shaped_device_id() {
        let value = uuid_v4();
        assert_eq!(value.len(), 36, "UUID 字符串长度必须是 36: {value}");
        let parts: Vec<&str> = value.split('-').collect();
        assert_eq!(parts.len(), 5, "必须是 5 段: {value}");
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'));
        // 版本位与变体位
        assert!(value.chars().nth(14) == Some('4'), "版本位应为 4: {value}");
        assert!(matches!(value.chars().nth(19), Some('8') | Some('9') | Some('a') | Some('b')));
        // 两次生成不相同
        assert_ne!(uuid_v4(), uuid_v4());
    }

    /// nonce 是去掉横线的 UUID
    #[test]
    fn nonce_is_hex_without_dashes() {
        let value = x_nonce();
        assert_eq!(value.len(), 32);
        assert!(value.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
