//! Antigravity 的 `ProviderAdapter` 实现：账号 / token 刷新 / 模型目录 /
//! **聊天转发**。
//!
//! ── 转发的落点（一句话：加一个 `UpstreamResponse` 变体 + 一层翻译）────
//! 上游是**无状态 HTTP**（一次 `POST …:streamGenerateContent?alt=sse` = 一次生成），
//! 只是「响应帧不是 OpenAI 方言」：SSE 每帧是 v1internal 的信封
//! （`data: {"response":{…gemini 响应…}}`，规格 §4.2），且字段路径、思考位、
//! 工具调用全按 Gemini 的形状给。本仓对这类上游的既定解法是
//! [`UpstreamResponse`](crate::server::core::providers::adapter::UpstreamResponse)
//! **加一个变体 + 翻译层** —— 本家落的正是 `UpstreamResponse::AntigravityGemini`：
//! ```text
//!   请求转换   core::protocol::antigravity_outbound（信封）
//!             + core::protocol::antigravity_schema（工具 schema 清洗）
//!   响应翻译   core::protocol::antigravity_stream（Gemini SSE → chat SSE）
//!             壳：core::upstream::translate::AntigravityToChatStream
//!   分派       core::upstream::provider_loop 的第三个分支
//! ```
//! 于是账号轮换、限额冷却、退避重试、usage 记账与取消处理全部留在编排层。
//! 改成 `is_stateful = true` + `forward_conversation` 会把那五样在适配器里重写
//! 一遍，而本家并没有多步会话协议 —— 没有理由付那份代价。
//!
//! ── 本家有 / 没有的东西（如实声明，别照抄别家）────────────────
//!   - **有网页登录**（`supports_web_login = true`）：Google OAuth 授权码 +
//!     loopback 回调（授权地址由本模块的 `build_login_url` 本地拼、回调落网关
//!     `/oauth-callback`、换码见 `oauth::exchange_code`）。与粘贴 refresh token
//!     两条入口共用 `add_antigravity_account` 落账号；
//!   - **没有签到**（`core::auto_checkin` 的清单不含本家：Antigravity 没有可自动
//!     领取的奖励活动）；
//!   - **没有余额查询**（`supports_usage` 保持默认 false）：额度是「每模型剩余
//!     比例」（`fetchAvailableModels` 的 `quotaInfo.remainingFraction`，随模型
//!     条目一起进目录缓存），不是账号级余额 —— 界面上要展示时走模型条目里的
//!     `quota` 键，不需要一条独立的余额链路；
//!   - **不发 `x-goog-api-client`**、不采集设备指纹（见 `endpoints.rs` 的模块头）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, RetryAdvice,
    UpstreamErrorClass,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::core::{model_rules, protocol};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{credentials, endpoints, models, oauth, project};

/// Antigravity 适配器（无状态单例；身份全在账号记录里）
pub struct AntigravityAdapter;

/// 静态单例（`adapter_for(ProviderKind::Antigravity)` 返回这一个）
pub static ANTIGRAVITY_ADAPTER: AntigravityAdapter = AntigravityAdapter;

impl ProviderAdapter for AntigravityAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Antigravity
    }

    /// 模型清单来自远程目录（`models.rs`：进程缓存 + 落盘缓存 + 内置兜底）。
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 v1internal 的 `:streamGenerateContent?alt=sse` 请求（信封见
    /// `protocol::antigravity_outbound` 的模块头；响应协议标
    /// `UpstreamResponse::AntigravityGemini`）。
    ///
    /// ── 为什么流式与非流式客户端都用流式端点 ─────────────────────
    /// 非流式客户端由编排层把翻译后的 chat SSE 聚合出完整响应
    /// （`provider_loop` 的两处出口共用同一台翻译机）。两个端点的信封完全一样
    /// （规格 §3.3），只维护一条 URL 判定就少一处「非流式走了另一套信封」的
    /// 分叉面；真要用 `:generateContent` 也只是换 `endpoints::generate_url`。
    ///
    /// ── 基址为什么取 daily（不是 sandbox，也不是 prod）───────────
    /// 规格 §3.1：Manager 优先 sandbox、9router 的聊天流量**固定 daily**。
    /// 编排层的重试是「换账号 / 换家」而不是「换域名」（本仓没有
    /// per-URL fallback 的钩子），所以这里选两参考交集里被 9router 长期使用、
    /// 且不是最容易 429 的 prod 的那条 —— **端点级 failover（规格 §7.15）本步
    /// 未实现**，报告中已列为下一步（`endpoints::V1_BASE_URLS` 已经有三条基址，
    /// 缺的只是编排层让适配器换 URL 重发的那条通路）。
    ///
    /// ── 账号字段从哪读（会话形态，与另外几家同一约定）────────────
    /// `auth.accessToken`（公开形态里没有它）＋ 本家的两个会话附加键
    /// （`account_store::store` 的 `session_from_record` 注入）：`projectId`
    /// （cloudaicompanionProject，OAuth 刷新与目录刷新时发现并回写）与 `email`
    /// （决定信封里的 `userAgent` 标记：非 gmail/googlemail → `jetski`，
    /// Manager 的判定逐字）。`projectId` 缺失时不写信封里的 `project` 键并留
    /// 一行日志 —— 上游可能因此限制这次请求（规格 §7.16），但账号在添加 /
    /// 刷新 / 拉目录时都会尽力发现一次，走到这里为空是少数路径。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let access_token = account
            .pointer("/auth/accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if access_token.is_empty() {
            return Err(GatewayError::with_status(
                401,
                "Antigravity 账号缺少 access token，无法转发（请重新粘贴 refresh token，\
                 或等令牌自动刷新后再试）",
            ));
        }
        let requested = body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if requested.is_empty() {
            return Err(GatewayError::with_status(
                400,
                "Antigravity 请求缺少 model 字段（本家的模型名放在请求体里，\
                 没有路径段的模型名可用）",
            ));
        }
        // 对外 id → 上游真名（表见 models.rs；对不上的原样透传）
        let wire_model = models::upstream_model_id(requested);
        if wire_model != requested {
            logging::verbose(
                "[Antigravity]",
                &format!("按本家映射表改写模型名 {requested} → {wire_model}"),
            );
        }
        let project = account
            .get("projectId")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if project.is_empty() {
            logging::verbose(
                "[Antigravity]",
                "账号缺 cloudaicompanionProject，本次信封不带 project 字段\
                 （上游可能限制该请求；拉一次模型目录或重新登录即可发现）",
            );
        }
        let email = account
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let enterprise = !email.is_empty()
            && !email.ends_with("@gmail.com")
            && !email.ends_with("@googlemail.com");
        let envelope =
            protocol::antigravity_outbound::antigravity_request_from_chat(
                body,
                &wire_model,
                project,
                enterprise,
            )
            .map_err(|message| {
                GatewayError::with_status(
                    400,
                    format!("Antigravity 请求转换失败：{message}"),
                )
            })?;
        let url = endpoints::stream_generate_url(endpoints::V1_BASE_URL_DAILY);
        // content 请求的头集合（不带 `x-goog-user-project`、不带
        // `x-goog-api-client`，见 endpoints.rs 的模块头）
        let headers = endpoints::content_headers(access_token);
        Ok(ChatRequestPlan::antigravity_gemini(url, headers, envelope))
    }

    /// 转发已接通（`build_chat_request` 返回
    /// `UpstreamResponse::AntigravityGemini` 那条翻译通道）。
    ///
    /// 骨架期这里曾是 `false`（「先上账号管理、后接转发」的过渡态，trait 的
    /// 文档把它留给的正是那种情形）；现在 `build_chat_request` 已接真身，
    /// 两件事必须同步改 —— 否则界面会继续说「未接通」、`pick_current` 也不会
    /// 把本家账号排进队首（`supports_chat` 的两个消费方见 trait 的文档）。
    fn supports_chat(&self) -> bool {
        true
    }

    /// **账号未配代理时跟随系统代理**（`true`；仓内唯一覆写这一位的家）。
    ///
    /// ── 为什么本家是特例 ────────────────────────────────────────
    /// 上游是 Google：多数网络里只有经代理才可达，直连必然 TCP 超时
    /// （用户实测 `os error 10060`）。而用户机器上「已经能打开 Google 的
    /// 那个代理」就写在系统设置里 —— 浏览器能打开授权页正是靠它。账号若
    /// 没单独配代理，跟随它是唯一合理的默认；系统没配代理时 reqwest 探测
    /// 不到，等价于直连，不引入新的失败面。
    ///
    /// 口径与登录链路一致（那边本来就无从挂账号代理，走
    /// `egress::client_for_system_proxy()`；见 `super::client_for` 的文档）。
    /// **别把这一位抄到别家** —— 对国内可直连的上游，「直连就是直连」才是
    /// 正确口径（trait 文档里有完整论证）。
    fn system_proxy_when_unset(&self) -> bool {
        true
    }

    /// 把「映射上绑的思考等级」翻译成本家认的字段：写成 body 顶层的
    /// `reasoning_effort`，由请求转换（`protocol::antigravity_outbound` 的
    /// `thinking_budget`）映射成 `generationConfig.thinkingConfig.thinkingBudget`。
    ///
    /// ── 为什么中间落一个通用字段而不是直接改信封 ──────────────────
    /// `build_chat_request` 的入参是 **chat 形态**的 body（此时还没有信封），
    /// 而本仓的注入点（`upstream::payload::apply_reasoning`）只做「按适配器给的
    /// 字段名写进 body」。于是这里复用客户端本来就会用的通用键
    /// `reasoning_effort`（`model_rules::read_client_level` 的第一优先键）：
    ///   1. 请求转换读它 → 档位进 `thinkingBudget`（Manager 的档位规范值）；
    ///   2. `outbound_reasoning`（默认实现）也读它 → 请求日志的「上游等级」列
    ///      因此能看到绑定生效后的档位，与另外两家的显示口径一致。
    ///
    /// 判据（三条，与 CatPaw / Qoder 的实现同一闸门）：
    ///   - 客户端已显式指定档位 → 让位（用户的明确意图比映射默认值更具体）；
    ///   - `off` / `none`：注入点已经拦下（不会问到这里，见 trait 的文档）；
    ///   - 表外自定义等级（`model_rules::reasoning_rank` 返回 None）→ 不注入。
    fn reasoning_patch(&self, level: &str, _model: &str, body: &Value) -> ReasoningPatch {
        if model_rules::read_client_level(body)
            .filter(|value| !model_rules::reasoning_is_off(value))
            .is_some()
        {
            return ReasoningPatch::Skip {
                reason: "客户端请求体里已指定思考档位",
            };
        }
        if model_rules::reasoning_rank(level).is_none() {
            return ReasoningPatch::Skip {
                reason: "该等级不在本家接受的档位内（本家只认通用档位）",
            };
        }
        ReasoningPatch::Set {
            field: "reasoning_effort",
            value: Value::String(level.trim().to_string()),
        }
    }

    /// 上游错误分类（规格 §7 的映射表 + 本仓的四档口径）。
    ///
    /// ── 判据为什么先看文案再看状态码 ────────────────────────────
    /// Google 把业务语义放在 `error.status` 里（`PERMISSION_DENIED` /
    /// `RESOURCE_EXHAUSTED` / `UNAVAILABLE`），HTTP 状态只是它的投影：
    ///   - `RESOURCE_EXHAUSTED` → 限额（**即使状态码不是 429** —— 规格坑 #11 记
    ///     载了「官方客户端在 agent 路径撞无细节 429」这一现象，文案才是可靠信号）；
    ///   - `PERMISSION_DENIED` → 403，本家的含义是「这个账号没有资格 / 未开通」
    ///     （规格 §6：受限地区可能 403）—— 归 `TokenExpired`：编排层会先刷一次
    ///     凭证、再按队列换下一个账号，那正是这种账号该走的路；
    ///   - 其余 401 / 403 → `TokenExpired`（凭证被拒）；
    ///   - 429 → `QuotaLimited`；400 档交给共用的内容拦截判定；
    ///   - 408 / 5xx → `Fatal` + [`Self::retry_advice`] 的原地退避（可重试）；
    ///   - 其余 → `Fatal`（原样透出）。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        let message = format!("上游返回 {status}: {raw}");
        let code = error_body.get("code").and_then(Value::as_i64);
        let lowered = raw.to_ascii_uppercase();
        if lowered.contains("RESOURCE_EXHAUSTED") {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: code,
                // 429 是下游看到的语义（额度用尽 / 限流），即使上游回的是别的码
                status: 429,
            };
        }
        if status == 401 || status == 403 {
            return UpstreamErrorClass::TokenExpired { message };
        }
        if status == 429 {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: code,
                status,
            };
        }
        if status == 400 {
            return content_block::classify_or_fatal(status, error_body, message, code);
        }
        UpstreamErrorClass::Fatal {
            status,
            message,
            upstream_code: code,
        }
    }

    /// 「这个错误要不要原地退避重试」（模块头扩展 1）。
    ///
    /// ── 与编排层两条全局兜底的关系（先说清，免得误会成必需）──────
    /// 编排层自己已经有两档统一兜底：`transient_retry_advice`（408 / 5xx 按状态码）
    /// 与 `fallback_retry_advice`（一切 `Fatal` 在换账号前先原地重发一次）。
    /// 因此**「5xx 可重试」这件事不靠本方法也成立** —— 本方法的增量只有两点：
    ///   1. 给出一条本家措辞的原因（`RetryAdvice::reason` 会进请求日志的重试链）；
    ///   2. 覆盖「状态码不显眼、文案才是信号」的情形：Google 把瞬时故障写成
    ///      `error.status = UNAVAILABLE / INTERNAL / DEADLINE_EXCEEDED`，
    ///      message 里可能出现 `overloaded` / `try again` 这类措辞
    ///      （规格 §7.14 的首包空 / 流提前结束也归这一档）。
    ///
    /// ── 判据为什么只能看文案（如实说明）────────────────────────
    /// 契约只把**错误体**传进来，而 Google 的错误体把语义放在 `error.status` 里
    /// （归一化层读的是顶层 `code` 与 `error.message`），所以这里按 message 的
    /// 关键词判瞬时故障。429 不走这里 —— 它是 `QuotaLimited`，换账号比原地重试有用。
    fn retry_advice(&self, error_body: &Value, attempt: usize, budget: usize) -> Option<RetryAdvice> {
        if attempt >= budget {
            return None;
        }
        let raw = error_body
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let lowered = raw.to_ascii_lowercase();
        let transient = ["unavailable", "internal", "deadline_exceeded", "overloaded", "try again", "timeout"]
            .iter()
            .any(|pattern| lowered.contains(pattern));
        if !transient {
            return None;
        }
        let retry = crate::server::config::retry_settings();
        Some(RetryAdvice {
            delay_ms: retry.delay_ms(),
            reason: format!("上游暂时不可用（{raw}），稍后重试"),
        })
    }

    /// 取可用 access token：临期（提前 15 分钟）或没有 token 时刷新并回写。
    ///
    /// 刷新链（单飞 + 比较再写）在 `oauth::ensure_fresh` —— 本方法只做转调，
    /// 这样「401 后的强制刷新」与「维护任务的临期刷新」共用同一段实现。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let credentials = oauth::ensure_fresh(store, account_id, false).await?;
            if credentials.access_token.trim().is_empty() {
                return Err(GatewayError::with_status(
                    401,
                    "Antigravity 账号没有可用的 access token（刷新未返回令牌），请重新粘贴 refresh token",
                ));
            }
            Ok(credentials.access_token)
        })
    }

    /// 401 后的**强制**刷新：无视临期判定直接刷一次。
    ///
    /// 上游被拒时不会告诉我们令牌还剩多久（Google 的 401 也可能是服务端提前
    /// 失效），走临期门会原样返回刚被拒的串，让编排层的「刷新后重试一次」
    /// 退化成「拿同一个坏 token 再打一次」。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let credentials = oauth::ensure_fresh(store, account_id, true).await?;
            Ok(credentials.access_token)
        })
    }

    /// 本家有续期手段（`refresh_token` + Google token 端点）——维护任务要问本家。
    fn supports_refresh(&self) -> bool {
        true
    }

    /// Antigravity 支持网页登录（trait 扩展 7）：Google OAuth 授权码 + loopback
    /// 回调。授权页是 `accounts.google.com`，回调落到**本网关自己的端口**
    /// （`http://localhost:{port}/oauth-callback`，见 `oauth::CALLBACK_PATH`）。
    fn supports_web_login(&self) -> bool {
        true
    }

    /// 授权地址 + state（`antigravity::oauth::build_authorize_url`）。
    ///
    /// state 的生成复用 `raccoon::oauth::new_login_state()`（`new_request_id`
    /// 的 uuid v4 形态）：项目里没有 `rand` / `uuid` 依赖，为一个 state 引进
    /// 依赖不合算 —— 与 raccoon 的模块头同一条论证，也满足 trait 文档
    /// 「必须用不可预测随机源」的契约。逐字比对在
    /// `core::login::submit_login_callback` 的 Antigravity 分支里做。
    ///
    /// 返回 None = 回调端口还没写进进程级常量（bootstrap 之前）；上层会给
    /// 「未能生成网页登录授权地址，请重试」的通用文案。
    fn build_login_url(&self) -> Option<(String, String)> {
        let state = crate::server::core::providers::raccoon::oauth::new_login_state();
        let url = oauth::build_authorize_url(&state)?;
        Some((url, state))
    }

    /// 用回调里的一次性 `code` 换 Google 凭证并落账号（`oauth::exchange_code`）。
    ///
    /// `state` 由调用方逐字比对过（任务表按它索引）；实现里再做一次
    /// 非空 / 长度校验作为深度防御（trait 文档的要求）。
    fn exchange_login_code<'a>(
        &'a self,
        store: &'a AccountStore,
        code: &'a str,
        state: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { oauth::exchange_code(store, code, state).await })
    }

    /// 后台凭证维护问「这条账号是不是快到期了」（判据与刷新用的是同一把尺）。
    ///
    /// 没有 refresh_token 的账号返回 false：它连续期手段都没有，交给维护只会
    /// 变成每轮一条稳定失败的记录（与 Trae 的同一条处置）。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        let Some(record) = store.antigravity_account_record(account_id) else {
            return false;
        };
        match credentials::from_record(Some(&record)) {
            Ok(credentials) if credentials.can_refresh() => credentials.expiring(),
            _ => false,
        }
    }

    /// 有远程目录（`POST {base}:fetchAvailableModels`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// SSE 下发帧的 `model` 回写成**客户端请求的那个名字**。
    ///
    /// 翻译状态机产出的帧带的是**上游真名**（`gemini-3.6-flash-tiered` /
    /// `gemini-pro-agent` …，见 `models::upstream_model_id` 的映射表），而客户端
    /// 请求的是对外 id（`gemini-3.6-flash` / `gemini-3.1-pro-high`）——
    /// 正是 `sse_model_rewrite` 要处理的那种「上游回的名字与请求名不同」。
    /// 与 raccoon / AutoClaw / Cline 同一取舍（那三家的上游也回内部名）；
    /// Command Code / ZCode 那两条翻译通道没开它，是因为它们的发送名与请求名
    /// 逐字相同，没有可改写的差异。
    fn sse_model_rewrite(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 队首可用账号）的令牌打上游。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（与 Loomy / MonkeyCode / Command Code 同一处置：
    /// 脚本 / CI 不该看到一个红色失败）。点名的账号取不到则如实失败。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.antigravity_account_record(account_id);
            let Some(record) = record else {
                if account_id.trim().is_empty() {
                    logging::verbose("[Models]", "Antigravity 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            };
            let credentials = match credentials::from_record(Some(&record)) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            let proxy = match oauth::account_proxy(&record) {
                Ok(proxy) => proxy,
                // 代理配置坏了要如实失败（不静默直连）：直连会拿到一张「从本机
                // 看得到」的表，而用户以为刷的是那条账号的出口。
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            // 令牌：临期就刷一次（复用适配器那条链，成功会回写记录）
            let credentials = if credentials.expiring() {
                match oauth::ensure_fresh(store, account_id, false).await {
                    Ok(fresh) => fresh,
                    Err(error) => return ModelRefreshOutcome::failed(error.message),
                }
            } else {
                credentials
            };
            // project：只在缺失时发现一次（best-effort，失败不影响目录）
            let mut project_id = credentials.project_id.clone();
            if project_id.trim().is_empty() {
                match project::discover_project(&credentials.access_token, proxy.as_ref()).await {
                    Ok(found) => {
                        if store.set_antigravity_project(&credentials.id, &found).is_err() {
                            logging::verbose(
                                "[Antigravity]",
                                "project 发现结果未能写回账号记录（记录可能已被删除）",
                            );
                        }
                        project_id = found;
                    }
                    Err(error) => logging::verbose(
                        "[Antigravity]",
                        &format!("project 未发现（目录刷新继续）：{}", error.message),
                    ),
                }
            }
            models::refresh(&credentials.access_token, &project_id, proxy.as_ref(), force).await
        })
    }
}
