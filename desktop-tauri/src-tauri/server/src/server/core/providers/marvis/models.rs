//! Marvis 模型目录：**静态兜底，没有远程清单**。
//!
//! ── 「没有目录接口」的排除过程（SKILL 1.3 要求写清，别只写结论）──────
//! 上游只有一个推理域名 `yybadaccess.3g.qq.com`，逐条实测（2026-10-20，
//! 带完整 `Ual-Access-*` 头）：
//!
//! ```text
//!   GET  /v1/models              → 404（空 body）
//!   GET  /v1/usage               → 404
//!   GET  /v1/quota               → 404
//!   GET  /v1/balance             → 404
//!   GET  /v3/marvis_user_info    → 404
//!   GET  /v3/marvis_quota        → 404
//!   POST /v3/marvis_trag_search  → 400（端点存在，但那是检索不是目录）
//! ```
//!
//! 客户端侧的模型名来自**本地 SQLite**（`User/<openid>/database/data.db` 的
//! `llm_token_usage` 表记录的是实际用过的模型），不是一个可拉的清单接口。
//! 参考实现（macOS）也没有目录接口，同样只认 `main-auto`。
//!
//! ⇒ 因此 `supports_model_refresh = false`，清单是**静态兜底**：
//! 上游对外只暴露 `main-auto` 一个路由别名，服务端自己路由到真实模型
//! （实测 `main-auto` → `ark_deepseek-v4-flash-ga-260731`，响应头
//! `Marvis-Actual-Model` 给出真名）。
//!
//! ── 为什么不把真实模型名也列出来 ─────────────────────────────
//! 客户端**只能**点 `main-auto`（真实名是服务端内部路由结果，直接发
//! `ark_deepseek-v4-flash-ga-260731` 未必被接受）。列一个点不动的名字
//! 比不列更糟 —— 与 SKILL 第 0 节「一个必然报错的按钮比没有按钮更糟」同一口径。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use super::client::DEFAULT_MODEL;

/// 静态清单：只有一个 `main-auto` 路由别名。
///
/// 能力位取「保守但可用」：文本 + 图像输入（上游是 OpenAI 兼容网关，
/// 实测接受标准 messages）、工具调用（实测接受 `tools` 并返回 `tool_calls`）。
/// 上下文窗口给 250000 —— 与同类国产网关（QClaw 的 modelroute）同量级；
/// 这是**兜底声明**，不是上游给的数（上游没有目录接口，见模块头）。
pub fn list() -> Vec<Value> {
    vec![json!({
        "id": DEFAULT_MODEL,
        "name": "Auto（马维斯智能路由）",
        "maxInputTokens": 250_000,
        "maxOutputTokens": 32_000,
        "supportsImages": true,
        "supportsToolCall": true,
        "supportsReasoning": true,
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_exactly_one_route_alias() {
        let models = list();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], DEFAULT_MODEL);
    }

    #[test]
    fn manifest_declares_tool_call_support() {
        let models = list();
        assert_eq!(models[0]["supportsToolCall"], true);
    }
}
