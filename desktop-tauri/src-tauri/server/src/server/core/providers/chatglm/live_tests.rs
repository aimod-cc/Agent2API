//! 智谱清言适配器的**真实上游联调**（`#[ignore]`：需要出网，不进 CI 常规跑）。
//!
//! 跑法：`cargo test --lib chatglm_live -- --ignored --nocapture`
//!
//! 存在理由：单测覆盖的是纯函数与帧解析（用构造的字节），而这一层是唯一能
//! 证明「签名算法 + 请求信封 + 头集合」三件套与真实服务端对得上的地方。
//! 上游一旦改签名密钥 / 换响应形态，这里是第一个报出来的地方。

use serde_json::json;

use super::{credentials, http, models, plan, sign};

/// 端到端：换访客证 → 问一次模型目录 → 发一轮对话 → 校验帧差分结果
#[tokio::test]
#[ignore = "需要出网访问 chatglm.cn，本地联调用（cargo test -- --ignored）"]
async fn live_guest_chat_round_trip() {
    // 设备号必须是 UUID（非 UUID 会被上游判 40004，见 sign.rs 的实测表）
    let device_id = sign::uuid_v4();
    assert!(credentials::is_uuid_shaped(&device_id));

    // ── 一、换访客凭证（本家「零配置可用」的入口）──────────────────
    let (access, refresh, uid) = http::guest_access(&device_id, None)
        .await
        .expect("访客换证应当成功（空 body + 签名头）");
    assert!(!access.is_empty(), "换证必须返回 access_token");
    assert!(!refresh.is_empty(), "换证必须返回 refresh_token");
    assert!(!uid.is_empty(), "换证必须返回 user_id");
    // 令牌是 JWT，且声明里确实标着访客
    assert!(
        credentials::token_is_guest(&access, false),
        "换来的令牌应当是访客令牌（JWT is_guest=true）"
    );
    assert_eq!(credentials::token_uid(&access), uid, "JWT 的 uid 应与 user_id 一致");
    // 有效期写在 exp 里（刚换到的新令牌不该判成临期）
    assert!(
        !credentials::token_expiring_soon(&access),
        "刚换到的令牌不应被判成临期"
    );

    // ── 二、模型目录（登录/访客都读得到，但访客清单更短）───────────
    let outcome = models::refresh(&access, &device_id, true).await;
    let listed = models::list();
    assert!(
        !listed.is_empty(),
        "目录为空（连兜底都没有）：{outcome:?}"
    );
    // 实测访客至少能看到 glm-5.3 与 glm-5.3-flash 两个名字
    let names: Vec<String> = listed
        .iter()
        .filter_map(|model| model.get("selected_model"))
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect();
    assert!(
        names.iter().any(|name| name.starts_with("glm-")),
        "目录里应当有 glm- 开头的模型名：{names:?}"
    );

    // ── 三、发一轮对话，用真实字节喂翻译状态机 ────────────────────
    // 走 http 层发出请求、拿回原始 SSE 文本（这里刻意不用编排层：
    // 本测试的目的是「适配器的请求构造 + 帧解析对得上上游」，
    // 编排层的账号轮换/冷却已由 provider_loop 的既有测试覆盖）
    let body = plan::build_chat_body(
        &json!({
            "messages": [
                { "role": "system", "content": [{ "type": "text", "text": "只回答数字，不要任何解释。" }] },
                { "role": "user", "content": [{ "type": "text", "text": "1 加 1 等于几？" }] }
            ]
        }),
        "glm-5.3-flash",
    );
    // 请求体形态：content 必须是数组（字符串会被上游 400 invalid param）
    assert!(body["messages"][0]["content"].is_array());
    // 多轮已被拍平成单条（system 与 user 都在同一段文本里）
    let flat = body["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(flat.contains("只回答数字"), "system 段应当被拍平进正文：{flat}");
    assert!(flat.contains("1 加 1"), "用户问题应当被拍平进正文：{flat}");
    assert!(flat.contains("<|sytstem|>"), "system 标记应当保留上游那个拼写");

    let raw = http::post_text(
        super::endpoints::PATH_ENGINE_STREAM,
        &access,
        &device_id,
        &body,
        None,
        "对话联调",
    )
    .await
    .expect("对话请求应当成功（访客走 engine/stream 实测可用）");
    assert!(
        raw.contains("data:"),
        "上游应当回 SSE 数据；实际前 300 字：{}",
        raw.chars().take(300).collect::<String>()
    );

    // ── 四、把真实字节喂给差分状态机，校验拼出来的正文 ─────────────
    use crate::server::core::protocol::chatglm_stream::ChatFromChatglmStream;
    let mut machine = ChatFromChatglmStream::new("glm-5.3-flash");
    let mut frames = machine.push(raw.as_bytes());
    frames.extend(machine.finish());
    let mut text = String::new();
    let mut saw_finish = false;
    for frame in &frames {
        let line = String::from_utf8_lossy(frame);
        let Some(payload) = line.trim().strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        for choice in value.get("choices").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(delta) = choice.get("delta") {
                if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                    text.push_str(content);
                }
            }
            if choice
                .get("finish_reason")
                .and_then(|v| v.as_str())
                .is_some_and(|reason| !reason.is_empty())
            {
                saw_finish = true;
            }
        }
    }
    println!("[live] 差分后正文 = {text:?}");
    assert!(!text.is_empty(), "差分后应当有正文（快照差分没吃掉内容）");
    assert!(saw_finish, "正常流应当以 finish_reason 收尾");
}

/// 续期链路联调：`user/refresh` 换新令牌（访客令牌也支持）
#[tokio::test]
#[ignore = "需要出网访问 chatglm.cn，本地联调用（cargo test -- --ignored）"]
async fn live_refresh_rotates_tokens() {
    let device_id = sign::uuid_v4();
    let (access, refresh, uid) = http::guest_access(&device_id, None)
        .await
        .expect("访客换证应当成功");
    let (new_access, new_refresh, new_uid) = http::refresh_token(&refresh, &device_id, None)
        .await
        .expect("续期应当成功");
    assert!(!new_access.is_empty(), "续期必须返回新 access_token");
    assert!(!new_refresh.is_empty(), "续期必须返回 refresh_token（可能轮换）");
    assert_eq!(new_uid, uid, "续期后仍是同一个用户");
    println!(
        "[live] access 变化={} refresh 轮换={}",
        new_access != access,
        new_refresh != refresh
    );
}
