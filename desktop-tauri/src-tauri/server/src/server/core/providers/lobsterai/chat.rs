//! LobsterAI 的转发：**同方言透传 + 四处上游强制的改写**。
//!
//! ── 为什么这一层也很薄 ──────────────────────────────────────
//! 对话端点是 OpenAI 兼容的原生协议（`POST /api/proxy/v1/chat/completions`），
//! 网关本身就是 OpenAI Chat Completions，所以 body 几乎逐字节上行、上游的 SSE
//! 帧逐字节下发（`executor.go:16-27` 的模块头：语言不同，但「原样透传 + 只改
//! 必须改的」这条口径一样）。剩下的只有四件格式本身表达不了的事。
//!
//! ── 只改四处，每处都有它非改不可的理由（全部实测）────────────────
//!   1. **`role: "developer"` → `"system"`**：上游不认 OpenAI 新版角色，
//!      带它的请求被网关以 **HTTP 502「角色信息不正确」**拒绝（system / user
//!      均 200）。两者在 OpenAI 规范里语义相同，按 `system` 下发
//!      （`executor.go:83-86` 的实测注 + `demoteDeveloperMessages`）。
//!      ⚠️ 本仓的通用转发层在「开发者角色兼容」开关打开时也会做同一件事
//!      （`upstream::payload::send_body` 的 ⓪ 角色归一层，默认开）—— 这里再写
//!      一遍不是冗余：开关关掉时它就是唯一的一道，且对已归一的体是**幂等**的。
//!   2. **`tool_choice` 在 `""` / `"none"` / `null` 时删掉**：客户端可能原样
//!      转发一个上游拒绝的取值（`executor.go:114-123`）。
//!   3. **绝不下发 `prompt_cache_key`**：那是腾讯后端的机制，本家不知道认不认
//!      （`executor.go:63-64` 的原文）；存在就删。
//!   4. **`model` 走一次名字解析**（展示名 / 规范化别名 → 线上 id）：
//!      与 `atomcode/chat.rs` 同一手法，认不出时**原样发**，不猜。
//!      注意通用转发层已经按目录把 body 里的模型名改写成了线上名
//!      （`catalog::wire_target_for_provider`），所以这一步通常是一次幂等的
//!      确认；留着它是为了「客户端直接点名一个规范化别名」那条路。
//!
//! ── 两处**刻意不做**的改写（都是实测结论）──────────────────────
//!   · **不动 `stream`**：本家 `stream:false` 上游直接回 HTTP 500
//!     （`executor.go:18-21`），而本网关的转发入口「无论客户端要不要流式，上游
//!     恒以 `stream:true` 请求」（`core/upstream/mod.rs` 的注释原文）——
//!     这条要求天然满足，**碰它反而会把它弄坏**。
//!   · **不转换图片**：上游只收 `{type:"image_url",image_url:{url}}`，收到
//!     `{type:"image"}` 会回 HTTP 500。我们**不做**形状修补 —— 让上游把它的
//!     真实错误如实报给客户端，好过我们替它猜一种可能不对的形状
//!     （与本仓「广告能力要有实测依据、不编造」的口径一致）。
//!
//! ── 客户端版本头为什么要一个进程级快照 ──────────────────────────
//! `X-LobsterAI-Client-Version` 是功能性头（缺它 / 给空串会被按「不认识的
//! 客户端」对待，见 `endpoints.rs` 的模块头），而版本号要**异步**从上游更新
//! 接口取（`version::resolve_client_version()`）。但 `ProviderAdapter` 的
//! `build_chat_request` 是**同步**的（trait 契约，不能改成 async），所以这里
//! 用一份进程级快照：异步侧（[`prime_client_version`]，由目录刷新 / 签到 /
//! 余额这些本来就要取版本的异步路径调用）写、同步侧（[`client_version_header`]）
//! 读；快照还没填时退回已知可用的兜底常量，**绝不发空版本头**（与 Go 参考
//! `headers.go::resolveHeaderVersion` 同一口径）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::providers::adapter::ChatRequestPlan;
use crate::server::errors::GatewayError;

use super::endpoints::{CLIENT_CAPABILITIES, USER_AGENT, chat_url};
use super::models;
use super::version;

/// 进程级版本快照（由 [`prime_client_version`] 写、[`client_version_header`] 读）
static CLIENT_VERSION: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn version_slot() -> &'static Mutex<Option<String>> {
    CLIENT_VERSION.get_or_init(|| Mutex::new(None))
}

/// 解析一次客户端版本并写进快照，返回解析结果。
///
/// **异步入口**：目录刷新 / 签到 / 余额查询这些异步路径都调它 —— 一次调用既
/// 拿到自己要用的版本，也顺手把 [`client_version_header`] 的快照填上（幂等，
/// 底层 `version::resolve_client_version` 自带 12 小时缓存）。
pub async fn prime_client_version() -> String {
    let resolved = version::resolve_client_version().await;
    store_client_version(&resolved);
    resolved
}

/// 写入快照（空串不覆盖：宁可留着上一次的好值，也不要为一次瞬时失败清空）
fn store_client_version(value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    let mut guard = match version_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(trimmed.to_string());
}

/// 同步读取 `X-LobsterAI-Client-Version` 的值。
///
/// 快照没填过（进程刚起、还没走过任何异步取版本的路径）时退回兜底常量 ——
/// **绝不发空版本头**（空串会被服务端按「不认识的客户端」对待）。
pub fn client_version_header() -> String {
    let cached = match version_slot().lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    match cached {
        Some(value) if !value.trim().is_empty() => value,
        _ => version::fallback_version().to_string(),
    }
}

/// 上游不认的 OpenAI 角色名（带它会被网关以 HTTP 502「角色信息不正确」拒绝）
const DEVELOPER_ROLE: &str = "developer";
/// 归一后的角色名（两者在 OpenAI 规范里语义相同）
const DEMOTED_ROLE: &str = "system";

/// 把 `role: "developer"` 的消息改成 `"system"`（原地改写，非对象条目原样放过）。
///
/// 见模块头的实测表：这是上游的硬要求，不是风格偏好。
fn demote_developer_messages(object: &mut serde_json::Map<String, Value>) {
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut() {
        let Some(entry) = message.as_object_mut() else {
            continue;
        };
        let is_developer = entry
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role == DEVELOPER_ROLE);
        if is_developer {
            entry.insert("role".to_string(), Value::String(DEMOTED_ROLE.to_string()));
        }
    }
}

/// `tool_choice` 是否属于「上游会拒绝、必须删掉」的取值（`""` / `"none"` / `null`）。
fn tool_choice_must_go(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty() || text.trim().eq_ignore_ascii_case("none"),
        _ => false,
    }
}

/// 准备一次上游调用。
pub fn build_chat_request(
    account: &Value,
    body: &Value,
    _client_headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    // 令牌读取口径与 `atomcode/chat.rs` 一致（会话形态的 `auth.accessToken`）
    let access_token = account
        .get("auth")
        .and_then(|auth| auth.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if access_token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "LobsterAI 账号缺少访问令牌，无法转发（请重新登录）",
        ));
    }

    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if requested.is_empty() {
        return Err(GatewayError::with_status(400, "请求缺少 model 字段"));
    }

    let mut payload = body.clone();
    let Some(object) = payload.as_object_mut() else {
        return Err(GatewayError::with_status(400, "请求体不是 JSON 对象"));
    };

    // ① developer → system（上游 502「角色信息不正确」的唯一成因，见模块头）
    demote_developer_messages(object);
    // ② tool_choice：`""` / `"none"` / `null` 一律删掉
    if object.get("tool_choice").is_some_and(tool_choice_must_go) {
        object.remove("tool_choice");
    }
    // ③ 绝不下发 prompt_cache_key（腾讯后端机制，本家不认）
    object.remove("prompt_cache_key");
    // ④ 模型名解析：展示名 / 规范化别名 → 线上 id；认不出原样发
    let wire_model =
        models::upstream_id_for(&requested).unwrap_or_else(|| requested.clone());
    object.insert("model".to_string(), Value::String(wire_model));

    // `stream` **不动**：本网关的转发入口恒以 `stream:true` 请求上游（见模块头），
    // 本家 `stream:false` 会回 HTTP 500 —— 碰它反而会把它弄坏。

    let headers: Vec<(String, String)> = vec![
        (
            "authorization".to_string(),
            format!("Bearer {access_token}"),
        ),
        // 只收 SSE：Accept 两个都报（照 Go `chatHeaders` 的 `text/event-stream, application/json`）
        (
            "accept".to_string(),
            "text/event-stream, application/json".to_string(),
        ),
        // ⚠️ `Content-Type` **必须显式发**：本仓的转发层用 `client.post(url).body(bytes)`
        // （`core/upstream/request.rs` 的发送点），reqwest 的 `.body()` **不会**自动补
        // 这个头 —— 只有 `.json()` 才会。而剥掉它之后上游回的是**鉴权之后的**
        // `{"code":500,"message":"服务器内部错误"}`（HTTP 500，约 46ms 即返），
        // 跨模型稳定复现，看上去像「网关坏了」而不是「少了个头」。
        // 判据：同一账号打同一个域名的其它 POST（换码 `/api/auth/exchange`、
        // 签到）都带了它且成功；伪令牌探针证明上游**先鉴权**（401 code 40100），
        // 所以 500 出在鉴权之后。两个参考实现也都把它列在四个基础头里
        // （TS `lobsteraiAuthHeaders`、Go `authHeaders`）。
        (
            "content-type".to_string(),
            "application/json".to_string(),
        ),
        // 两个客户端头都是功能性的（能力位过滤模型、版本决定被当成哪个客户端）
        (
            "x-lobsterai-client-capabilities".to_string(),
            CLIENT_CAPABILITIES.to_string(),
        ),
        (
            "x-lobsterai-client-version".to_string(),
            client_version_header(),
        ),
        // UA 固定不跟随真实版本（照抄参考实现，不是装饰）
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ];

    Ok(ChatRequestPlan::chat(chat_url(), headers, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Value {
        json!({ "auth": { "accessToken": "at-1" } })
    }

    fn build(body: Value) -> ChatRequestPlan {
        match build_chat_request(&account(), &body, &HeaderMap::new()) {
            Ok(plan) => plan,
            Err(error) => panic!("应构造成功，却报错：{}", error.message),
        }
    }

    fn header_of<'a>(plan: &'a ChatRequestPlan, name: &str) -> Option<&'a str> {
        plan.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// **首条护栏**：`developer` 角色必须被归一成 `system`（否则上游 502）。
    #[test]
    fn developer_role_is_demoted_to_system() {
        let plan = build(json!({
            "model": "glm-5.2",
            "messages": [
                {"role": "developer", "content": "a"},
                {"role": "user", "content": "b"},
                {"role": "system", "content": "c"}
            ]
        }));
        let messages = plan.body.get("messages").and_then(Value::as_array).expect("messages");
        assert_eq!(messages[0].get("role").and_then(Value::as_str), Some("system"));
        assert_eq!(messages[1].get("role").and_then(Value::as_str), Some("user"));
        assert_eq!(messages[2].get("role").and_then(Value::as_str), Some("system"));
        // 内容不动
        assert_eq!(messages[0].get("content").and_then(Value::as_str), Some("a"));
    }

    /// **第二道护栏**：chat 请求必须自带 `Content-Type: application/json`。
    ///
    /// 本仓转发层用 `client.post(url).body(bytes)` 发包，reqwest 的 `.body()`
    /// **不补**这个头（只有 `.json()` 会）。缺了它上游回的是**鉴权之后**的
    /// HTTP 500「服务器内部错误」，跨模型稳定复现 —— 症状像网关坏了，实际
    /// 只是少了一个头（成因与判据见 `build_chat_request` 里的注释）。
    #[test]
    fn chat_request_carries_json_content_type() {
        let plan = build(json!({"model": "glm-5.2", "messages": [{"role": "user", "content": "a"}]}));
        assert_eq!(header_of(&plan, "content-type"), Some("application/json"));
    }

    /// `tool_choice` 的三种「必须删」形态；`"auto"` / 对象形态**保留**。
    #[test]
    fn rejected_tool_choice_values_are_dropped() {
        for value in [json!(""), json!("none"), json!("NONE"), json!(null)] {
            let plan = build(json!({"model": "m", "messages": [], "tool_choice": value}));
            assert!(
                plan.body.get("tool_choice").is_none(),
                "tool_choice={value} 应被删掉"
            );
        }
        let kept = build(json!({"model": "m", "messages": [], "tool_choice": "auto"}));
        assert_eq!(
            kept.body.get("tool_choice").and_then(Value::as_str),
            Some("auto")
        );
        let forced = build(json!({
            "model": "m", "messages": [],
            "tool_choice": {"type": "function", "function": {"name": "f"}}
        }));
        assert!(forced.body.get("tool_choice").is_some());
    }

    /// `prompt_cache_key` 无论客户端怎么给都不得下发。
    #[test]
    fn prompt_cache_key_is_never_sent() {
        let plan = build(json!({
            "model": "m", "messages": [], "prompt_cache_key": "abc"
        }));
        assert!(plan.body.get("prompt_cache_key").is_none());
    }

    /// **`stream` 不得被改写**（上游 stream:false 回 500；本网关入口恒置 true）。
    #[test]
    fn stream_field_is_forwarded_untouched() {
        let plan = build(json!({"model": "m", "messages": [], "stream": true}));
        assert_eq!(plan.body.get("stream").and_then(Value::as_bool), Some(true));
        let plan = build(json!({"model": "m", "messages": []}));
        assert!(plan.body.get("stream").is_none(), "原本没有就别替它加");
    }

    /// 头部：两个功能性客户端头 + UA + Bearer 必须在，且版本头非空。
    #[test]
    fn load_bearing_headers_are_present() {
        let plan = build(json!({"model": "m", "messages": []}));
        assert_eq!(
            header_of(&plan, "authorization"),
            Some("Bearer at-1")
        );
        assert_eq!(
            header_of(&plan, "x-lobsterai-client-capabilities"),
            Some(CLIENT_CAPABILITIES)
        );
        let version = header_of(&plan, "x-lobsterai-client-version").expect("版本头必须存在");
        assert!(!version.trim().is_empty(), "绝不发空版本头");
        assert_eq!(header_of(&plan, "user-agent"), Some(USER_AGENT));
        assert_eq!(
            header_of(&plan, "accept"),
            Some("text/event-stream, application/json")
        );
        assert!(plan.url.ends_with("/api/proxy/v1/chat/completions"));
    }

    /// 其余字段**逐字不动**（同方言透传的全部价值）。
    #[test]
    fn everything_else_is_forwarded_untouched() {
        let original = json!({
            "model": "glm-5.2",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "f"}}],
            "temperature": 0.3,
            "max_tokens": 1234,
            "reasoning_effort": "high"
        });
        let plan = build(original.clone());
        for key in ["messages", "tools", "temperature", "max_tokens", "reasoning_effort"] {
            assert_eq!(plan.body.get(key), original.get(key), "字段 {key} 必须逐字透传");
        }
    }

    /// 缺令牌 / 缺 model 时报可读错误，而不是发一个空请求出去。
    #[test]
    fn missing_credentials_or_model_fail_fast() {
        // `ChatRequestPlan` 没有实现 Debug（跨家共用类型），所以手写 match
        let error = match build_chat_request(
            &json!({"auth": {}}),
            &json!({"model": "m", "messages": []}),
            &HeaderMap::new(),
        ) {
            Ok(_) => panic!("缺令牌应当报错"),
            Err(error) => error,
        };
        assert_eq!(error.status_code, 401);

        let error = match build_chat_request(&account(), &json!({"messages": []}), &HeaderMap::new())
        {
            Ok(_) => panic!("缺 model 应当报错"),
            Err(error) => error,
        };
        assert_eq!(error.status_code, 400);
    }

    /// 快照写入读回；空值不覆盖上一次的好值。
    #[test]
    fn version_snapshot_ignores_blank_writes() {
        store_client_version("2026.9.4");
        assert_eq!(client_version_header(), "2026.9.4");
        store_client_version("   ");
        assert_eq!(client_version_header(), "2026.9.4", "空串不得覆盖好值");
        store_client_version("2026.10.1");
        assert_eq!(client_version_header(), "2026.10.1");
    }
}
