//! 自定义上游的发送入口。相同协议交给原生分支，其他请求使用 Chat 转换链。
//! 账号选路、重试与冷却由 provider_loop 持有；发送分支持有流式连接直至结束。

use std::sync::Arc;

use axum::http::HeaderMap;
use crate::server::core::protocol::native::WireProtocol;

mod native;
mod translate;
use translate::ProtocolTranslateStream;

use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::custom_providers;
use crate::server::core::model_rules;
use crate::server::core::protocol::{anthropic_outbound, responses_outbound};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::custom::emulation;
use crate::server::core::upstream::connections::ConnectionGuard;
use crate::server::core::upstream::request::{read_upstream_error, send_chat_request, TransportRequest};
// `FramePolicy::with_rewrite(...)`：自定义家只带 model 回写，保活换行的丢弃
// 保持关闭 —— 那一项的依据是 WorkBuddy / AutoClaw 两条上游的实测，不能替
// 用户自配的上游猜。
use crate::server::core::upstream::sse::{FramePolicy, ModelRewrite};
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, ForwardStream, InFlightGuard};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 一次自定义家转发的入口（编排层的 `attempt_custom` 调用）。
///
/// `account_id` 是空串表示这一轮没有账号记录 —— 自定义家**没有环境变量旁路**
/// （凭证只来自账号记录，见 `custom_accounts` 的模块头），所以直接 503，
/// 不尝试任何默认登录态。
///
/// `body` 是已经过 `send_body` 处理的 chat 请求体（系统提示词/脱敏已落副本）；
/// 本函数只做「自定义语义」的最后一次改写（模型名 + 思考等级）再出门。
///
/// `pub(crate)` 而不是 `pub`：签名里的 `InFlightGuard` 是 `upstream` 的内部
/// 凭证类型（`pub(crate)`）—— 函数可见性不能超过它暴露的类型。调用面本来
/// 就只有 `upstream::provider_loop` 的编排层，`pub(crate)` 是准确的边界。
pub(crate) async fn forward(
    store: &AccountStore,
    provider_id: &str,
    account_id: &str,
    body: &Value,
    native_protocol: Option<WireProtocol>,
    client_headers: &HeaderMap,
    route_model: Option<&str>,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
    slot: &mut Option<InFlightGuard>,
    connections: &mut ConnectionGuard,
) -> Result<ForwardOutcome, GatewayError> {
    // ── 配置与凭证（顺序：先家后账号，错误文案各自指向要修的地方）──────
    let provider = custom_providers::get(provider_id).ok_or_else(|| {
        GatewayError::with_status(503, format!("自定义提供商不存在: {}", provider_id.trim()))
    })?;
    let protocol = provider
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or(custom_providers::PROTOCOL_CHAT_COMPLETIONS);
    // ── 协议分派（见模块头）────────────────────────────────────
    // 配置校验只放行三种协议，未知值仍 400 兜底 —— 明确的一句比「上游报了
    // 一个看不懂的错」有用得多。
    let kind = match protocol {
        custom_providers::PROTOCOL_CHAT_COMPLETIONS => None,
        custom_providers::PROTOCOL_RESPONSES => Some(OutboundKind::Responses),
        custom_providers::PROTOCOL_ANTHROPIC => Some(OutboundKind::Anthropic),
        other => {
            return Err(GatewayError::with_status(
                400,
                format!(
                    "自定义提供商「{}」的协议 {other} 无法识别，请检查提供商配置",
                    custom_providers::label_of(provider_id).unwrap_or_else(|| provider_id.to_string()),
                ),
            ));
        }
    };
    if account_id.trim().is_empty() {
        return Err(GatewayError::with_status(
            503,
            "没有可用账号，无账号可转发：请在账号页为该自定义提供商添加并启用账号",
        ));
    }
    // 凭证（内部形态：含 apiKey 明文与已解析代理）。取不到说明账号在两次
    // 读盘之间被删了 —— 报一条明确的错，与「顺延」语义同等可操作。
    let credential_store = store.clone();
    let credential_id = account_id.to_string();
    let credential = crate::server::core::blocking::run(move || {
        credential_store.custom_credential_by_id(&credential_id)
    }).await?.ok_or_else(|| {
        GatewayError::with_status(503, format!("自定义账号不存在或已被删除: {account_id}"))
    })?;
    // baseUrl：账号覆盖项优先（缺省回落到提供商的配置）。两层都拦过空值，
    // 这里防御一次 —— 没有基址的转发只会变成一条难定位的连接错误
    let base_url = credential
        .base_url_override
        .clone()
        .or_else(|| {
            provider
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if base_url.trim().is_empty() {
        return Err(GatewayError::with_status(
            400,
            "该自定义提供商没有可用的 baseUrl（提供商与账号上都没有配置）",
        ));
    }
    // 翻译协议在这里分出去（凭证与基址已就绪；chat 路径继续往下走）
    let quirks = ProviderQuirks::from_provider(&provider);
    // 存量记录里可能还写着 OpenCode 免费档的**旧端点**（`/zen/v1`，2026-10 起
    // 打它一律 403）—— 提供商记录与账号覆盖项两层都可能是旧值，所以在**解析出
    // 最终地址之后**再过一次读侧迁移（提供商层已在 `custom_providers::item_of`
    // 过了一遍，账号覆盖项只在这里能碰到）。
    let base_url = custom_providers::migrate_base_url(&base_url, quirks.emulate_opencode);
    if let Some(native_protocol) = native_protocol {
        if WireProtocol::from_provider(protocol) != Some(native_protocol) {
            return Err(GatewayError::with_status(409, "提供商协议在转发前发生变化，请重试；未发送错误协议的请求"));
        }
        return native::forward(
            native_protocol, provider_id, account_id, &credential.api_key, &credential.user_agent,
            &base_url, &quirks, body, route_model, client_headers, proxy, stream, telemetry, slot, connections,
        ).await;
    }
    if let Some(kind) = kind {
        return forward_translated(
            kind,
            provider_id,
            account_id,
            &credential.api_key,
            &credential.user_agent,
            &base_url,
            &quirks,
            body,
            route_model,
            proxy,
            stream,
            telemetry,
            slot,
            connections,
        )
        .await;
    }
    // chat 透传出口：入口翻译（Anthropic / Responses）暂存的内部字段（`_wb_*`，
    // 见 `protocol::mod` 的说明）绝不发上游 —— 严格校验的 OpenAI 兼容上游会拒绝
    // 消息上的未知字段整轮 400。翻译分支（上方已 return）**不**剥：那些字段正是
    // 出站翻译要消费的（cache_control / is_error / encrypted_content 的恢复源）。
    //
    // 剥之前先过原生工具闸门：自定义家的契约是透传（chat 入口带上来的 chat 方言
    // 原生工具，如智谱的 `{"type":"web_search","web_search":{…}}`，原样上行），
    // 但**带标记的跨协议声明**（下游走 /v1/messages 或 /v1/responses 带过来的
    // `web_search_20250305` / `web_search` 那类）没有 chat 形态，硬发只会换来
    // 上游一次 400 —— 剔除 + 留痕，见 `protocol::native_tool` 模块头。
    let cross = crate::server::core::protocol::native_tool::strip_cross_protocol(
        body,
        crate::server::core::protocol::native_tool::ORIGIN_CHAT,
    );
    if let Some((_, dropped)) = &cross {
        logging::log(
            "[CustomProvider]",
            &dropped.describe(
                Some(provider_id),
                Some("该家按 chat 协议透传，承载不了别的协议的原生声明"),
            ),
        );
    }
    let stripped = crate::server::core::protocol::strip_internal_fields(
        cross.as_ref().map(|(next, _)| next).unwrap_or(body),
    );
    let body: &Value = &stripped;
    let url = format!(
        "{}/chat/completions{}",
        base_url.trim_end_matches('/'),
        quirks.url_suffix
    );

    // ── 请求体改写：模型名（映射 alias → 上游真名）+ 思考等级注入 ──────
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let (mut outbound, rewrite) = rewrite_for_route(body, provider_id, &requested, route_model);
    // 客户端形态伪装（OpenCode 免费档）：补桩工具。未开启时不碰请求体。
    // 放在 `rewrite_body` 之后、序列化之前 —— 会话种子取自**改写后**的体，
    // 与最终发出去的字节同源（模型名换了不影响 messages，两种取法等价，
    // 但同源更不容易在将来改坏）
    quirks.apply_emulation_to_body(&mut outbound, &base_url);
    telemetry.note_upstream_reasoning(
        model_rules::read_client_level(&outbound).filter(|level| !model_rules::reasoning_is_off(level)),
    );
    let payload = serde_json::to_string(&outbound)
        .map_err(|error| GatewayError::with_status(500, format!("请求体序列化失败: {error}")))?;
    // ── 出站头（三段拼装，顺序不可换）────────────────────────────
    //   ① 默认头：**只有 apiKey 非空才带凭证头** —— 无需鉴权的上游
    //      （`noAuth` 账号）拿到一个空 `Bearer ` 会当成凭证错误，
    //      与「这家不要凭证」的语义正好相反；
    //   ② 伪装头（OpenCode 两个面，按端点分派）：免费档面补齐官方客户端的头
    //      与匿名凭证 `public`；Go 面只补会话头与产品 UA（见方法说明）；
    //   ③ 记录上的静态额外头（`quirks.apply_to`）：用户手写的最优先。
    let api_key = credential.api_key.trim();
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if !api_key.is_empty() {
        headers.push((
            "Authorization".to_string(),
            format!("Bearer {api_key}"),
        ));
    }
    quirks.apply_emulation_headers(&mut headers, api_key, &outbound, &base_url);
    quirks.apply_to(&mut headers);
    // 账号级自定义 User-Agent 放在**最后**：伪装头与用户手写的静态头都可能
    // 写了 UA（`quirks.headers` 里就有），而账号设置里填的值是用户对**这一条
    // 账号**的最新意图，优先级最高。大小写不敏感地摘掉同名头再追加，避免留下
    // 两个 `user-agent` 让上游取哪个变成实现细节。
    apply_user_agent_override(&mut headers, &credential.user_agent);

    logging::verbose(
        "[CustomProvider]",
        &format!(
            "POST {url} model={} stream={stream} account={account_id} 出口={}{}",
            if requested.is_empty() { "(默认)" } else { &requested },
            describe_proxy(proxy.as_ref()),
            if quirks.emulate_opencode { " 伪装=opencode" } else { "" },
        ),
    );
    let transport = TransportRequest {
        url,
        headers,
        payload,
        proxy,
        // 自定义家没有能力位（用户自己配的出口就是唯一口径：配了就代理，没配就直连）
        system_proxy_when_unset: false,
    };
    let response = send_chat_request(&transport)
        .await
        .map_err(|error| error.to_gateway_error())?;

    // ── 响应：2xx 打包 outcome；非 2xx 分类（见模块头）────────────────
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // Retry-After 要在读 body **之前**取（read_upstream_error 会吃掉响应）
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let detail = read_upstream_error(response, telemetry.capture().as_deref()).await;
        return Err(classify_custom_error(
            status,
            &detail.message,
            retry_after.as_deref(),
        ));
    }

    if stream {
        // 槽位与连接计数移交给流（与无状态路径同一时序，见模块头）
        Ok(ForwardOutcome::Stream {
            status,
            stream: Box::new(ForwardStream::new(
                response,
                slot.take(),
                connections.handoff(),
                telemetry.clone(),
                FramePolicy::with_rewrite(rewrite),
            )),
        })
    } else {
        // 非流式：聚合内部同样按 `rewrite` 回写 body 的 model 字段；
        // 凭证留在调用方栈帧，随本函数返回析构（与无状态路径同）
        let aggregated = crate::server::core::upstream::aggregate::aggregate_sse_completion(
            response,
            telemetry.clone(),
            FramePolicy::with_rewrite(rewrite),
        )
        .await?;
        Ok(ForwardOutcome::Completion {
            body: aggregated.body,
        })
    }
}

/// 出站翻译协议（[`forward_translated`] 两个分支的差异点收敛在这个枚举上）
#[derive(Clone, Copy)]
enum OutboundKind {
    /// OpenAI Responses（`POST {base}/responses`）
    Responses,
    /// Anthropic Messages（`POST {base}/v1/messages`）
    Anthropic,
}

impl OutboundKind {
    /// 日志与文案里的协议名
    fn label(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }
}

/// 提供商记录上的 **per-provider 特判**（`urlSuffix` / `headers` /
/// `anthropicToolType` / `clientEmulation`，形状见 `custom_providers` 的模块头）。
///
/// 这些字段由预置目录在创建时写入（参考实现 9Router 每家适配器里的修正：
/// GLM / MiniMax 的 `?beta=true` 与 `Anthropic-Beta`、OpenRouter 的来源头、
/// MiniMax 的工具 type 补齐），手写的家没有这几个键 —— 提取时缺省即「无特判」，
/// 旧记录不用迁移。
struct ProviderQuirks {
    /// 原样追加到出站 URL 末尾的查询串（如 `?beta=true`）
    url_suffix: String,
    /// 合并到默认头上的静态额外头（同名覆盖默认头）
    headers: Vec<(String, String)>,
    /// 发 anthropic 上游时给每个工具补 `type: "custom"`（MiniMax 拒绝无 type 工具）
    tool_type_custom: bool,
    /// 是否按 **OpenCode 官方 CLI 的形状**补齐出站请求（`clientEmulation`，
    /// 见 [`emulation`]）：无 key 用匿名凭证 `public`、补官方会话头、请求体补
    /// 两个桩工具 —— OpenCode Zen 的免费档只有这个形状才放行。**会改写请求体**，
    /// 所以只在记录显式声明时生效。
    emulate_opencode: bool,
}

impl ProviderQuirks {
    fn from_provider(provider: &Value) -> Self {
        // 盘上数据已经过读侧归一（custom_providers::item_of），这里只做防
        // 御性提取：类型不对的值按缺省跳过，绝不让特判把转发拖成 panic
        let headers = provider
            .get("headers")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|text| (key.clone(), text.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            url_suffix: provider
                .get("urlSuffix")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            headers,
            tool_type_custom: provider
                .get("anthropicToolType")
                .and_then(Value::as_str)
                .is_some_and(|text| {
                    text.eq_ignore_ascii_case(custom_providers::TOOL_TYPE_CUSTOM)
                }),
            emulate_opencode: provider
                .get("clientEmulation")
                .and_then(Value::as_str)
                .is_some_and(|text| {
                    text.eq_ignore_ascii_case(custom_providers::CLIENT_EMULATION_OPENCODE)
                }),
        }
    }

    /// 把特判头按名合并进默认头：同名（ASCII 不区分大小写）的默认头被覆盖，
    /// 其余追加在后面。覆盖默认凭证头（Authorization / x-api-key）意味着换一种
    /// 鉴权方式 —— 那是记录持有者的选择，这里只负责如实执行。
    fn apply_to(&self, headers: &mut Vec<(String, String)>) {
        for (key, value) in &self.headers {
            match headers.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(key)) {
                Some(slot) => slot.1 = value.clone(),
                None => headers.push((key.clone(), value.clone())),
            }
        }
    }
    /// 伪装头与匿名凭证（只在 `clientEmulation: "opencode"` 时动手）。
    ///
    /// `api_key` 为空时用匿名凭证 `public` —— 这正是「不填 Key 也能用 OpenCode
    /// Zen 免费档」的落点；填了 key 就用 key（付费模型必须）。
    ///
    /// **调用顺序**：默认头 → 本函数 → `apply_to`（静态额外头）。放在 `apply_to`
    /// 之前是刻意的：手写在记录上的头优先级最高，伪装不该盖掉用户的显式选择。
    ///
    /// ── 两个面（按 `base_url` 分派，判据见 `custom_providers::is_opencode_go_endpoint`）──
    ///   - **Go 面**（`/zen/go/v1`）：只补会话头与产品 UA（见
    ///     [`emulation::apply_go_headers`]）。它**不套伪装开关** —— 那条要求是
    ///     端点自身公开的路由要求，不是伪装；用户就算把开关打开（或关掉），
    ///     Go 面该发的会话头照发、该走自己的 Key 照走；
    ///   - **免费档面**（其余端点）：四会话头 + 匿名/账号凭证的完整伪装，
    ///     仍然只在开关打开时生效。
    fn apply_emulation_headers(
        &self,
        headers: &mut Vec<(String, String)>,
        api_key: &str,
        body: &Value,
        base_url: &str,
    ) {
        let session = emulation::session_id(&emulation::conversation_seed(body));
        if custom_providers::is_opencode_go_endpoint(base_url) {
            emulation::apply_go_headers(headers, &session);
            return;
        }
        if !self.emulate_opencode {
            return;
        }
        emulation::apply_headers(headers, api_key, &session);
    }

    /// 请求体补成「智能体形态」（`stream: true` + `bash` / `read` 桩工具）。
    /// 未开启伪装时**一个字节都不动** —— 透传是自定义家的默认语义。
    ///
    /// **Go 面同样一个字节都不动**：它拒绝被塞桩工具的请求（社区实测
    /// #44300/#44382），那条桩工具逻辑只属于免费档面。
    fn apply_emulation_to_body(&self, body: &mut Value, base_url: &str) {
        if !self.emulate_opencode || custom_providers::is_opencode_go_endpoint(base_url) {
            return;
        }
        let _ = emulation::ensure_agent_shape(body);
    }
}

/// 账号级自定义 User-Agent 的落地：摘掉同名头（不区分大小写）再追加一条。
///
/// 位置固定在各协议的「头已拼完」之后 —— 伪装头（OpenCode 两个面）与用户
/// 手写在提供商记录上的静态头都可能带 UA，而账号设置里填的值是用户对**这一条
/// 账号**的最新意图，优先级最高。留两条同名头会让上游取哪个变成实现细节。
/// 值在落库时已校验（无控制字符、能转成合法头值），这里不做二次兜底。
fn apply_user_agent_override(headers: &mut Vec<(String, String)>, user_agent: &str) {
    crate::server::core::user_agent::apply(headers, user_agent);
}

/// **翻译协议**（responses / anthropic）的一次转发。
///
/// 与 chat 分支同构的「一次发送」，差异只有三处（都在 [`OutboundKind`] 上
/// 分派）：URL 与鉴权头、请求体翻译（`protocol::*_outbound`）、上游 SSE 的
/// 翻译机（[`ProtocolTranslateStream`]）。其余环节 —— 模型名与思考等级的
/// 改写规则（[`rewrite_body`]，仍落在 chat 体上再整体翻译）、上游恒以
/// `stream:true` 被请求、非 2xx 的错误分类 —— 三种协议共用同一套。
async fn forward_translated(
    kind: OutboundKind,
    provider_id: &str,
    account_id: &str,
    api_key: &str,
    user_agent: &str,
    base_url: &str,
    quirks: &ProviderQuirks,
    body: &Value,
    route_model: Option<&str>,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
    slot: &mut Option<InFlightGuard>,
    connections: &mut ConnectionGuard,
) -> Result<ForwardOutcome, GatewayError> {
    // ── 请求体：先在 chat 体上做自定义语义的改写，再整体翻译 ──────
    // （模型名映射与思考等级绑定是「客户端语义」的修正，与协议无关；
    // 两种上游各自的思考字段由转换器从改写后的 chat 体读取 —— responses
    // 读 reasoning_effort 进 reasoning.effort，anthropic 折成 thinking）
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let (mut outbound_chat, rewrite) = rewrite_for_route(body, provider_id, &requested, route_model);
    // 客户端形态伪装：与 chat 分支同一时机（改写后、翻译前）—— 桩工具要由
    // 转换器一起翻成上游协议的形态（anthropic 的 tools 数组）；Go 面不改请求体
    quirks.apply_emulation_to_body(&mut outbound_chat, base_url);
    telemetry.note_upstream_reasoning(
        model_rules::read_client_level(&outbound_chat).filter(|level| !model_rules::reasoning_is_off(level)),
    );
    // 发给上游的真名 = 改写后的 model 字段（rewrite_body 的产物；请求没带
    // model 时是空串，与 chat 分支「没有回写答案」的口径一致）
    let wire_model = outbound_chat
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // 出站头里的凭证（三段拼装的顺序说明见 chat 分支；这里按协议选凭证头的名字）
    let api_key = api_key.trim();
    let has_key = !api_key.is_empty();

    let (url, headers, payload) = match kind {
        OutboundKind::Responses => {
            let url = format!(
                "{}/responses{}",
                base_url.trim_end_matches('/'),
                quirks.url_suffix
            );
            let mut payload = responses_outbound::responses_request_from_chat(
                &outbound_chat,
                &wire_model,
            )
            .map_err(|message| GatewayError::with_status(400, message))?;
            // 上游恒以 stream:true 被请求（与 chat 协议同一条策略：流式才是
            // 完整能力，非流式下游由聚合路径收流）—— 转换器输出的 stream 只是
            // chat 体的如实搬运，这里统一覆写，不依赖编排层的注入约定
            if let Some(object) = payload.as_object_mut() {
                object.insert("stream".to_string(), Value::Bool(true));
            }
            let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
            if has_key {
                headers.push(("Authorization".to_string(), format!("Bearer {api_key}")));
            }
            quirks.apply_emulation_headers(&mut headers, api_key, &outbound_chat, base_url);
            quirks.apply_to(&mut headers);
            apply_user_agent_override(&mut headers, user_agent);
            (url, headers, payload)
        }
        OutboundKind::Anthropic => {
            // anthropic 惯例 baseUrl 不带 /v1（官方是 https://api.anthropic.com），
            // 但兼容网关常按 OpenAI 习惯填带 /v1 的基址 —— 两种都认：已以 /v1
            // 结尾就不再重复拼。拼出 /v1/v1/messages 必然 404，而它看起来像
            // 「上游挂了」，排查方向会被带偏，所以在这里归一
            let base = base_url.trim_end_matches('/');
            let url = if base.ends_with("/v1") {
                format!("{base}/messages{}", quirks.url_suffix)
            } else {
                format!("{base}/v1/messages{}", quirks.url_suffix)
            };
            let mut payload = anthropic_outbound::anthropic_request_from_chat(
                &outbound_chat,
                &wire_model,
            )
            .map_err(|message| GatewayError::with_status(400, message))?;
            if let Some(object) = payload.as_object_mut() {
                object.insert("stream".to_string(), Value::Bool(true));
            }
            // 工具 type 补齐（MiniMax 的 Claude 兼容端点拒绝无 type 的工具，
            // 见 `custom_providers::TOOL_TYPE_CUSTOM`）：翻译器输出的是无 type
            // 的老式形态，特判开启时逐个补上 `type: "custom"`
            if quirks.tool_type_custom {
                if let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) {
                    for tool in tools.iter_mut() {
                        if let Some(object) = tool.as_object_mut() {
                            object.insert(
                                "type".to_string(),
                                Value::String("custom".to_string()),
                            );
                        }
                    }
                }
            }
            // anthropic 的鉴权头不是 Bearer（与 `custom_providers::
            // fetch_upstream_models` 同一套），版本头按官方当前稳定值。
            // **凭证为空时不发 x-api-key**（理由见 chat 分支的①）
            let mut headers = vec![
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ];
            if has_key {
                headers.push(("x-api-key".to_string(), api_key.to_string()));
            }
            quirks.apply_emulation_headers(&mut headers, api_key, &outbound_chat, base_url);
            quirks.apply_to(&mut headers);
            apply_user_agent_override(&mut headers, user_agent);
            (url, headers, payload)
        }
    };
    let level = match kind {
        OutboundKind::Anthropic => crate::server::core::protocol::anthropic_thinking::effort(&payload),
        OutboundKind::Responses => payload.pointer("/reasoning/effort").and_then(Value::as_str).map(str::to_string),
    };
    telemetry.note_upstream_reasoning(level);
    if let Some(capture) = telemetry.capture() {
        capture.reset_request(&url, provider_id, &headers, &payload);
    }
    let payload = serde_json::to_string(&payload)
        .map_err(|error| GatewayError::with_status(500, format!("请求体序列化失败: {error}")))?;

    logging::verbose(
        "[CustomProvider]",
        &format!(
            "POST {url} protocol={} model={} stream={stream} account={account_id} 出口={}",
            kind.label(),
            if wire_model.is_empty() { "(默认)" } else { &wire_model },
            describe_proxy(proxy.as_ref()),
        ),
    );
    let transport = TransportRequest {
        url,
        headers,
        payload,
        proxy,
        // 自定义家没有能力位（用户自己配的出口就是唯一口径：配了就代理，没配就直连）
        system_proxy_when_unset: false,
    };
    let response = send_chat_request(&transport)
        .await
        .map_err(|error| error.to_gateway_error())?;

    // ── 响应：2xx 打包 outcome；非 2xx 分类（与 chat 分支同一套）──────
    // 两种协议的错误体形状不同（responses 是 {error:{message}}、anthropic
    // 是 {type:"error",error:{message}}），read_upstream_error 的提取链
    // （message → msg → error.message → 前 500 字符兜底）两条都覆盖
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // Retry-After 要在读 body **之前**取（read_upstream_error 会吃掉响应）
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let detail = read_upstream_error(response, telemetry.capture().as_deref()).await;
        return Err(classify_custom_error(
            status,
            &detail.message,
            retry_after.as_deref(),
        ));
    }

    // 上游字节流先过协议翻译（→ 标准 chat SSE），再交给既有消费层：
    // 流式进 ForwardStream（reasoning 合并 / usage 提取 / model 回写），
    // 非流式进聚合器 —— 槽位与连接计数的移交时序与 chat 分支逐字同构
    let translated =
        Box::pin(ProtocolTranslateStream::new(kind, &wire_model, response, telemetry));
    if stream {
        Ok(ForwardOutcome::Stream {
            status,
            stream: Box::new(ForwardStream::from_translated(
                translated,
                slot.take(),
                connections.handoff(),
                telemetry.clone(),
                FramePolicy::with_rewrite(rewrite),
            )),
        })
    } else {
        let aggregated = crate::server::core::upstream::aggregate::aggregate_frame_stream(
            translated,
            telemetry.clone(),
            FramePolicy::with_rewrite(rewrite),
        )
        .await?;
        Ok(ForwardOutcome::Completion { body: aggregated.body })
    }
}

/// 请求体改写：返回（要发出去的体, SSE/聚合回写参数）。
///
/// ── 两道改写的细节（与内置家的对应关系）────────────────────────
///   - **模型名**：`custom_providers::wire_model_for` 一次解析出「上游真名 +
///     思考等级」（两者同源，见那个函数的说明）。真名与请求名忽略大小写相同
///     时**不复制**（零改写是常态）；不同才升级成副本；
///   - **思考等级**：绑定非空且不是关闭思考时，清理冲突的客户端思考参数，再
///     强制写入 `reasoning_effort`；后续协议转换只读取覆盖后的等级。
///     `off` / `none` 仍不注入关闭思考字段，与内置家共用判据。
///
/// `rewrite` 是 SSE/聚合的 model 回写参数：请求带了 model 才给 ——
/// 客户端没点名模型时（上游用自家默认）没有「回写成什么」的答案。
fn rewrite_for_route(body: &Value, provider_id: &str, requested: &str, route_model: Option<&str>) -> (Value, Option<ModelRewrite>) {
    if let Some(client_model) = route_model {
        // 组内模型已由请求快照选定，不再经同名映射改写。
        return (body.clone(), Some(ModelRewrite { requested: client_model.to_string() }));
    }
    rewrite_body(body, provider_id, requested)
}

fn rewrite_body(body: &Value, provider_id: &str, requested: &str) -> (Value, Option<ModelRewrite>) {
    let mut outbound = body.clone();
    if requested.is_empty() {
        return (outbound, None);
    }
    let (wire_model, reasoning) = custom_providers::wire_model_for(provider_id, requested);
    if !wire_model.eq_ignore_ascii_case(requested) {
        logging::verbose(
            "[CustomProvider]",
            &format!("provider={provider_id} 按映射改写模型名 {requested} → {wire_model}"),
        );
        if let Some(object) = outbound.as_object_mut() {
            object.insert("model".to_string(), Value::String(wire_model));
        }
    }
    if let Some(level) = reasoning
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        if model_rules::reasoning_is_off(level) {
            logging::verbose(
                "[CustomProvider]",
                &format!(
                    "provider={provider_id} 映射 {requested} 绑定的思考等级为「{level}」\
                     （关闭思考），本网关不向任何上游发「关闭思考」字段，跳过注入"
                ),
            );
        } else {
            model_rules::clear_reasoning_controls(&mut outbound);
            if let Some(object) = outbound.as_object_mut() {
                object.insert(
                    "reasoning_effort".to_string(),
                    Value::String(level.to_string()),
                );
                logging::verbose(
                    "[CustomProvider]",
                    &format!(
                        "provider={provider_id} 映射 {requested} 覆盖思考等级 reasoning_effort={level}"
                    ),
                );
            }
        }
    }
    (
        outbound,
        Some(ModelRewrite {
            requested: requested.to_string(),
        }),
    )
}

/// 非 2xx 的响应 → 分类后的 `GatewayError`（分类语义见模块头）。
///
/// 三档处理，与 `UpstreamErrorClass` 的对应关系写在模块头：
///   - 401/403 → 凭证被拒；
///   - 429 → 限额；`Retry-After` 解析成功时把恢复时间以 UTC+8 文案并进
///     message（`rotate::mark_account_limited` 会从文案里再解析一次，
///     见模块头的说明）；
///   - 其余 → 原样透传状态码与上游原文摘要。
fn classify_custom_error(status: u16, raw: &str, retry_after: Option<&str>) -> GatewayError {
    let raw = raw.trim();
    let raw = if raw.is_empty() { "上游错误" } else { raw };
    match status {
        401 | 403 => GatewayError::with_status(
            i32::from(status),
            format!("上游凭证被拒绝（HTTP {status}）: {raw}"),
        ),
        429 => {
            let mut message = format!("上游返回 {status}: {raw}");
            // 恢复时间的解析链：Retry-After 头（秒数 / HTTP 日期）→
            // 错误文案里的日期形态（parse_quota_reset_at 的既有口径）。
            // 文案以「（<UTC+8 时间> 恢复）」附录形态并回 message ——
            // 与 rotate 的 reset_hint 同形状，用户与解析器都能读。
            if let Some(text) = retry_after
                .and_then(retry_after_reset_text)
                .or_else(|| text_of_reset_at(&message))
            {
                message.push_str(&format!("（{text} 恢复）"));
            }
            GatewayError::with_status(429, message)
        }
        _ => GatewayError::with_status(
            i32::from(status),
            format!("上游返回 {status}: {raw}"),
        ),
    }
}

/// `Retry-After` 头 → 恢复时间文案（UTC+8，`parse_quota_reset_at` 认的形态）。
///
/// 两种合法形态都认：delay-seconds（整数秒）与 HTTP-date（RFC 2822）。
/// 无法识别（负数、畸形日期）时给 None —— 调用方继续走文案解析的兜底。
fn retry_after_reset_text(retry_after: &str) -> Option<String> {
    let text = retry_after.trim();
    if let Ok(seconds) = text.parse::<i64>() {
        if seconds > 0 {
            return Some(reset_text_at(logging::now_ms() + seconds * 1000));
        }
        return None;
    }
    chrono::DateTime::parse_from_rfc2822(text)
        .ok()
        .map(|date| reset_text_at(date.timestamp_millis()))
        .filter(|text| !text.is_empty())
}

/// 错误文案里已有的恢复时间（`errors::parse_quota_reset_at` 的既有解析）。
/// 解析不出（0）时给 None —— 文案里不加空括号。
fn text_of_reset_at(message: &str) -> Option<String> {
    let reset_at = crate::server::errors::parse_quota_reset_at(message);
    (reset_at > 0).then(|| reset_text_at(reset_at))
}

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS UTC+8` 文案。
///
/// 与 `upstream::format_reset_text` 同口径（上游给的恢复时间按 UTC+8 标定，
/// 用本机时区会让用户对不上原文），只是格式改成连字符 + 显式 UTC+8 ——
/// `parse_quota_reset_at` 认的正是这一形态（斜杠形态它不认）。
/// 时间戳非法 / 偏移构造失败时给空串（这只是日志与文案，不能把请求带崩）。
fn reset_text_at(reset_at: i64) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp_millis(reset_at) else {
        return String::new();
    };
    let Some(offset) = chrono::FixedOffset::east_opt(8 * 3600) else {
        return String::new();
    };
    utc.with_timezone(&offset)
        .format("%Y-%m-%d %H:%M:%S UTC+8")
        .to_string()
}

/// 出口的可读描述（与 `upstream::describe_proxy` 同口径；那一个是
/// `pub(super)`，为了一行 verbose 日志不值得放宽 —— 留一份两行实现）。
fn describe_proxy(proxy: Option<&ResolvedProxy>) -> String {
    match proxy {
        Some(proxy) if !proxy.label.is_empty() => proxy.label.clone(),
        Some(proxy) => proxy.host.clone(),
        None => "直连".to_string(),
    }
}

/// 这条**账号公开形态**是不是「轮询出网」档（`proxy.config.source == "rotate"`）。
///
/// 编排层（`attempt_custom`）用它决定「429 时要不要原地换出口」：轮询档的 429
/// 是**按 IP** 判的（这个出口的免费额度用完，见 `core::proxy_rotate` 的模块头），
/// 换一个出口重发往往就过了；别的档没有这层语义，429 就是账号/模型的限额。
///
/// 判据读账号公开形态里的 `proxy.config`（路由层拿到的就是它，形状见
/// `core::proxies` 的模块头）而不是转发用的 `ResolvedProxy`：解析结果里
/// `source` 是候选条目自己的来源（pool / clash / custom），轮询取用后已经看不出
/// 「这是轮询档」了。
pub fn is_rotate_account(account: Option<&Value>) -> bool {
    account
        .and_then(|item| item.get("proxy"))
        .and_then(|proxy| proxy.get("config"))
        .map(crate::server::core::proxies::is_rotate_config)
        .unwrap_or(false)
}

/// 这一轮的**限额冷却键**（该家实际收到的上游真名；编排层 `attempt_custom` 用）。
///
/// 与 [`rewrite_body`] 内部同一解析（`wire_model_for` 是纯函数：读配置快照、
/// 无 IO、无账号参数，两次调用的结果必然相同）。单独开一个入口而不是让编排层
/// 直接调 `wire_model_for`：那个函数是「发送名怎么算」的实现细节，编排层不该
/// 认识它 —— 它只该问「这家的冷却键是什么」。
///
/// 请求体没带 model 时给空串（与 `SendBody::wire_model` 的空键口径一致）。
pub fn cooldown_model(provider_id: &str, requested: &str) -> String {
    let requested = requested.trim();
    if requested.is_empty() {
        return String::new();
    }
    custom_providers::wire_model_for(provider_id, requested).0
}
