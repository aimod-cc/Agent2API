//! MonkeyCode 适配器（两个地区共用一套实现，按 `Region` 参数化）。
//!
//! ── 为什么走会话式转发（`is_stateful`）────────────────────────
//! MonkeyCode 的上游不是「一次 HTTP 请求 = 一次回答」的 OpenAI 协议：对话要
//! **先建任务**（`POST /api/v1/users/tasks`，带 `image_id` / `model_id` /
//! `cli_name`），再挂 **WebSocket** 任务流
//! （`GET /api/v1/users/tasks/stream?id=&mode=`）读 ACP 事件
//! （见 `Acankao/.../docs/04-websocket/*`）。单请求构造容纳不了这条链，
//! 因此 `is_stateful()` 为 true —— 与 Kuku / CatPaw / Qoder 同一处境
//! （「一次发送要适配器自己完成」）。
//!
//! ── 会话转发的实现分工（已接线）────────────────────────────
//! 协议实现收在同目录的几个文件里，本文件只做装配（转调 `chat::run_chat`）：
//!   `task.rs`      输入侧翻译（messages → prompt + system）与建任务（含错误码翻译）
//!   `stream.rs`    WS 连接 / 起始消息（auto-approve + user-input）/ 心跳 / 超时 /
//!                  四个终态出口（task-ended、task-error、断连、超时）
//!   `translate.rs` 下行消息与 ACP 事件 → OpenAI chunk（含自动回复 Agent 提问）
//!   `chat.rs`      装配（凭证 → 目录反查 → 建任务 → 连流 → 流式或聚合）
//!
//! ── 本家没有的东西（如实声明，别照抄别家）──────────────────────
//!   - **没有续期**：session 30 天硬限制，上游无 refresh 接口 →
//!     `supports_refresh = false`（与 Loomy 同一处境）；
//!   - **没有网页登录**：登录要么带验证码、要么要 OAuth 窗口，本网关只接
//!     「粘贴 session」这一条 → `supports_web_login` 保持默认 false；
//!   - **不做签到与余额**：没有实测可用的余额接口，
//!     `supports_usage` 保持默认 false，`query_usage` 走 trait 默认实现；
//!   - **任务流不挂账号代理**：tokio-tungstenite 没有代理支持，参考实现同样
//!     只给 REST 调用挂代理（见 `stream::connect` 的说明）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::ProviderKind;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::endpoints::cli_name_for;
use super::region::Region;
use super::{credentials, models};

/// MonkeyCode 适配器：**按地区参数化**（与 Qoder / AutoClaw 同款）。
///
/// 拆家后两个地区是两家 provider（`monkeycode` 国内版 / `monkeycode-intl`
/// 国际版），两个静态实例由 `adapter_for` 按 kind 给出；地区 → 身份的互查在
/// `region::Region`。适配器内部链路（凭证、目录、会话转发）一律以本字段
/// 的 `region` 为准。
pub struct MonkeyCodeAdapter {
    region: Region,
}

/// 国内版静态实例
pub static MONKEYCODE_ADAPTER: MonkeyCodeAdapter =
    MonkeyCodeAdapter { region: Region::Cn };
/// 国际版静态实例
pub static MONKEYCODE_INTL_ADAPTER: MonkeyCodeAdapter =
    MonkeyCodeAdapter { region: Region::Intl };

impl ProviderAdapter for MonkeyCodeAdapter {
    fn kind(&self) -> ProviderKind {
        self.region.kind()
    }

    /// 上游是「建任务 → WebSocket 任务流」的多步会话（见模块头）。
    fn is_stateful(&self) -> bool {
        true
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存，按地区分格）
    fn list_models(&self) -> Vec<Value> {
        models::list(self.region)
    }

    /// **防御性报错**：本家走会话式转发，不走单请求路径。
    ///
    /// 与 Kuku 同一处置：编排层已按 `is_stateful` 分流，走到这里说明是内部
    /// 契约错误，报错比「构造出一个半成品请求发出去」安全。
    fn build_chat_request(
        &self,
        _account: &Value,
        _body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        Err(GatewayError::with_status(
            503,
            format!(
                "MonkeyCode {} 走会话式转发，不走单请求路径（内部错误：编排层未按 is_stateful 分流）",
                self.region.label()
            ),
        ))
    }

    /// 上游错误分类：
    ///   - HTTP 401 / 403 → `TokenExpired`（登录态失效，重新粘贴 session）；
    ///   - HTTP 402 → `Fatal`（额度不足：换账号也没用，用户得去官网处理）；
    ///   - HTTP 429 → `QuotaLimited`（上游不给结构化恢复时间）；
    ///   - 其余 → `Fatal`。
    ///
    /// ── 这条路径在本家事实上走不到（如实说明）──────────────────
    /// 本家是 `is_stateful`，所有上游错误都在 `task.rs` / `stream.rs` 里现场
    /// 归一到 `GatewayError`（状态码与文案已按建任务 / 任务流的码表翻译好，
    /// 见那两个模块头）；`classify_error` 只服务单请求路径，而
    /// `build_chat_request` 是防御性 503。保留这些分支是让契约完整 +
    /// 排障时一眼看到码表的映射意图；402 的具体文案在 `task::status_error`。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let upstream_message = error_body
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let hint = match status {
            402 => Some("（账户额度不足，请在 MonkeyCode 官网确认订阅 / 余额）"),
            _ => None,
        };
        let message = match (upstream_message, hint) {
            (Some(text), Some(hint)) => format!("上游返回 {status}: {text}{hint}"),
            (Some(text), None) => format!("上游返回 {status}: {text}"),
            (None, Some(hint)) => format!("上游返回 HTTP {status}{hint}"),
            (None, None) => format!("上游返回 HTTP {status}"),
        };
        if status == 401 || status == 403 {
            return UpstreamErrorClass::TokenExpired { message };
        }
        if status == 429 {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: error_body.get("code").and_then(Value::as_i64),
                status,
            };
        }
        UpstreamErrorClass::Fatal {
            status,
            message,
            upstream_code: error_body.get("code").and_then(Value::as_i64),
        }
    }

    /// 取可用 session。本家**没有续期**：只做「记录里有没有 session」的检查，
    /// 临期与否只影响账号页的提示（`credentials_expiring`），不影响转发 ——
    /// 上游拒了就是 401，编排层把它归成 `TokenExpired` 并如实报给客户端。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.monkeycode_account_record(self.region, account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!(
                        "MonkeyCode {} 账号 {account_id} 不存在（请重新添加）",
                        self.region.label()
                    ),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.expiring() {
                logging::verbose(
                    "[MonkeyCode]",
                    "账号凭证已过期或临期（会话 30 天、无续期接口），如遇 401 请重新粘贴 session",
                );
            }
            Ok(credentials.session)
        })
    }

    /// 没有续期手段（上游没有 refresh 接口）——维护任务不问本家
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 临期判定：取记录 → 凭证的 `expiring()`（登录时间 + 30 天，提前 1 天）
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        if account_id.is_empty() {
            return false;
        }
        match store.monkeycode_account_record(self.region, account_id) {
            Some(record) => credentials::from_record(Some(&record))
                .map(|credentials| credentials.expiring())
                .unwrap_or(false),
            None => false,
        }
    }

    /// 有远程目录（`GET /api/v1/users/models`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 本地区组内第一个可用账号）的 session。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（与 Loomy / AutoClaw 同一处置：脚本 / CI 不该看到一个
    /// 红色失败）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.monkeycode_account_record(self.region, account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose(
                        "[Models]",
                        &format!("MonkeyCode {} 模型目录刷新跳过：尚未添加账号", self.region.label()),
                    );
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(self.region, &credentials.session, force).await
        })
    }

    /// **会话式转发入口**：转调 `chat::run_chat`。
    ///
    /// 本函数只做装配：把编排层给的散装入参（`store` / `account_id` / 原始
    /// body / 出网代理 / 流式标志 / 记账槽）原样交给 `chat.rs`。协议时序
    /// （建任务 → WS → ACP 翻译）与各处错误码翻译都在那三个文件里，
    /// 账号选路 / 限额冷却 / telemetry 记账仍在编排层。
    fn forward_conversation<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        body: &'a Value,
        _client_headers: &'a HeaderMap,
        proxy: Option<crate::server::core::proxies::ResolvedProxy>,
        stream: bool,
        telemetry: &'a std::sync::Arc<crate::server::core::upstream::usage::RequestTelemetry>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::server::core::upstream::ForwardOutcome, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            super::chat::run_chat(
                store,
                self.region,
                account_id,
                body,
                proxy,
                stream,
                telemetry,
            )
            .await
        })
    }
}

/// 接口类型 → CLI 名的转发口（供会话转发与排障复用；判据唯一写在 `endpoints`）
pub fn cli_name(interface_type: &str) -> &'static str {
    cli_name_for(interface_type)
}
