//! 客户端版本解析：`X-LobsterAI-Client-Version` 要的是**日期式版本**（如 `2026.9.4`），
//! 不是插件自己的版本号。
//!
//! ── 从哪来 ──────────────────────────────────────────────────
//! 上游的更新接口（`CLIENT_VERSION_API`，第三方主机）报当前桌面端版本。
//! ⚠️ 它的响应**不是业务信封**：版本号在 `data.value`（Go 参考
//! `lobsterai-product.ts:443` 的实测说明）。
//!
//! ── 为什么要缓存 ────────────────────────────────────────────
//! 版本在每个请求上都要用（换码 / 续期 / 目录 / 签到），而它一天也变不了几次；
//! 参考实现缓存 12 小时。取不到就退回已知可用的常量，**绝不发空版本头** ——
//! 空版本头会被服务端按「不认识的客户端」对待。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use crate::server::core::auth_http::send_raw;
use crate::server::logging;

use super::endpoints::{
    CLIENT_VERSION_API, FALLBACK_CLIENT_VERSION, REQUEST_TIMEOUT_MS, USER_AGENT, VERSION_CACHE_TTL_MS,
};

/// 进程级缓存的版本（`Some` 即已解析过；空串表示「刚查过但失败」，靠 TTL 过期）
struct Cache {
    value: String,
    fetched_at: i64,
}

static CACHE: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Cache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 从更新接口的响应里取版本号。
///
/// ⚠️ **路径是 `data.value.version`，不是 `data.value`** —— `value` 是个**对象**，
/// 版本号在它的 `version` 成员里（Go 参考 `version.go:57-71` 的
/// `parseClientVersionFromUpdateValue` 逐层 `asRecord`）。
///
/// 这一处我最初写成把 `data.value` 当标量读，而单测夹具又是照着自己的假设编的
/// （于是测试全绿、代码全错，线上表现是版本头永远退化成兜底常量）。
/// 现在夹具按 Go 的路径编，形状对不对由它守。
///
/// 单独抽出来是为了能脱网单测：这一段是上游契约，改错就是整家发错版本头。
pub fn version_from_update_payload(payload: &serde_json::Value) -> Option<String> {
    let value = payload.get("data")?.get("value")?;
    let version = value.get("version")?;
    let text = match version {
        serde_json::Value::String(text) => text.trim().to_string(),
        other => other.to_string(),
    };
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// 解析当前客户端版本（带 12 小时缓存；失败退回常量）。
pub async fn resolve_client_version() -> String {
    {
        let guard = match cache().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = guard.as_ref() {
            if logging::now_ms() - entry.fetched_at < VERSION_CACHE_TTL_MS && !entry.value.is_empty()
            {
                return entry.value.clone();
            }
        }
    }

    let response = send_raw(
        "GET",
        CLIENT_VERSION_API,
        None,
        &[
            ("accept".to_string(), "application/json".to_string()),
            ("user-agent".to_string(), USER_AGENT.to_string()),
        ],
        None,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await;

    let resolved = match response {
        Ok(response) if response.ok => response
            .payload
            .as_ref()
            .and_then(version_from_update_payload)
            .unwrap_or_default(),
        Ok(response) => {
            logging::verbose(
                "[LobsterAI]",
                &format!("版本接口返回 HTTP {}，改用兜底版本", response.status),
            );
            String::new()
        }
        Err(error) => {
            logging::verbose(
                "[LobsterAI]",
                &format!("版本接口不可达（{error}），改用兜底版本"),
            );
            String::new()
        }
    };

    let mut guard = match cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(Cache {
        value: resolved.clone(),
        fetched_at: logging::now_ms(),
    });
    if resolved.is_empty() {
        FALLBACK_CLIENT_VERSION.to_string()
    } else {
        resolved
    }
}

/// 同步版的兜底取值：给「已经拿着版本、只是要修空串」的场景用。
pub fn fallback_version() -> &'static str {
    FALLBACK_CLIENT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 版本号在 `data.value.version`（**嵌套对象**，见 `version.go:57-71`）。
    /// 夹具按上游真实形状编，而不是按实现的假设编 —— 我第一版就是两边一起错、
    /// 测试还全绿。
    #[test]
    fn version_is_read_from_data_value_version() {
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": {"version": "2026.9.4"}}})),
            Some("2026.9.4".to_string())
        );
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": {"version": " 2026.10.1 "}}})),
            Some("2026.10.1".to_string())
        );
        // 数字形态（上游未必给字符串）
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": {"version": 20260904}}})),
            Some("20260904".to_string())
        );
    }

    /// 形状不对时返回 None —— 调用方据此退回常量，**绝不发空版本头**。
    ///
    /// 特别钉住「把 `data.value` 当标量」这个曾经的错法：那种形状必须**读不到**，
    /// 否则改了路径也发现不了。
    #[test]
    fn malformed_payload_yields_none_instead_of_a_wrong_version() {
        assert_eq!(version_from_update_payload(&json!({})), None);
        assert_eq!(version_from_update_payload(&json!({"data": {}})), None);
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": ""}})),
            None,
            "标量 value 是旧错法，必须读不到"
        );
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": "2026.9.4"}})),
            None,
            "标量 value 是旧错法，必须读不到"
        );
        assert_eq!(
            version_from_update_payload(&json!({"data": {"value": {"version": ""}}})),
            None
        );
        assert_eq!(version_from_update_payload(&json!({"value": {"version": "x"}})), None);
        // 兜底常量本身永远可用
        assert!(!fallback_version().is_empty());
    }
}
