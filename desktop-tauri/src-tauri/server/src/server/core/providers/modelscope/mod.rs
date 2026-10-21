//! ModelScope（魔搭）适配器：**无状态 OpenAI 兼容转发 + 每日签到 + 魔粒余额**。
//!
//! ── 上游长什么样（2026-10-16 实测；接口清单来自 CPA 的 `modelscope-ratelimit`
//!    插件源码，逐条 curl 复核）───────────────────────────────
//! ```text
//!   推理      POST {推理域}/chat/completions     Authorization: Bearer <ms-令牌>
//!   目录      GET  {推理域}/models               同上
//!   签到      GET  {网页站}/api/v1/users/login/info        Cookie: m_session_id=<令牌>
//!   魔粒余额  GET  {网页站}/openapi/v1/magicubes/balance   同上
//!   今日用量  GET  {网页站}/api/v1/inference/rate-limit    同上
//! ```
//! 两个域（推理 / 网页站）各一套地址，两地（国内 / 国际）各一套账号 ——
//! 全部收在 [`region`] 里，别处不要再写域名。
//!
//! 本家**无状态**（一次 HTTP 请求 = 一次回答，标准 OpenAI 协议），与 Loomy /
//! AutoClaw / OfficeAce 同构：`supports_chat` / `is_stateful` 走默认值，
//! 转发链只需 `build_chat_request`。
//!
//! ── 三个实测事实决定了本家的形状 ─────────────────────────────
//!   1. **一个 `ms-` 令牌两用**（Bearer 调推理、cookie 值调网页站）⇒ 只要一个
//!      凭证字段（见 `credentials.rs`）；
//!   2. **两个站点的账号体系是分开的**（国内令牌打 `www.modelscope.ai` 得
//!      `401 access forbidden`）⇒ 地区必须是 provider 身份而不是账号上的字段
//!      （见 `region.rs`）；
//!   3. **模型清单是账号相关的**（实测 36 条，不是全站几千个）⇒ 不编静态兜底
//!      表（见 `models.rs`）。
//!
//! ── 与别家的取舍（都写在各自模块头，这里给索引）──────────────
//!   1. **没有续期**（`supports_refresh = false`）：`ms-` 令牌长期有效、上游没有
//!      refresh 接口，失效只能重新粘贴 —— 见 `credentials.rs`；
//!   2. **没有环境变量覆盖**：两个域名是官方硬编码的稳定值，加一层没人会用的
//!      分支只是过度设计（KISS）—— 见 `region.rs`；
//!   3. **没有静态兜底模型清单**：编一份会让用户看到自己其实调不通的模型 ——
//!      见 `models.rs`。
//!
//! ── 思考等级 ────────────────────────────────────────────────
//! 上游是标准 OpenAI 兼容，`/v1/models` 不给能力位（实测只有 id），因此
//! **不做** `reasoning_patch`：按 id 猜思考位会在两个方向上一起猜错
//! （把非思考模型当思考模型、反之亦然）。要绑定的用户走模型规则层显式指定。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   region.rs       地区（两套域名 + 两条 provider 身份）
//!   credentials.rs  账号记录 → 凭证快照（只有一个令牌字段）
//!   client.rs       网页站（cookie）/ 推理域（Bearer）两条出站链路 + 两种响应信封
//!   models.rs       GET {推理域}/models（10 分钟缓存 + 落盘缓存，两个地区两格）
//!   probe.rs        添加账号时的令牌探活（换身份 + 顺带触发当日签到）
//!   checkin.rs      每日签到（调登录信息接口即触发，幂等）
//!   balance.rs      魔粒钱包 + 今日已用调用数
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod balance;
pub mod checkin;
pub mod client;
pub mod credentials;
pub mod errors;
pub mod models;
pub mod probe;
pub mod region;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

pub use region::Region;

/// 适配器实例：**按地区参数化**（一份实现、两个静态实例）。
///
/// 与 AutoClaw / ZCode / MonkeyCode 同一形态：地区不是账号字段而是 provider 身份，
/// 因此 `adapter_for` 按 kind 给出两个实例之一 —— 这样「这条账号打哪个站」
/// 由身份保证，不需要在每一处转发/查询里再读一次账号上的地区。
///
/// **无状态**：它不持有任何数据 —— 凭证在账号存储、目录在 `models.rs` 的
/// 进程级句柄（按 `region` 分格）。
pub struct ModelScopeAdapter {
    region: Region,
}

/// 国内站适配器（`adapter_for(ProviderKind::Modelscope)` 返回它）
pub static MODELSCOPE_ADAPTER: ModelScopeAdapter = ModelScopeAdapter { region: Region::Cn };

/// 国际站适配器（`adapter_for(ProviderKind::ModelscopeIntl)` 返回它）
pub static MODELSCOPE_INTL_ADAPTER: ModelScopeAdapter = ModelScopeAdapter { region: Region::Intl };

/// 出站请求体：原样透传 + 把 `stream_options.include_usage` 置真。
///
/// 实测依据见 `build_chat_request` 的说明：不置真时魔搭的流式响应**永远不给
/// 真值 usage**（每个 chunk 都带全零），网关的记账与客户端的 token 显示会一起归零。
///
/// 非对象 body（畸形请求）原样返回，不在这一层报错 —— 让上游去拒绝它。
fn request_body(body: &Value) -> Value {
    let mut payload = body.clone();
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    // 保留客户端已设的其余 `stream_options` 键，只把 include_usage 置真
    let mut options = object
        .get("stream_options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    options.insert("include_usage".to_string(), Value::Bool(true));
    object.insert("stream_options".to_string(), Value::Object(options));
    payload
}

impl ProviderAdapter for ModelScopeAdapter {
    fn kind(&self) -> ProviderKind {
        self.region.kind()
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存，两个地区各一格）
    fn list_models(&self) -> Vec<Value> {
        models::list(self.region)
    }

    /// 构造 `POST {推理域}/chat/completions`（标准 OpenAI 协议，body 原样透传
    /// **外加一个 `stream_options.include_usage`**，理由见下）。
    ///
    /// ── 为什么必须注入 `include_usage`（2026-10-16 实测，一个会静默毁账的坑）──
    /// 魔搭的流式响应是**非标准形态**：**每一个 chunk 都带一个 `usage` 对象**，
    /// 而且**值全是 0**（标准 OpenAI 是除最后一块外都为 `null`）。真值只在末尾那个
    /// `choices: []` 的 usage-only 块里 —— **而且那一块只在请求显式带上
    /// `stream_options.include_usage: true` 时才发**。实测对照（同一模型同一提问）：
    ///
    /// ```text
    ///   不带 include_usage：最后一帧是 {"choices":[{"finish_reason":"stop"}],
    ///                                  "usage":{0,0,0}}          ← 真值根本没出现
    ///   带   include_usage：{"choices":[], "usage":{62,32,94}}    ← 真值在这一块
    /// ```
    ///
    /// 后果链：本网关**恒以流式请求上游**（`upstream::payload` 的既有口径），
    /// 客户端要非流式时再由聚合器把流折成完整响应（`aggregate.rs` 模块头）。
    /// 不注入这个开关时，聚合器读到的最后一个 usage 就是那串全零 ——
    /// 于是**非流式请求的 usage 恒为 0**，请求日志、报表与前端 token 显示一起归零，
    /// 而内容完全正常（这个故障只在统计上可见，排查时极难联想到「上游少发了一帧」）。
    /// 流式请求受影响较小（真值帧本来就没下发，客户端同样看到全零）。
    ///
    /// 与 CodeArts（`codearts/chat.rs`）的处置一致：**总是请求 usage**。
    ///
    /// ── 为什么**不**改别的（模型与流式意向都原样透传）────────────────
    /// 魔搭的 `/v1/models` 里混着非对话模型（生图、基座，见 `models.rs`），
    /// 但「哪些模型支持什么」本网关没有任何可信依据去猜（按 id 猜会猜错），
    /// 让上游自己判 —— 它给的 400 会被 `classify_error` 如实透出（不是 `Fatal`，
    /// 会换账号）。模型 id 也原样发（`组织/模型名`，没有路由前缀那套）。
    ///
    /// 客户端已经自己写了 `stream_options` 时**保留它的其余键**、只把
    /// `include_usage` 置真：那两个字段本就是标准 OpenAI 的同一个对象，
    /// 覆盖整个对象会把用户显式设过的东西悄悄丢掉。
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
                "ModelScope 账号缺少令牌，无法转发（请重新粘贴 `ms-` 令牌）",
            ));
        }
        let headers: Vec<(String, String)> = vec![
            // send_raw 会自动补 Content-Type；这里只加鉴权与 Accept
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Accept".to_string(), "text/event-stream".to_string()),
        ];
        Ok(ChatRequestPlan::chat(
            format!("{}/chat/completions", self.region.inference_base_url()),
            headers,
            request_body(body),
        ))
    }

    /// 上游错误分类。**顺序要紧**：两条「账号级」判据必须在裸 401/403 与裸 429 之前，
    /// 否则它们会被归成「令牌失效」或「速率窗口」，各自的正确处置就都丢了。
    ///
    ///   1. **魔粒耗尽**（429 + `insufficient balance`）→ `QuotaLimited` +
    ///      冷却到下一个北京零点。它不是速率窗口：充值之前这把令牌**一直**不可用，
    ///      按分钟冷却只会在当天反复白撞；
    ///   2. **未验证邮箱**（403 + 特征串）→ 同样冷却到次日。⚠️ 漏了这条的后果最糟：
    ///      403 会被当成「令牌失效」，界面提示用户去**重新粘贴一个本来就好好的令牌**，
    ///      而真正要做的是去账号设置页完成邮箱验证 —— 一个把人引向错误方向的提示；
    ///   3. **裸 401 / 403** → `TokenExpired`（本家无续期，刷新链会走到「重新粘贴」提示）；
    ///   4. **其余 429** → `QuotaLimited`，上游不给结构化恢复时间（`reset_at: None`，
    ///      由 `mark_rate_limited` 给 10 分钟兜底）；
    ///   5. 其余 → 内容拦截分类（审核文案不罚账号）或原样透传。
    ///
    /// 前两条的判据依据（上游真实报文）见 `errors.rs` 的模块头。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .or_else(|| error_body.get("Message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        let upstream_code = error_body
            .get("code")
            .or_else(|| error_body.get("Code"))
            .and_then(Value::as_i64);
        let message = format!("上游返回 {status}: {raw}");
        let label = self.region.label();

        if errors::is_insufficient_balance(status, raw) {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: Some(errors::next_day_boundary_ms(logging::now_ms())),
                message: format!(
                    "ModelScope {label}账号的魔粒已耗尽（上游：{raw}），将在下一个自然日\
                     （北京时间 0 点）自动恢复；给账号充值后立即恢复"
                ),
                upstream_code,
                status,
            };
        }
        if errors::is_unverified_account(status, raw) {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: Some(errors::next_day_boundary_ms(logging::now_ms())),
                message: format!(
                    "ModelScope {label}账号未完成邮箱验证，魔粒推理接口拒绝服务\
                     （上游：{raw}）——请到魔搭账号设置页完成邮箱验证，验证后次日自动恢复"
                ),
                upstream_code,
                status,
            };
        }
        if status == 401 || status == 403 {
            return UpstreamErrorClass::TokenExpired { message };
        }
        if status == 429 {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code,
                status,
            };
        }
        content_block::classify_or_fatal(status, error_body, message, upstream_code)
    }

    /// 取可用令牌。本家**没有续期**：这里只做「记录里有没有令牌」的检查，
    /// 服务端拒了就是 401，编排层会把它归成 `TokenExpired` 并在刷新失败后
    /// 如实报给客户端（「请重新粘贴令牌」）。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        let region = self.region;
        Box::pin(async move {
            let record = store.modelscope_account_record(region, account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!(
                        "ModelScope {}账号 {account_id} 不存在（请重新添加）",
                        region.label()
                    ),
                ));
            }
            let credentials = credentials::from_record(region, record.as_ref())?;
            Ok(credentials.token)
        })
    }

    /// 没有续期手段（上游没有 refresh 接口）——维护任务不问本家
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 有远程目录（`GET {推理域}/models`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 本家库里第一个可用账号）的令牌。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（与 Loomy / AutoClaw 在无登录态时的处置一致：
    /// 脚本 / CI 不该看到一个红色失败）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        let region = self.region;
        Box::pin(async move {
            let record = store.modelscope_account_record(region, account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    crate::server::logging::verbose(
                        "[Models]",
                        &format!("ModelScope {}模型目录刷新跳过：尚未添加账号", region.label()),
                    );
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(region, record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(region, &credentials.token, force).await
        })
    }

    /// 有余额概念（魔粒钱包）
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查魔粒余额与今日已用调用数（见 `balance.rs`）。
    /// 401 由调用方走统一的「刷新后重试」链路 —— 本家刷新必失败，最终如实报重贴令牌。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>>
    {
        let region = self.region;
        Box::pin(async move { balance::query_usage(region, store, account_id).await })
    }
}

#[cfg(test)]
mod tests {
    use super::request_body;
    use serde_json::json;

    /// 这是整个渠道最关键的一处注入：漏了它，非流式请求的 usage 恒为 0
    /// （实测依据见 `build_chat_request` 的说明）
    #[test]
    fn include_usage_is_injected() {
        let body = json!({"model": "Qwen/Qwen3.8-Flash-Next", "stream": true});
        let out = request_body(&body);
        assert_eq!(
            out.pointer("/stream_options/include_usage").and_then(|v| v.as_bool()),
            Some(true)
        );
        // 其余字段原样保留
        assert_eq!(out.get("model").and_then(|v| v.as_str()), Some("Qwen/Qwen3.8-Flash-Next"));
        assert_eq!(out.get("stream").and_then(|v| v.as_bool()), Some(true));
        // 调用方传进来的 body 不被就地改动（`build_chat_request` 拿的是借用）
        assert!(body.get("stream_options").is_none());
    }

    /// 客户端自己写过 `stream_options` 时，只补 include_usage、不覆盖整个对象
    #[test]
    fn other_stream_option_keys_survive() {
        let out = request_body(&json!({"stream_options": {"custom_flag": 1}}));
        assert_eq!(out.pointer("/stream_options/custom_flag").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            out.pointer("/stream_options/include_usage").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    /// 畸形 body（非对象）原样返回 —— 让上游去拒绝它，不在这一层报错
    #[test]
    fn a_non_object_body_is_passed_through() {
        assert_eq!(request_body(&json!(null)), json!(null));
        assert_eq!(request_body(&json!([1, 2])), json!([1, 2]));
    }

    /// 错误分类的分支顺序：两条「账号级」判据必须先于裸 401/403 与裸 429 命中
    /// —— 顺序反了它们就会被归成「令牌失效」或「速率窗口」，各自的正确处置都丢。
    #[test]
    fn classify_error_splits_account_level_failures_from_rate_windows() {
        use crate::server::core::providers::adapter::{ProviderAdapter, UpstreamErrorClass};

        let adapter = &super::MODELSCOPE_ADAPTER;

        // ① 魔粒耗尽（上游真实报文）→ 限流 + 冷却到明天（reset_at 必须给出来）
        match adapter.classify_error(429, &json!({"message": "insufficient balance"})) {
            UpstreamErrorClass::QuotaLimited { reset_at, status, .. } => {
                assert_eq!(status, 429);
                let reset_at = reset_at.expect("魔粒耗尽的冷却必须给到次日零点，不能是 None");
                assert!(reset_at > crate::server::logging::now_ms(), "冷却时刻必须在未来");
            }
            other => panic!("魔粒耗尽应当归成限流，实际是 {other:?}"),
        }

        // ② 未验证邮箱（上游真实文案的一段）→ 同样是次日冷却，**不是**「令牌失效」
        let unverified = json!({
            "message": "For account security, please complete your profile via email \
                        verification. ModelScope Magicubes is only available to verified \
                        accounts. Please refer to https://modelscope.cn/my/settings/account"
        });
        match adapter.classify_error(403, &unverified) {
            UpstreamErrorClass::QuotaLimited { reset_at, .. } => {
                assert!(reset_at.is_some(), "未验证邮箱也要冷却到次日");
            }
            other => panic!("未验证邮箱应当归成限流而不是令牌失效，实际是 {other:?}"),
        }

        // ③ 裸 403（没有特征串）→ 令牌失效
        assert!(matches!(
            adapter.classify_error(403, &json!({"message": "access forbidden"})),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        // 裸 401 同理
        assert!(matches!(
            adapter.classify_error(401, &json!({"message": "unauthorized"})),
            UpstreamErrorClass::TokenExpired { .. }
        ));

        // ④ 普通 429（速率窗口）→ 限流且**不**给 reset_at（交给存储层 10 分钟兜底）
        match adapter.classify_error(429, &json!({"message": "rate limit exceeded"})) {
            UpstreamErrorClass::QuotaLimited { reset_at, .. } => {
                assert!(reset_at.is_none(), "速率窗口不该被冷却到明天");
            }
            other => panic!("速率窗口应当归成限流，实际是 {other:?}"),
        }

        // ⑤ 「insufficient balance」配非 429 状态码时**不**走魔粒耗尽那条
        assert!(matches!(
            adapter.classify_error(400, &json!({"message": "insufficient balance"})),
            UpstreamErrorClass::Fatal { .. } | UpstreamErrorClass::ContentBlocked { .. }
        ));
    }
}