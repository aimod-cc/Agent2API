//! 流内错误信封：CodeArts 的失败**有时不体现在 HTTP 状态上**。
//!
//! ── 为什么单独立一个模块 ────────────────────────────────────
//! 上游把「福利额度用尽」这类失败塞在 **HTTP 200 的 SSE 体里**：流干净地开始、
//! 干净地结束，中间只有一帧 `{"error_code":"InferHub.4291.200", "error_msg":
//! "insufficient quota"}`。如果只看 HTTP 状态，这就成了「成功但一个字都没答」——
//! 而**跨账号降级链的触发条件完全依赖错误分类是否正确**：分类错了，编排层就
//! 不会换下一个账号，用户拿到一个假的成功。所以这套判定是整套移植里权重最高的
//! 逻辑之一，值得单独成模块并逐条钉住。
//!
//! ── 三条不能想当然的规则（照参考实现，逐条都有测试）───────
//!   1. **`error_code` 为空、或 ∈ {`0`, `0000`, `success`} 不算故障**。空是普通
//!      数据帧；这三个值是这套信封的「成功」写法。
//!   2. **同一帧里只要带了任何答案，就绝不判故障**。`delta.content` /
//!      `delta.reasoning_content` / `delta.tool_calls` / `choices[].text` /
//!      非 `[DONE]` 的 `text` 都算答案 —— 带错误码又带内容的那种帧，丢内容
//!      比漏报一次故障更糟。
//!   3. **`details[]` 只取一条「短且不含冒号」的**。实测里 `details` 装的是
//!      `requestId: …` / `traceId: …` 这类 trace 装饰，它们重复同一个错误码、
//!      对排障毫无价值，还会把真正的消息挤没。含冒号的一律跳过（那正是
//!      `requestId: xxx` 的形状）。
//!
//! 分类（对应编排层的动作）与参考实现一致：
//! `insufficient quota` → 403（额度耗尽，换账号）；含 `429` / `rate limit` /
//! `too many` → 429（限流）；其余 → 502。

use serde_json::Value;

use crate::server::errors::GatewayError;

/// SSE 的结束哨兵；它出现在 `text` 字段里时不算答案。
pub const DONE_SENTINEL: &str = "[DONE]";

/// 这套信封里的「成功」码（大小写不敏感）。
fn is_success_code(code: &str) -> bool {
    matches!(code.to_ascii_lowercase().as_str(), "0" | "0000" | "success")
}

/// 一帧里带的答案内容；`null` / 空 / 只有 role 的 delta 都算「没内容」。
fn delta_has_content(delta: &Value) -> bool {
    let Some(object) = delta.as_object() else {
        return false;
    };
    let content = match object.get("content") {
        // 字符串形态
        Some(Value::String(text)) => !text.is_empty(),
        // 数组形态（多模态分段）：里面只要有非空 text 就算
        Some(Value::Array(parts)) => parts.iter().any(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        }),
        _ => false,
    };
    if content {
        return true;
    }
    if object
        .get("reasoning_content")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
    {
        return true;
    }
    object
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty())
}

/// `choices[]` 里任何一处带了答案都算「这帧有内容」。
fn choices_have_content(choices: &Value) -> bool {
    let Some(items) = choices.as_array() else {
        return false;
    };
    items.iter().any(|choice| {
        delta_has_content(choice.get("delta").unwrap_or(&Value::Null))
            || delta_has_content(choice.get("message").unwrap_or(&Value::Null))
            || choice
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
    })
}

/// 一帧的失败判定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamFault {
    /// 上游业务码（写进日志与错误信封）
    pub code: String,
    /// 面向人的消息（含上游码与原始 error_msg）
    pub message: String,
    /// 映射后的 HTTP 状态：403 额度 / 429 限流 / 502 其它
    pub status: u16,
}

/// 判一帧（已去掉 `data:` 前缀并解成 JSON 的载荷）是不是故障帧。
///
/// 载荷不是 JSON 对象、是空串、是 `[DONE]`、是成功码、或**带了任何答案**时一律
/// 返回 `None` —— 宁可漏报一帧，也不要把内容丢掉。
pub fn stream_frame_fault(payload: &str) -> Option<StreamFault> {
    let payload = payload.trim();
    if payload.is_empty() || payload == DONE_SENTINEL {
        return None;
    }
    let frame: Value = serde_json::from_str(payload).ok()?;
    if !frame.is_object() {
        return None;
    }
    let code = frame
        .get("error_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if code.is_empty() || is_success_code(&code) {
        return None;
    }
    // 有答案就不算故障（规则 2）。三个位置都要看：顶层 `delta`（原生协议把
    // 增量放在这里）、`choices[].delta|message|text`（OpenAI 协议）、顶层 `text`
    let text = frame.get("text").and_then(Value::as_str).unwrap_or("").trim();
    let has_answer = delta_has_content(frame.get("delta").unwrap_or(&Value::Null))
        || choices_have_content(frame.get("choices").unwrap_or(&Value::Null))
        || (!text.is_empty() && text != DONE_SENTINEL);
    if has_answer {
        return None;
    }

    let mut message = format!("上游报告 {code}");
    if let Some(detail) = frame.get("error_msg").and_then(Value::as_str).map(str::trim) {
        if !detail.is_empty() {
            message.push_str("：");
            message.push_str(detail);
        }
    }
    // details 里只认「一条短且不含冒号」的补充说明（规则 3）
    if let Some(items) = frame.get("details").and_then(Value::as_array) {
        for item in items {
            // error_msg 优先，**取不到就退到 error_code**：尺寸那两道墙的标记恰好只写在
            // `error_code` 上（`PARSE_REQUEST_DATA_EXCEPTION`），只看 error_msg 会把它
            // 整条丢掉 —— 于是这句面向人的话里再也没有任何尺寸线索，门也就无从登记
            // （丢掉时实测过：8 MB 连打两发，两发都完整发了出去）。
            let detail = ["error_msg", "error_code"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            if detail.is_empty() || message.contains(detail) || detail.contains(':') {
                continue;
            }
            message.push_str(&format!("（{detail}）"));
            break;
        }
    }
    // benefit 档案不存在（账号没在官方体系里注册过权益，2026-10-09 取证：官方
    // 客户端登录用一下即恢复 —— 那边的 initBenefit 每次启动都 POST claim）。
    // 余额查询链路会自动补这条注册（见 `balance::claim_benefit`），给用户一句
    // 「会自愈」的指引，别让人以为账号坏了。指引里不含「insufficient quota」
    // 等状态词，放在 status 判定之前不会影响下面的分类。
    if message.to_lowercase().contains("benefit not found") {
        message.push_str("（该账号还没有福利档案，余额查询会自动向官方注册，稍等片刻重试即可）");
    }

    let lowered = message.to_lowercase();
    let status = if lowered.contains("insufficient quota") {
        403
    } else if code.contains("429") || lowered.contains("rate limit") || lowered.contains("too many") {
        429
    } else {
        502
    };
    Some(StreamFault { code, message, status })
}

/// 把流内故障转成编排层能用的错误：**带上分类后的状态码**，好让上游循环按
/// 「额度/限流」去换账号，而不是当成一次普通失败。
///
/// 这是整条降级链的接合点：换成 200 空回答就等于把换账号的机会扔掉。
///
/// `code` 是给**客户端**的第二条判据（状态码那条太粗：403 在本家既可能是额度
/// 也可能是内容闸门）。与参考实现在同一件事上产出的串逐字一致 ——
/// 2026-09-27 与本机 CPA 对拍时抓到的差异就是这里：同一封额度错误，
/// CPA 回 `{"type":"permission_error","code":"insufficient_quota"}`，
/// 我们回 `{"type":"invalid_request_error"}` 且没有 `code`，
/// 按 `code` 分支的客户端（不少 SDK 拿 `insufficient_quota` 决定要不要停手）
/// 会读不到信号。`type` 保持本仓统一映射（`errors.rs` 照抄 Node 版那条），
/// 不为一家破例 —— 那会让所有家的错误形状各说一套。
pub fn fault_to_error(fault: &StreamFault) -> GatewayError {
    let error = GatewayError::with_status(i32::from(fault.status), fault.message.clone());
    match fault.status {
        403 => error.with_code("insufficient_quota"),
        429 => error.with_code("rate_limit_exceeded"),
        _ => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-21 从**真实**的上游响应里抓到的额度耗尽信封（HTTP 200 的 SSE 体
    /// 内），一字未改 —— 参考实现也是拿它当金向量。
    const QUOTA_FAULT_FRAME: &str = r#"{"error_code":"InferHub.4291.200","error_msg":"insufficient quota","details":[{"error_code":"InferHub.4291.200","error_msg":"requestId: abe754a5417344caa306d5b38a57923b"},{"error_code":"InferHub.4291.200","error_msg":"timestamps: 202609211921"},{"error_code":"InferHub.4291.200","error_msg":"modelId: glm-5.3-flash"},{"error_code":"InferHub.4291.200","error_msg":"traceId: 17c59748cd004b6fadf4660383301ad1"}]}"#;

    #[test]
    fn quota_envelope_is_classified_as_forbidden() {
        let fault = stream_frame_fault(QUOTA_FAULT_FRAME).expect("额度信封必须被判成故障");
        assert_eq!("InferHub.4291.200", fault.code);
        assert_eq!(403, fault.status, "额度耗尽要映射成 403（换账号）");
        assert!(fault.message.contains("InferHub.4291.200"));
        assert!(fault.message.contains("insufficient quota"));
        assert!(
            !fault.message.contains("requestId") && !fault.message.contains("traceId"),
            "trace 装饰不能进消息（会把真正的诊断挤掉）：{}",
            fault.message
        );
    }

    /// 参考实现里那组「绝不能报故障」的帧，逐条照搬。
    #[test]
    fn ordinary_traffic_never_reports_a_fault() {
        for (name, payload) in [
            ("空", ""),
            ("结束哨兵", "[DONE]"),
            ("半截 JSON", r#"{"error_code":"#),
            ("成功信封", r#"{"error_code":"0000","error_msg":"success","result":{"daily_token_limit":1}}"#),
            ("零码带内容", r#"{"error_code":"0","choices":[{"delta":{"content":"hi"}}]}"#),
            ("普通增量", r#"{"choices":[{"index":0,"delta":{"content":"2"},"finish_reason":"stop"}]}"#),
            ("用量帧", r#"{"id":"x","model":"glm-5.2","usage":{"total_tokens":9}}"#),
            ("带错误码但有内容", r#"{"error_code":"InferHub.4291.200","choices":[{"delta":{"content":"partial"}}]}"#),
            ("原生阶段帧", r#"{"type":"stage","stage":[{"name":"plan"}]}"#),
        ] {
            assert!(
                stream_frame_fault(payload).is_none(),
                "{name}：不该被判成故障（丢了内容比漏报更糟）"
            );
        }
    }

    #[test]
    fn rate_limit_and_unknown_codes_map_to_their_status() {
        let rate = stream_frame_fault(r#"{"error_code":"Gate.429","error_msg":"Too Many Requests"}"#)
            .expect("限流信封应当被识别");
        assert_eq!(429, rate.status);
        let unknown = stream_frame_fault(r#"{"error_code":"InferHub.5000","error_msg":"backend exploded"}"#)
            .expect("未知错误应当被识别");
        assert_eq!(502, unknown.status);
        assert_eq!("InferHub.5000", unknown.code);
    }

    /// 规则 2 的几个分支要单独钉：只带 role 的 delta 不算答案；`[DONE]` 放在
    /// `text` 里也不算答案；但 reasoning / tool_calls / 多模态分段都算。
    #[test]
    fn answer_detection_covers_every_partial_answer_shape() {
        for (name, payload) in [
            ("只有 role", r#"{"error_code":"E","choices":[{"delta":{"role":"assistant"}}]}"#),
            ("text 是 [DONE]", r#"{"error_code":"E","text":"[DONE]"}"#),
            ("null content", r#"{"error_code":"E","choices":[{"delta":{"content":null}}]}"#),
            ("空数组", r#"{"error_code":"E","choices":[]}"#),
        ] {
            assert!(stream_frame_fault(payload).is_some(), "{name}：这些都没有答案，应当判成故障");
        }
        for (name, payload) in [
            ("reasoning", r#"{"error_code":"E","choices":[{"delta":{"reasoning_content":"想"}}]}"#),
            ("tool_calls", r#"{"error_code":"E","choices":[{"delta":{"tool_calls":[{"id":"1"}]}}]}"#),
            ("choices.text", r#"{"error_code":"E","choices":[{"text":"答案"}]}"#),
            ("顶层 delta", r#"{"error_code":"E","delta":{"content":"答案"}}"#),
            ("顶层 text", r#"{"error_code":"E","text":"答案"}"#),
            ("多模态 text 段", r#"{"error_code":"E","choices":[{"delta":{"content":[{"type":"text","text":"答案"}]}}]}"#),
        ] {
            assert!(stream_frame_fault(payload).is_none(), "{name}：带了答案，绝不能判故障");
        }
    }

    /// `details` 的取舍：含冒号的 trace 装饰要跳过，但不含冒号的真实补充说明
    /// 要带上（否则会把上游唯一的线索丢掉）。
    #[test]
    fn details_only_contribute_a_short_message_without_a_colon() {
        let with_plain_detail = stream_frame_fault(
            r#"{"error_code":"E1","error_msg":"boom","details":[{"error_msg":"requestId: abc"},{"error_msg":"模型未开通"}]}"#,
        )
        .expect("应当判成故障");
        assert!(with_plain_detail.message.contains("模型未开通"), "无冒号的补充说明该带上");
        assert!(!with_plain_detail.message.contains("requestId"));

        // 只有 trace 装饰时，一条都不该带
        let only_decorations = stream_frame_fault(
            r#"{"error_code":"E1","error_msg":"boom","details":[{"error_msg":"requestId: abc"},{"error_msg":"traceId: def"}]}"#,
        )
        .expect("应当判成故障");
        assert_eq!("上游报告 E1：boom", only_decorations.message);
    }

    #[test]
    fn fault_maps_to_an_error_the_orchestrator_can_cool_over() {
        let fault = stream_frame_fault(QUOTA_FAULT_FRAME).unwrap();
        let error = fault_to_error(&fault);
        assert_eq!(403, error.status_code, "状态码必须带出去，否则编排层不会换账号");
        assert!(error.message.contains("insufficient quota"));

        // 客户端那一侧的判据（与参考实现对拍的产物，见 fault_to_error 的注释）
        let payload = error.payload()["error"].as_object().cloned().unwrap_or_default();
        assert_eq!(
            "insufficient_quota",
            payload.get("code").and_then(Value::as_str).unwrap_or_default(),
            "缺 code 会让按 code 分支的客户端读不到「是额度问题」"
        );
        assert!(payload.contains_key("type"), "type 必须一直在（本仓统一映射）");

        let limited = stream_frame_fault(r#"{"error_code":"InferHub.4291.429","error_msg":"too many requests"}"#)
            .expect("限流信封");
        let error = fault_to_error(&limited);
        assert_eq!(429, error.status_code);
        assert_eq!("rate_limit_exceeded", error.payload()["error"]["code"].as_str().unwrap_or_default());

        // 认不出类别的那种**不该**有 code：编一个出来会把客户端引向错误的分支
        let unknown = fault_to_error(&stream_frame_fault(r#"{"error_code":"E9","error_msg":"boom"}"#).unwrap());
        assert_eq!(502, unknown.status_code);
        assert!(unknown.payload()["error"].get("code").is_none(), "不确定的分类不要伪造 code：{}", unknown.payload());
    }

    /// `details` 只给 `error_code`（不给 error_msg）时，那句面向人的话要把它带上。
    ///
    /// 尺寸墙的真实形状就是这样：标记**只**在 `details[].error_code` 里。丢掉它不只是
    /// 少一条线索 —— `codearts` 的尺寸门就是靠这句话登记的，丢掉等于每发大请求都白跑一趟。
    #[test]
    fn a_detail_with_only_an_error_code_still_reaches_the_message() {
        let frame = r#"{"error_code":"InferHub.001001005.400","error_msg":"The request param is invalid, Please check it","details":[{"error_code":"PARSE_REQUEST_DATA_EXCEPTION"}]}"#;
        let fault = stream_frame_fault(frame).expect("这应当被认成故障帧");
        assert!(
            fault.message.contains("PARSE_REQUEST_DATA_EXCEPTION"),
            "消息里要留得下那句标记：{}",
            fault.message
        );
        // 有 error_msg 时仍以它为先（不给 error_code 抢占位置，保持既有形状）
        let both = r#"{"error_code":"E1","error_msg":"outer","details":[{"error_code":"C2","error_msg":"inner"}]}"#;
        let fault = stream_frame_fault(both).expect("这应当被认成故障帧");
        assert!(fault.message.ends_with("（inner）"), "error_msg 优先：{}", fault.message);
    }

    /// 大小写不敏感：上游的 `error_code` 大小写并不稳定。
    #[test]
    fn success_codes_are_case_insensitive() {
        for code in ["0", "0000", "SUCCESS", "Success", "success"] {
            let payload = format!(r#"{{"error_code":"{code}"}}"#);
            assert!(stream_frame_fault(&payload).is_none(), "{code} 应当算成功");
        }
    }
}
