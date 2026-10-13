//! Command Code 适配器：**无状态**（一次 HTTP 请求 = 一次生成），
//! 但响应是 NDJSON 而不是 SSE —— 走翻译层，不走会话式转发。
//!
//! ── 为什么不上有状态路线（`is_stateful` / `forward_conversation`）──
//! Command Code 的上游确实是「一次请求一次生成」，只是**响应体**是
//! `application/x-ndjson`（一行一个 JSON 事件、HTTP 恒 200、错误在流内）。
//! 这属于「上游响应说哪套协议」，本仓对此的既定解法是
//! [`UpstreamResponse`](crate::server::core::providers::adapter::UpstreamResponse)
//! 加一个变体、编排层按它选翻译层（ZCode 活动套餐通道那条先例）——
//! 于是账号轮换、限额冷却、退避重试、usage 记账与取消处理全部留在编排层。
//! 若改走 `forward_conversation`，那五样都要在适配器里重写一遍（`adapter.rs`
//! 的文档专门论证过这件事），且本家并没有多步会话协议，没有理由付那份代价。
//!
//! ── 本家没有的东西（如实声明，别照抄别家）──────────────────────
//!   - **没有续期**：单个静态 API Key，上游没有 refresh 接口 →
//!     `supports_refresh = false`（与 Loomy / MonkeyCode 同一处境）；
//!   - **没有网页登录**：粘贴式凭证，不开窗口（`supports_web_login` 默认 false）；
//!   - **不做签到**：没有可自动领的奖励活动（`core::auto_checkin` 的清单不含本家）；
//!   - **不做余额查询**：`/alpha/billing/*` 只在添加账号时用来探活
//!     （见 `credentials::probe_key`），`supports_usage` 保持默认 false。
//!
//! ── 指纹预请求的落点（`ensure_access_token`）────────────────────
//! 上游要求「发正式请求前先上报设备指纹」（每 key 首次 + 每 8h+2h 抖动，
//! 见 `fingerprint.rs`）。落点选 `ensure_access_token`：它本来就是每次转发前
//! 的异步凭证准备钩子（`provider_loop` 在 `build_chat_request` 之前 await 它），
//! 语义与「发正式请求前确保伪装已就位」正好对上；而 `build_chat_request` 是
//! 同步的，不能在里面发网络请求。预请求是 **best-effort**：失败只告警，
//! 绝不影响生成（参考实现同款）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{credentials, fingerprint, models, plan};

/// Command Code 适配器（无状态单例）
pub struct CommandCodeAdapter;

/// 静态单例（`adapter_for(ProviderKind::CommandCode)` 返回这一个）
pub static COMMANDCODE_ADAPTER: CommandCodeAdapter = CommandCodeAdapter;

impl ProviderAdapter for CommandCodeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CommandCode
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存 + 内置兜底）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST /alpha/generate`（自有信封；`UpstreamResponse::CommandCodeNdjson`）。
    ///
    /// `account` 是会话形态（`store.get_session_by_id` 的返回值），key 从
    /// `auth.accessToken` 取 —— 与另外几家同一约定（公开形态里只有尾号，
    /// 拿它拼不出 Authorization）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let api_key = account
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if api_key.is_empty() {
            return Err(GatewayError::with_status(
                401,
                "Command Code 账号缺少 API Key，无法转发（请重新粘贴 user_ 开头的 key）",
            ));
        }
        // 形态不对就地拦下（本地零网络）：发一个 sk-xxx 出去只会换回一条
        // 语焉不详的 401，而用户该做的是**重新粘一次**
        if !credentials::shape_ok(&api_key) {
            return Err(GatewayError::with_status(
                401,
                format!(
                    "Command Code API Key 形态不对（应为 user_ 开头，当前 {}）：\
                     请在 commandcode.ai/studio 重新获取后重新粘贴",
                    credentials::masked(&api_key)
                ),
            ));
        }
        let built = plan::build(&api_key, body, client_headers);
        Ok(ChatRequestPlan::commandcode_ndjson(
            built.url,
            built.headers,
            built.body,
        ))
    }

    /// 上游错误分类（规格 §7.1 的映射表）：
    ///   - 400 / 422 → `Fatal(400)`（请求体非法，不重试；命中内容拦截文案时
    ///     走共用的 [`content_block`] 判定 —— 那档不罚账号、换提示词补救）；
    ///   - 401 / 403 → `TokenExpired`（编排层刷新一次（本家刷新必然拿回同一个
    ///     key）再换账号 —— 规格 §7.4 的「必须换 key」）；
    ///   - 402 → `QuotaLimited`（**下游状态码按 429**：规格 §7.1 明确
    ///     「付费失败按限流」，换账号比直接报错更有用）；
    ///   - 404 → `Fatal(404)`；429 → `QuotaLimited(429)`；
    ///   - 500 / 502 → `Fatal(502)`；503 → `Fatal(503)`；其余 → `Fatal(502)`。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        let message = format!("上游返回 {status}: {raw}");
        let code = error_body.get("code").and_then(Value::as_i64);
        match status {
            400 | 422 => content_block::classify_or_fatal(400, error_body, message, code),
            401 | 403 => UpstreamErrorClass::TokenExpired { message },
            402 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message: format!(
                    "{message}（余额 / 套餐不足：请在 commandcode.ai 充值或换一个账号，本网关按限流处理）"
                ),
                upstream_code: code,
                // 规格 §7.1 的有意映射：402 → 下游 429
                status: 429,
            },
            429 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: code,
                status: 429,
            },
            404 => UpstreamErrorClass::Fatal {
                status: 404,
                message,
                upstream_code: code,
            },
            503 => UpstreamErrorClass::Fatal {
                status: 503,
                message,
                upstream_code: code,
            },
            // 500 / 502 与未列出的一切 → 502（规格 §7.1 的兜底）
            _ => UpstreamErrorClass::Fatal {
                status: 502,
                message,
                upstream_code: code,
            },
        }
    }

    /// 取可用 API Key，并在此之前确保**设备指纹已上报**（见模块头）。
    ///
    /// 没有续期：这里只做「记录里有没有 key、形态对不对」的检查，
    /// 上游拒了就是 401，编排层把它归成 `TokenExpired`（先原地试一次，
    /// 再按队列换账号）。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let record = store.commandcode_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("Command Code 账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            // best-effort：失败只告警，绝不影响生成（见 `fingerprint.rs`）
            fingerprint::ensure_initialized(&credentials.api_key).await;
            Ok(credentials.api_key)
        })
    }

    /// 没有续期手段（静态 key，上游无 refresh 接口）——维护任务不问本家
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 有远程目录（`GET /provider/v1/models`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一个可用账号）的 API Key。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（与 Loomy / MonkeyCode 同一处置：脚本 / CI 不该
    /// 看到一个红色失败）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.commandcode_account_record(account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "Command Code 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials.api_key, force).await
        })
    }
}
