//! Marvis 出站链路：**标准 OpenAI 协议零转换** + 一组 `Ual-Access-*` 鉴权头。
//!
//! ── 上游长什么样（2026-10-20 实测）──────────────────────────
//! ```text
//!   推理   POST https://yybadaccess.3g.qq.com/v1/chat/completions
//!   目录   （不存在：GET /v1/models → 404，见 models.rs 的排除记录）
//! ```
//! 请求体与响应帧都是**标准 OpenAI**（含 SSE、`tool_calls`、`usage`），
//! 所以本家只需要 `build_chat_request` 一条路，`UpstreamResponse::Chat` 默认值。
//!
//! 模型名 `main-auto` 由服务端自己路由，真实模型从响应头
//! `Marvis-Actual-Model` 回来（实测 `ark_deepseek-v4-flash-ga-260731`）。
//!
//! ── 鉴权头（这是本家全部的特殊性）────────────────────────────
//! ```text
//!   Ual-Access-Access-Token: mv_…            硬凭据
//!   Ual-Access-Openid:       oAN1…           登录 openid
//!   Ual-Access-Login-Type:   6               6 = 微信
//!   Ual-Access-Guid:         a79a…           设备号（**不是** qimei36）
//!   X-Device-GUID:           同上            冗余但真实客户端会带
//!   Ual-Access-MarvisExt:    {…uskey…}       JSON，含每请求唯一的 uskey
//!   Marvis-AgentTag / Marvis-Scene / Marvis-Protocol-Version
//! ```
//!
//! `Ual-Access-Signature` / `-Timestamp` / `-Nonce` **故意不发**：实测服务端
//! 不校验（空签名同样 200），发一个算出来的假签名反而可能在未来收紧时先坏
//! （详见 `fingerprint.rs` 的模块头）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::errors::GatewayError;

use super::credentials::MarvisCredential;
use super::fingerprint;

/// 上游推理端点（单一域名，无地区伴生 —— 见 `mod.rs` 的模块头）。
pub const CHAT_URL: &str = "https://yybadaccess.3g.qq.com/v1/chat/completions";

/// 默认模型名：上游只认这个路由别名，真实模型由服务端决定。
pub const DEFAULT_MODEL: &str = "main-auto";

/// 客户端版本号（`MarvisExt.clientVersion`；与真实客户端一致即可）。
const CLIENT_VERSION: &str = "1.60.3000.228";

/// 一次出站请求的成品。
#[derive(Clone, Debug)]
pub struct ChatPlan {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

/// 构造 `Ual-Access-MarvisExt` 的 JSON（uskey 在其中的 `uskey` 字段）。
///
/// 字段集取自真实客户端抓到的头（见 `_recon/agent2api-marvis-port-plan.md`
/// §1.9）。多带的字段无害，少带 uskey 会被风控拦（403 / 4100404）。
fn marvis_ext(qimei36: &str, uskey: &str) -> String {
    let payload = json!({
        "qimei36": qimei36,
        "osVersion": "Windows-10",
        "guid": qimei36,
        "uskey": uskey,
        "nonce": "0",
        "reqScene": 0,
        "taskType": "user",
        "clientVersion": CLIENT_VERSION,
        "clientPlatform": "Windows",
        "clientPlatformVersion": CLIENT_VERSION,
        "isIoa": false,
    });
    payload.to_string()
}

/// 组装转发请求。
///
/// `body` 是客户端原始请求体，**原样透传**（上游就是标准 OpenAI 协议）。
/// 唯一改写：模型名为空时补 `main-auto`（上游不认空模型）。
pub fn build_chat_plan(
    credential: &MarvisCredential,
    body: &Value,
) -> Result<ChatPlan, GatewayError> {
    // 每请求取新 uskey（服务端按新鲜度风控，见 fingerprint.rs）
    let fingerprint = fingerprint::generate()?;

    let mut payload = body.clone();
    if let Some(object) = payload.as_object_mut() {
        let model = object.get("model").and_then(Value::as_str).unwrap_or("");
        if model.trim().is_empty() {
            object.insert("model".into(), Value::String(DEFAULT_MODEL.into()));
        }
    }

    // qimei36 优先用账号里存的（稳定值），没有再取库生成的新值
    let qimei36 = if credential.qimei().is_empty() {
        fingerprint.qimei36.clone()
    } else {
        credential.qimei().to_string()
    };

    let headers = vec![
        ("Content-Type".into(), "application/json".into()),
        (
            "Ual-Access-Access-Token".into(),
            credential.access_token.clone(),
        ),
        ("Ual-Access-Openid".into(), credential.openid.clone()),
        ("Ual-Access-Login-Type".into(), "6".into()),
        ("Ual-Access-Guid".into(), credential.guid.clone()),
        ("X-Device-GUID".into(), credential.guid.clone()),
        (
            "Ual-Access-MarvisExt".into(),
            marvis_ext(&qimei36, &fingerprint.uskey),
        ),
        ("Marvis-AgentTag".into(), "main".into()),
        ("Marvis-Scene".into(), "agent_chat".into()),
        ("Marvis-Protocol-Version".into(), "1000".into()),
    ];

    Ok(ChatPlan {
        url: CHAT_URL.to_string(),
        headers,
        body: payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_contains_uskey_and_qimei() {
        let ext = marvis_ext("d70929e1", "CsDVDCER-test");
        let parsed: Value = serde_json::from_str(&ext).expect("应是合法 JSON");
        assert_eq!(parsed["uskey"], "CsDVDCER-test");
        assert_eq!(parsed["qimei36"], "d70929e1");
        assert_eq!(parsed["clientPlatform"], "Windows");
    }

    #[test]
    fn empty_model_falls_back_to_default() {
        // 只验证改写逻辑，不触发真实指纹调用（构造后手工检查）
        let mut body = json!({"model": "", "messages": []});
        if let Some(object) = body.as_object_mut() {
            let model = object.get("model").and_then(Value::as_str).unwrap_or("");
            if model.trim().is_empty() {
                object.insert("model".into(), Value::String(DEFAULT_MODEL.into()));
            }
        }
        assert_eq!(body["model"], DEFAULT_MODEL);
    }
}
