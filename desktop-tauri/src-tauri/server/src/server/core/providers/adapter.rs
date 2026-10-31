//! ProviderAdapter 契约（Agent2API 改造 W2b-T3，架构文档 §4.2）。
//!
//! ── 这一层解决什么问题 ──────────────────────────────────────
//! 改造前，转发链路的每一步都默认「上游就是 WorkBuddy」：URL 是写死的
//! `/v2/chat/completions`、鉴权头按 workbuddy 的字段拼、429/6004 与 11128 的
//! 判定散在 `upstream/request.rs` 与 `rotate.rs`。多提供商之后，`upstream/`
//! 只做**协议无关的编排**（去重、逐家轮询、SSE 透传、聚合），
//! 所有「这一家长什么样」的知识收进各自的适配器 —— 本模块定义的就是那份契约。
//!
//! ── 四个动作（语义不得改，架构文档 §4.2 的硬要求 + 内容拦截一档）──
//!   1. [`UpstreamErrorClass::QuotaLimited`] → 标记账号对该模型冷却 + 换下一个账号；
//!   2. [`UpstreamErrorClass::TokenExpired`] → 刷新凭证后**同一账号**重试一次；
//!   3. [`UpstreamErrorClass::ContentBlocked`] → **不罚账号**：换中性提示词后同一
//!      账号立即重试一次，并触发降级状态机（`core::degrade`；照搬参考项目的
//!      `ErrContentBlocked` 语义 —— 那是审核误报，不是账号问题）；
//!   4. [`UpstreamErrorClass::Fatal`]        → 原样透传给客户端。
//!
//! 这四档是转发编排唯一认识的错误处理方式，适配器只能在这四档里归类 ——
//! 多出来的分类在编排层没有对应分支，只会静默落进 Fatal，
//! 等于把「该轮换账号」的错误当成终态发给用户。
//!
//! ── 注册表为什么是「静态实例 + match」而不是 HashMap ─────────
//! 适配器是**无状态单例**（凭证在账号存储、目录在 `core::models` 的进程级句柄，
//! 适配器自己只持有常量与纯函数）。因此 `&'static dyn ProviderAdapter` 就够了：
//! 没有锁、没有初始化顺序、`adapter_for` 是 O(1) 且绝不失败。
//! 用 `OnceLock<HashMap<…>>` 反而要处理「注册表还没装好时被调用」的路径，
//! 而那条路径在 panic=abort 的 release 里只能 panic —— 得不偿失。
//!
//! ── 契约之外的十一处扩展（都是带默认实现的加法，不改 §4.2 的方法）──
//!   1. `retry_advice`：识别「可退避重试的错误」并给出间隔与日志文案。
//!      §4.3 要求「11128 退避逻辑保持在转发层」——**循环**留在编排层，
//!      但「哪个码要退避、退多久」是 provider 知识（11128 是 workbuddy 的
//!      敏感词拦截码），放在这里才能让 upstream/ 一个 provider 分支都不留。
//!   2. `refresh_access_token`：401 后的**强制**刷新。`ensure_access_token`
//!      的语义是「取可用 token（含临期主动刷新）」，而 401 意味着 token
//!      被服务端拒绝 —— 它可能远未到临期窗口，只调 ensure 不会真的刷新。
//!      默认实现直接转调 `ensure_access_token`，provider 需要区分时再覆盖。
//!   3. `allows_anonymous_default_session`：本 provider 是否允许「一个账号
//!      都没有时用环境变量凭证/默认登录态转发」。workbuddy 有这条旁路
//!      （`WORKBUDDY_TOKEN`），小浣熊也有（`RACCOON_TOKEN`，实现在插件里）。
//!   4. `sse_model_rewrite`（W3-T4 追加）：SSE 透传**要不要把下发帧的 `model`
//!      改写成客户端请求的那个名字**。小浣熊上游网关会回自己的内部模型名
//!      （这类上游会把 chunk 里的 `model` 换成内部名，客户端认自己请求的那个），
//!      而 workbuddy 上游原样回显。帧改写发生在 `upstream::ForwardStream`
//!      内部（那里就是 SSE 逐帧下发的唯一出口），所以这里只回答「要不要写」，
//!      通用层不出现任何 provider 分支 —— 与 `retry_advice` 同一分工。
//!   5. `supports_refresh` + `credentials_expiring`（凭证自动维护）：
//!      「这家能不能主动续期」「这个账号的凭证此刻算不算临期」是**各家的知识**
//!      （过期时间在哪个字段、用什么窗口，各家各不相同），因此由适配器回答，
//!      维护任务（`core::credential_maintenance`）只负责遍历与调用。
//!      放在这里而不是壳侧：壳侧只能按账号公开形态里的字段名判断，而那个字段名
//!      各家不同（小浣熊的过期时间在 `tokenExpiresAt`，公开形态里没有 `expiresAt`）
//!      —— 写死一个名字就会让别家的账号永远被判成「无需刷新」。
//!      契约：**判不出来就返回 false**（没有过期信息 / 账号不存在），
//!      宁可漏刷也不要凭猜测去打上游。
//!   6. `is_stateful` + `forward_conversation`（W4a 加钩子）：本 provider 的
//!      上游是不是**多步会话协议**（round → turn → 工具循环 → completed）。
//!      默认 false；由插件侧那些走会话式转发的家覆写。`forward_conversation`
//!      是会话式转发的入口（默认 503），编排层的 `provider_loop` 按
//!      `is_stateful` 分流到它。
//!   7. `supports_web_login` + `build_login_url` + `exchange_login_code`
//!      （「网页登录」）：拉起官方登录页 → 用户完成登录 → 回调里带回一次性 code
//!      → 网关用 code 换凭证并**落进账号库**。
//!      workbuddy **不走这三个钩子**：它的 state/authUrl 来自上游 `auth/state`，
//!      由 `core::login` 的后台任务轮询 `auth/token` 取回凭证，是另一套协议
//!      （见 `api::session::login_start`），既有的 `supports_web_login` 对它
//!      保持默认 false，以免路由层以为它能用这套回调链路。
//!      为什么整条链都放进 trait 而不是在 `api::session` 里写 provider 分支：
//!      「授权地址怎么拼、回调长什么样、用哪个接口换凭证、响应里哪几个字段是
//!      凭证、落账号走哪条添加路径」全是 provider 知识，与 `sse_model_rewrite`
//!      同一性质；路由层只该回答「这家支不支持」，不该认识小浣熊的字段名。
//!   8. `supports_usage` + `query_usage`（余额 / 积分查询）：「这家有没有余额概念」
//!      「这个账号的余额怎么查」是 provider 知识（各家的接口地址、鉴权头、
//!      凭证来源全不相同：workbuddy 走腾讯计费接口、小浣熊要 Bearer JWT、
//!      CatPaw 要网页会话 cookie `token2`、AutoClaw 走带签名的 userapi 域），
//!      所以由适配器回答，`api::accounts` 只负责并发调度与单账号失败的收敛。
//!      改造前这条能力**只有 workbuddy 有**：`resolve_batch_targets` 显式按
//!      provider 过滤目标集合，前端也只在 workbuddy 卡片上渲染「积分」按钮。
//!      统一形状的契约写在 [`ProviderAdapter::query_usage`] 的文档里。
//!   9. `supports_model_refresh` + `refresh_models(force)`（「刷新模型清单」）：
//!      「这家有没有远程目录可拉」与「这次要不要绕过缓存」是两件事，拆成两个
//!      入口（前者恒定能力，后者单次语义）。**现在各家都覆写成 true** ——
//!      每家都有远程目录（下列是早期五家的接口，后来接入的家见各自
//!      `models.rs` 的模块头）：WorkBuddy `GET /v3/config`、小浣熊
//!      `GET {llmBase}/model_catalog`、Qoder `GET {gateway}algo/api/v2/model/list`、
//!      CatPaw `POST /api/agent/maas/model-types`、AutoClaw
//!      `GET .../proxy/autoclaw-model-config`。
//!      （CatPaw 与 AutoClaw 曾保持默认 false，理由是「上游没有目录接口」——
//!      那两个前提后来都被证伪，接口一直都在，只是形态看错了，见各自
//!      `catalog.rs` 的模块头。）`refresh_models` 的返回类型是
//!      [`ModelRefreshOutcome`]：自动路径不看它（照旧只打日志），
//!      手动路径靠它逐家如实汇报，见 [`refresh_implemented_forced`]。
//!      默认实现仍是 false：那是给「将来新接入、目录还没拉通」的 provider 留的
//!      过渡态（与 `supports_chat` 同一性质）。
//!  10. `reasoning_patch`（思考等级绑定的翻译，R7 的后半段）：「通用 8 档怎么
//!      翻译成本家上游认识的字段与取值」是**各家的知识**（CatPaw 只认
//!      low/high/max，多一个值当场 400；Qoder 按模型自己声明的 efforts 归一；
//!      另外几家原样透传 body、上游不认识这类字段），所以由适配器回答，
//!      编排层（`upstream::payload`）只负责在正确的时机问一次、按结果改写 body。
//!      **默认实现是「不接」**（返回 `NotSupported`）—— 理由见那个方法的文档：
//!      接一家要有一家的证据，没证据就注入等于把未知参数推给上游。
//!  11. `sse_strip_newline_chunks`（下发帧丢掉「整片只有换行」的 content 分片）：
//!      WorkBuddy 与 AutoClaw 的上游会在相邻正文分片之间插一帧纯换行的 content
//!      （那是它们的保活节拍，两天样本里 34.5% 的请求带、脏请求中位占正文分片
//!      13.4%，见 `upstream::sse` 模块头）。丢不丢是 provider 知识 —— 同样的帧在别家
//!      可能真是模型写的换行，所以由适配器回答，执行落在 `upstream::sse` 这个
//!      SSE 逐行解析的唯一出口（与 `sse_model_rewrite` 同一分工、同一位置）。
//!      **默认 false**：没声明的 provider 下发帧逐字节不变。
//!      声明了也不等于会丢 —— 还要 `stripNewlineKeepalive` 这个**部署开关**开着
//!      （默认关，见 `config::KEY_STRIP_NEWLINE_KEEPALIVE`）：能力位回答"这家的
//!      上游会不会发"，开关回答"这台机器要不要为它改字节"，两者相与，且开关只能
//!      收窄不能扩张（见 `upstream::sse::FramePolicy::assembled`）。
//!
//! ── 现在谁实现这些方法 ──────────────────────────────────────
//! 一家都没有：29 个 `ProviderKind` 的实现**全部**搬到了提供商插件
//! （`agent2api-workers` 形态的独立子进程，不随本仓库发布）。[`adapter_for`]
//! 因此对任何 kind 都返回 `missing_plugin::placeholder` —— 如实报「需安装
//! 插件」，而不是在网关里留一份会随时间漂移的旁路实现。
//!
//! 本 trait 仍然存在的理由：它是**插件宿主的转发接口形状**（`plugin_host` 的
//! `PluginAdapter` 逐方法实现它），编排层因此不需要知道「这次是插件还是内置」。
//! 空清单 + 503 的语义保留在 `catalog` / `accounts` 的校验里：`/v1/models`
//! 不会广告清单为空的那家。

use axum::http::HeaderMap;
use serde_json::Value;

pub use super::retry::RetryDecision;

use std::sync::Arc;

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::ProviderKind;

/// 转发前对「账号 + 请求体」的完整构造计划（架构文档 §4.2 的 ChatRequestPlan）。
///
/// ── 与 `upstream::request::TransportRequest` 的区别（重要）───
/// 那个是**传输层**形态：已序列化好的 `payload: String` + 出网代理，
/// 直接交给 `send_chat_request` 发出去。本形态是 **provider 视角**的：
/// body 还是 `Value`（不同家的改写规则不同，比如 workbuddy 要注入首条
/// system 消息），且**不含出网代理** —— 代理是账号级的、与 provider 无关，
/// 由编排层从账号会话里解析后自己带上。
pub struct ChatRequestPlan {
    /// 上游完整 URL（含路径）
    pub url: String,
    /// 请求头（不含出网代理相关；Authorization 在此）
    pub headers: Vec<(String, String)>,
    /// 已按 provider 规则改写过的请求体
    pub body: Value,
    /// 上游**响应**说的是哪套协议（默认 [`UpstreamResponse::Chat`]）
    pub response: UpstreamResponse,
    /// 插件出站归一后的档位；外层 None 为未提供，Some(None) 为明确无档位。
    pub reasoning: Option<Option<String>>,
    pub policy: super::policy::ChatPolicy,
}

impl ChatRequestPlan {
    /// 标准形态：上游说 OpenAI Chat（请求体与响应帧都是 chat 形态）。
    ///
    /// 绝大多数内置上游（以及自定义家）都是这一种；三处例外各有一个专用
    /// 构造器：ZCode 的活动套餐通道说 Anthropic（[`UpstreamResponse::Anthropic`]）、
    /// 插件家说 AI SDK NDJSON（[`UpstreamResponse::AiSdkNdjson`]）、
    /// Antigravity 说 Gemini v1internal 信封
    /// （[`UpstreamResponse::AntigravityGemini`]）。写成构造器而不是
    /// 让各家手写字段，是为了「响应协议」这一个新字段不给各处调用点各留一次
    /// 写错的机会。
    pub fn chat(url: String, headers: Vec<(String, String)>, body: Value) -> Self {
        Self {
            url,
            headers,
            body,
            response: UpstreamResponse::Chat,
            reasoning: None,
            policy: super::policy::ChatPolicy::default(),
        }
    }

    /// AI SDK v5 NDJSON 形态：请求体是各家自己的信封（由插件构造），响应是
    /// NDJSON（见 [`UpstreamResponse::AiSdkNdjson`]）。
    ///
    /// 与 [`Self::chat`] 同一动机：调用点只回答「响应是哪套协议」，
    /// 不自己拼 `ChatRequestPlan` 的字段。
    pub fn ai_sdk_ndjson(
        url: String,
        headers: Vec<(String, String)>,
        body: Value,
    ) -> Self {
        Self {
            url,
            headers,
            body,
            response: UpstreamResponse::AiSdkNdjson,
            reasoning: None,
            policy: super::policy::ChatPolicy::default(),
        }
    }

    /// Antigravity 形态：请求体是 Cloud Code Assist 的 v1internal 信封
    /// （`{project, model, userAgent, requestId, request:{…}}`），响应是
    /// **Gemini SSE**（`data: {"response":{…}}`，见
    /// [`UpstreamResponse::AntigravityGemini`]）。
    ///
    /// 与另外两个构造器同一动机（调用点只回答「响应是哪套协议」），
    /// 单独一个名字是因为它要连**请求信封**一起表达 —— 那种信封只有
    /// `antigravity_outbound` 会造。
    pub fn antigravity_gemini(
        url: String,
        headers: Vec<(String, String)>,
        body: Value,
    ) -> Self {
        Self {
            url,
            headers,
            body,
            response: UpstreamResponse::AntigravityGemini,
            reasoning: None,
            policy: super::policy::ChatPolicy::default(),
        }
    }
}

/// 上游响应的协议（**请求体与响应必须同源**：这套标记由适配器在构造请求时
/// 一并给出，编排层只按它选翻译层，不做二次推断）。
///
/// ── 为什么需要它 ────────────────────────────────────────────
/// 本项目的历史前提是「所有上游都说 Chat」（见 `protocol` 的模块头），于是
/// 无状态转发路径的下行帧一律按 chat SSE 处理。ZCode 的活动套餐通道打破了这个
/// 前提：它的推理端点是 Anthropic Messages（`stream:true` 时吐 Anthropic 事件
/// 流）。与其为一家新写一条「适配器自己转发」的路（那会丢掉账号轮换、限额冷却、
/// 退避重试、usage 与取消处理，见 `upstream::provider_loop` 的有状态路径说明），
/// 不如把「响应要说另一种协议」做成计划里的一个字段 —— 编排层只多一次分支，
/// 其余全都共用。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UpstreamResponse {
    /// OpenAI Chat SSE（默认）
    #[default]
    Chat,
    /// Anthropic Messages SSE：下发前折回标准 chat SSE（见 `upstream::translate`）
    Anthropic,
    /// **AI SDK v5 风格 NDJSON**（`application/x-ndjson`）：一行一个 JSON 事件、
    /// **HTTP 恒 200**、错误以流内 `{"type":"error"}` 表达，因此连接的成败与
    /// 「这一轮生成成没成」是两件事。下发前折回标准 chat SSE
    /// （见 `upstream::translate` 与 `protocol::ai_sdk_ndjson`）——
    /// 走翻译层的理由与 Anthropic 那条逐字相同：账号轮换、限额冷却、退避重试、
    /// usage 记账与取消处理全部留在编排层，只有字节形态在翻译器里变。
    ///
    /// 这是**公开格式**而不是某一家的私有协议：网关只认事件词表，
    /// 「谁在用这套格式、请求体长什么样」由插件声明。
    AiSdkNdjson,
    /// Antigravity 的 **Gemini v1internal 信封 + SSE**：上游是 Google Cloud Code
    /// Assist 的 `:streamGenerateContent?alt=sse`，每帧是 `data: {json}`，且真正的
    /// Gemini 响应在 `response` 键下（缺省回退顶层，见规格 §4.2）。
    /// 字段路径（`candidates[0].content.parts[*]`、`usageMetadata`、
    /// `finishReason`、`thought` 位、`thoughtSignature`）全是 Gemini 方言，
    /// 因此与 Anthropic / NDJSON 并列另开一台状态机（`upstream::translate` 的
    /// `AntigravityToChatStream` ← `protocol::antigravity_stream`）。
    /// 走翻译层的理由同上：账号轮换、限额冷却、退避重试、usage 记账与取消处理
    /// 全部留在编排层，只有字节形态在翻译器里变。
    AntigravityGemini,
    /// Images 端点的 JSON 或 Gemini inlineData 输出，不走 Chat 翻译器。
    Images(super::images::ImageResponseKind),
    /// 智谱清言的 **SSE 混合帧**：帧是 SSE（`data: {json}`）没错，但一轮回答里
    /// 混着两种语义 —— `part.status == "init"` 的帧给的是**本拍新增的一小段**，
    /// 带 finish 的帧给的是**整段全文快照**。而 OpenAI 要的是统一口径的 delta：
    /// 全文快照直接透传会让下游把整段回答拼两遍（实测故障），
    /// 把增量帧当快照差分则会丢内容。
    /// 因此另开一台**按帧语义差分**的状态机（`upstream::translate` 的
    /// `ChatglmToChatStream` ← `protocol::chatglm_stream`）。
    ///
    /// 它与另外三台的一处关键差别：本家的**业务失败走 HTTP 200 +
    /// `application/json`**（`-1001 剩余积分不足`），不进这台翻译机 ——
    /// 那一档由插件侧的 `is_error_response` 在编排层拦下（网关「先问插件，
    /// 插件不说话才按自身判据」，见 `plugin_host::rpc`），
    /// 走既有的错误分类与账号轮换（与 ZCode 同一处置）。
    ChatglmSnapshot,
}

/// 上游错误分类（架构文档 §4.2；三个动作的语义见模块头）。
///
/// ── 文案口径：`message` 是**客户端可见的最终文案**（含前缀）────
/// 改造前客户端的错误体是 `上游返回 {status}: {上游原文}{提示}`，其中提示只在
/// workbuddy 的 11128（敏感词拦截）时追加。三者合成为一条文案的规则是
/// provider 知识（提示的措辞是 workbuddy 专属的），因此在适配器里拼好、
/// 由编排层原样使用 —— 既有用户的错误文案因此**逐字不变**。
#[derive(Clone, Debug)]
pub enum UpstreamErrorClass {
    /// 限额/风控 → 标记账号冷却并换下一账号（workbuddy 的 429 / code 6004）
    QuotaLimited {
        /// 上游给出的恢复时间戳（毫秒）；解析不出时 None（调用方给兜底冷却）
        reset_at: Option<i64>,
        /// 客户端可见的完整文案（`上游返回 {status}: {上游原文}`）
        message: String,
        /// 上游业务码（进 GatewayError 的 `upstream_code` 与限额事件）
        upstream_code: Option<i64>,
        /// HTTP 状态码
        status: u16,
    },
    /// token 失效 → 刷新凭证后用同一账号重试一次
    TokenExpired {
        /// 客户端可见的完整文案（`上游返回 401: {上游原文}`）
        message: String,
    },
    /// 内容策略拦截（HTTP 400 + 审核文案）→ **不罚账号**：换中性提示词后
    /// 同一账号立即重试一次，并触发降级状态机（见 `core::degrade`）。
    ///
    /// 它是「误报」信号而不是账号问题：账号余额健康、未限流、session 未死，
    /// 换账号再试只会白扔另一个账号的额度（同一份 body 换谁发都会被拦）。
    /// 判定规则是各家共用的（见 `providers::content_block`），编排层的动作见
    /// `upstream::provider_loop` 的「动作 0」。
    ContentBlocked {
        status: u16,
        /// 客户端可见的完整文案（含 11-128 之类的 provider 提示）
        message: String,
        /// 上游业务码（原样进 GatewayError 的 `upstream_code`）
        upstream_code: Option<i64>,
    },
    /// 透传给客户端的错误
    Fatal {
        status: u16,
        /// 客户端可见的完整文案（含 11-128 之类的 provider 提示）
        message: String,
        /// 上游业务码（原样进 GatewayError 的 `upstream_code`）
        upstream_code: Option<i64>,
    },
}

/// 一次「可退避重试」的建议（见模块头扩展 1）。
///
/// 适配器把「要不要退避重试」连同**原因**一起给出，编排层负责睡这一觉、
/// 再发一次，并把这条建议记进请求日志的尝试明细 —— 于是 11128/「敏感词」
/// 这类 provider 专属知识不会漏进 `upstream/`。
///
/// ── 为什么是 `reason` 而不是一整句 `log_message` ─────────────
/// 改造前这里给的是**完整日志文案**（含「5 秒后重试（第 n/N 次）」那半句），
/// 编排层原样打一行运行日志。现在同一条建议有两个消费方：请求日志的重试链
/// （要的是「为什么」）与终端控制台（要的是完整可读的一行）。若继续由适配器
/// 给整句，那「第 n/N 次」这种只有编排层才知道的读数就得由适配器猜，或者
/// 干脆两处各写一套文案 —— 前者的次数会错，后者迟早分叉。
/// 拆成「适配器给原因 + 编排层补进度」之后，两条通道说的是同一件事，
/// 且运行日志那一行的措辞与改造前逐字相同（拼法见 `provider_loop::retry_log_line`）。
#[derive(Clone, Debug)]
pub struct RetryAdvice {
    /// 退避时长（毫秒）
    pub delay_ms: u64,
    /// 重试原因（provider 专属措辞，如「上游敏感词拦截（11128）」）。
    /// 编排层用它拼日志行与请求日志的重试链。
    pub reason: String,
}

/// 一次思考等级翻译的结果（`ProviderAdapter::reasoning_patch` 的返回）。
///
/// ── 为什么是「要么一个字段、要么一个原因」而不是 `Option<Value>` ──
/// 注入点（`upstream::payload`）要做两件事：改写 body、**把结果记进详细日志**。
/// 「不注入」有四五种成因（这家不支持 / 客户端已指定 / 等级不在能力范围内 /
/// 该等级语义是关闭思考……），对用户来说原因完全不同 —— 用 `Option` 会让
/// 日志只能写一句「未注入」，那正是「我绑了为什么不生效」最难查的情形。
/// 把原因做成返回值的一部分，适配器就必须为自己的判断给出一句话。
#[derive(Clone, Debug)]
pub enum ReasoningPatch {
    /// 注入：把 body 顶层的 `field` 设成 `value`（覆盖同名的旧值）。
    ///
    /// `Value` 而不是 `String`：当前覆写这个钩子的几家要的都是字符串档位，
    /// 但上游表达「开思考 + 档位」的形态未必都是字符串（布尔开关、嵌套对象
    /// 都常见），留成 `Value` 让将来那家不必先改这个枚举的**形状**。
    Set {
        /// body 顶层的字段名（各家自己那个 resolver 认识的键，如
        /// `reasoning_effort`）—— 注入点只负责写，不认识字段的语义。
        field: &'static str,
        value: Value,
    },
    /// 不注入（body 一个字节都不动，保持本功能之前的行为）。`reason` 进详细日志。
    Skip {
        /// 给用户看的一句话原因（「这家不支持思考等级」/「客户端请求体里已指定」
        /// /「该等级不在本家接受的档位内」……）。
        reason: &'static str,
    },
}

/// 单个提供商的转发适配能力（架构文档 §4.2）。
///
/// 实现者必须是**无状态或内部自带共享句柄**的：`adapter_for` 返回的
/// `&'static` 引用会被并发调用，而 trait 没有 `&mut self` 方法。
///
/// `Send + Sync` 是必需的：适配器会被转发任务在 await 点之间持有，
/// 而转发跑在 tokio 的多线程运行时上。
///
/// ── 为什么异步方法写成 `-> Pin<Box<dyn Future>>` ─────────────
/// trait 里不能写 `async fn`（Rust 1.77 的 MSRV 不支持原生 async fn in trait，
/// 而 `async_trait` crate 会给每个方法加一层 Box 分配）。手写 Box 的代价
/// 只在这几个签名上，收益是 trait 保持 object-safe 且不引入新依赖。
pub trait ProviderAdapter: Send + Sync {
    /// 本适配器对应的 provider
    fn kind(&self) -> ProviderKind;

    /// 本适配器的 provider id（`&'static str`）。
    ///
    /// ── 为什么内置家不能靠 `kind_id(kind())` 推 ─────────────────
    /// 插件家的 `kind()` 只是占位（第三方 provider 没有枚举值），真正的
    /// 身份是 id 字符串。内置家的默认实现走 [`kind_id`]；`RpcAdapter`
    /// 覆写成自己的 id。新代码一律用这个方法，别写 `kind_id(adapter.kind())`。
    fn provider_id(&self) -> &'static str {
        super::kind_id(self.kind())
    }

    /// 展示名（日志、错误文案、界面）。
    ///
    /// 默认从内置注册表取；插件家覆写成握手带回来的 `label`（leak 成
    /// `&'static`，理由与拒绝标记相同 —— 见 `RpcAdapter` 的字段说明）。
    fn label(&self) -> &'static str {
        super::meta(self.kind()).label
    }

    /// 该 provider 的模型清单（供聚合目录 / 路由判定）。
    ///
    /// 返回的是**上游原始形态**的记录数组（字段名与 `/v3/config` 或
    /// 小浣熊 `/model_catalog` 一致），聚合层用 `models::list_item` 做字段映射。
    ///
    /// ── 这个方法的返回值**同时**喂两条路（改它之前先读这段）──────
    ///   - **能力判定**（`catalog::providers_for_model` → 路由候选链）：某家
    ///     「有没有这个名字」，决定候选链把请求派给谁；
    ///   - **广告视图的原料**（`/v1/models` 与管理页的条目，再经
    ///     [`Self::advertise_models`] 收窄）。
    ///
    /// 两者共用一份清单是刻意的（「校验说没有、转发却发了」的自相矛盾由此
    /// 杜绝）。因此**不要**在这里做任何「按用户偏好收窄」的过滤：要按偏好
    /// 收窄只影响广告时，覆写 [`Self::advertise_models`]。
    ///
    /// ── 收窄掉的模型**不可点名调用**（2026-09 起）──────────────
    /// 入口校验（`api::pipeline::resolve_model`）以**广告视图**为准：客户端
    /// 手里的清单来自 `/v1/models`，点一个那里没有的名字直接 400
    /// `model_not_found`，不再转给上游。因此被 [`Self::advertise_models`]
    /// 收窄掉的模型（如 Cline 按额度池收窄的那些）**不再可点名调用** ——
    /// 「列表里能看到什么，就只允许点什么」。
    fn list_models(&self) -> Vec<Value>;

    /// 对外广告前的清单收窄（默认为恒等）。
    ///
    /// `manifest` 是 [`Self::list_models`] 的返回值，本方法只决定「其中的哪些
    /// 条目要出现在 `/v1/models` 与管理页里」。
    ///
    /// ── 收窄是**门禁**，不只是展示偏好（2026-09 起）────────────
    /// 入口校验以广告视图为准（`api::pipeline::resolve_model` →
    /// `catalog::advertised_manifest_contains`）：收窄掉的模型客户端看不到、
    /// 也点不动（400 `model_not_found`）。所以这里的口径就是「这个账号能用
    /// 哪些模型」，写错会把用户真能用的模型挡在门外。
    ///
    /// ── `store` 为什么是参数（而不是让实现自己造一个）──────────
    /// 需要按账号状态收窄的实现必须读**调用方手上那个 store 句柄**：
    /// `AccountStore` 是 `Clone` 但每个 `with_config_dir()`
    /// 各自持一把锁，自己再造一个就成了「绕过主句柄的第二把锁」——
    /// 读到的可能不是最新状态，而且它保护的读-改-写周期与主句柄互不可见。
    /// 由调用方传入既省一次文件读，也让「读的是同一份账号状态」这件事成立。
    ///
    /// 默认恒等（现在各家都没有「同一个模型的两个通道」这种结构），
    /// 因此默认实现忽略 `store`。
    fn advertise_models(&self, _store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
        manifest
    }

    /// 用账号凭证构造上游请求（URL / 头 / 体）。
    ///
    /// ── `account` 传什么（实现与调用的约定）────────────────────
    /// 传的是**该账号的会话形态**（`store.get_session_by_id` /
    /// `auth.get_current_session` 的返回值：含 `auth.accessToken`、
    /// `endpoint`、`edition`、`account.uid`、`proxy`），而不是 accounts.json 的
    /// 公开形态 —— 公开形态按设计**不含** accessToken（只有 tokenTail），
    /// 拿它拼不出 Authorization。契约里写「account 为 accounts.json 的账号对象」
    /// 是就「这是谁的凭证」而言；具体形态由编排层保证（见 §4.2 末段的
    /// 「允许为贴合现有代码微调参数」）。
    ///
    /// `body` 是客户端请求体（未经改写）；`client_headers` 是客户端入站头
    /// （本期实现不读它，保留参数是为了将来「透传客户端 UA / 自定义头」
    /// 的 provider 不必改契约）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError>;

    /// [`Self::build_chat_request`] 的异步形态（插件架构新增）。
    ///
    /// ── 为什么需要两个方法 ──────────────────────────────────────
    /// 编译期内置的 21 家实现是纯计算（拼 URL / 头 / body），同步就够；
    /// 而插件实现在另一个进程里，只能异步问。把它们统一成异步会逼着 21 家
    /// 全部改签名，并且给每个调用点套一层 `Box::pin` —— 那是纯噪音。
    ///
    /// 所以：**默认实现直接转调同步版**，只有跨进程的实现（`RpcAdapter`）
    /// 覆写它。编排层一律调这个异步版（见 `upstream::provider_loop`）——
    /// 于是「内置家走同步、插件家走 RPC」这件事对编排层完全透明。
    ///
    /// ── 覆写者必须让两者语义一致 ────────────────────────────────
    /// 同步版在插件实现里应当返回「不支持」（它被调到就说明调用链坏了），
    /// 而不是返回一份与异步版不同的结果 —— 两份实现分叉的下场是「有的
    /// 路径用 A、有的路径用 B」，而那种错误没有任何日志会提示。
    fn build_chat_request_async<'a>(
        &'a self,
        account: &'a Value,
        body: &'a Value,
        client_headers: &'a HeaderMap,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { self.build_chat_request(account, body, client_headers) })
    }

    fn supports_image_generation(&self) -> bool {
        false
    }

    fn supports_image_editing(&self) -> bool {
        false
    }

    fn build_image_request_async<'a>(
        &'a self,
        _account: &'a Value,
        _body: &'a Value,
        _client_headers: &'a HeaderMap,
        _operation: super::images::ImageOperation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            Err(GatewayError::with_status(501, "当前提供商插件尚未接入图片生成，请更新插件"))
        })
    }

    /// 把「映射上绑的思考等级」翻译成本家上游认识的字段与取值（模块头扩展 10）。
    ///
    /// ── 调用时机与调用者 ────────────────────────────────────────
    /// `upstream::payload::send_body`：某一家 provider 的凭证已就绪、即将发送，
    /// 在按家改写完模型名之后调一次（**与模型名改写同一步**，因为等级与发送名
    /// 来自同一条映射，见 `catalog::WireTarget`）。注入点只做三件事：把
    /// `level` 原样递进来、按返回结果改写 body、把 `reason` 记进详细日志。
    /// 它不认识任何一家的字段名，也不做任何「档位该叫什么」的判断。
    ///
    /// ── `level` 是什么、不是什么（实现必须先读这段）────────────────
    /// 传进来的是**映射上原样存着的那个字符串**（已过 `normalize`：去空白、
    /// 非空、不超 32 字符），**没有**预先按通用表过滤过 —— 判断「这个值能不能
    /// 翻译」是实现的责任，因为各家的能力范围不同（CatPaw 只有三档，Qoder
    /// 看模型自己声明的 efforts）。
    ///
    /// 两条**共用**的判据在 `model_rules` 里，实现应当直接用它们而不是自己抄一份：
    ///   - `model_rules::reasoning_is_off(level)`：`off` / `none` —— 一律不注入。
    ///     本项目没有安全的「关闭思考」表达（Qoder 的 `enable_thinking = false`
    ///     会让 Qwen3.8 系列行为异常，CatPaw 没有这一档），**不要**把它翻译成
    ///     任何一个「关」的字段，`Skip` 就是正确答案；
    ///   - `model_rules::reasoning_rank(level)`：通用 6 档的强弱序号
    ///     （0 = minimal … 5 = max），表外的自定义等级返回 None。
    ///     CatPaw 拿它 `rank / 2` 折成自己的三档（见那家适配器的实现）；
    ///     表外值没有通用强弱序；只有本家目录明确声明支持时才可注入。
    ///
    /// ── 为什么默认是「不接」而不是「透传一个通用字段」────────────
    /// 默认实现返回 `Skip`。字段名和模型档位范围必须由实现确认，不能仅凭
    /// supportsReasoning 推断可调；WorkBuddy 的判定与归一在提供商插件内。
    ///
    /// ── 实现必须自己判的第三条：客户端是不是已经指定了 ──────────────
    /// 客户端在请求体里显式传了档位（CatPaw 的 `reasoning_effort` /
    /// `reasoningEffort` / `effort`，Qoder 的 `reasoning_effort` / `reasoning` /
    /// `thinking`）时，绑定**不覆盖**它 —— 那是用户的明确意图，比映射上的默认值
    /// 更具体。判据请**直接复用本家那个 resolver**（`catpaw::models::resolve_effort`
    /// 的取值链、`qoder::protocol::resolve_thinking` 读的那三个键），不要在这里
    /// 另抄一份键名清单：抄一份的下场是上游将来加一个别名时，绑定会开始覆盖
    /// 一个「客户端其实已经指定了」的请求，而那种错误没有任何日志会提示。
    ///
    /// ── 与 `build_chat_request` 的分工（为什么不做成后者的参数）──────
    /// 那个方法的入参是「账号 + 客户端原始 body」，而等级来自**映射**、由
    /// 编排层在按家改写模型名时才解析出来（同一处，见 `catalog::WireTarget`）；
    /// 把它塞进 `build_chat_request` 会让适配器看到一份与它无关的编排层状态，
    /// 也会让 `forward_conversation` 那条有状态路径（CatPaw / Qoder 都走它）
    /// 拿不到这个值 —— 而这两家恰恰也需要翻译档位。
    ///
    /// `model` 是**即将发给上游的那个名字**（已按家改写，见 `WireTarget.model`）：
    /// 需要按模型判断档位的家（Qoder 要拿它去查模型的 `efforts`）用它，
    /// 不看模型的家忽略它。
    fn reasoning_patch(&self, _level: &str, _model: &str, _body: &Value) -> ReasoningPatch {
        ReasoningPatch::Skip {
            reason: "该提供商不支持思考等级绑定（上游无对应字段）",
        }
    }

    /// [`Self::reasoning_patch`] 的异步形态（插件架构新增，理由同
    /// [`Self::build_chat_request_async`]）。
    ///
    /// ── 默认转调同步版 ──────────────────────────────────────────
    /// 内置家只查本地的枚举表，同步就够；插件家要回子进程问（各家的字段名
    /// 与档位取值表都在插件里）。
    fn reasoning_patch_async<'a>(
        &'a self,
        level: &'a str,
        model: &'a str,
        body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ReasoningPatch> + Send + 'a>> {
        Box::pin(async move { self.reasoning_patch(level, model, body) })
    }

    /// 从**即将发给本家的**发送体里读出随行的思考等级（请求日志「上游等级」
    /// 列的采集口；`None` = 这条请求没有等级随行）。
    ///
    /// ── 调用时机与读的是哪份 body ─────────────────────────────
    /// `upstream::payload::send_body` 在按家改写模型名、注入映射绑定的等级
    /// （[`Self::reasoning_patch`]）**之后**调用一次。此时 body 顶层的等级字段
    /// 要么是客户端显式传的原值（绑定让位时），要么是绑定注入的值 ——
    /// 读出来的就是上游收到的档位：覆写的家（CatPaw / Qoder）用**本家
    /// resolver 的同一条取值链**读，与转发行为完全同源；默认实现读的是
    /// 透传字段（见下）。
    ///
    /// ── 为什么默认读透传字段（而不是 None）─────────────────────
    /// `send_body` 只改写 model 名、注入映射绑定的等级，**不删客户端字段**：
    /// 客户端显式指定的等级（`reasoning_effort` 等通用键）对每一家都原样
    /// 随发送体上行 —— 那就是实际发出去的档位，与接不接绑定是两回事
    /// （「不接」说的是不**注入**，见 [`Self::reasoning_patch`] 的默认 `Skip`；
    /// 客户端自己传的字段没有理由在显示上抹掉）。默认实现因此用展示侧
    /// 读取器的并集链读发送体；「关闭思考」两档不算随行档位（与 Qoder
    /// 覆写同一口径），不预支一个「没发出去」的值。
    ///
    /// ── 显示值与真实字节的边界 ─────────────────────────────────
    /// 返回的是「随请求上行的档位意图值」。Qoder 的协议层还会按模型声明的
    /// efforts 二次归一（`minimal` → `low`、不支持的档位退默认），那一步需要
    /// 模型目录上下文，发送体阶段拿不到 —— 显示的因此是意图值而不是归一终值
    /// （CatPaw 的归并在 `reasoning_patch` 内已完成，无此差异）。
    fn outbound_reasoning(&self, body: &Value) -> Option<String> {
        crate::server::core::model_rules::read_client_level(body)
            .filter(|level| !crate::server::core::model_rules::reasoning_is_off(level))
    }

    /// 响应头是否表明上游错误；默认沿用 HTTP 非 2xx 的判定。
    ///
    /// 部分流式上游用 HTTP 200 + application/json 返回业务错误，适配器可
    /// 在成功流交给客户端之前将它送入既有错误读取、分类与账号轮换流程。
    fn is_error_response(&self, status: u16, _headers: &HeaderMap) -> bool {
        !(200..300).contains(&status)
    }

    fn is_error_response_async<'a>(
        &'a self,
        status: u16,
        headers: &'a HeaderMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, GatewayError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.is_error_response(status, headers)) })
    }

    /// 判定上游错误类型（status + 已解析的错误体）。
    ///
    /// `error_body` 是**已归一化**的错误对象：至少含 `code`（上游业务码，
    /// 可能为 null）与 `message`（上游文案，取自 message/msg/error.message，
    /// 非 JSON 响应则是截断后的原文）。归一化由传输层做（
    /// `upstream::request::read_upstream_error`），因为「怎么读一个 HTTP 错误体」
    /// 是协议层的事、与哪一家无关。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass;

    /// [`Self::classify_error`] 的异步形态（插件架构新增，理由同
    /// [`Self::build_chat_request_async`]）。
    ///
    /// ── 默认转调同步版 ──────────────────────────────────────────
    /// 内置家的分类是纯函数（看状态码与文案），同步就够。插件家的分类要
    /// 回子进程问（各家的判据差异极大，不能搬进网关）。
    ///
    /// ── 覆写者注意：这条在错误路径上 ────────────────────────────
    /// 每次上游报错都会走一次。插件侧必须快速返回（不做网络请求）——
    /// 那正是「分类是纯函数」这条既有约定的延续。
    fn classify_error_async<'a>(
        &'a self,
        status: u16,
        error_body: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = UpstreamErrorClass> + Send + 'a>,
    > {
        Box::pin(async move { self.classify_error(status, error_body) })
    }

    fn error_policy_async<'a>(
        &'a self,
        _status: u16,
        _body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = super::policy::ErrorPolicy> + Send + 'a>> {
        Box::pin(async { super::policy::ErrorPolicy::default() })
    }

    /// 会话式转发（`attempt_stateful`）失败后的分类。
    ///
    /// ── 为什么这个钩子必须存在 ─────────────────────────────────
    /// 会话式路径的错误编排历来是「一律透传（Fatal 语义）：不换账号、不冷却、
    /// 不重试」（见 `provider_loop::attempt_stateful`）。但「透传」只该是
    /// **编排**的默认，不该堵死分类：CodeArts 因会话准入走有状态路径，它的
    /// 流内额度信封（HTTP 200 SSE → 首包门 403）在这条路径上永远到不了
    /// [`Self::classify_error`]，结果福利池耗尽后每个请求都从头撞一遍全部
    /// 账号、一个冷却标记都不落。分类本身是 provider 专属知识（403 是额度
    /// 还是封禁，只有适配器知道），所以由适配器供给。
    ///
    /// ── 为什么默认是 Fatal ─────────────────────────────────────
    /// 既有会话式家（CatPaw）的行为就是透传，默认值让它们逐字不变；
    /// 只有明确声明「我的会话式错误里有可记账的限额」的家才覆写。
    /// 注意编排层只取这里 QuotaLimited 的**记账**语义（标记冷却后仍按队列
    /// 顺延）—— 不会对同一条错误再套用无状态路径的重试/刷新动作。
    fn classify_conversation_error(&self, error: &GatewayError) -> UpstreamErrorClass {
        UpstreamErrorClass::Fatal {
            status: u16::try_from(error.status_code).unwrap_or(500),
            message: error.message.clone(),
            upstream_code: error.upstream_code,
        }
    }

    /// 一次限额要给哪些**模型冷却键**记账。
    ///
    /// 默认只有本次的上游真名一条。 CodeArts 的福利池是**账号级**日额度：
    /// 池子耗尽时该账号的全部福利模型（下一轮哪怕请求的是另一个名字）都会
    /// 撞同样的 `insufficient quota`，只记一个键挡不住下一次顺延白撞 ——
    /// 所以这类家返回整组真名。返回值必须是**上游真名**（与
    /// `routing::CooldownKeys` 写读两侧同一口径），传入的 `wire_model` 就是
    /// 本次发出去的真名。
    ///
    /// `account_id` 供按账号取目录的家使用（当前 CodeArts 的目录是全 provider
    /// 共享缓存，不用它；留在签名里避免下一个这类家改签名）。
    fn quota_cooldown_models(&self, _account_id: &str, wire_model: &str) -> Vec<String> {
        vec![wire_model.to_string()]
    }

    /// 取可用 access token（含临期主动刷新；刷新结果回写 store）。
    ///
    /// `account_id` 为空串表示「没有指定账号」：用默认登录态
    /// （环境变量凭证或存储里派生的当前账号）。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    >;

    /// 刷新模型目录。
    ///
    /// ── `force` 是干什么的（本方法唯一的行为开关）──────────────
    ///   - `force = false`：**自动路径**的语义（`GET /v1/models` 的后台刷新、
    ///     启动刷新）。有缓存机制的家照常早退 —— 小浣熊的 10 分钟 TTL 就是为
    ///     自动路径设的：客户端每次拉模型列表都真打一次上游既没必要也招风控。
    ///   - `force = true`：**用户手动点「刷新模型清单」**的语义，跳过缓存/TTL
    ///     早退、真打一次上游。**这个参数不是可有可无的优化**：用户按下按钮的
    ///     全部预期就是「现在、真的去拉一次」，复用自动路径的实现会让小浣熊在
    ///     TTL 窗口内什么都不做就返回 —— 界面显示「刷新成功」而清单一个字没变，
    ///     与功能坏掉无法区分。缓存该不该绕过只由**谁发起**决定，因此把判断权
    ///     交给调用方（本参数），而不是让实现在内部猜。
    ///
    /// ── `account_id` 是干什么的（第二个行为开关）───────────────
    /// 指定用**哪个账号**的凭证去打上游目录接口（模型管理页「获取模型」弹窗
    /// 每行的「模型来源」下拉）：
    ///   - 空串 = 该家的默认选取：队首的可用账号（与转发默认使用的账号一致），
    ///     没有账号时各家自己回落到环境变量 / 桌面登录态；
    ///   - 非空 = 用户点名的那条账号。**点名了就按 id 直取**：取不到返回
    ///     失败原因（"账号不存在或不可用"），不回落到队首 —— 那会变成
    ///     「选了 A、用的是 B」的静默错误。
    /// 目录接口多数是账号级的（凭证不同、可见的清单可能不同），所以界面上
    /// 这一列要可见、可切换。
    ///
    /// ── 失败与返回 ──────────────────────────────────────────────
    /// 刷新失败**不返回错误**：目录刷新是维护动作，失败时保留现有清单
    /// （与改造前 `refresh_with_current_account` 的取向一致 —— 只打日志）。
    /// 返回值 [`ModelRefreshOutcome`] 是给**手动路径**如实汇报用的
    /// （自动路径不看它）：失败要在结果里说清原因，否则用户点了刷新只能看到
    /// 「已刷新」，而清单其实没动。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>,
    >;

    /// 模型目录刷新是否走「账号」这一维（默认 true；Cline 覆写为 false）。
    ///
    /// 「获取模型」弹窗每行的「模型来源」下拉据此决定显示与否：对不走账号维度
    /// 的家（目录接口无鉴权、清单是全局的），显示一个选了也一样的下拉是误导。
    /// 与 [`Self::refresh_models`] 的 `account_id` 参数配对 —— 那边忽略参数的
    /// 家，这边就该声明 false（逐条结果里也不再带 `accountId`）。
    fn refresh_uses_account(&self) -> bool {
        true
    }

    /// 本 provider 是否有**已接入的推理转发能力**（各家现在都是 true）。
    ///
    /// ── 这条声明曾经区分过什么（历史，别误会成现在还有 false）─────
    /// Qoder 在接入推理协议之前返回 false（它当时只有账号管理能力），
    /// 判据的用途正是下面这条「与 `list_models` 的分工」。那家接上转发后本方法
    /// 在生产代码里已无 false 分支；保留这个入口是因为它编码的**语义**仍然成立 ——
    /// 「清单这次是空的」与「这家根本没有转发能力」是两件事，将来再有新 provider
    /// 的过渡期（先上账号管理、后接转发）仍需要它。
    ///
    /// ── 与 `list_models` 的分工（为什么不看清单是否为空）──────────
    /// workbuddy 与 raccoon 的清单来自远程目录，拉取失败或没登录时**本来就可能
    /// 为空**，但它们能转发。拿清单当判据会把这两家在那种时刻误判成「不能转发」。
    /// 因此本方法是一个**恒定能力声明**（编译期常量），与 `supports_usage` /
    /// `supports_refresh` 同一性质；也正因为它是常量，调用点（账号存储派生全局
    /// 队首）可以逐账号调用而不必付 `list_models` 的克隆开销。
    ///
    /// 消费方：`account_store::pick_current`（全局队首 = `/api/session` 的
    /// `currentAccountId`、退出登录的删除目标、界面 ★）。没有转发能力却排进队首，
    /// 会让顶栏把它显示成「当前登录态」、并让「退出登录」把它删掉。
    fn supports_chat(&self) -> bool {
        true
    }

    /// 本 provider 是否有**可拉取的远程模型目录**（模块头扩展 9）。
    ///
    /// 判据与 `supports_refresh` 同一口径：**上游到底有没有那个接口**，
    /// 不是「我们想不想实现」。当前各家**全部覆写成 true**（下表是早期五家，
    /// 后来接入的家见各自 `models.rs` 的模块头）：
    /// ```text
    ///   WorkBuddy   GET  /v3/config
    ///   小浣熊       GET  {llmBase}/model_catalog
    ///   Qoder       GET  {gateway}algo/api/v2/model/list
    ///   CatPaw      POST /api/agent/maas/model-types
    ///   AutoClaw    GET  .../proxy/autoclaw-model-config
    /// ```
    /// 默认 false 是给「将来新接入、目录还没拉通」的 provider 留的过渡态。
    ///
    /// ── 为什么要有这个查询（而不靠「反正刷新是空操作」）──────────
    /// 这是**用户可见的按钮**，界面对不支持的家必须如实说明，
    /// 否则「声称支持却拿不到新清单」只会让用户以为是 bug。
    /// 与 `supports_usage` 同一取舍。
    ///
    /// ── 一条历史（避免后来者重犯）─────────────────────────────
    /// CatPaw 与 AutoClaw 曾返回 false，理由是「上游没有目录接口、清单是静态表」。
    /// 那个结论**两次都错**：接口一直都在，只是找法错了 —— CatPaw 的
    /// `tenant/scene/env` 是 POST body 不是 query（看着像 query 参数），
    /// AutoClaw 的目录在同 host 的 `/proxy/` 一级而不是对话用的 `/proxy/autoclaw`
    /// （顺着 `chat/completions` 找永远找不到）。**「这家没有某接口」的结论
    /// 要能说清是怎么排除的**，否则它只是一个还没找到的接口。
    fn supports_model_refresh(&self) -> bool {
        false
    }

    /// token 被上游拒绝（401）后的**强制**刷新（模块头扩展 2）。
    ///
    /// 默认直接转调 `ensure_access_token`：对「401 即临期」的 provider 够用。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        self.ensure_access_token(store, account_id)
    }

    /// 「可退避重试的错误」的建议；None = 无专属建议，允许通用规则。
    ///
    /// `attempt` 是**已经重试过**的次数（0 表示首次失败、还没重试过）。
    ///
    /// `budget` 是**本轮的原地重发预算**（还能退避几次），由编排层按
    /// 「同一账号重试」那一档给出（见 `config::RetrySettings`）。
    /// 适配器据此判断该不该再退避 —— 而不是自己去读全局设置：那样写的话
    /// 「哪一档管什么」这条规则就会漏进每个适配器里各实现一遍。
    fn retry_advice(&self, _error_body: &Value, _attempt: usize, _budget: usize) -> Option<RetryAdvice> {
        None
    }

    /// [`Self::retry_advice`] 的异步伴生方法（插件家走 RPC）。
    ///
    /// ── 为什么需要它 ──────────────────────────────────────────
    /// 同步签名没法 await，而插件家在另一个进程里 —— 没有这条伴生方法，
    /// 插件永远给不出 provider 专属的退避建议（workbuddy 的 11-128 那类），
    /// 只能吃到编排层的两档通用兜底。内置家转调同步实现，零成本。
    fn retry_advice_async<'a>(
        &'a self,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<RetryAdvice>> + Send + 'a>> {
        Box::pin(async move { self.retry_advice(error_body, attempt, budget) })
    }

    /// 可选的重试决定；Stop 终止本次请求，Default 才允许通用兜底。
    fn retry_decision_async<'a>(
        &'a self,
        _status: u16,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RetryDecision> + Send + 'a>> {
        Box::pin(async move {
            match self.retry_advice_async(error_body, attempt, budget).await {
                Some(advice) => RetryDecision::Retry {
                    delay_ms: advice.delay_ms,
                    reason: advice.reason,
                },
                None => RetryDecision::Default,
            }
        })
    }

    /// 哪些 `finish_reason` 表示**上游拒答**（HTTP 200 的假成功），默认空 = 不认。
    ///
    /// ── 为什么需要它（本家独有的上游形态）────────────────────────
    /// 有些上游（实测 OfficeAce / 华为 AgentArts 网关）把内容审核拒绝包装成一次
    /// 正常答复：`HTTP 200`、SSE 干净收尾，但正文是一句固定拒绝话术，终帧
    /// `choices[].finish_reason = content_filter`。只看 HTTP 状态这就是**假成功**
    /// —— 编排层不换家，客户端把套话当答案（真实现象：ZCode 的 agentic 请求打到
    /// 本家条条如此，见 `upstream::refusal` 模块头）。
    ///
    /// 判定由编排层执行：一旦置了本家的 `UpstreamResponse`/流式门控，就会把
    /// 该 `finish_reason` 视为一次**可轮换失败** —— 队列里还有别的账号/家就换，
    /// 只剩这一家时给客户端一句**可读错误**（而不是那句上游套话）。
    ///
    /// ── 为什么默认空、以及代价 ──────────────────────────────────
    /// 默认空 = **不认任何 `finish_reason`**，别家逐字节不变。声明是加法的：
    /// 一旦声明，本家所有流式请求会先在**有界**的头部缓冲里拉一小段以判定
    /// （见 `upstream::refusal`，上限 32 KiB）—— 真答案因此被推迟最多一节首字节。
    /// 这个代价只在声明的家上发生，且拒绝话术远在缓冲上限之内 ⇒ 对拒绝必中。
    ///
    /// ⚠️ 别把这里的值写成「上游偶发会回的任意值」：`length`（预算截断）是**正常**
    /// 收尾，不该换家；只有「上游明确不给你这次答复」的取值才属于本表。
    fn stream_refusal_finishes(&self) -> &'static [&'static str] {
        &[]
    }

    /// 本家的「**正文**拒绝标记」：上游把拒绝写成正文时，用来识别它的固定子串。
    ///
    /// ── 为什么还要一条按正文判的钩子 ────────────────────────────
    /// 有些上游的拒绝**不在 `finish_reason` 上**：它回 `200` + 一条正常的 SSE
    /// 内容帧（正文是写死的话术），`finish_reason` 干脆不存在，然后 `[DONE]`。
    /// 实测：AtomCode 的 CodingPlan 用量窗口耗尽后就是这样（2026-10-18 在 dev 上
    /// 复现，客户端侧 426 字节，正文
    /// 「模型「xxx」在当前 CodingPlan 用量窗口内已达上限。当前滚动窗口约在当日
    /// HH:MM 结束并重置额度，请稍后重试。」）。只看状态码与 `finish_reason` 的话，
    /// 这既不是失败也不算拒答 —— 于是一次**配额耗尽**被当成一次成功答复，
    /// 账号不冷却、不换家，下一发还打它。
    ///
    /// ── 命中后的处置与 `finish_reason` 那条**不同**（别混）───────
    /// `stream_refusal_finishes` 判的是**内容策略**拒绝：那是内容问题、换谁都会拒，
    /// 所以处置是「不罚账号、换家」。
    /// 本方法判的是**本账号的额度**问题：必须**罚账号**（按上游给的窗口重置时刻
    /// 写冷却，见 `rotate::mark_account_limited` 的文本兜底）+ 换家。
    ///
    /// 取值是**逐字固定子串**（大小写敏感、按字节比对）——只写实测见过的那句，
    /// 别写「可能出现的措辞」。默认空 = 不认任何正文标记，别家逐字节不变。
    fn stream_refusal_markers(&self) -> &'static [&'static str] {
        &[]
    }

    /// 本 provider 是否支持**主动**刷新凭证（模块头扩展 5）。
    ///
    /// 默认 false = 这家没有续期手段（没有 refreshToken、上游也没有刷新接口）。
    /// 判断依据是「上游到底有没有那个接口」，不是「我们想不想实现」：
    /// 声称支持却刷新不了，会让维护任务每次都白跑一趟并往日志里灌失败记录。
    ///
    /// 与 `refresh_access_token` 的关系：后者是「怎么刷」（401 之后的强制续期），
    /// 本方法是「能不能刷」（恒定能力）。维护任务先问这一个再动手 ——
    /// 对必然失败的家调用刷新只会制造噪音。
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 该账号的凭证此刻是否**已过期或临期**（需要刷新）（模块头扩展 5）。
    ///
    /// 判据是各家自己的知识：workbuddy 看会话里的 `expiresAt`、小浣熊看 JWT 的
    /// `exp`、AutoClaw 看凭证里的 `expiresAt`；各自的**临期窗口也不同**
    /// （5 分钟 / 5 分钟 / 5 分钟），所以不能在这里写一个统一的阈值。
    ///
    /// ── 为什么「判不出来」一律返回 false ────────────────────────
    /// 没有过期时间（opaque token、JWT 里没有 `exp`）或账号不存在时返回 false：
    /// 无从判断就说「需要刷新」会让维护任务每轮都去打一次上游，而那种请求
    /// 要么白刷（token 本来还好），要么稳定失败（没有 refreshToken）。
    /// 这类凭证的续期只能等 401 时的那条懒刷新链路。
    ///
    /// 例外是 Cline 与 AutoClaw：它们各自在这条判据上叠了一层**低频兜底**
    /// （Cline 按官方的 25 分钟、AutoClaw 按官方的每小时），让「判不出过期时间」
    /// 或「长期闲置」的账号也能被定期刷到 —— 见各自 adapter 的实现说明。
    ///
    /// **实现不得在内部取账号锁后跨 await**：本方法是同步的，取快照即返回；
    /// 真正的网络动作由 [`Self::refresh_access_token`] 在锁外做。
    fn credentials_expiring(&self, _store: &AccountStore, _account_id: &str) -> bool {
        false
    }

    /// [`Self::credentials_expiring`] 的异步形态（插件架构新增，理由同
    /// [`Self::build_chat_request_async`]）。
    ///
    /// ── 默认转调同步版 ──────────────────────────────────────────
    /// 内置家读本地凭证字段即返回；插件家的凭证在它自己的存储视图里，要问。
    ///
    /// ── 「判不出来就 false」这条不变 ────────────────────────────
    /// 覆写者同样必须遵守：无从判断就说「需要刷新」会让维护任务每轮白跑一趟
    /// 并灌失败日志（见同步版的说明）。
    fn credentials_expiring_async<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { self.credentials_expiring(store, account_id) })
    }

    /// 本 provider 是否允许「一个账号都没有时用默认登录态转发」
    /// （模块头扩展 3；workbuddy 的 `WORKBUDDY_TOKEN` 旁路）。
    fn allows_anonymous_default_session(&self) -> bool {
        false
    }

    /// 账号未显式配代理时，本家的出网是否**跟随系统代理**（默认 false = 直连）。
    ///
    /// ── 默认口径为什么是直连 ────────────────────────────────────
    /// 转发出口由账号的代理配置决定，不被机器上的 `HTTPS_PROXY` / 系统代理
    /// 悄悄改写（见 `egress::build_client` 的说明）—— 否则界面上显示「直连」
    /// 的账号实际走了代理，与用户看到的不符。
    ///
    /// ── 谁会覆写成 true ─────────────────────────────────────────
    /// 上游在多数网络里**只有经代理才可达**的家：Antigravity（Google）。
    /// 用户机器上「已经能打开 Google 的那个代理」就写在系统设置里（浏览器能
    /// 打开授权页正是靠它），账号若没单独配代理，跟随它是唯一合理的默认 ——
    /// 系统没配代理时它等价于直连，不引入新的失败面。覆写方要在自己的模块头
    /// 说明这条口径的适用范围（别让别的家照抄）。
    ///
    /// 只影响**转发链路**的客户端选择；登录链路没有账号可挂，本来就走
    /// `egress::client_for_system_proxy()`（见 `upstream::request` 的三选一）。
    fn system_proxy_when_unset(&self) -> bool {
        false
    }

    /// 本 provider 的**环境变量凭证现在是否真的存在**（模块头扩展 3 的配套）。
    ///
    /// 与 `allows_anonymous_default_session` 的分工：那个回答「这家支持这条旁路
    /// 吗」（恒定能力），这个回答「此刻环境变量里有没有那串凭证」（运行时事实）。
    /// 聚合目录要用后者决定「这家现在算不算有可用登录态」——只认前者会让
    /// 一个没配环境变量的机器把 `/v1/models` 广告成「小浣熊可用」。
    ///
    /// 默认 false（不认环境变量）。实现里只做「存在且非空」的判定，不发网络。
    fn env_credentials_present(&self) -> bool {
        false
    }

    /// 本 provider 是否有「默认模型」概念（模块头扩展 4）。
    ///
    /// 语义（架构文档 §4.4 末句）：客户端**未指定 `model`** 时，网关仍取
    /// config.json 的 `defaultModel`（那是 workbuddy 语义）注入 —— 但只在
    /// 「这个默认值命中的 provider 里有家认这个概念」时才注入；
    /// 都不认就按「未指定」处理，让上游用它自己的默认模型。
    /// 否则会出现「注入了一个 A 家的模型名、请求却被路由到 B 家」。
    fn supports_default_model(&self) -> bool {
        false
    }

    /// SSE 下发帧要不要把 `model` 改写成客户端请求的名字（模块头扩展 4）。
    ///
    /// 默认 false（workbuddy 上游原样回显客户端给的模型名）。小浣熊上游会回自己
    /// 的内部名，因此它的适配器覆盖成 true —— 见 `upstream::sse` 的
    /// `model_rewrite` 参数与插件侧小浣熊实现的说明。
    fn sse_model_rewrite(&self) -> bool {
        false
    }

    /// SSE 下发帧要不要丢掉「整片只有换行」的 content 分片（模块头扩展 11）。
    ///
    /// 默认 false（帧的字节逐字不变）。目前声明 true 的只有 WorkBuddy 与
    /// AutoClaw —— 那两家的上游会在正文分片之间插纯换行的帧作为
    /// 保活节拍，客户端按 markdown 渲染就被排成一行一个词。
    /// 判据与实测口径见 [`crate::server::core::upstream::sse::is_newline_keepalive`]；
    /// **执行不在这里**：本方法只回答「要不要」，丢帧动作落在 `upstream::sse`
    /// 这个 SSE 逐行解析的唯一出口，与 [`Self::sse_model_rewrite`] 同一分工。
    fn sse_strip_newline_chunks(&self) -> bool {
        false
    }

    /// 本 provider 的上游是否为**有状态（conversation 协议）**（模块头扩展 6，
    /// 架构文档 §4.2.1）。
    ///
    /// 默认 false（一次 HTTP 请求 = 一次对话）。CatPaw 覆写成 true：它的上游是
    /// round → event(running) → turn(SSE) → 工具循环 → event(completed) 的多步
    /// 会话（§9），单请求构造容纳不了。编排层据此把它分流到 adapter 自带的
    /// **会话式转发入口** [`Self::forward_conversation`]；产出与无状态路径同一种
    /// `ForwardOutcome`，于是 `chat.rs` 的其余链路（脱敏、记账、错误写出）零改动。
    ///
    /// 覆写成 true 的是走会话式转发入口的家（CatPaw / Qoder / Accio /
    /// CodeArts / Trae / Kuku / MonkeyCode，见各自 `mod.rs` 的模块头）。
    fn is_stateful(&self) -> bool {
        false
    }

    /// **会话式转发入口**（模块头扩展 6 的配套；架构文档 §4.2.1）。
    ///
    /// 只有有状态 provider 覆写它（`is_stateful` 为 true 的家，见各自 `mod.rs`）。
    /// 默认实现返回 503：走到这里
    /// 说明编排层把一家无状态 provider 当成了有状态的 —— 那是**内部契约错误**，
    /// 报错比让 `build_chat_request` 与 `forward_conversation` 各发一半安全。
    ///
    /// ── 与 `build_chat_request` 的分工 ──────────────────────────
    /// 无状态路径的编排是「构造请求 → 发 → 读错误 → 按分类换账号」；
    /// 有状态路径把这四步全收进适配器（CatPaw 是 round/turn/工具循环 + 注册表
    /// 状态机），编排层只保留它**必然共用**的那部分：账号选路循环、限额冷却、
    /// provider 轮询、telemetry 记账（见 `upstream::provider_loop` 的分流）。
    ///
    /// ── 参数语义（各 provider 通用，不是 CatPaw 专属）───────────
    ///   - `store`：账号存储（取凭证；**不持锁穿越 await** 由实现保证）；
    ///   - `account_id`：本次承载的账号 id（空串 = 没指定账号，用默认登录态）；
    ///   - `body`：客户端请求体（**原始形态**，归一化是实现内部的事）；
    ///   - `client_headers`：客户端入站头（会话类头如 `x-session-id` 在这里取）；
    ///   - `proxy`：账号级出网代理（已由编排层解析好，实现直接用于出网）；
    ///   - `stream`：客户端是否要流式（决定 `ForwardOutcome` 的形态）；
    ///   - `telemetry`：usage / 尝试次数的旁路槽（实现按需写入，可忽略）。
    fn forward_conversation<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _body: &'a Value,
        _client_headers: &'a HeaderMap,
        _proxy: Option<crate::server::core::proxies::ResolvedProxy>,
        _stream: bool,
        _telemetry: &'a std::sync::Arc<crate::server::core::upstream::usage::RequestTelemetry>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::server::core::upstream::ForwardOutcome, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        let kind = self.kind();
        Box::pin(async move {
            Err(GatewayError::with_status(
                503,
                format!(
                    "该提供商不支持会话式转发（{}）：这是内部契约错误，请检查 is_stateful 的分流",
                    super::kind_id(kind)
                ),
            ))
        })
    }
    /// 本 provider 是否支持「拉起官方登录页、由网关换取凭证」这种**网页登录**
    /// （模块头扩展 7）。
    ///
    /// 默认 false。当前**只有小浣熊**覆写成 true：它的官方登录页在登录成功后会
    /// 跳转到一个固定形态的自定义协议回调（`office-raccoon://auth/callback?code=…`），
    /// 网关拿到那个一次性 code 就能换到独立凭证。CatPaw / AutoClaw 没有这样的
    /// 协议（后者的桌面端登录态只能从本机 auth.json 解密读出），因此保持默认值。
    ///
    /// workbuddy 也**刻意保持**默认 false：它的「网页登录」是上游 `auth/state`
    /// 那套无头流程（`core::login`），与这里的回调换码是两条链路。把它标成 true
    /// 会让路由层按回调链去调 `build_login_url`→None，把一个能用的功能变成 400。
    fn supports_web_login(&self) -> bool {
        false
    }

    /// 构造网页登录的授权地址，返回 `(auth_url, state)`（模块头扩展 7）。
    ///
    /// `state` 由实现生成与保管（不落盘）：它既是 CSRF 口径的一次性随机串，也是
    /// 回调进来时「这次授权确实由本进程发起」的唯一凭据，因此**必须**用不可预测
    /// 的随机源，且回调侧要逐字比对。
    ///
    /// 默认 None = 本家不支持（与 [`Self::supports_web_login`] 的默认值一致）。
    fn build_login_url(&self) -> Option<(String, String)> {
        None
    }

    /// [`Self::build_login_url`] 的异步形态（插件架构新增）。
    ///
    /// ── 为什么需要它 ────────────────────────────────────────────
    /// 插件家拼授权地址要走一次 RPC（`login.start`），而同步签名不能 await。
    /// 与 [`Self::build_chat_request_async`] 同一处置：默认转调同步版
    /// （内置家零改动），只有跨进程的实现覆写它。
    fn build_login_url_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<(String, String)>> + Send + 'a>,
    > {
        Box::pin(async move { self.build_login_url() })
    }

    /// 完整的登录启动信息（**含流程形态**，插件架构新增）。
    ///
    /// ── 为什么需要它，而不是复用 `build_login_url_async` ──────────
    /// `(url, state)` 这个二元组把「怎么等结果」这件事丢掉了。登录有两种形态：
    ///   - **回调型**：授权页跳回本机（或跳自定义协议、由壳抓下来转给网关），
    ///     网关等 `login.callback` 到达（`poll_interval_ms == 0`）；
    ///   - **轮询型**：授权回调在上游域名终结、**永不回到本机**（AtomCode /
    ///     OfficeAce / ZCode 这三家），网关只能自己按间隔问上游「完成了没」
    ///     （`poll_interval_ms > 0`）。
    ///
    /// 丢掉这个位之后，轮询型插件家的登录会「地址出来了、界面一直挂着等一个
    /// 永远不到的跳转」—— 这是最难查的一类失败（没有任何报错）。
    ///
    /// 默认实现从 `(url, state)` 合成一个**回调型**结果，所以内置家与
    /// 已实现 `build_login_url_async` 的插件家零改动。
    fn login_start_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<agent2api_provider_protocol::LoginStart>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let (url, state) = self.build_login_url_async().await?;
            Some(
                agent2api_provider_protocol::LoginStart {
                    flow: state,
                    url,
                    instructions: String::new(),
                    // 0 = 回调型（网关等 `login.callback` 到达）
                    poll_interval_ms: 0,
                    external_browser: false,
                    allowed_hosts: Vec::new(),
                },
            )
        })
    }

    /// 动态登录方式的可选元数据通道（不新增协议类型）。
    /// 只读 `auth.methods`：旧插件返回 404 / 501 时给 None，旧登录入口不依赖它。
    /// 返回值仍不可信；管理路由必须投影成纯文本字段，不能透出 HTML 或凭证值。
    fn login_methods_metadata<'a>(
        &'a self,
        store: &'a AccountStore,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Value>, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            match self.admin_action(store, "", "auth.methods", &serde_json::json!({})).await {
                Ok(metadata) => Ok(Some(metadata)),
                Err(error) if matches!(error.status_code, 404 | 501) => Ok(None),
                Err(error) => Err(error),
            }
        })
    }

    /// 参数化登录启动：options 是已校验的 `{methodIndex, inputs, proxy}`。
    /// 默认委托旧入口；跨进程实现把整份 options 放进 `login.start.payload`，
    /// 因为旧 SDK 不会把顶层 methodIndex / inputs / proxy 传给 provider。
    fn build_login_flow_with_options<'a>(
        &'a self,
        _options: Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<agent2api_provider_protocol::LoginStart>, GatewayError>>
                + Send + 'a,
        >,
    > {
        Box::pin(async move { Ok(self.login_start_async().await) })
    }

    /// 轮询一次登录进度（**轮询型流程用**，插件架构新增）。
    ///
    /// 只有平台把 `poll_interval_ms` 报成 `> 0` 时才会被调用。默认返回错误：
    /// 走到默认实现说明分流坏了（一家声称是轮询型却没实现轮询）—— 报错比
    /// 静默返回 pending 到一个 5 分钟超时好。
    fn login_poll_async<'a>(
        &'a self,
        _flow: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<agent2api_provider_protocol::LoginPoll, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Err(GatewayError::with_status(
                501,
                "该提供商没有实现登录轮询：这是内部契约错误，请检查 poll_interval_ms 的分流",
            ))
        })
    }

    /// 把一条已换好的账号落进账号库，返回账号 id。
    ///
    /// ── 为什么单独一个方法（而不是让调用方自己落库）───────────────
    /// 落库规则是**这家自己的**：id 取插件的 identity 还是时间戳、名字取
    /// `public.name` 还是 label、记录进 `pluginSecret` 还是内置家的强类型字段
    /// —— 只有实现知道。轮询型登录拿到 `NormalizedAccount` 之后必须走这条路，
    /// 否则它会与回调型那条慢慢分叉（回调型走 [`Self::exchange_login_code`]，
    /// 里面已经落过库了）。
    ///
    /// 默认 501：内置家的登录都自己落库，不会走到这里。
    fn persist_login_account(
        &self,
        _account: &agent2api_provider_protocol::NormalizedAccount,
    ) -> Result<String, GatewayError> {
        Err(GatewayError::with_status(
            501,
            "该提供商不支持直接落库登录结果：这是内部契约错误",
        ))
    }

    /// 用回调里的一次性 `code` 换取凭证并**存入账号库**，成功返回账号 id
    /// （模块头扩展 7）。
    ///
    /// `state` 必须与 [`Self::build_login_url`] 返回的那个逐字一致 —— 实现要在这里
    /// 再校验一次（深度防御：调用链上游已校验过，但把校验放在「只有实现知道
    /// 该长什么样」的地方最可靠）。
    ///
    /// 落账号必须走本家**既有**的添加路径（小浣熊是 `add_raccoon_account`），
    /// 不另写一份落盘逻辑 —— 否则「手动添加」与「网页登录」两条路会在
    /// 校验、id 生成、优先级分配上慢慢分叉。
    ///
    /// 默认返回 501：走到这里说明路由层把一家不支持网页登录的 provider 送进来了，
    /// 那是内部契约错误，报错比静默什么都不做好。
    fn exchange_login_code<'a>(
        &'a self,
        _store: &'a AccountStore,
        _code: &'a str,
        _state: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        let kind = self.kind();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!(
                    "该提供商不支持网页登录（{}）：这是内部契约错误，请检查 supports_web_login 的分流",
                    super::kind_id(kind)
                ),
            ))
        })
    }

    /// 不落库的回调入口：保留完整 URL、授权码与 Pending 语义。
    /// 默认 None 表示继续走既有 `exchange_login_code`；插件覆写后只在 Done 时
    /// 由登录服务调用 `persist_login_account`，Pending 只表示已接受、继续等待。
    fn login_callback_poll_async<'a>(
        &'a self,
        _store: &'a AccountStore,
        _callback: agent2api_provider_protocol::LoginCallback,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<agent2api_provider_protocol::LoginPoll>, GatewayError>>
                + Send + 'a,
        >,
    > {
        Box::pin(async { Ok(None) })
    }

    /// 取消插件侧登录流程（释放监听端口及暂存凭证），不删除已落库账号。
    /// 默认无操作以兼容旧适配器；网关任务仍须先被取消，避免迟到 Done 落库。
    fn login_cancel_async<'a>(
        &'a self,
        _flow: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), GatewayError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }

    /// 本 provider 是否支持**余额 / 积分查询**（模块头扩展 8）。
    ///
    /// 默认 false。支持的家各有各的语义，下面举四家为例：
    ///   - workbuddy：腾讯计费接口的「积分简报」（`core::billing`，改造前唯一有的一家）；
    ///   - 小浣熊：官方积分钱包 + 订阅权益（`raccoon/balance.rs`）；
    ///   - CatPaw：美团 credit 域的额度接口，**需要额外配置网页会话凭证 token2**
    ///     （`catpaw/balance.rs`，未配置时返回可识别的「未配置」而不是失败）；
    ///   - AutoClaw：资产钱包 + 订阅信息（`autoclaw/balance.rs`）。
    ///
    /// 为什么这个方法而不是「能查就查」：前端按它决定卡片上要不要渲染「积分」按钮、
    /// 批量查询要不要把这个账号算进去。一家不支持却给了入口，用户点下去只会得到
    /// 一条必然失败的记录 —— 与 `supports_refresh` 同一取舍（如实回答恒定能力）。
    fn supports_usage(&self) -> bool {
        false
    }

    /// 查询某账号的余额 / 积分，返回**归一化后的 JSON**（模块头扩展 8）。
    ///
    /// ── 形状契约（`api::accounts` 的 `{results:[{id,name,usage,error}]}` 里
    ///    那个 `usage` 字段）────────────────────────────────────
    /// 两家新移植的实现（raccoon / autoclaw）遵守下面这套统一形状，
    /// **workbuddy 例外**：它的 `query_credits_summary` 是改造前就有的既有契约
    /// （`{kind, unlimited, totalLeft, planLeft, bonusLeft}`），前端积分面板
    /// 一直按它渲染，因此本方法对 workbuddy 直接透出既有形状、不做二次包装 ——
    /// 强行归一化会让那个面板的显示退化（这是明令禁止的）。
    /// ```jsonc
    /// {
    ///   "available": 1234.5,         // 可用总量（数字；判不出给 null）
    ///   "unit": "积分",               // 展示单位（前端拼文案用）
    ///   "wallets": [                  // 明细，可为空数组
    ///     { "type": "daily_points", "displayName": "每日积分", "balance": 800 }
    ///   ],
    ///   "subscription": {             // 可空
    ///     "planName": "…", "status": "…", "expireAt": 1789…,
    ///     "remainQuota": 100, "totalQuota": 1000
    ///   },
    ///   "raw": { }                    // 各家原始响应（排障用；前端默认不展示）
    /// }
    /// ```
    /// 上面那套之外还有几个**可选展示键**，各家按自己的读数形态自带、前端按形状探测
    /// （不按 provider 判）：`availableView`（读数的紧凑展示串）、`balanceView`
    /// （明细行自己的展示串，含重置时刻那类补充）、`credits[]`（逐份积分读数）、
    /// `windows[]`（逐窗口读数，如 OpenCode 的 5 小时 / 每周 / 每月三个窗口，
    /// 界面一格画三条）。不带这些键的家走的仍是最上面那套基础形状。
    ///
    /// ── 契约（调用方按这几条写）────────────────────────────────
    ///   1. **不抛 HTTP 错误的语义**：实现返回 `Err` 表示「这次查询失败」，
    ///      由调用方把 message 放进结果的 `error` 字段（批量查询不因单账号失败中断）；
    ///   2. **超时由实现自己设**：余额接口是外部 HTTP，`egress` 的默认
    ///      read_timeout 是 600 秒（给 SSE 长连接用的），不设总超时会让一个挂住的
    ///      余额查询把前端转圈转到天荒地老（源项目给 15~20 秒）；
    ///   3. **401 由实现原样透出**（`GatewayError::with_status(401, …)`）：
    ///      调用方据此走「刷新后重试一次」的既有处置；
    ///   4. 未配置查询凭证这类**用户可修复的前置条件缺失**，用
    ///      [`usage_not_configured`] 那条文案报 400（带
    ///      [`USAGE_NOT_CONFIGURED_CODE`] 标记）—— 前端把它显示成中性的
    ///      「未配置查询」而不是红色失败。
    fn query_usage<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let kind = self.kind();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!(
                    "该提供商不支持余额查询（{}）：这是内部契约错误，请检查 supports_usage 的分流",
                    super::kind_id(kind)
                ),
            ))
        })
    }

    /// 把一份「添加账号」的输入规范化成一条可落库的账号（插件架构新增）。
    ///
    /// ── 为什么是插件架构才有的方法 ──────────────────────────────
    /// 内置家的添加路径是 `account_store::add_*_account` 那些**强类型**入口
    /// （网关知道每一家的凭证形状），因此 trait 上从来不需要这个。插件家的
    /// 凭证是不透明 JSON，只有插件知道怎么解析、怎么算稳定身份、要不要出网
    /// 探活 —— 于是规范化必须回到插件里做。
    ///
    /// ── 默认实现返回 501 ────────────────────────────────────────
    /// 内置家不该被调到（`api::accounts` 按「枚举里有没有」分流）；真被调到
    /// 说明分流坏了，报错比返回一条形状不对的账号安全。
    fn normalize_account<'a>(
        &'a self,
        _store: &'a AccountStore,
        _input: agent2api_provider_protocol::NormalizeInput,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<agent2api_provider_protocol::NormalizedAccount, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        let label = self.label();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!("{label}不支持通过网关添加账号：这是内部契约错误，请检查分流"),
            ))
        })
    }

    /// 每日签到 / 每日活跃领取（插件架构新增的异步入口）。
    ///
    /// ── 与内置家分派的分工 ────────────────────────────────────
    /// 内置家的签到分派在 `core::billing::checkin::checkin_for`（一大串
    /// `match provider_id`，每家一条上游链路）。插件家的执行体在插件里，
    /// 那条 match 不该再长出 21 个分支 —— 所以插件家走这个方法。
    ///
    /// ── 返回形状为什么是 `{success, msg}` ────────────────────────
    /// 与内置家各 `claim_*` 的返回**逐字一致**（`billing::checkin` 的
    /// `claim_result` 只认这两个字段）。插件把 `CheckinReport` 翻成这个形状
    /// （`RpcAdapter` 负责），上层因此不必区分「内置 / 插件」。
    ///
    /// 默认 501：内置家走自己的分派，真被调到说明分流坏了。
    fn checkin_claim<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let label = self.label();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!("{label}不支持插件签到入口：这是内部契约错误，请检查分流"),
            ))
        })
    }

    fn checkin_claim_with_options<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        _manual: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        self.checkin_claim(store, account_id)
    }

    fn plugin_account_view<'a>(
        &'a self,
        _secret: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async { Err(GatewayError::with_status(501, "不支持插件账号视图")) })
    }

    /// 领取类活动（新手任务 / 限时套餐）。
    ///
    /// `claim` 是活动标识（各家自己命名；空串 = 一键领取全部可领项）。
    /// 返回形状由插件决定，与内置家各 `claim_all` 同形（`api::onboarding`
    /// 直接透给前端）。
    fn claim<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _claim: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let label = self.label();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!("{label}不支持领取入口：这是内部契约错误，请检查分流"),
            ))
        })
    }

    /// 领取类活动的**只读**状态（`api::onboarding::status` 的插件家入口）。
    ///
    /// `refresh = true` 是用户手动点了「查询任务」（跳过本地结算记忆实查一次）。
    /// 默认 502 而不是 501：调用方在插件家上会拿它当「这家没有新手任务」处理，
    /// 报一个明确的「不支持」比返回一份空任务表更不容易被误读成「都领完了」。
    fn claim_status<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _refresh: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let label = self.label();
        Box::pin(async move {
            Err(GatewayError::with_status(502, format!("{label}没有新手任务")))
        })
    }

    /// 插件家的「有每日签到」能力位（`None` = 内置家，走既有分派）。
    ///
    /// ── 为什么返回 `Option<bool>` 而不是 `bool` ────────────────
    /// 内置家是否有签到**不是**适配器的事：它由 `auto_checkin::CHECKIN_PROVIDERS`
    /// 与 `billing::checkin::supports_checkin` 那套（含 edition / isGuest 等
    /// 账号级判据）共同决定。给内置家返回 `false` 会让那套判据被短路。
    /// 插件家没有那些账号级判据，能力位就在握手里 —— 所以只有它们给 `Some`。
    fn plugin_checkin_capability(&self) -> Option<bool> {
        None
    }

    /// **厂商专属的管理动作**（成长任务 / 每日活跃保活 / 平台福利 / 套餐领取…）。
    ///
    /// ── 为什么是一条通道 ────────────────────────────────────────
    /// 这些动作彼此没有共性（有的是任务树、有的是运营活动、有的是套餐激活），
    /// 参数与返回形状只有厂商自己知道。给每个都开一个 trait 方法，trait 会随
    /// 长尾动作无限膨胀，而网关本来就不该理解它们 —— 它只负责透传。
    /// 完整动机见协议侧 `methods::gateway::ADMIN_ACTION` 的说明。
    ///
    /// ── 默认实现给 501 ──────────────────────────────────────────
    /// 与 `claim_status` 的 502 不同：那是「这家概念上就没有新手任务」，
    /// 而这里多半是**插件版本旧了**（新网关 + 老插件）—— 501 让调用方能区分
    /// 「这家没这个能力」与「这个动作没被实现」。
    fn admin_action<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        action: &'a str,
        _payload: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let label = self.label();
        Box::pin(async move {
            Err(GatewayError::with_status(
                501,
                format!("{label}没有实现管理动作「{action}」（可能需要更新提供商插件）"),
            ))
        })
    }
}

/// 「该账号没有配置余额查询凭证」的**机器可识别标记**（`GatewayError::code`）。
///
/// ── 为什么要一个 code 而不是让前端匹配文案 ───────────────────
/// 前端要把这种情况显示成中性的「未配置查询」，而其余失败显示成红色错误。
/// 按文案匹配会在任何一次措辞调整后静默失效（变成一片红），按状态码又会与
/// 真实的 400 参数错误混在一起 —— 一个稳定的字符串标记是这里唯一可靠的判据。
pub const USAGE_NOT_CONFIGURED_CODE: &str = "usage_not_configured";

/// 「该账号没有配置余额查询凭证」的统一文案（`GatewayError::code` 同为
/// [`USAGE_NOT_CONFIGURED_CODE`]）。
///
/// ── 为什么这条要各家共用一份，而不是各家自己拼 ─────────────────
/// 前端按 `code`（不是文案）判定「这是未配置、不是失败」，但**文案仍会显示
/// 在面板上**；各家各写一句会让同一个「去设置里补一下凭证」的动作在不同家
/// 的账号上呈现不同措辞。会真正走到这里的是「余额接口另需一份查询凭证」的家
/// （如 CatPaw 的网页会话凭证 `token2`、ZCode 的 Coding Plan JWT —— 见各自
/// 的 `balance.rs`）。
///
/// `field_hint` 说明去哪儿补（「账号设置里的余额查询凭证」），拼进 message；
/// 状态码用 **400**：这是前置条件缺失、用户自己能修，不是服务端故障 ——
/// 前端据此把它显示成中性提示而不是红色错误。
pub fn usage_not_configured(provider_label: &str, field_hint: &str) -> GatewayError {
    GatewayError::with_status(
        400,
        format!(
            "该账号没有余额查询凭证（{provider_label} 的 {field_hint}），\
             请在账号设置里配置后再查询"
        ),
    )
    .with_code(USAGE_NOT_CONFIGURED_CODE)
}

/// 注册表：provider → 适配器实现。
///
/// ── 这里现在只有一件事：返回占位 ──────────────────────────────
/// 29 个 `ProviderKind` 的实现**全部**搬到了提供商插件（`agent2api-providers`，
/// 不随本仓库发布）。网关没有任何内置转发 / 账号解析实现可返回，所以本函数
/// 对任何 kind 都返回 [`missing_plugin::placeholder`] —— 它如实报「需安装
/// 插件」，而不是在网关里留一份会随时间漂移的旁路实现。
///
/// 保留这个函数（而不是让调用点直接拿占位）是因为它是适配器层的**唯一入口**：
/// 将来若真要接回某个内置实现，改动只在这里；反过来，任何人想往网关里塞
/// 「怎么打某个上游」的代码，都必须先改这里 —— review 时一眼能看见。
///
/// 硬约束：**不要**在这里加任何真实现。插件缺失时如实报错是产品语义的一部分
/// （见 `missing_plugin` 的模块头），不是待补的坑。
pub fn adapter_for(kind: ProviderKind) -> &'static dyn ProviderAdapter {
    crate::server::core::providers::missing_plugin::placeholder(kind)
}

/// 运行时注册表：provider id → 适配器（插件架构新增）。
///
/// ── 与 [`adapter_for`] 的分工 ───────────────────────────────
/// `adapter_for` 按**编译期枚举**查内置家，返回 `&'static`，O(1) 且永不失败。
/// 插件家没有 `ProviderKind`（第三方 provider 的 id 是运行期才知道的），
/// 只能按字符串查这张表。
///
/// ── 为什么返回值是 `Option` ────────────────────────────────
/// 插件是**用户安装**的：没装那个包、或者包被停用，provider 就不存在。
/// 这是真实状态，必须表达出来 —— 不能像内置家那样假装「一定有」。
///
/// ── 优先级：插件 → 旧 Kind 占位 ─────────────────────────────
/// 一个 id 同时被内置家与插件声明时，插件赢。三条理由：
///   1. 迁移已经完成：21 家全部有插件实现，内置那份只剩「卸载插件后的
///      兜底」这一个用途 —— 让兜底赢等于让插件永远不生效；
///   2. 用户装了新版插件包就该用新版（发版节奏与网关解耦，那正是插件的
///      收益之一）；内置赢会让「升级插件」这个动作静默无效；
///   3. 回退路径仍然成立：卸载 / 停用插件包后 `lookup` 返回 `None`，自然
///      落回内置实现（账号数据也由
///      `account_store::plugin_accounts` 的镜像保证两边都能读）。
///
/// ── 但内置实现仍然要能被**显式**拿到 ──────────────────────────
/// 所以另开 [`builtin_adapter_by_id`]：需要「一定走内置那份」的调用点
/// （迁移期对照、插件实现出问题时的诊断入口）用它，而不是依赖这里的优先级。
pub fn adapter_by_id(id: &str) -> Option<Arc<dyn ProviderAdapter>> {
    if super::availability::is_suspended(id) { return None; }
    // 插件家优先（见上面第 3 条：没装/停用时自然落到内置）
    if let Some(adapter) = crate::server::core::plugin_host::lookup(id) {
        return Some(adapter);
    }
    // 内置家兜底
    super::kind_from_id(id).map(Into::into)
}

pub async fn adapter_by_id_ready(id: &str) -> Result<Option<Arc<dyn ProviderAdapter>>, GatewayError> {
    if super::availability::is_suspended(id) { return Ok(None); }
    if let Some(adapter) = crate::server::core::plugin_host::lookup_ready(id).await? {
        return Ok(Some(adapter));
    }
    Ok(super::kind_from_id(id).map(Into::into))
}

/// **只**查内置实现（跳过插件注册表）。
///
/// ── 什么时候该用它 ──────────────────────────────────────────
/// 需要「内置那份一定在」的场合：迁移期的差分验证、插件实现疑似出问题时的
/// 诊断对照、以及任何「想知道这家有没有编译进网关」的判定。
/// 生产转发路径一律用 [`adapter_by_id`]（插件优先）。
pub fn builtin_adapter_by_id(id: &str) -> Option<Arc<dyn ProviderAdapter>> {
    super::kind_from_id(id).map(Into::into)
}

/// 内置家的 `Arc` 包装（让 `adapter_by_id` 的返回类型统一）。
///
/// ── 为什么需要这层包装 ──────────────────────────────────────
/// `adapter_for` 返回 `&'static dyn`，而插件家返回 `Arc<dyn>`。要让两者
/// 装进同一个返回类型，只能把前者也包成 `Arc` —— 这个包装不复制任何状态，
/// 只转发每个方法到静态实例（每次转发是一次 match + 一次虚调用，与直接
/// 调 `adapter_for` 同量级）。
struct BuiltinAdapter(ProviderKind);

impl From<ProviderKind> for Arc<dyn ProviderAdapter> {
    fn from(kind: ProviderKind) -> Self {
        Arc::new(BuiltinAdapter(kind))
    }
}

impl ProviderAdapter for BuiltinAdapter {
    // ── 为什么用宏而不是手写 34 个转发 ──────────────────────────
    // 手写 34 个方法的转发既冗长又容易漏（漏一个就会落到 trait 的默认实现，
    // 行为静默变化）。宏把「调用目标」与「参数列表」分开写，加方法时只改
    // 一处签名。
    fn kind(&self) -> ProviderKind {
        self.0
    }

    fn list_models(&self) -> Vec<Value> {
        adapter_for(self.0).list_models()
    }

    fn advertise_models(&self, store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
        adapter_for(self.0).advertise_models(store, manifest)
    }

    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        adapter_for(self.0).build_chat_request(account, body, client_headers)
    }

    fn build_chat_request_async<'a>(
        &'a self,
        account: &'a Value,
        body: &'a Value,
        client_headers: &'a HeaderMap,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).build_chat_request_async(account, body, client_headers)
    }

    fn reasoning_patch(&self, level: &str, model: &str, body: &Value) -> ReasoningPatch {
        adapter_for(self.0).reasoning_patch(level, model, body)
    }

    fn reasoning_patch_async<'a>(
        &'a self,
        level: &'a str,
        model: &'a str,
        body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ReasoningPatch> + Send + 'a>> {
        adapter_for(self.0).reasoning_patch_async(level, model, body)
    }

    fn outbound_reasoning(&self, body: &Value) -> Option<String> {
        adapter_for(self.0).outbound_reasoning(body)
    }

    fn is_error_response(&self, status: u16, headers: &HeaderMap) -> bool {
        adapter_for(self.0).is_error_response(status, headers)
    }

    fn is_error_response_async<'a>(
        &'a self,
        status: u16,
        headers: &'a HeaderMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, GatewayError>> + Send + 'a>> {
        adapter_for(self.0).is_error_response_async(status, headers)
    }

    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        adapter_for(self.0).classify_error(status, error_body)
    }

    fn classify_error_async<'a>(
        &'a self,
        status: u16,
        error_body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpstreamErrorClass> + Send + 'a>> {
        adapter_for(self.0).classify_error_async(status, error_body)
    }

    fn classify_conversation_error(&self, error: &GatewayError) -> UpstreamErrorClass {
        adapter_for(self.0).classify_conversation_error(error)
    }

    fn quota_cooldown_models(&self, account_id: &str, wire_model: &str) -> Vec<String> {
        adapter_for(self.0).quota_cooldown_models(account_id, wire_model)
    }

    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).ensure_access_token(store, account_id)
    }

    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        adapter_for(self.0).refresh_models(store, account_id, force)
    }

    fn refresh_uses_account(&self) -> bool {
        adapter_for(self.0).refresh_uses_account()
    }

    fn supports_chat(&self) -> bool {
        adapter_for(self.0).supports_chat()
    }

    fn supports_image_generation(&self) -> bool {
        adapter_for(self.0).supports_image_generation()
    }

    fn supports_image_editing(&self) -> bool {
        adapter_for(self.0).supports_image_editing()
    }

    fn build_image_request_async<'a>(
        &'a self,
        account: &'a Value,
        body: &'a Value,
        client_headers: &'a HeaderMap,
        operation: super::images::ImageOperation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).build_image_request_async(account, body, client_headers, operation)
    }

    fn supports_model_refresh(&self) -> bool {
        adapter_for(self.0).supports_model_refresh()
    }

    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).refresh_access_token(store, account_id)
    }

    fn retry_advice(&self, error_body: &Value, attempt: usize, budget: usize) -> Option<RetryAdvice> {
        adapter_for(self.0).retry_advice(error_body, attempt, budget)
    }

    fn retry_advice_async<'a>(
        &'a self,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<RetryAdvice>> + Send + 'a>> {
        adapter_for(self.0).retry_advice_async(error_body, attempt, budget)
    }

    fn retry_decision_async<'a>(
        &'a self,
        status: u16,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RetryDecision> + Send + 'a>> {
        adapter_for(self.0).retry_decision_async(status, error_body, attempt, budget)
    }

    fn stream_refusal_finishes(&self) -> &'static [&'static str] {
        adapter_for(self.0).stream_refusal_finishes()
    }

    fn stream_refusal_markers(&self) -> &'static [&'static str] {
        adapter_for(self.0).stream_refusal_markers()
    }

    fn supports_refresh(&self) -> bool {
        adapter_for(self.0).supports_refresh()
    }

    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        adapter_for(self.0).credentials_expiring(store, account_id)
    }

    fn credentials_expiring_async<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        adapter_for(self.0).credentials_expiring_async(store, account_id)
    }

    fn allows_anonymous_default_session(&self) -> bool {
        adapter_for(self.0).allows_anonymous_default_session()
    }

    fn system_proxy_when_unset(&self) -> bool {
        adapter_for(self.0).system_proxy_when_unset()
    }

    fn env_credentials_present(&self) -> bool {
        adapter_for(self.0).env_credentials_present()
    }

    fn supports_default_model(&self) -> bool {
        adapter_for(self.0).supports_default_model()
    }

    fn sse_model_rewrite(&self) -> bool {
        adapter_for(self.0).sse_model_rewrite()
    }

    fn sse_strip_newline_chunks(&self) -> bool {
        adapter_for(self.0).sse_strip_newline_chunks()
    }

    fn is_stateful(&self) -> bool {
        adapter_for(self.0).is_stateful()
    }

    fn forward_conversation<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        body: &'a Value,
        client_headers: &'a HeaderMap,
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
        adapter_for(self.0).forward_conversation(
            store,
            account_id,
            body,
            client_headers,
            proxy,
            stream,
            telemetry,
        )
    }

    fn supports_web_login(&self) -> bool {
        adapter_for(self.0).supports_web_login()
    }

    fn build_login_url(&self) -> Option<(String, String)> {
        adapter_for(self.0).build_login_url()
    }

    fn build_login_url_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<(String, String)>> + Send + 'a>,
    > {
        adapter_for(self.0).build_login_url_async()
    }

    fn login_start_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<agent2api_provider_protocol::LoginStart>>
                + Send
                + 'a,
        >,
    > {
        adapter_for(self.0).login_start_async()
    }

    fn login_methods_metadata<'a>(
        &'a self,
        store: &'a AccountStore,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Value>, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).login_methods_metadata(store)
    }

    fn build_login_flow_with_options<'a>(
        &'a self,
        options: Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<agent2api_provider_protocol::LoginStart>, GatewayError>>
                + Send + 'a,
        >,
    > {
        adapter_for(self.0).build_login_flow_with_options(options)
    }

    fn login_callback_poll_async<'a>(
        &'a self,
        store: &'a AccountStore,
        callback: agent2api_provider_protocol::LoginCallback,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<agent2api_provider_protocol::LoginPoll>, GatewayError>>
                + Send + 'a,
        >,
    > {
        adapter_for(self.0).login_callback_poll_async(store, callback)
    }

    fn login_cancel_async<'a>(
        &'a self,
        flow: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).login_cancel_async(flow)
    }

    fn login_poll_async<'a>(
        &'a self,
        flow: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<agent2api_provider_protocol::LoginPoll, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        adapter_for(self.0).login_poll_async(flow)
    }

    fn persist_login_account(
        &self,
        account: &agent2api_provider_protocol::NormalizedAccount,
    ) -> Result<String, GatewayError> {
        // 内置家的登录都在自己的 `start_*_login` 里落库，走不到这里
        adapter_for(self.0).persist_login_account(account)
    }

    fn normalize_account<'a>(
        &'a self,
        store: &'a AccountStore,
        input: agent2api_provider_protocol::NormalizeInput,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<agent2api_provider_protocol::NormalizedAccount, GatewayError>,
                > + Send
                + 'a,
        >,
    > {
        adapter_for(self.0).normalize_account(store, input)
    }

    fn exchange_login_code<'a>(
        &'a self,
        store: &'a AccountStore,
        code: &'a str,
        state: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).exchange_login_code(store, code, state)
    }

    fn checkin_claim<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).checkin_claim(store, account_id)
    }

    fn claim<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        claim: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).claim(store, account_id, claim)
    }

    fn claim_status<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        refresh: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).claim_status(store, account_id, refresh)
    }

    fn plugin_checkin_capability(&self) -> Option<bool> {
        adapter_for(self.0).plugin_checkin_capability()
    }

    fn admin_action<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        action: &'a str,
        payload: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).admin_action(store, account_id, action, payload)
    }

    fn supports_usage(&self) -> bool {
        adapter_for(self.0).supports_usage()
    }

    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        adapter_for(self.0).query_usage(store, account_id)
    }
}

/// 已**完整实现**的适配器对应的 kind 列表（顺序 = 注册表顺序）。
///
/// ── 现在恒为空，且这是**终点状态** ────────────────────────────
/// 这个函数回答的问题是「网关编译进去的执行体还剩哪几家」。全部提供商的转发与
/// 账号实现都已搬到独立的提供商插件（`agent2api-providers`，不随本仓库发布），
/// 所以答案就是「一家都不剩」。
///
/// 保留空实现而不是删掉函数：它的两个消费方（`catalog_refresh` 的逐家刷新与
/// `reschedule`）都是「遍历内置家」的形状，删掉函数会让那两处改写成
/// `if false` 之类的形状，反而不如一张空表读得明白。**不要往这张表里加东西**
/// —— 往里加一家，等于把该家的实现重新带回网关，正是拆家要消除的东西。
///
/// 插件家不走这里：`catalog_refresh` 另有基于 `plugin_host::installed_ids()` 的
/// 第二段循环，与内置家共用同一套「支持才刷」的判据。
pub fn implemented_kinds() -> Vec<ProviderKind> {
    Vec::new()
}

/// 一次模型目录刷新的结果（模块头扩展 9）。
///
/// ── 为什么不用 `core::models::RefreshOutcome` ────────────────────
/// 那个类型是 **workbuddy 单家**的既有契约（`{refreshed, count, source, reason}`，
/// 与 Node 版逐字对应），拴在 `ModelCatalog` 上。适配器层还要表达另外几家的不同事实
/// （小浣熊的「TTL 跳过」、没有远程目录的家的「跳过」等），拿它覆盖各家会逼着这些家填
/// 无意义的 `source`/`reason`，还要动 `core::models` 的既有类型。本类型是**最小
/// 适配器层形态**：workbuddy 在实现里把自己的 `RefreshOutcome` 搬运过来即可。
///
/// ── 三档语义（与 `credential_maintenance` 的状态常量同一口径）──
///   - `refreshed = true`：真的拿到了新清单（`count` 是落地后的条目数）。
///   - 否则 `message = None`：**没刷**（TTL 未到 / 上游没给可用清单 / 缺少登录态）。
///     「没刷」不等于「失败」：自动路径的意图本来就是「有缓存就用缓存」。
///   - 否则 `message = Some(..)`：**尝试了但失败**（状况见文案）。清单按既有
///     行为保留（不清空），但用户该看到为什么。
///
/// `count` 只在 `refreshed = true` 时有意义（其余情形为 0）。
#[derive(Clone, Debug, Default)]
pub struct ModelRefreshOutcome {
    /// 是否真的落地了一份新清单
    pub refreshed: bool,
    /// 落地后的目录条目数（`refreshed = true` 时有效）
    pub count: usize,
    /// 失败原因（`None` = 没失败：或成功，或本次按缓存/TTL 跳过）
    pub message: Option<String>,
}

impl ModelRefreshOutcome {
    /// 真刷新成功（`count` 是落地后的条目数）
    pub fn refreshed(count: usize) -> Self {
        Self { refreshed: true, count, message: None }
    }

    /// 没有刷（按缓存/TTL 跳过、上游没给可用清单、没有可用的家等）——
    /// 不是失败，只是这次没有新东西
    pub fn unchanged() -> Self {
        Self::default()
    }

    /// 尝试刷新但失败（`reason` 是会显示给用户的原因）
    pub fn failed(reason: impl Into<String>) -> Self {
        Self { refreshed: false, count: 0, message: Some(reason.into()) }
    }
}

/// 对**当前缓存清单**里的模型补一次默认规则种子（各家一条）。
///
/// ── 小浣熊的这一手已经不需要了 ──────────────────────────────
/// 它的 `raccoon-<hex>` 默认禁用与 `sn-` 去前缀映射，依赖「网关手里有这家的
/// 清单」。目录已随实现搬到插件，网关只拿插件返回的成品清单（其中已带
/// `visibility`），默认规则改由插件在 `models.refresh` 里自己申报 ——
/// 网关这边再种一次只会与插件打架（两处都在写同一批 `seeded` 记账）。
///
/// 对**当前缓存清单**里的 WorkBuddy 模型补一次默认规则种子（默认只启用白名单
/// 内的模型，见 `model_rules::seed_workbuddy_defaults`）。
///
/// 与 `seed_current_qoder_defaults` 同理：刷新可能因失败 / 无登录态而不落地
/// 新清单 —— 那条路径上没有种子可挂，启动后手里的这份清单（内置或旧缓存）
/// 也要有同样的默认值。幂等：种过的 id 不会再动。
///
/// **两个地区各跑一遍**（拆家后各有各的清单与 provider 键）：种子按
/// `(provider, id)` 记账，只种国内版会让国际版的新模型停在全开状态，
/// 而两家的模型名很可能同名。
fn seed_current_workbuddy_defaults() {
    // 一次性迁移：解开旧白名单（hy3 / hy4-preview-f / deepseek-v4.1-flash 之外
    // 全关）误关的那批模型。必须在可见性种子之前，理由与 Qoder 那边相同。
    if let Some(summary) = crate::server::core::model_rules::migrate_workbuddy_default_enabled() {
        crate::server::logging::log("[Models]", &summary);
    }
    for region in super::workbuddy::Region::ALL {
        let ids: Vec<String> = super::adapter::adapter_by_id(region.provider_id())
            .map(|adapter| adapter.list_models())
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        if let Some(summary) = crate::server::core::model_rules::seed_workbuddy_defaults(
            region.provider_id(),
            &ids,
        ) {
            crate::server::logging::log("[Models]", &summary);
        }
    }
}

/// 对**当前缓存清单**里的 Qoder 模型补一次默认规则种子（默认只启用白名单内的
/// 模型，见 `model_rules::seed_qoder_defaults`）。
///
/// 清单来自插件缓存（`rpc_cache`：握手带回的内置兜底或上一次刷新的结果），
/// 按地区各取各的、各按各的 provider id 种（拆家后两家是两套
/// `(provider, id)` 命名空间）。要这一手补种的理由与 WorkBuddy 相同：Qoder 在
/// 没有账号 / 远程刷新失败时手里只剩插件带来的兜底清单，而**升级用户**的清单
/// 正是那份兜底 —— 不补种的话，他们打开管理页看到的仍是旧的全开状态。
fn seed_current_qoder_defaults() {
    // 先跑一次性迁移：把旧白名单（只留 `Qwen3.8-Flash`）误关的那批模型重新
    // 打开。必须在可见性种子**之前** —— 否则 visibility 种子会把它们记成
    // 「已经处理过」，迁移再来解就晚了（虽然结果一样，但日志顺序会看不懂）。
    if let Some(summary) = crate::server::core::model_rules::migrate_qoder_default_enabled() {
        crate::server::logging::log("[Models]", &summary);
    }
    seed_qoder_defaults_from_catalog();
}

/// 按地区把当前清单里的 id 交给 `model_rules::seed_qoder_defaults`。
///
/// 清单从**插件缓存**取（`adapter_by_id(provider).list_models()`）：网关不再
/// 持有 Qoder 的目录实现，而种子必须按 `(provider, id)` 记账 —— 两个地区各
/// 种各的。
fn seed_qoder_defaults_from_catalog() {
    for region in super::qoder::endpoints::Region::ALL {
        let provider = region.provider_id();
        let ids: Vec<String> = match super::adapter::adapter_by_id(provider) {
            Some(adapter) => adapter
                .list_models()
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        };
        if ids.is_empty() {
            continue;
        }
        if let Some(summary) = crate::server::core::model_rules::seed_qoder_defaults(
            provider,
            &format!("Qoder {}", region.label()),
            &ids,
        ) {
            crate::server::logging::log("[Models]", &summary);
        }
    }
}

/// 对**当前清单**里的 Cline 模型补一次默认映射种子（带池前缀的模型自动获得
/// 去前缀别名，如 `deepseek-v4.1-flash → cline-free/deepseek-v4.1-flash`，
/// 见 `model_rules::seed_cline_defaults`）。
///
/// 要这一手补种的理由与另外三家相同：Cline 的目录在「没有账号 / 远程刷新失败」
/// 时只有静态兜底清单，那条路径上没有种子可挂 —— 而**静态兜底里也有带前缀的
/// 模型**，不补种的话用户装上就看到 `cline-free/deepseek-v4.1-flash` 这种名字。
/// 幂等：种过的 id 不会再动（用户删掉自动映射后不会被改回来）。
///
/// ── 两个池各跑一遍（顺序即「谁先拿到同名短名」）─────────────
/// 两池有同名模型（`deepseek-v4.1-flash`），去前缀种子按「先到先得」只给一家
/// 建映射，因此**遍历顺序决定短名先落在谁家**。这里按 `Pool::ALL` 的顺序
/// （Free 在前）—— 免费池无门槛（不需要订阅），让它当默认归属更合理；
/// 另一家由 `model_rules::EXTRA_ALIASES` 点名补上，所以**短名对两家都能路由**，
/// 顺序只影响候选链里谁在前，不影响可用性。
///
/// 不再需要 `store`：早先要按「账号库里有哪个池」重排（池是账号属性时的
/// 遗留），现在池是身份，两家的种子各按各的清单跑。
fn seed_current_cline_defaults() {
    for pool in super::cline::models::Pool::ALL {
        // 清单来自插件缓存（`rpc_cache`）：握手带回的内置兜底或上一次刷新的结果。
        let provider = pool.provider_id();
        let ids: Vec<String> = super::adapter::adapter_by_id(provider)
            .map(|adapter| {
                adapter
                    .list_models()
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            continue;
        }
        if let Some(summary) =
            crate::server::core::model_rules::seed_cline_defaults(provider, &ids)
        {
            crate::server::logging::log("[Models]", &summary);
        }
    }
}

/// 对**当前清单**里「上游说有、官方客户端不展示」的模型补一次默认禁用种子。
///
/// ── 为什么单列一个函数（而不并进上面那几个）──────────────────
/// 上面几个走的是**白名单制**（由人挑几个「装上就能用」的），本函数走
/// **可见性制**：判据来自各家 `models.rs` 算出的 `visibility` 字段（Antigravity
/// 按世代、Qoder 读上游 `enable`、CodeArts 读
/// `display_enabled`）。判据在谁手里，就在谁那里算 —— 聚合层只负责读。
///
/// ── 为什么「默认关闭」= 真正不可用 ─────────────────────────
/// 本仓的路由门是「默认绑定关掉 = 这个名字调不动」
/// （`catalog::builtin_target` 先过 `default_enabled`，不过就返回 `None`），
/// 所以关掉默认绑定同时达到两个效果：不出现在 `/v1/models`，也不接受点名调用。
/// 只隐藏列表不关绑定会留下「列表里没有、但点名能用」的矛盾状态。
///
/// 只对**有判据的四家**生效（其余家的清单没有 `visibility` 键，这里自然跳过）：
/// 别的家目前没有可靠判据，按「没依据就不藏」处理（见
/// `officeace/probe.rs` 的同一纪律）。
pub(crate) fn seed_hidden_visibility_defaults() {
    for kind in [
        super::ProviderKind::WorkBuddy,
        super::ProviderKind::WorkBuddyIntl,
        super::ProviderKind::Antigravity,
        super::ProviderKind::Qoder,
        super::ProviderKind::QoderIntl,
    ] {
        let provider = crate::server::core::providers::kind_id(kind);
        let Some(adapter) = adapter_by_id(provider) else { continue };
        let entries: Vec<(String, &str)> = adapter
            .list_models()
            .iter()
            .filter_map(|item| {
                let id = item.get("id").and_then(Value::as_str)?.trim().to_string();
                if id.is_empty() {
                    return None;
                }
                // 没有可见性判据的条目直接跳过（不给种子记账 —— 将来这家补上
                // 判据时还能正常种）
                let visibility = item.get("visibility").and_then(Value::as_str)?;
                // `'static` 生命周期：这里把三种取值转成常量字符串，
                // 免得把 Json 值借给 `seed_hidden_disabled` 活过本次循环
                let owned: &'static str = match visibility {
                    "hidden" => "hidden",
                    "shown" => "shown",
                    _ => "undeclared",
                };
                Some((id, owned))
            })
            .collect();
        if entries.is_empty() {
            continue;
        }
        if let Some(summary) = crate::server::core::model_rules::seed_hidden_disabled(
            provider,
            &crate::server::core::providers::label_of(provider),
            &entries,
        ) {
            crate::server::logging::log("[Models]", &summary);
        }
    }
}

/// 让所有**已实现**的 provider 各刷新一次模型目录（**自动**路径入口）。
///
/// 调用点：`api::chat::spawn_catalog_refresh`（客户端拉 `/v1/models` 的后台刷新）
/// 与「模型目录刷新」定时任务。真正的循环在 `providers::catalog_refresh` ——
/// 与手动路径共用同一段实现，两条入口只在 `manual` 这一个开关上分叉。
///
/// **自动路径受排期与冷却约束**：每家的刷新间隔、在途占位、失败冷却都由
/// `core::task_state` 持久化（`modelRefresh:<provider>`），因此「重启一次就重刷
/// 一遍」「客户端每次拉列表都真打一次上游」都不会再发生。这也是它返回逐家结果
/// 的原因：定时任务要按状态统计（成功 / 跳过 / 失败）并写进任务摘要。
///
/// 各家自身的缓存/TTL 仍然生效（`force = false`）：排期说「可以刷了」之后，
/// 由各家决定这次是否真的打网络。手动按钮走 [`refresh_implemented_forced`]。
pub async fn refresh_implemented(store: &AccountStore) -> Vec<Value> {
    seed_current_workbuddy_defaults();
    seed_current_qoder_defaults();
    seed_current_cline_defaults();
    seed_hidden_visibility_defaults();
    let results = super::catalog_refresh::refresh(store, &serde_json::Map::new(), None, false).await;
    // 刷新落地后补种一次：新增的带前缀 / 别名模型在这一刻才出现在清单里
    // （与开头那次同一件事 —— 那时种的是缓存恢复的清单）
    seed_current_cline_defaults();
    // 可见性种子必须在**刷新之后**再跑一次：远程清单一落地，「上游说有但官方
    // 不展示」的那批 id 才第一次出现（缓存恢复时它们可能还不存在）。
    seed_hidden_visibility_defaults();
    results
}

/// 缓存装入后的目录预热与规则种子。插件缓存由装配时的 `set_defaults` 恢复，
/// 这里在库就绪及插件装配完成后均调用，保证恢复的模型默认规则已经就位。
pub fn restore_cached_catalogs() {
    for kind in implemented_kinds() {
        let _ = adapter_for(kind).list_models();
    }
    seed_current_workbuddy_defaults();
    seed_current_qoder_defaults();
    seed_current_cline_defaults();
    // 启动这一刻手里是缓存恢复的清单（或内置兜底）：可见性种子要跟着就位，
    // 否则「重启后不可见模型又变成可用了」。
    seed_hidden_visibility_defaults();
}

/// **手动**刷新模型清单：只刷「支持刷新」的家，且强制绕过缓存。
///
/// 逐家结果：`[{ provider, providerLabel, status, count?, refreshedAt, fixed?, message? }, ...]`，
/// 顺序 = 注册表顺序，**每家都有一条**（不支持的家也在里面，status = `skipped`
/// 并说明原因）—— 界面的汇总（成功 N / 失败 M / 跳过 K）与逐条明细因此能对上
/// 总数（与 `credential_maintenance::refresh_expiring_accounts` 同一取舍）。
///
/// ── 结果字段（前后端契约）──────────────────────────────────────
///   - `status`：`"refreshed" | "skipped" | "failed"`（三档语义见下）；
///   - `count`：仅在 `refreshed` 时出现，落地后的条目数；
///   - `refreshedAt`：**这家清单当前的拉取时刻**（毫秒，0 = 从未成功过），
///     每行都有。取的是清单自身的时刻而不是「本次请求的时刻」—— 失败 / 跳过的
///     家清单没变，它的时间就该是上次成功那次（界面「更新日期」列读它）；
///   - `fixed`：仅在「这家不支持刷新」时出现且为 true —— **机器可识别的标记**，
///     前端据此把「能力边界」（固定清单，永远刷不出东西）与「本次没取到新内容」
///     分开说；按 `message` 文案匹配会在措辞调整后静默失效
///     （与 `USAGE_NOT_CONFIGURED_CODE` 同一取舍）；
///   - `message`：`skipped` / `failed` 时给用户看的原因。
///
/// ── 与 `refresh_implemented` 的三处差异（都是有意的）───────────
///   1. **`force = true`**：用户按下按钮的全部预期是「现在真的去拉一次」。
///      小浣熊的 TTL 早退会让「点了没反应、清单没变」，与功能坏掉无法区分。
///      缓存只该为后台自动路径服务，用户显式要求时一律绕过。
///      同理，**普通排期也一并跳过**（`manual` 开关，见 `catalog_refresh`）——
///      手动刷新可以提前，但仍受在途占位与失败冷却约束（那两条是上游限流，
///      不该由界面按钮解除）。
///   2. **只刷支持的家**：静态清单的家不去打那次必然白跑的网络请求。
///   3. **返回逐家结果**：自动路径失败只写日志（没有人在等它）；手动路径必须
///      把「哪家刷到了几个、哪家为什么没刷」交给界面 —— 一句笼统的「已刷新」
///      会让「其实失败了」和「其实跳过了」都显示成成功。
///      （自动路径现在同样返回逐家结果：定时任务要按它统计本轮摘要。）
///
/// ── `skipped` 与 `failed` 的区别（界面的文案完全依赖这个区分）──
///   - `skipped`：**这次没有可刷的东西，且不是错误**。两种来源：这家没有
///     远程目录（固定清单，带 `fixed: true`）、或这次刷新没有落地新内容
///     （上游返回的清单为空 / 没有可用登录态）。
///   - `failed`：**尝试了，但上游或本地出错了**（HTTP 非 2xx、网络错误、
///     解析失败）。`message` 里带原因，用户可能要排查（网络 / 凭证）。
///   把 `failed` 报成 `skipped` 会让真实故障静默；把 `skipped` 报成 `failed`
///   会让「这家本次没有新清单」这种正常事实变成一条红色错误。
///
/// 失败不抛错、逐家串行：一家的失败不影响其余家（`refresh_models` 契约本身就
/// 失败不返回错误），串行的理由与自动路径相同（provider 个位数、日志顺序稳定）。
///
/// `accounts` 是「这家用哪个账号去拉」的点名表（`{providerId: accountId}`，
/// 来自「获取模型」弹窗每行的「模型来源」下拉）：缺失或空串 = 该家按默认选取
/// （队首可用账号，判据同 [`AccountStore::current_entry_for_provider`]）。
/// 逐条结果里带上 `accountId`（本次**实际**用的账号，前端据此回读那一列 ——
/// 点名了就是它，没点名就是解析出的队首），供界面显示「这次用的是谁」。
///
/// `providers` 是**本次要刷的范围白名单**：`None` = 全部已实现的家（定时任务
/// 与不带范围的调用方）；`Some(list)` = 只刷名单内的家 —— 「获取模型」弹窗按
/// 「模型管理页实有清单的家 ∪ 有启用账号的家」收窄（见 `api/models.rs`）。
/// 名单外与名单为空的家**既不打网络、也不进结果**：对用户在界面上根本看不到的
/// 家（没有启用账号、清单也为空），刷它只会得到一行「缺少登录态」的噪音。
pub async fn refresh_implemented_forced(
    store: &AccountStore,
    accounts: &serde_json::Map<String, Value>,
    providers: Option<&[String]>,
) -> Vec<Value> {
    seed_current_workbuddy_defaults();
    seed_current_qoder_defaults();
    seed_current_cline_defaults();
    seed_hidden_visibility_defaults();
    let results = super::catalog_refresh::refresh(store, accounts, providers, true).await;
    seed_current_cline_defaults();
    // 手动刷新落地的新清单里可能第一次出现「不可见」的条目，种子要跟上
    seed_hidden_visibility_defaults();
    results
}

// ─── 占位适配器（W2b-T3 起三次演化，W6 删除）─────────────────
//
// W2b-T3 这里曾有一个 `PlaceholderAdapter`（构造请求返回 501）。
// W4a 之后「未实现的已注册 provider」成了真实情形（四家里两家是占位），它被
// 参数化成 `pending::PendingAdapter` 并**真正接进** `adapter_for` 的
// CatPaw / AutoClaw 分支（状态码也从 501 改成语义更准的 503，
// 并补上 `is_stateful` 的如实声明）。
// 两家先后接上真身（W5-T-d4 / W4b-T-c2）后，占位实现不再有任何生产调用点，
// W6 收尾时连文件一起删除 —— 现在加 provider 的过渡方案是：先在
// `adapter_for` 里给一个最小实现，而不是重新引入一层占位结构。
