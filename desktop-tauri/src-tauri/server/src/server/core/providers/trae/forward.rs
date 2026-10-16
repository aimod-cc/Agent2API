//! Trae 的转发（**有状态路径**：`is_stateful() = true`）。
//!
//! ── 为什么本家不能走通用无状态路径 ─────────────────────────
//! 无状态路径的形状是「通用层序列化 body 原样发出、响应按 OpenAI 的 SSE 逐行
//! **透传」**（`upstream::ForwardStream` → `sse::ReasoningCoalescer`，中间没有
//! 任何"按家翻译帧"的钩子 —— 全仓只有 `custom/forward.rs` 那一个协议翻译器，
//! 而它要的两把凭证（`InFlightGuard` / `ConnectionGuard`）不会交给适配器）。
//! Trae 的出站方向两头都不合这个假设：
//!
//!   * 请求体是自定义白名单信封（`payload::prepare_body`，多带字段是被拒而不是
//!     被忽略）；
//!   * 响应是另一套 SSE 方言：**没有 `[DONE]`**（结束帧是 `event: done`）、
//!     `token_usage` 是欠到下一帧的、`event: error` 之后上游**还会继续发帧**。
//!
//! 所以本家与 Accio / Qoder 同一处置：实现 `forward_conversation`，自己发请求、
//! 自己翻帧、自己产出对客户端可见的字节。
//!
//! ── 首包门（本家最要紧的一段）──────────────────────────────
//! 上游的额度/凭证类失败**不带 HTTP 状态**：HTTP 200 + SSE 里一条
//! `event: error`（1005 / 4008 / 4001…）。不拦的话客户端拿到的是"成功但没话",
//! 而网关这边一次失败连账号都不换。做法是先把流头预读出来（`prefetch_head`）：
//! 第一帧是错误就**在这里**判成带状态的 `GatewayError` 返回，一个字节都不下发；
//! 第一帧是内容才交回 `ForwardOutcome::Stream`。Accio / Qoder 同一手法，
//! 而 CPA 那边是插件自己做过一遍的同类修正。
//!
//! 状态码映射表（本家特有，别家照抄会错）：
//!   `SessionDead` → 401、`SoftRate` / `PlanLimit` → 429、`InputTooLarge` → 413、
//!   `ModelUnavailable` → 404、其余 → 502。
//! 为什么 1005/4008 在 agent2api 这边记 429 而不是参考实现内部那 12 小时冷却：
//! 429 是宿主唯一认识的「换账号」信号（有状态路径不自动换号，见下），把冷却
//! 时长留在自己手里更可控 —— 落盘标记走 `mark_rate_limited`，与另外几家同源。
//!
//! ── 有状态路径**不会**换账号重试（必须自己续期）──────────────
//! `provider_loop::attempt_stateful` 对错误是「一律透传」：不换号、不冷却、
//! 不重试。所以两件别家由编排层代劳的事在这里自己做：
//!   ① 临期主动续期（发送前）；② 真被拒 401 时**强制续一次再发一次**。
//! 第二条不能省：Trae 的 refreshToken 一次一换，被拒时手里那份 accessToken
//! 多半是"服务端已吊销"而不是"过期"，不重发就直接把一次可救的调用报成失败。
//!
//! ── 「没等到 `event: done`」到底算不算失败（与参考实现有意不同）──
//! 参考实现把「流结束了但没给 `done`」当**正常形状**（向量
//! `上游没给 done（EOF 兜底 [DONE]）` / `usage 之后没有 done` 钉的就是它），
//! 本家此前照抄。代价在 2026-10-15 的上游 issue #286 上露了出来：上游把连接撑断
//! 时，那份前缀在客户端看起来是一次**正常完成**（200 / 错误列为空 / 一次都不重试），
//! 连自动续写都接不住（判据要的 `finish_reason` 被兜成了 `stop`）。
//! 现在的口径：**产出过内容却没等到 `done`** 时，流式在 `[DONE]` 之前补一帧错误、
//! 非流式直接 502。转换层（`stream.rs` 的 `SoloStream` / `aggregate`）仍然只
//! 如实报告事实，回包形状一个字不改 —— 那两条向量因此一条都没动。
//! 判据与开关见 `missing_terminal` / `require_terminal`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap / expect / panic；网络不在锁内。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, stall};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::Credential;
use super::errors::ErrorKind;
use super::headers::{IDE_VERSION, solo_headers};
use super::payload;
use super::stream::{Frame, SoloStream};
use super::{AGENT_BASE_URL, CHAT_PATH, PROVIDER_ID};

/// 流式的读空闲上限（与其它几家同一个配置源，不是本家自己拍的数）。
fn stream_idle() -> Duration {
    Duration::from_millis(config::timeout_settings().stream_idle_ms())
}

/// 一次出站请求（URL + 头 + 已改写的 body）。
pub struct Plan {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// 客户端要的那个模型名（回显给客户端用；上游收的裸名在 `body` 里）。
    pub requested_model: String,
    pub body: Value,
}

/// 构造出站请求。
///
/// `resolved` 传空：**广告名带 `-solo` 后缀，出站名要剥回来**，这件事
/// `prepare_body` 内部就做（它只剥一层，所以真以 `-solo` 结尾的 config 仍往返）。
pub fn build_plan(credential: &Credential, body: &Value, base: &str) -> Result<Plan, GatewayError> {
    if credential.access_token.trim().is_empty() {
        return Err(GatewayError::with_status(401, "Trae 账号缺少 accessToken，无法转发（请重新登录）"));
    }
    // 谱系闸门（**转发侧也放一份**，不只是落账号时）：账号可能是从迁移导入或
    // 手工写库进来的，那条路不经过 `add_trae_account`。Intl 凭据打到 CN host
    // 只会收到一句没头没尾的 401，与其让人猜，不如在这里说清楚。
    if super::credentials::is_intl_variant(credential.variant()) {
        return Err(GatewayError::with_status(400, super::credentials::INTL_UNSUPPORTED));
    }
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let identity = super::headers::HeaderIdentity {
        access_token: credential.access_token.trim(),
        uid: credential.uid.trim(),
        machine_id: credential.machine_id.trim(),
        device_id: credential.device_id.trim(),
    };
    // 上游恒 `stream:true`（`prepare_body` 写死），Accept 因此恒 event-stream。
    let headers = solo_headers(&identity, true).into_iter().collect();
    Ok(Plan {
        url: format!("{}{CHAT_PATH}", base.trim_end_matches('/')),
        headers,
        requested_model: requested,
        body: payload::prepare_body(body, credential.variant(), ""),
    })
}

/// 发一次请求（**不设总超时**：长推理会被总超时截断，读空闲由 `idle_guard` 兜）。
async fn send(plan: &Plan, proxy: Option<&ResolvedProxy>) -> Result<reqwest::Response, GatewayError> {
    // **不设 reqwest 总超时**：`timeout(0)` 不是"不限时"而是"立刻超时"
    // （实测报 operation timed out，症状是随机 502），而不带总超时的长流
    // 才符合本家的形状 —— 读空闲由下面的 `idle_guard` 管，整流时长不该被截。
    let mut request = egress::client_for(proxy)
        .post(&plan.url)
        .header("User-Agent", format!("Trae/{IDE_VERSION} antigravity-cockpit-tools"));
    for (name, value) in &plan.headers {
        request = request.header(name.as_str(), value.clone());
    }
    let response = request
        .json(&plan.body)
        .send()
        .await
        .map_err(|error| GatewayError::with_status(502, format!("Trae 请求发不出去：{}", egress::describe_error_detail(&error))))?;
    Ok(response)
}

/// 限额标记的上下文（有状态路径不换号，所以冷却要自己记）。
pub struct Limit {
    pub store: Arc<AccountStore>,
    pub account_id: String,
    pub model: String,
}

impl Limit {
    /// 按错误类别落冷却/禁用标记。返回的是**给客户端的状态码**。
    fn punish(&self, kind: ErrorKind, status: u16, message: &str) -> u16 {
        if self.account_id.is_empty() {
            return status;
        }
        match kind {
            // 软限流与计划限额：冷却这个账号（上游不给恢复时刻，存储层落兜底时长）
            ErrorKind::SoftRate | ErrorKind::PlanLimit => {
                self.store.mark_rate_limited(&self.account_id, &self.model, i64::from(status), None, None, message);
                logging::log("[Trae]", &format!("⚠️ 账号 {} 对模型 {} 已限额（{status}），进入冷却", self.account_id, self.model));
            }
            // 会话失效：上游明说这串凭据不行了，落同样的标记让它下一轮被跳过
            ErrorKind::SessionDead => {
                self.store.mark_rate_limited(&self.account_id, &self.model, i64::from(status), None, None, message);
                logging::log("[Trae]", &format!("⚠️ 账号 {} 会话失效（{status}），请重新登录", self.account_id));
            }
            // 过大 / 通道不可用 / 其它：**请求级**，一个字都不写账号库
            _ => {}
        }
        status
    }
}

/// 错误类别 → 客户端可见状态码（映射表见模块头）。
///
/// `pub(crate)` 给 `usage.rs` 复用：积分/额度那条链的 401/429 判档与转发用的是
/// **同一张表**（`errors::classify` 的产出），两处各写一份的话，"余额查询报 401
/// 时上层该不该刷新重试"就会和转发侧悄悄不一致。
pub(crate) fn status_for(kind: ErrorKind) -> u16 {
    match kind {
        ErrorKind::SessionDead => 401,
        ErrorKind::SoftRate | ErrorKind::PlanLimit => 429,
        ErrorKind::InputTooLarge => 413,
        ErrorKind::ModelUnavailable => 404,
        ErrorKind::NotFound => 404,
        _ => 502,
    }
}

/// 非 2xx 响应 → 网关错误（错误体只读一次，读完就丢）。
async fn http_error(status: u16, response: reqwest::Response, limit: &Limit) -> GatewayError {
    let text = response.text().await.unwrap_or_default();
    let head: String = text.chars().take(240).collect();
    let kind = super::errors::classify(status, &head);
    let message = format!("上游返回 {status}: {head}");
    let status = limit.punish(kind, status_for(kind), &message);
    GatewayError::with_status(i32::from(status), if head.is_empty() { format!("Trae 上游返回 HTTP {status}") } else { message })
}

/// 流内错误帧 → 网关错误（首包门专用：此时一个字节都还没下发）。
fn frame_error(code: i64, message: &str, limit: &Limit) -> GatewayError {
    let kind = super::errors::stream_error_kind(code, message);
    let text = format!("solo error code={code} msg={message}");
    let status = limit.punish(kind, status_for(kind), &text);
    GatewayError::with_status(i32::from(status), format!("Trae 上游错误：{text}")).with_code(PROVIDER_ID)
}

/// 预读流头：返回「已读出的内容帧」+「剩下的流」。第一帧是错误就直接 Err。
///
/// 半行换手与 accio 同一处置：缓冲区里没成行的尾巴必须拼回流头，否则被切在
/// chunk 边界的那一帧会静默丢内容（这是本文件最难查的一类 bug，症状是"偶尔
/// 少开头几个字"）。
pub(crate) async fn prefetch_head(
    response: reqwest::Response,
    limit: &Limit,
) -> Result<(Vec<Frame>, futures::stream::BoxStream<'static, Result<bytes::Bytes, io::Error>>, SoloStream), GatewayError> {
    let source = response.bytes_stream().map(|item| item.map_err(|error| io::Error::other(egress::describe_error_detail(&error))));
    let mut source = stall::idle_guard(Box::pin(source), stream_idle());
    let mut scanner = ByteLines::default();
    // `id` / `created` 从这里开始就得有值：预读到的帧要原样补发给客户端。
    let mut stream = SoloStream::new(request_id(), logging::now_ms() / 1000);
    let mut frames: Vec<Frame> = Vec::new();
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| GatewayError::with_status(502, format!("Trae 上游流式传输中断: {error}")))?;
        // ★ 一整段 chunk 处理完才返回 —— 中途 return 会把**同一段里后面的行**
        //   连同字节一起丢掉（那些行已经从 socket 读进来了，换手也换不回来）。
        //   上游经常把 `output`+`token_usage`+`done` 挤在同一段里，所以这不是
        //   理论问题：症状是"用量恒为 0"和"error 帧凭空消失"。
        for line in scanner.push(&chunk) {
            for frame in stream.feed_line(&line) {
                if let Frame::Error { code, message, .. } = &frame {
                    // 门只拦"第一帧就是错误"：内容已经出去过之后再报错，
                    // HTTP 200 已经发出去了，只能留在流里如实告诉客户端。
                    if frames.is_empty() {
                        return Err(frame_error(*code, message, limit));
                    }
                }
                frames.push(frame);
            }
        }
        if !frames.is_empty() {
            // 半行换手：缓冲区里还没成行的尾巴要拼回流头（见 `take_pending`）——
            // 不做这一步，被切在 chunk 边界的那一帧会静默丢内容。
            let leftover = scanner.take_pending();
            let rest: futures::stream::BoxStream<'static, Result<bytes::Bytes, io::Error>> = if leftover.is_empty() {
                source
            } else {
                Box::pin(futures::stream::once(async move { Ok(bytes::Bytes::from(leftover)) }).chain(source))
            };
            return Ok((frames, rest, stream));
        }
    }
    // 流在没有任何内容帧之前就结束了：交给下游按「空回答」收尾（不假装成功
    // 也不报错 —— 真异常在 HTTP 状态那一层已经判过了）。
    Ok((frames, source, stream))
}

/// 逐行切分（**按字节缓冲**，只在完整行上解码 UTF-8）。
///
/// 为什么不直接把 chunk 转成 String 再切：一个多字节字符被切在 chunk 边界时
/// `from_utf8_lossy` 会就地吐出 U+FFFD，中文回答就会偶发变成乱码 —— 本家的
/// 上游正文里中文占比很高，这不是理论问题。
#[derive(Default)]
pub struct ByteLines {
    pending: Vec<u8>,
}

impl ByteLines {
    /// 喂一段字节，返回其中**完整**的行（不含结尾换行）。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        self.pending.extend_from_slice(chunk);
        while let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=index).collect();
            let raw = raw[..raw.len() - 1].to_vec(); // 去掉 '\n'
            let raw = if raw.last() == Some(&b'\r') { raw[..raw.len() - 1].to_vec() } else { raw };
            lines.push(String::from_utf8_lossy(&raw).into_owned());
        }
        lines
    }

    /// 还没成行的尾巴（换手用）。
    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

/// 把一帧写成对客户端可见的 SSE 字节。
///
/// `model` 在这里改写成**客户端请求的那个名字**：上游帧里 `model` 恒为空串
/// （参考实现如此，本家的 `SoloStream` 逐字节对齐它也如此），而 OpenAI 客户端
/// 拿空 model 会显示成"未知模型"。这与 `sse_model_rewrite()` 无关 —— 那条是
/// 通用透传路径的钩子，本家自己发字节，所以自己改（与 Qoder / Accio 同一处置）。
fn render(frame: &Frame, requested: &str) -> Option<String> {
    match frame {
        Frame::Chunk(chunk) => {
            let mut chunk = chunk.clone();
            if !requested.is_empty() {
                if let Some(object) = chunk.as_object_mut() {
                    object.insert("model".to_string(), Value::String(requested.to_string()));
                }
            }
            Some(format!("data: {chunk}\n\n"))
        }
        Frame::Error { code, message, .. } => Some(format!("data: {}\n\n", json!({"error": {"message": format!("solo error code={code} msg={message}"), "type": "upstream_error"}}))),
        Frame::Done => Some("data: [DONE]\n\n".to_string()),
    }
}

fn request_id() -> String {
    format!("chatcmpl-{}", logging::now_ms() * 1000 % 1_000_000_000_000_000)
}

/// 用一条凭据打一次上游并产出对客户端可见的 outcome（有状态路径的入口）。
/// 生产入口：打默认的上游主机。
pub(crate) async fn forward(
    store: &AccountStore,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    forward_at(store, account_id, body, proxy, stream, telemetry, AGENT_BASE_URL).await
}

/// 同上，但上游基址由调用方给。
///
/// 这一层参数**不是**为了好看：状态机（首包门、半行换手、usage 欠一帧、
/// error 之后继续读）只有在真 HTTP 上才验得到，而测试不能拿真上游去试 ——
/// Trae 每一次聊天都烧账号额度。基址做成入参（而不是读环境变量）是为了
/// 并发测试之间不互相踩：`std::env::set_var` 在同一进程里是全局可见的。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_at(
    store: &AccountStore,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream: bool,
    telemetry: &Arc<RequestTelemetry>,
    base: &str,
) -> Result<ForwardOutcome, GatewayError> {
    if account_id.is_empty() {
        return Err(GatewayError::with_status(503, "没有可用的 Trae 账号：请在账号页添加并启用账号"));
    }
    let record = store.trae_account_record(account_id).ok_or_else(|| GatewayError::with_status(401, "找不到该 Trae 账号"))?;
    let mut credential = Credential::from_payload(&record).map_err(GatewayError::new)?;
    credential = super::adapter::renew_if_due(store, &record, &credential, proxy.as_ref()).await?;
    let limit = Limit { store: Arc::new(store.clone()), account_id: account_id.to_string(), model: body.get("model").and_then(Value::as_str).unwrap_or("").to_string() };
    let account_proxy = super::adapter::account_proxy(&record)?;
    let effective = proxy.or(account_proxy);

    let mut plan = build_plan(&credential, body, base)?;
    let capture = telemetry.capture();
    if let Some(capture) = capture.as_deref() {
        capture.reset_request(&plan.url, PROVIDER_ID, &plan.headers, body);
    }
    let mut response = send(&plan, effective.as_ref()).await?;
    // 401 只救一次：强制换发后重发（这条路径不换号，见模块头）。
    if response.status().as_u16() == 401 {
        let _ = response.text().await;
        credential = super::adapter::renew_forced(store, &record, &credential, effective.as_ref()).await?;
        plan = build_plan(&credential, body, base)?;
        response = send(&plan, effective.as_ref()).await?;
    }
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(http_error(status, response, &limit).await);
    }
    if let Some(capture) = capture.as_deref() {
        capture.attach_response(status, response.headers());
    }

    if !stream {
        // 上游恒流式，因此非流式是"我们这边聚合"（与 Accio 同结构）。
        return drive_aggregate(response, plan.requested_model.clone(), telemetry, &limit).await;
    }

    let (prefetched, source, mut solo) = prefetch_head(response, &limit).await?;
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, io::Error>>(64);
    let telemetry = telemetry.clone();
    let requested = plan.requested_model.clone();
    crate::spawn_task(async move {
        drive_stream(source, &mut solo, prefetched, &requested, &telemetry, &sender).await;
    });
    Ok(ForwardOutcome::Stream { status: 200, stream: Box::new(tokio_stream::wrappers::ReceiverStream::new(receiver)) })
}

/// 「上游给了内容、却始终没给完成事件」的对客户端文案。
///
/// 流式的错误帧与非流式的 502 用**同一句** —— 同一个成因不该在两条路上有两个说法。
const MISSING_TERMINAL: &str = "Trae 上游未给出完成事件（event: done）即结束，本轮回答可能被截断";

/// 这一轮是不是「没等到上游收场就结束了」。
///
/// 为什么需要这条判据：上游的结束帧是 `event: done`，而「流结束了」与
/// 「上游说完成了」是**两件事**，此前被压成一件（`SoloStream::finish` 只补
/// `[DONE]`、`aggregate` 的 `finish_reason` 直接兜 `stop`），于是截断在客户端
/// 看起来是一次完全正常的完成 —— 2026-10-15 的上游 issue #286 就是它：trae 的
/// `deepseek-v4.1-flash-solo` 推理失控把上游连接撑断，报告人 13/15 次只收到前缀，
/// 而网关侧 status=200、错误列为空、一次重试都没有。
///
/// 判据只要两半：**产出过内容**（`saw_output`）且**上游没给过完成事件**
/// （`!saw_done`）。两个例外刻意排除在外：
///   · 一句不吐就结束 → 归既有的空回答兜底链，不是这条要管的；
///   · `event: error` 之后结束 → `saw_done` 同样为真（错误已经如实下发过了）。
fn missing_terminal(saw_done: bool, saw_output: bool, require: bool) -> bool {
    require && !saw_done && saw_output
}

/// [`missing_terminal`] 的开关：`AGENT2API_TRAE_NO_REQUIRE_TERMINAL=1` 关掉这条
/// 判据（读一次定死）。
///
/// 为什么要留一格：参考实现的向量把「没给 done 也结束」当正常形状，也就是这条
/// 判据万一在某个我们没见过的上游形状上误伤，兜底不该依赖发版
/// （与 `zcode::captcha` 的 `ZCODE_REQUIRE_CAPTCHA` 同一取向：读一次定死，
/// 部署方能一行环境变量把行为切回去）。
fn require_terminal() -> bool {
    static REQUIRE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REQUIRE.get_or_init(|| require_terminal_from(std::env::var("AGENT2API_TRAE_NO_REQUIRE_TERMINAL").ok().as_deref()))
}

/// 开关的判据本体。拆开只为能测：`std::env::set_var` 在同一进程里全局可见，
/// 用例里改它会污染并发跑的其它用例（与 `forward_at` 把上游基址做成入参、
/// 而不是读环境变量，是同一个理由）。
fn require_terminal_from(value: Option<&str>) -> bool {
    !matches!(
        value.map(|raw| raw.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// 流式下发（在 spawn 的后台任务里跑）。
async fn drive_stream(
    mut source: futures::stream::BoxStream<'static, Result<bytes::Bytes, io::Error>>,
    solo: &mut SoloStream,
    prefetched: Vec<Frame>,
    requested: &str,
    telemetry: &RequestTelemetry,
    sender: &tokio::sync::mpsc::Sender<Result<bytes::Bytes, io::Error>>,
) {
    let mut scanner = ByteLines::default();
    let mut usage_seen: Option<Value> = None;
    for frame in prefetched {
        note(&frame, telemetry, &mut usage_seen);
        if let Some(text) = render(&frame, requested) {
            if sender.send(Ok(bytes::Bytes::from(text))).await.is_err() {
                return;
            }
        }
    }
    while let Some(item) = source.next().await {
        match item {
            Err(error) => {
                telemetry.note_error(&format!("Trae 上游流中断: {error}"));
                let _ = sender.send(Err(error)).await;
                return;
            }
            Ok(chunk) => {
                for line in scanner.push(&chunk) {
                    for frame in solo.feed_line(&line) {
                        note(&frame, telemetry, &mut usage_seen);
                        if let Some(text) = render(&frame, requested) {
                            if sender.send(Ok(bytes::Bytes::from(text))).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
    // EOF 收尾：上游没给过 `done` 时由 `finish()` 补那一个 `[DONE]`。
    // 这里**不再自己添一个** —— `[DONE]` 必须恰好一个，而 `SoloStream` 已经
    // 保证「done / error / EOF 三条路各发一次且只一次」。
    //
    // 但「补 `[DONE]` 让客户端能收尾」不等于「上游说完成了」（见 `missing_terminal`）：
    // 产出过内容却没等到 `event: done` 时，先补一帧错误（形状与流内错误一致）
    // 再补 `[DONE]` —— 顺序不能反，客户端是拿 `[DONE]` 收尾的。
    if missing_terminal(solo.saw_done(), solo.saw_output(), require_terminal()) {
        telemetry.note_error(MISSING_TERMINAL);
        let frame = bytes::Bytes::from(format!("data: {}\n\n", json!({"error": {"message": MISSING_TERMINAL, "type": "upstream_error"}})));
        if sender.send(Ok(frame)).await.is_err() {
            return;
        }
    }
    for frame in solo.finish() {
        note(&frame, telemetry, &mut usage_seen);
        if let Some(text) = render(&frame, requested) {
            if sender.send(Ok(bytes::Bytes::from(text))).await.is_err() {
                return;
            }
        }
    }
    let _ = &usage_seen;
}

/// 一帧的旁读（首帧时刻与用量上报），不改字节。
fn note(frame: &Frame, telemetry: &RequestTelemetry, usage_seen: &mut Option<Value>) {
    match frame {
        Frame::Chunk(chunk) => {
            telemetry.note_first_frame();
            // 上游的 `token_usage` 事件载荷本身是 `{"usage": {…}}`，而 `SoloStream`
            // 把它整包挂到下一帧的 `usage` 键上（与参考实现逐字节一致），所以这里
            // 取**内层**才对得上 `extract_usage` 的那三把键 —— 报外层的后果是
            // 账目静默为 0（`extract_usage` 找不到 `prompt_tokens` 就整个返回 None）。
            let inner = chunk.get("usage").and_then(|value| value.get("usage"));
            let usage = inner.or_else(|| chunk.get("usage")).or_else(|| chunk.get("token_usage"));
            if let Some(usage) = usage {
                *usage_seen = Some(usage.clone());
                telemetry.report_usage(usage);
            }
        }
        Frame::Error { code, message, .. } => {
            telemetry.note_error(&format!("solo error code={code} msg={message}"));
        }
        Frame::Done => {}
    }
}

/// 非流式：内部仍按流式拉全，再折成一个 `chat.completion`。
async fn drive_aggregate(
    response: reqwest::Response,
    requested: String,
    telemetry: &RequestTelemetry,
    limit: &Limit,
) -> Result<ForwardOutcome, GatewayError> {
    let source = response.bytes_stream().map(|item| item.map_err(|error| io::Error::other(egress::describe_error_detail(&error))));
    let mut source = stall::idle_guard(Box::pin(source), stream_idle());
    let mut scanner = ByteLines::default();
    let mut text = String::new();
    while let Some(item) = source.next().await {
        let chunk = item.map_err(|error| GatewayError::with_status(502, format!("Trae 上游流式传输中断: {error}")))?;
        for line in scanner.push(&chunk) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    match super::stream::aggregate(&text, &request_id(), logging::now_ms() / 1000) {
        Ok(aggregated) => {
            // 非流式这条能**直接失败**：内容一个字节都还没发给客户端，而一份带前缀的
            // 200 比一次明确的失败坏得多（客户端会把它当成完整答案，issue #286 里
            // 报告人测到的正是它：非流式 12/15 截断、状态 200）。
            if missing_terminal(aggregated.saw_done, aggregated.saw_output, require_terminal()) {
                telemetry.note_error(MISSING_TERMINAL);
                return Err(GatewayError::with_status(502, MISSING_TERMINAL));
            }
            let mut completion = aggregated.body;
            if !requested.is_empty() {
                if let Some(object) = completion.as_object_mut() {
                    object.insert("model".to_string(), Value::String(requested));
                }
            }
            if let Some(usage) = completion.get("usage") {
                telemetry.report_usage(usage);
            }
            Ok(ForwardOutcome::Completion { body: completion })
        }
        // 流内错误：与流式同一张状态码表（这里换号是没用的，但**必须**带状态，
        // 否则客户端拿到的是 200 + 空内容 —— 参考实现摔过的正是这一种）。
        Err(error) => Err(frame_error(error.code, &error.message, limit)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::account_store::AccountStore;

    /// 假上游：把脚本里的**字节段**原样吐成一个 SSE 流。
    ///
    /// 分段是这套测试的全部意义所在 —— 上游会在任意位置切一个 chunk（切在
    /// 行中间、甚至切在一个汉字的三个字节中间），翻译层必须在这些情况下都
    /// 不丢内容。整段一次喂进去的测试只能证明"逻辑没写反"，证明不了这件事。
    struct FakeUpstream {
        base: String,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl FakeUpstream {
        async fn spawn(chunks: Vec<Vec<u8>>) -> Self {
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let captured = seen.clone();
            let app = axum::Router::new().route(
                "/api/agent/v3/llm_utils_chat",
                axum::routing::post(move |req: axum::extract::Request| {
                    let captured = captured.clone();
                    let chunks = chunks.clone();
                    async move {
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        captured.lock().unwrap_or_else(|error| error.into_inner()).push(format!("{method} {path}"));
                        let stream = futures::stream::iter(chunks.into_iter().map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::from(chunk))));
                        axum::response::Response::builder()
                            .status(200)
                            .header("content-type", "text/event-stream")
                            .body(axum::body::Body::from_stream(stream))
                            .unwrap()
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("假上游监听");
            let address = listener.local_addr().expect("本地地址");
            tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
            Self { base: format!("http://{address}"), seen }
        }

        fn path_seen(&self) -> String {
            self.seen.lock().unwrap_or_else(|error| error.into_inner()).first().cloned().unwrap_or_default()
        }
    }

    /// 一个只装本家账号的账号库（真 SQLite，临时文件）。
    /// 第二个返回值是**删文件的守卫**，调用点必须一起解构（见 `test_temp::TempDb`）。
    fn store_with_account(token: &str) -> (AccountStore, crate::server::db::test_temp::TempDb) {
        let (db, guard) = crate::server::db::test_temp::TempDb::open(&format!("trae-forward-{token}"));
        let store = AccountStore::with_db(Some(db));
        store
            .add_trae_account(
                &Credential {
                    access_token: token.to_string(),
                    refresh_token: String::new(),
                    expires_at: 0,
                    api_host: "https://api.trae.cn".into(),
                    domain: "trae.cn".into(),
                    machine_id: "m-1".into(),
                    device_id: "1234567890123456".into(),
                    variant: "solo".into(),
                    uid: "u-1".into(),
                    nickname: "测试号".into(),
                    ..Default::default()
                },
                None,
                "web",
            )
            .expect("账号要能落库");
        (store, guard)
    }

    fn account_id(store: &AccountStore) -> String {
        let list = store.list_accounts();
        list.get("accounts")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn body(model: &str) -> Value {
        json!({"model": model, "messages": [{"role": "user", "content": "你好"}]})
    }

    /// 把 outcome 里的流读干，返回客户端可见的完整文本。
    async fn drain(outcome: ForwardOutcome) -> String {
        let ForwardOutcome::Stream { stream, .. } = outcome else {
            panic!("流式路径应当给 Stream");
        };
        let mut stream = stream;
        let mut out = String::new();
        while let Some(item) = stream.next().await {
            out.push_str(&String::from_utf8_lossy(&item.expect("帧下发不该失败")));
        }
        out
    }

    /// 把若干「事件」拼成上游方言的一条流（每个事件都是 `event:` + `data:` + 空行）。
    fn sse(events: &[(&str, &str)]) -> Vec<Vec<u8>> {
        vec![events.iter().map(|(event, data)| format!("event: {event}\ndata: {data}\n\n")).collect::<String>().into_bytes()]
    }

    #[test]
    fn an_intl_credential_is_refused_before_anything_is_sent() {
        // 这条守的是"导入进来的 Intl 凭据"那条路（不经过 add_trae_account）。
        // 症状本会是 CN host 回一句 401，用户完全看不出是自己谱系选错了。
        // 判的是 `Credential::variant()` 归一**之后**的谱系：未知值（含大写 INTL）
        // 本来就兜到 solo，所以这里只列真实会被写进凭据的那两个 Intl 值。
        for variant in ["intl", "solo-intl"] {
            let error = build_plan(
                &Credential { access_token: "JWT".into(), variant: variant.into(), ..Default::default() },
                &json!({"model": "glm-5.2-solo", "messages": []}),
                "https://example.invalid",
            )
            .err()
            .unwrap_or_else(|| panic!("Intl 谱系（{variant}）不该被路由"));
            assert_eq!(400, error.status_code, "variant={variant}");
            assert!(error.message.contains("国际版"), "文案要说明为什么：{}", error.message);
        }
        // 国内两个谱系在转发面等价（都发 solo_work_lite），不能一起拒了
        for variant in ["solo", "cn", ""] {
            assert!(
                build_plan(
                    &Credential { access_token: "JWT".into(), variant: variant.into(), ..Default::default() },
                    &json!({"model": "glm-5.2-solo", "messages": []}),
                    "https://example.invalid",
                )
                .is_ok(),
                "variant={variant} 应放行"
            );
        }
    }

    #[tokio::test]
    async fn the_stream_reaches_the_client_whole_even_when_upstream_splits_mid_character() {
        let (store, _db) = store_with_account("FW-SPLIT");
        let id = account_id(&store);
        // 「你好」= E4 BD A0 / E5 A5 BD：把第二个汉字的三个字节劈开，
        // 再在行中间劈一刀（`data: {"choices"` | `:[…]}\n\n`）。
        let payload = "event: output\ndata: {\"response\":\"你好\"}\n\n";
        let bytes = payload.as_bytes();
        let cut = payload.find("你").unwrap() + 2; // 劈在「好」的第一个字节后
        let chunks = vec![
            bytes[..30].to_vec(),
            bytes[30..cut].to_vec(),
            bytes[cut..].to_vec(),
            "event: done\ndata: {\"finish_reason\":\"stop\"}\n\n".as_bytes().to_vec(),
        ];
        let upstream = FakeUpstream::spawn(chunks).await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, true, &telemetry, &upstream.base)
            .await
            .expect("流式转发应当成功");
        let text = drain(outcome).await;
        assert!(text.contains("\"content\":\"你好\""), "内容被切碎的字节必须拼回来：{text}");
        assert!(!text.contains('\u{FFFD}'), "不该出现替换字符（那就是按字节解码错了）：{text}");
        assert!(text.contains("\"model\":\"glm-5.2-solo\""), "客户端要看见它请求的那个名字：{text}");
        assert_eq!(1, text.matches("data: [DONE]").count(), "结束标记必须恰好一个（多了客户端会当成两段回答，少了会一直等）：{text}");
        assert!(upstream.path_seen().ends_with(CHAT_PATH), "打的是本家的聊天路径：{}", upstream.path_seen());
    }

    #[tokio::test]
    async fn the_head_gate_fails_the_request_before_any_bytes() {
        // 上游给的是一次额度错误，但 HTTP 是 200、错误在帧里（1005/4008/4001 都这样）。
        for (code, want) in [(4008_i64, 429_u16), (4001, 404), (20101, 502)] {
            let (store, _db) = store_with_account(&format!("FW-ERR{code}"));
            let id = account_id(&store);
            let upstream = FakeUpstream::spawn(sse(&[(
                "error",
                &format!("{{\"code\":{code},\"message\":\"上游文案 {code}\"}}"),
            )]))
            .await;
            let telemetry = Arc::new(RequestTelemetry::new());
            let error = match forward_at(&store, &id, &body("glm-5.2-solo"), None, true, &telemetry, &upstream.base).await {
                Err(error) => error,
                //  outcome 成功 = 首包门没拦住，那个错误就会以 200 + 半截流的形式
                //  发到客户端脸上，所以这里必须 panic 而不是放宽。
                Ok(_) => panic!("码 {code} 必须被首包门拦成错误，不能下发任何字节"),
            };
            assert_eq!(i32::from(want), error.status_code, "码 {code} 的状态映射不对：{}", error.message);
            assert!(error.message.contains(&code.to_string()), "文案要带上游码：{}", error.message);
        }
    }

    #[tokio::test]
    async fn an_error_after_content_stays_in_band_and_the_stream_still_ends() {
        let (store, _db) = store_with_account("FW-MID");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[
            ("output", "{\"response\":\"先说一句\"}"),
            ("error", "{\"code\":4008,\"message\":\"额度耗尽\"}"),
        ]))
        .await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, true, &telemetry, &upstream.base)
            .await
            .expect("内容已经出去，HTTP 只能 200");
        let ForwardOutcome::Stream { status, .. } = &outcome else { panic!("流式") };
        assert_eq!(200, *status);
        let text = drain(outcome).await;
        assert!(text.contains("先说一句"), "{text}");
        assert!(text.contains("\"error\""), "错误要原样进流里：{text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "尾巴必须收住：{text}");
        assert!(telemetry.snapshot().error.is_some(), "错误要进遥测（请求日志的「错误」列读它）");
    }

    #[tokio::test]
    async fn token_usage_is_reported_once_and_rides_the_next_frame() {
        let (store, _db) = store_with_account("FW-USAGE");
        let id = account_id(&store);
        // 上游的 `token_usage` 不立刻出帧：它欠到下一帧（参考实现如此），
        // 所以用量是随 finish 帧一起到的 —— 计费必须仍然读到它。
        let upstream = FakeUpstream::spawn(sse(&[
            ("output", "{\"response\":\"答\"}"),
            ("token_usage", "{\"prompt_tokens\":7,\"completion_tokens\":5,\"total_tokens\":12}"),
            ("done", "{\"finish_reason\":\"stop\"}"),
        ]))
        .await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("kimi-k2.6-solo"), None, true, &telemetry, &upstream.base)
            .await
            .expect("转发应当成功");
        let text = drain(outcome).await;
        let snapshot = telemetry.snapshot();
        assert_eq!(Some(7), (snapshot.prompt_tokens > 0).then_some(snapshot.prompt_tokens), "用量没记进遥测：{snapshot:?}");
        assert_eq!(5, snapshot.completion_tokens);
        assert_eq!(12, snapshot.total_tokens);
        assert!(text.contains("\"usage\""), "客户端也要拿到那一帧 usage：{text}");
    }

    /// 上游给了内容、却**不给完成事件**就结束 —— 上游把连接撑断时的形状
    /// （issue #286：trae 的 `deepseek-v4.1-flash-solo` 推理失控）。
    #[tokio::test]
    async fn a_stream_that_ends_without_done_is_flagged_in_band() {
        let (store, _db) = store_with_account("FW-NODONE");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[("output", "{\"response\":\"半句\"}")])).await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, true, &telemetry, &upstream.base)
            .await
            .expect("内容已经出去了，HTTP 只能是 200");
        let text = drain(outcome).await;
        assert!(text.contains("半句"), "内容照常下发：{text}");
        assert!(text.contains("\"type\":\"upstream_error\""), "截断要能在流里看出来：{text}");
        assert!(text.contains("完成事件"), "文案要点出是没等到完成事件：{text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "错误帧在 [DONE] 之前，且尾巴照常收住：{text}");
        assert_eq!(1, text.matches("data: [DONE]").count(), "[DONE] 仍然只有一个：{text}");
        assert!(telemetry.snapshot().error.is_some(), "错误要进遥测（请求日志的「错误」列读它）");
    }

    /// 正对照：上游给了 `done` 就一个字节都不多（这条守的是误伤）。
    #[tokio::test]
    async fn a_stream_that_ends_with_done_is_untouched() {
        let (store, _db) = store_with_account("FW-HASDONE");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[
            ("output", "{\"response\":\"完整回答\"}"),
            ("done", "{\"finish_reason\":\"stop\"}"),
        ]))
        .await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, true, &telemetry, &upstream.base)
            .await
            .expect("转发应当成功");
        let text = drain(outcome).await;
        assert!(!text.contains("upstream_error"), "收过场的一轮不许补错误帧：{text}");
        assert!(text.contains("\"finish_reason\":\"stop\""), "收尾帧照常：{text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "{text}");
        assert!(telemetry.snapshot().error.is_none(), "这一轮不该有任何错误留痕");
    }

    /// 非流式：没等到完成事件时**直接失败**，不给「200 + 前缀」那份假成功
    /// （issue #286 里报告人测到的就是它：非流式 12/15 截断、状态 200）。
    #[tokio::test]
    async fn an_aggregate_that_ends_without_done_fails_instead_of_a_prefix() {
        let (store, _db) = store_with_account("FW-AGGNODONE");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[("output", "{\"response\":\"半句\"}")])).await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let error = match forward_at(&store, &id, &body("glm-5.2-solo"), None, false, &telemetry, &upstream.base).await {
            Err(error) => error,
            Ok(_) => panic!("没等到完成事件就不能当成功返回"),
        };
        assert_eq!(502, error.status_code, "{}", error.message);
        assert!(error.message.contains("完成事件"), "{}", error.message);
    }

    /// 「一句不吐就结束」不算截断：非流式照旧返回空回答（既有兜底链在管那一类）。
    #[tokio::test]
    async fn an_aggregate_that_produced_nothing_is_still_tolerated() {
        let (store, _db) = store_with_account("FW-AGGSILENT");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[("token_usage", "{\"prompt_tokens\":1,\"completion_tokens\":0,\"total_tokens\":1}")])).await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, false, &telemetry, &upstream.base)
            .await
            .expect("空回答不是这条判据要管的");
        let ForwardOutcome::Completion { body } = outcome else { panic!("非流式路径要给 Completion") };
        assert_eq!("", body["choices"][0]["message"]["content"].as_str().unwrap_or("x"));
    }

    /// 判据本体与开关：**不碰进程环境变量**（那会污染并发跑的其它用例）。
    #[test]
    fn the_missing_terminal_judgment_can_be_switched_off() {
        assert!(missing_terminal(false, true, true), "给了内容却没完成 → 要报");
        assert!(!missing_terminal(true, true, true), "收过场 → 不报");
        assert!(!missing_terminal(false, false, true), "一句不吐 → 不报（空回答链在管）");
        assert!(!missing_terminal(false, true, false), "开关关掉 → 退回旧行为");
        for off in ["1", "true", "YES", " on "] {
            assert!(!require_terminal_from(Some(off)), "{off} 应当关掉判据");
        }
        for on in [None, Some("0"), Some(""), Some("false")] {
            assert!(require_terminal_from(on), "{on:?} 应当保持判据");
        }
    }

    #[tokio::test]
    async fn the_non_streaming_path_aggregates_the_same_upstream_stream() {
        let (store, _db) = store_with_account("FW-AGG");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[
            ("output", "{\"response\":\"你\"}"),
            ("output", "{\"response\":\"好\"}"),
            ("token_usage", "{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}"),
            ("done", "{\"finish_reason\":\"stop\"}"),
        ]))
        .await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let outcome = forward_at(&store, &id, &body("glm-5.2-solo"), None, false, &telemetry, &upstream.base)
            .await
            .expect("非流式应当内部聚合后整体返回");
        let ForwardOutcome::Completion { body } = outcome else { panic!("非流式路径要给 Completion") };
        assert_eq!(
            "你好",
            body["choices"][0]["message"]["content"].as_str().unwrap_or(""),
            "聚合后的正文：{body}"
        );
        assert_eq!("glm-5.2-solo", body["model"].as_str().unwrap_or(""), "回显客户端请求的名字");
        assert_eq!(Some(5), body["usage"]["total_tokens"].as_i64());
    }

    #[tokio::test]
    async fn an_in_band_error_on_the_aggregate_path_still_carries_a_status() {
        // 非流式这条更要盯：不判状态就成了「200 + 空 content + usage 全 0」的
        // 假成功（参考实现摔过的正是它），客户端界面上一句话都没有还不知道为什么。
        let (store, _db) = store_with_account("FW-AGGERR");
        let id = account_id(&store);
        let upstream = FakeUpstream::spawn(sse(&[("error", "{\"code\":1005,\"message\":\"plan limit\"}")])).await;
        let telemetry = Arc::new(RequestTelemetry::new());
        let error = match forward_at(&store, &id, &body("glm-5.2-solo"), None, false, &telemetry, &upstream.base).await {
            Err(error) => error,
            Ok(_) => panic!("聚合路径拿到流内错误却返回了成功"),
        };
        assert_eq!(429, error.status_code, "1005 = 计划限额 → 换账号信号：{}", error.message);
    }

    #[tokio::test]
    async fn the_outbound_body_is_the_solo_envelope_with_the_bare_model_name() {
        let (_store, _db) = store_with_account("FW-SHAPE");
        let credential = Credential { access_token: "FW-SHAPE".into(), variant: "solo".into(), ..Default::default() };
        let plan = build_plan(&credential, &body("kimi-k3-solo"), "https://example.invalid").expect("规划应当成功");
        assert_eq!("kimi-k3", plan.body["config_name"].as_str().unwrap_or(""), "出站用裸 config 名");
        assert!(plan.body["stream"].as_bool().unwrap_or(false), "上游恒流式");
        assert_eq!("solo_work_lite", plan.body["function"].as_str().unwrap_or(""));
        assert!(plan.url.starts_with("https://example.invalid"), "基址由调用方给：{}", plan.url);
        let names: Vec<&str> = plan.headers.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.iter().any(|name| name.eq_ignore_ascii_case("x-ide-version-code")), "目录/版本闸门头要在：{names:?}");
    }
}
