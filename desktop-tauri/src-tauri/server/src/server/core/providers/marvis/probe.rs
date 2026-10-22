//! Marvis 添加账号时的探活：**发一次最小对话**，确认令牌 + uskey 都通。
//!
//! ── 为什么探活要真的发一次对话（而不是查个 whoami）────────────
//! 本家**没有**任何只读的身份接口（`/v1/*` 除 `chat/completions` 全 404，
//! 见 `models.rs` 的排除记录）。所以「凭证是否可用」只能靠一次真实请求验证。
//! 用 `max_tokens: 1` 把成本压到最低。
//!
//! ── 三个错误码的归因（实测，决定提示文案）──────────────────────
//! ```text
//!   401 / 4100403  → 令牌失效：让用户重新提取
//!   403 / 4100404  → uskey 被风控：本机指纹库问题（多见于未装客户端）
//!   200            → 通
//! ```
//! 前两种都算「探活失败」，但文案必须区分 —— 让用户重贴 token 却其实是
//! 指纹库没找到，是这次接入最容易让人白折腾的一处。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::json;

use crate::server::errors::GatewayError;

use super::{client, credentials};

/// 探活结果（添加账号弹窗用来决定「保存并提示什么」）。
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    /// 是否可用
    pub ok: bool,
    /// 给用户看的一句话
    pub message: String,
    /// 上游真实模型名（响应头 `Marvis-Actual-Model`，可用时带回）
    pub actual_model: Option<String>,
}

/// 发一次最小对话验证凭证。
///
/// 网络错误不抛（返回 `ok: false` + 可读 message），让界面能「仍然保存」——
/// 离线添加账号是合理场景（用户先录进去、联网后再用）。
pub async fn probe(record: &serde_json::Value) -> ProbeOutcome {
    let credential = match credentials::from_record(Some(record)) {
        Ok(credential) => credential,
        Err(error) => {
            return ProbeOutcome {
                ok: false,
                message: error.to_string(),
                actual_model: None,
            }
        }
    };

    let body = json!({
        "model": client::DEFAULT_MODEL,
        "messages": [{ "role": "user", "content": "hi" }],
        "stream": false,
        "max_tokens": 1,
    });

    let plan = match client::build_chat_plan(&credential, &body) {
        Ok(plan) => plan,
        Err(error) => {
            return ProbeOutcome {
                ok: false,
                message: format!("无法构造请求：{error}"),
                actual_model: None,
            }
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ProbeOutcome {
                ok: false,
                message: format!("HTTP 客户端初始化失败：{error}"),
                actual_model: None,
            }
        }
    };

    let mut request = client.post(&plan.url);
    for (key, value) in &plan.headers {
        request = request.header(key.as_str(), value.as_str());
    }

    match request.json(&plan.body).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let actual_model = response
                .headers()
                .get("Marvis-Actual-Model")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let text = response.text().await.unwrap_or_default();
            if status == 200 {
                return ProbeOutcome {
                    ok: true,
                    message: match &actual_model {
                        Some(model) => format!("凭证可用（上游路由到 {model}）"),
                        None => "凭证可用".to_string(),
                    },
                    actual_model,
                };
            }
            ProbeOutcome {
                ok: false,
                message: explain(status, &text),
                actual_model,
            }
        }
        Err(error) => ProbeOutcome {
            ok: false,
            message: format!("连接上游失败（{error}）：已保存，联网后可重试"),
            actual_model: None,
        },
    }
}

/// 把上游失败翻译成「用户该做什么」（判据见模块头）。
fn explain(status: u16, body: &str) -> String {
    if body.contains("4100403") {
        return "令牌已失效：请在客户端重新登录后，用 _recon/marvis-cred-extract.py 重新提取".into();
    }
    if body.contains("4100404") {
        return "设备指纹被风控：请确认已安装马维斯客户端（需要 beacon_wrapper.dll），\
                或用环境变量 WORKBUDDY_MARVIS_DIR 指定安装目录"
            .into();
    }
    format!("上游返回 {status}：{}", body.chars().take(200).collect::<String>())
}

/// 添加账号流程用的入口（`api::accounts` 调用）。
pub async fn verify(record: &serde_json::Value) -> Result<ProbeOutcome, GatewayError> {
    Ok(probe(record).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_expired_message_mentions_reextract() {
        let text = explain(401, r#"{"code":4100403,"msg":"登录态过期"}"#);
        assert!(text.contains("重新提取"));
    }

    #[test]
    fn risk_blocked_message_mentions_client_install() {
        let text = explain(403, r#"{"code":4100404,"msg":"风控"}"#);
        assert!(text.contains("beacon_wrapper.dll"));
    }
}
