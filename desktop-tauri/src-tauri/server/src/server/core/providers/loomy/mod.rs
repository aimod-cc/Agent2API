//! Loomy（讯飞）适配器：账号管理（手机号验证码登录）**加推理转发**。
//!
//! ── 上游长什么样（逆向来源：`D:\Program Files\Loomy\resources\app.asar`）──
//! 三套平面（细节见各子模块的模块头）：
//!
//! ```text
//!   账号 CAccount     POST https://account.xfinfr.com/login/phone/{sendMsgCode,checkCode}
//!                     HMAC-SHA1 签名头（客户端内置 AccessKey 对，见 sign.rs）
//!   集成网关（积分）  https://loomyad.xunfei.cn        header: token: <session>
//!   模型网关          {集成网关}/api/v1                 OpenAI 兼容，token + Bearer 双头
//! ```
//!
//! 本家**无状态**（一次 HTTP 请求 = 一次回答，OpenAI 协议），与 AutoClaw 同构：
//! `supports_chat` / `is_stateful` 走默认值，转发链只需 `build_chat_request`。
//!
//! ── 三处与别家不同的取舍（都写在各自模块头，这里给索引）──────
//!   1. **没有续期**（`supports_refresh = false`）：session 14 天，上游没有
//!      refresh 接口，过期（`020002` / `100002`）只能重新短信登录 ——
//!      见 `credentials.rs` 的模块头；
//!   2. **没有桌面端导入**：登录态不在可读文件里（Accio / ZCode 同一处境）；
//!   3. **没有内置兜底模型清单**：官方 imodel 网关 `/models` 拉不到就如实报错，
//!      不编模型名 —— 见 `models.rs` 的模块头。
//!
//! ── 思考等级 ────────────────────────────────────────────────
//! 客户端给模型挂 OpenCode variant（low/medium/high）→ AI SDK 发
//! `reasoning_effort`；网关侧把通用档位折成这三档注入（客户端显式指定时让位）。
//! 只有 spark-x 是真思考模型，但上游对非思考模型忽略该字段是已知行为
//! （客户端对全部 OpenAI 兼容模型都挂了 variants），因此不做按模型的收窄。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

pub mod balance;
pub mod checkin;
pub mod client;
pub mod credentials;
pub mod endpoints;
pub mod images;
pub mod login;
pub mod models;
pub mod onboarding;
pub mod sign;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, UpstreamErrorClass,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 适配器实例（`adapter_for(ProviderKind::Loomy)` 返回这一个）。
///
/// **无状态**：它不持有任何数据 —— 凭证在账号存储、目录在 `models.rs` 的
/// 进程级句柄。这与 `AutoClawAdapter` 持有 `region` 不同：本家没有两地区。
pub struct LoomyAdapter;

/// 静态单例
pub static LOOMY_ADAPTER: LoomyAdapter = LoomyAdapter;

/// 这一发是不是**这份请求太长/太大**被拒？判据取自上游文案，参数**要已小写**。
///
/// ── 为什么只取文案，不取状态码、也不取错误码 ───────────────────
/// 本家的长度墙实测有**两种形状**（dev 3066 真实流量）：
///   · 业务层 `400` + 中文原文「内容太长了，请精简输入或开启新对话」
///     —— 2026-10-15 那份 16,341,828 字节的会话（同一份体被重发 11 次）；
///   · 华为云 APIG 的 `413` + `{"error_msg":"Request entity too large","error_code":"APIG.0201"}`。
/// 判据若锁在 413 上，第一种（也就是实测在打的那条）一次都认不出来。
///
/// ⚠️ `APIG.0201` **不能**单独当判据：官方错误码表把这个**过载码**同时给了
/// 400/413/414/494/500/502/504，其中 5xx 是真该退避重发的上游抖动 —— 拿它当
/// 体积判据会把该重试的那一类一起治没。这条与 codearts 的
/// [`super::codearts::chat::size_rejection_text`] 同一条理由（同一句文案实测会换码）。
fn request_is_too_long(lower: &str) -> bool {
    [
        // 本家业务层原文（实测）
        "内容太长",
        "内容过长",
        "精简输入",
        // 华为云 APIG / 后端解析两层对超限体的措辞
        "request entity too large",
        "requestentitytoolarge",
        "payload too large",
        "exceeds the maximum size",
        "parse_request_data_exception",
        // OpenAI 兼容网关的通用上下文超限措辞
        "context_length_exceeded",
        "maximum context length",
        "too many tokens",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

impl ProviderAdapter for LoomyAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Loomy
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST {模型网关}/chat/completions`（OpenAI 协议，body 原样透传）。
    ///
    /// 鉴权是**双头**（客户端 `fetchModelsFromProvider` 的 authMode: 'token' 同款）：
    /// `token: <session>` 与 `Authorization: Bearer <session>` 都发 —— 上游两个
    /// 网关（模型 / 积分）对头名的偏好不同，双写是对两边都安全的做法。
    ///
    /// `account` 是会话形态（`store.get_session_by_id` 的返回值）：
    /// session 从 `auth.accessToken` 取，与另外几家同一约定。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let token = account
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if token.is_empty() {
            return Err(GatewayError::with_status(
                401,
                "Loomy 账号缺少 session，无法转发（请重新登录）",
            ));
        }
        let headers: Vec<(String, String)> = vec![
            // send_raw 会自动补 Content-Type；这里只加本家的鉴权双头与 Accept
            ("token".to_string(), token.clone()),
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Accept".to_string(), "text/event-stream".to_string()),
        ];
        // model 原样透传：Loomy 的模型 id 就是上游 id（没有路由前缀那套），
        // 入口校验已按目录收窄过，这里不需要二次映射。
        //
        // 图片格式适配（见 `images` 模块头）：deepseek 系模型只认
        // `<image_base64>` 标记，标准 image_url 会被上游当文本读（用户报的
        // 「图片被截断/不支持图片识别」）；其余模型保持 image_url。
        // 判断用**客户端请求名**（body.model 此刻还没被 wire 改写）：
        // 目录名与映射别名都含 `deepseek`，宽松匹配两段链路都成立。
        let requested_model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let payload = if images::prefers_tag_format(&requested_model) {
            let (rewritten, converted) = images::rewrite_to_tag(body);
            if converted > 0 {
                logging::verbose(
                    "[Loomy]",
                    &format!(
                        "模型 {requested_model} 只认 <image_base64> 标记：已转换 {converted} 个图片块"
                    ),
                );
            }
            rewritten
        } else {
            body.clone()
        };
        Ok(ChatRequestPlan::chat(
            format!("{}/chat/completions", endpoints::model_base_url()),
            headers,
            payload,
        ))
    }

    /// 上游错误分类：
    ///   - HTTP 401 / 403 → `TokenExpired`（刷新链对无续期的家会走到重登提示）；
    ///   - 业务码 `020002` / `100002`（HTTP 200 或非 2xx 里的 body）→ `TokenExpired`；
    ///   - **长度/体积超限**（文案判据见 [`request_is_too_long`]）→ `Fatal` + **413**；
    ///   - HTTP 429 → `QuotaLimited`（上游不给结构化恢复时间）；
    ///   - 其余 → 内容拦截分类（审核文案）或原样透传。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        let message = format!("上游返回 {status}: {raw}");
        let body_code = error_body
            .get("code")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if status == 401 || status == 403 || client::is_auth_error_code(body_code) {
            return UpstreamErrorClass::TokenExpired { message };
        }
        // ── 体积超限折成 413，且**必须先于 429 判掉** ─────────────────
        // 这条排在这里有两个理由，都要一起成立：
        //   ① 落进限额档的代价不是「多试一次」能兜住的：账号被记一次限额冷却并
        //      换号 —— 可它拒的是这份请求的体量，跟哪个号无关，换号后同一份体
        //      照样被拒，等于把下一个号也拖进同一笔白账；
        //   ② 折成 413 才有人接得住它：`upstream::fallback_retry_advice` 对
        //      「分类结果的状态码是 413」的失败**不原地重发**（判据取分类后的
        //      状态码），客户端那边 `413` 也早有 `request_too_large` 的映射
        //      （见 `protocol::anthropic`）—— 它是唯一一条能让客户端自己压缩
        //      上下文的路。原文不动地放进文案里，用户看得见的仍是上游那句。
        let lowered = error_body.to_string().to_ascii_lowercase();
        if request_is_too_long(&lowered) {
            return UpstreamErrorClass::Fatal {
                status: 413,
                message: format!(
                    "请求超过上游上限：{raw}（这条拒的是这份请求的体量，与账号无关：\
                     把历史清短一些、或开启新对话）"
                ),
                upstream_code: error_body.get("code").and_then(Value::as_i64),
            };
        }
        if status == 429 {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: error_body.get("code").and_then(Value::as_i64),
                status,
            };
        }
        content_block::classify_or_fatal(
            status,
            error_body,
            message,
            error_body.get("code").and_then(Value::as_i64),
        )
    }

    /// 取可用 session。本家**没有续期**：这里只做「记录里有没有 session」的检查，
    /// 临期与否只影响账号页的提示（`credentials_expiring`），不影响转发 ——
    /// 服务端拒了就是 401，编排层会把它归成 `TokenExpired` 并在刷新失败后
    /// 如实报给客户端。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.loomy_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("Loomy 账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.expiring() {
                logging::verbose(
                    "[Loomy]",
                    "账号凭证已过期或临期（会话 14 天、无续期接口），如遇 401 请重新短信登录",
                );
            }
            Ok(credentials.session)
        })
    }

    /// 没有续期手段（上游没有 refresh 接口）——维护任务不问本家
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 临期判定：取记录 → 凭证的 `expiring()`（登录时间 + 14 天，提前 1 天）
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        if account_id.is_empty() {
            return false;
        }
        match store.loomy_account_record(account_id) {
            Some(record) => credentials::from_record(Some(&record))
                .map(|credentials| credentials.expiring())
                .unwrap_or(false),
            None => false,
        }
    }

    /// Loomy **没有**「默认模型」概念（不指定模型时由目录决定）
    fn supports_default_model(&self) -> bool {
        false
    }

    /// 思考等级绑定 → `reasoning_effort`（low / medium / high）。
    ///
    /// 依据：客户端给 OpenAI 兼容模型挂的低中高三档 variant 最终就是
    /// `reasoningEffort`（`model-service.js` 的 `buildThinkingVariants`），
    /// 由 AI SDK 落到请求体的 `reasoning_effort`。通用 6 档折成三档：
    /// `minimal|low → low`、`medium → medium`、`high|max → high`。
    ///
    /// 客户端显式指定时让位（用户的明确意图比映射默认值更具体）；
    /// `off` / `none` 不注入（客户端没有「关闭思考」的可靠表达）；表外自定义
    /// 等级不注入（上游对未知档位要么忽略要么 400，不如明确跳过）。
    fn reasoning_patch(&self, level: &str, _model: &str, body: &Value) -> ReasoningPatch {
        if crate::server::core::model_rules::reasoning_is_off(level) {
            return ReasoningPatch::Skip {
                reason: "Loomy 没有「关闭思考」的可靠表达，跳过注入",
            };
        }
        if crate::server::core::model_rules::read_client_level(body).is_some() {
            return ReasoningPatch::Skip {
                reason: "客户端请求体里已指定思考等级，绑定让位",
            };
        }
        let Some(rank) = crate::server::core::model_rules::reasoning_rank(level) else {
            return ReasoningPatch::Skip {
                reason: "该等级不在通用档位表内，Loomy 不注入未知档位",
            };
        };
        let effort = match rank {
            0 | 1 => "low",
            2 | 3 => "medium",
            _ => "high",
        };
        ReasoningPatch::Set {
            field: "reasoning_effort",
            value: Value::String(effort.to_string()),
        }
    }

    /// 有远程目录（`GET {网关}/api/v1/models`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一个可用账号）的 session。
    ///
    /// 本家**没有环境变量旁路**，因此「一个账号都没有」时不是失败 ——
    /// 那是「用户还没添加账号」的正常状态，返回 `unchanged()`（与 AutoClaw
    /// 在无登录态时的处置一致：脚本 / CI 不该看到一个红色失败）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.loomy_account_record(account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "Loomy 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials.session, force).await
        })
    }

    /// 有积分概念（双账户：永久 + 每日赠送）
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查积分：`GET /api/v2/points/records` 的摘要（见 `balance.rs`）。
    /// 401 由调用方走统一的「刷新后重试」链路 —— 本家刷新必失败，最终如实报重登。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move { balance::query_usage(store, account_id).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::providers::adapter::adapter_for;
    use serde_json::json;

    /// 上游给「内容太长」时的**真实形态**（dev 3066 实测，2026-10-15）：
    /// HTTP 400 + 中文原文。`{code, message}` 与非 2xx 路径上
    /// `UpstreamErrorDetail::to_value` 给的形状逐字一致。
    fn length_rejection() -> Value {
        json!({
            "code": Value::Null,
            "message": "内容太长了，请精简输入或开启新对话",
        })
    }

    /// 同一件事在华为云层的另一种措辞、另一个状态码。
    fn apig_too_large() -> Value {
        json!({
            "code": Value::Null,
            "message": "{\"error_msg\":\"Request entity too large\",\"error_code\":\"APIG.0201\"}",
        })
    }

    /// 两种形状都要认，而且都要折成 413：只认 413 会把实测在打的那条（400
    /// 中文原文）整个漏掉，落进兜底那一档接着原地重发这份十几 MB 的体。
    #[test]
    fn both_length_rejection_shapes_become_a_413() {
        let adapter = adapter_for(ProviderKind::Loomy);
        for (wire, body) in [(400u16, length_rejection()), (413, apig_too_large())] {
            match adapter.classify_error(wire, &body) {
                UpstreamErrorClass::Fatal { status, message, .. } => {
                    assert_eq!(status, 413, "线路上是 {wire}，客户端要看到的是 413：{message}");
                    assert!(
                        message.contains("与账号无关"),
                        "文案要说明这不是账号的问题：{message}"
                    );
                }
                other => panic!("线路上 {wire} 的长度拒绝不该被归成 {other:?}"),
            }
        }
    }

    /// 折出来的 413 要让**上游原文**留住，同时给一句可执行的处置。
    ///
    /// ⚠️ 只断言「文案里有上游那句」是**空跑**：不折的时候那句本来就在
    /// （`上游返回 400: 内容太长了…`）。判据必须落在折本身与**我们补的那半句**上。
    #[test]
    fn the_upstream_wording_survives_the_fold() {
        let adapter = adapter_for(ProviderKind::Loomy);
        match adapter.classify_error(400, &length_rejection()) {
            UpstreamErrorClass::Fatal { status, message, .. } => {
                assert_eq!(status, 413, "折成 413 才有客户端那一侧的压缩动作");
                assert!(message.contains("内容太长了"), "保住上游原话：{message}");
                assert!(
                    message.contains("把历史清短一些"),
                    "处置要落在「这份请求的体量」上，不是转述上游那句话：{message}"
                );
            }
            other => panic!("应当折成 Fatal + 413，实际 {other:?}"),
        }
    }

    /// 反对照：本家别的失败一格都不许被这条折掉。
    ///
    /// 少了这一条，判据写成「400 一律折 413」也会全绿 —— 那会把「上游偶尔 400
    /// 抖一下」这类真正需要退避的抖动一起治没（症状是偶发失败不再自愈）。
    #[test]
    fn other_failures_keep_their_own_classification() {
        let adapter = adapter_for(ProviderKind::Loomy);
        // 报文非法（我们发坏了）：仍是原样的 400，还该退避重发
        match adapter.classify_error(
            400,
            &json!({"code": Value::Null, "message": "messages: expected array"}),
        ) {
            UpstreamErrorClass::Fatal { status, .. } => assert_eq!(status, 400),
            other => panic!("报文非法不该被折成别的：{other:?}"),
        }
        // 限额：该冷却、该换号
        assert!(matches!(
            adapter.classify_error(
                429,
                &json!({"code": Value::Null, "message": "request rate limit exceeded"})
            ),
            UpstreamErrorClass::QuotaLimited { status: 429, .. }
        ));
        // 凭据失效：该走刷新/重登
        assert!(matches!(
            adapter.classify_error(401, &json!({"code": "020002", "message": "登录已过期"})),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        // ⚠️ `APIG.0201` 单独出现**不算**体积：官方错误码表把这个过载码同时给了
        // 400/413/414/494/500/502/504，5xx 上的过载是真该退避重发的抖动。
        assert!(matches!(
            adapter.classify_error(
                500,
                &json!({"code": Value::Null, "message": "{\"error_code\":\"APIG.0201\"}"})
            ),
            UpstreamErrorClass::Fatal { status: 500, .. }
        ));
    }
}
