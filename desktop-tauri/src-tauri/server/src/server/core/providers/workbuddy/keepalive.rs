//! WorkBuddy 国际版的每日活跃保活请求构造。
//!
//! ── 这是什么 ──────────────────────────────────────────────
//! 国际版有一个「每日活跃奖励」：活动开放时领一份奖励，并且要求账号当天
//! 「活跃」。活跃的判定分两件事（issue #277 的抓包结论）：桌面端 `chat/
//! completions` 的一次最小对话维持「账号有调用」，而**点亮每日活跃计数的
//! 是网页通道的 agent 会话**（`/console/as/conversations/`，带 prompt 建会话
//! 即计）—— 参考客户端把「活动探测 → 条件领取 → 免费模型最小对话」接到
//! 既有签到调度器上；本仓的对应编排在 `billing::activity` 的
//! `workbuddy_daily_activity`，本文件负责**把两发保活请求的素材拼出来**
//! （URL / 头 / 体 / 代理）。
//!
//! ── 为什么两发请求长得很不一样 ────────────────────────────
//! 桌面端那发（[`build_daily_activity_request`]）必须长得和真对话一模一样：
//! 头集合与 URL 直接复用 `adapter::chat_headers` / `chat_completions_url`
//! （两套实现必然漂移，漂移的后果是被客户端画像识别拦下）。网页那发
//! （[`build_web_conversation_request`]）则要**剥掉全部桌面指纹**：网页链路
//! 只认 `Authorization: Bearer` 与 `X-User-Id` 两个凭据头，UA 也是浏览器的
//! —— 同一份账号凭据直接可用，无需网页登录。
//!
//! ── 模型链 ────────────────────────────────────────────────
//! [`DEFAULT_FREE_MODELS`] 是参考客户端实测不消耗账号积分的免费模型，按
//! 可用性优先级排列；运行时真正使用的模型链可在签到中心自定义
//! （`billing::keepalive` 读配置），列表为空或全失败时逐个回退到下一个。

use serde_json::{Value, json};

use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::request::TransportRequest;

use super::Region;
use super::adapter::{chat_completions_url, chat_headers, web_conversations_url};
use super::growth::WEB_UA;

/// 国际版每日活跃任务默认使用的免费模型，按可用性优先级尝试。
///
/// 这些模型在参考客户端中实测为 x0.00，不会消耗账号积分；它是**缺省值**
/// （配置里没写 `checkinKeepalive.models` 时用它），用户自定义后的链路
/// 存在 config.json，读写见 `billing::keepalive`。列表保持集中，
/// 便于上游模型调整时只改一处。
pub const DEFAULT_FREE_MODELS: &[&str] =
    &["deepseek-v4.1-flash", "hy3", "hy4-preview", "hy4-preview-f"];

/// 构造国际版每日活跃保活请求。
///
/// 活跃任务必须沿用 WorkBuddy 对话链路的端点、鉴权头和账号代理；这里只固定
/// 参考客户端使用的最小流式 body（16 个 token 上限的一次 "hi"）。调用方
/// （`billing::activity::poke_daily_activity`）负责通过传输层发送并消费响应体。
pub fn build_daily_activity_request(
    session: &Value,
    model: &str,
) -> Result<TransportRequest, String> {
    let body = json!({
        "model": model,
        "stream": true,
        "max_tokens": 16,
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "hi"},
        ],
    });
    let payload =
        serde_json::to_string(&body).map_err(|error| format!("活跃请求序列化失败: {error}"))?;
    // 头集合与真对话共用一份（chat_headers 已带全会话头族 / 链路头族与
    // Origin / Referer —— 正是 11-128 渠道校验的对齐项），这里不再补丁。
    let headers = chat_headers(session, Region::Intl, Some("text/event-stream"));
    let proxy = match ResolvedProxy::from_json(session.get("proxy").unwrap_or(&Value::Null)) {
        Ok(proxy) => proxy,
        Err(reason) => {
            crate::server::logging::log(
                "[Upstream]",
                &format!("⚠️ 账号代理不可用（{reason}），活跃请求回退直连"),
            );
            None
        }
    };
    Ok(TransportRequest {
        url: chat_completions_url(session, Region::Intl),
        headers,
        payload,
        proxy,
        // 保活是转发链路的旁支，口径跟转发一致（workbuddy 不覆写能力位 = 直连）
        system_proxy_when_unset: false,
    })
}

/// 构造网页通道的每日活跃会话请求（issue #277 的修复主体）。
///
/// 国际版上游把「当日活跃」（每日 30 积分的判定）记在网页通道的 agent 会话
/// 上：`POST /console/as/conversations/` 建**带 prompt 的会话**即计活跃，
/// 桌面端 `chat/completions` 与 `/v2/report` 上报都不进这个计数（参考实现
/// workbuddy2api-hub 的抓包结论，issue #277 评论附实测出处）。网页链路
/// **只认 `Authorization: Bearer` 与 `X-User-Id` 两个凭据头**——不带桌面端
/// `X-IDE-*` 指纹与 CLI UA，UA 用浏览器串（与 growth 的 web 域同一份），
/// 因此这里手动拼头而不复用 `chat_headers`（复用必带桌面指纹，正是要剥掉的）。
///
/// 会话体固定带 `conversationOrigin` 与 `plugins`（网页端建会话的抓包形态），
/// prompt 用最小的 "Hi"——它只是让上游「按该 prompt 起一次任务」。凭据缺失
/// （无 token / uid）返回 Err，调用方把原因收敛进结果字段。
pub fn build_web_conversation_request(
    session: &Value,
    model: &str,
) -> Result<TransportRequest, String> {
    let token = session
        .get("auth")
        .and_then(|auth| auth.get("accessToken"))
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "会话缺少 accessToken，无法走网页通道".to_string())?;
    let uid = session
        .get("account")
        .and_then(|account| account.get("uid"))
        .and_then(Value::as_str)
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "会话缺少账号 uid，无法走网页通道".to_string())?;
    let body = json!({
        "prompt": "Hi",
        "model": model,
        // 网页端建会话时固定带上这两项（抓包所得），保持请求形态一致
        "conversationOrigin": "workbuddy-app",
        "plugins": [{"name": "weixinpay", "marketplace": "codebuddy-builtin"}],
    });
    let payload = serde_json::to_string(&body)
        .map_err(|error| format!("网页会话请求序列化失败: {error}"))?;
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
        ("User-Agent".to_string(), WEB_UA.to_string()),
        (
            "Origin".to_string(),
            "https://www.workbuddy.ai".to_string(),
        ),
        (
            "Referer".to_string(),
            "https://www.workbuddy.ai/app".to_string(),
        ),
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-User-Id".to_string(), uid.to_string()),
    ];
    let proxy = match ResolvedProxy::from_json(session.get("proxy").unwrap_or(&Value::Null)) {
        Ok(proxy) => proxy,
        Err(reason) => {
            crate::server::logging::log(
                "[Upstream]",
                &format!("⚠️ 账号代理不可用（{reason}），网页会话请求回退直连"),
            );
            None
        }
    };
    Ok(TransportRequest {
        url: web_conversations_url(session, Region::Intl),
        headers,
        payload,
        proxy,
        // 与桌面端保活同一口径：转发链路的旁支，workbuddy 不覆写能力位 = 直连
        system_proxy_when_unset: false,
    })
}
