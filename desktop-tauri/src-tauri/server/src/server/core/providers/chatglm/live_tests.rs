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
    // 实测访客至少能看到 glm-5.3 与 glm-5.3-flash 两个名字。
    // 读 `id`（仓内约定键，见 `models::map_entry`）—— 不是上游的 `selected_model`：
    // 目录条目在归一化时已翻译成仓内键名，读旧键名会恒得空表
    let names: Vec<String> = listed
        .iter()
        .filter_map(|model| model.get("id"))
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
    assert!(!text.is_empty(), "差分后应当有正文（帧差分没吃掉内容）");
    assert!(saw_finish, "正常流应当以 finish_reason 收尾");
}

/// 把下行帧拼成正文（只取 `delta.content`）
fn delta_text(frames: &[bytes::Bytes]) -> String {
    let mut text = String::new();
    for frame in frames {
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
            if let Some(content) = choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(serde_json::Value::as_str)
            {
                text.push_str(content);
            }
        }
    }
    text
}

/// 从上游原始 SSE 里取「最终快照全文」：所有帧中最长的那段文本。
///
/// 刻意**不**用状态机自己算，否则就成了自我验证。占位段
/// （`meta_data.engine.action == "loading"`）跳过 —— 官方客户端也会把它替换掉。
fn final_snapshot_text(raw: &str) -> String {
    let mut longest = String::new();
    for line in raw.lines() {
        let Some(payload) = line.trim().strip_prefix("data: ") else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        let mut text = String::new();
        for part in value
            .get("parts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if part.pointer("/meta_data/engine/action").and_then(serde_json::Value::as_str)
                == Some("loading")
            {
                continue;
            }
            for item in part
                .get("content")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if item.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    text.push_str(item.get("text").and_then(serde_json::Value::as_str).unwrap_or(""));
                }
            }
        }
        if text.chars().count() > longest.chars().count() {
            longest = text;
        }
    }
    longest
}

/// **回归：长回答不得重复下发**（用户实测「AI 回答了两遍」）。
///
/// ── 为什么只有真上游能证明这件事 ────────────────────────────
/// 故障只在**真实帧序列**下出现：`init` 帧是增量、收尾帧是全文，两者混在一起。
/// 单测用构造帧锁得住规则，但构造帧出自我们对上游的理解 —— 上游哪天改回
/// 「每帧全量」，单测照样绿而线上又开始重复。这里拿真实字节对账：
/// **下发文本必须与上游最终快照逐字相同**（旧实现会拿到两份全文）。
///
/// 用**访客身份**跑，不往用户账号的对话列表里塞测试消息。
#[tokio::test]
#[ignore = "需要出网访问 chatglm.cn，本地联调用（cargo test -- --ignored）"]
async fn live_long_answer_is_emitted_exactly_once() {
    let device_id = sign::uuid_v4();
    let (access, _refresh, _uid) = http::guest_access(&device_id, None)
        .await
        .expect("访客换证应当成功");

    // 要一段**长**回答：只有多帧增量才能暴露「收尾全文被当增量再发一遍」
    let body = plan::build_chat_body(
        &json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "从 1 数到 20，用逗号分隔，只输出数字，不要任何解释" }] }] }),
        "glm-5.3-flash",
    );
    let raw = http::post_text(
        super::endpoints::PATH_ENGINE_STREAM,
        &access,
        &device_id,
        &body,
        None,
        "长回答联调",
    )
    .await
    .expect("长回答请求应当成功");

    let snapshot = final_snapshot_text(&raw);
    println!("[live] 上游最终快照（{} 字）= {snapshot:?}", snapshot.chars().count());
    assert!(
        snapshot.chars().count() > 20,
        "这段提问应当拿到长回答，实际 {} 字（拿构造不出发明者就证明不了重复）",
        snapshot.chars().count()
    );

    use crate::server::core::protocol::chatglm_stream::ChatFromChatglmStream;
    let mut machine = ChatFromChatglmStream::new("glm-5.3-flash");
    let mut frames = machine.push(raw.as_bytes());
    frames.extend(machine.finish());
    let emitted = delta_text(&frames);
    println!("[live] 我们下发（{} 字）= {emitted:?}", emitted.chars().count());

    assert_eq!(
        emitted, snapshot,
        "下发文本必须与上游最终快照逐字相同（旧实现这里会是两份全文）"
    );
    // 占位文案不能混进来（它在官方客户端里会被替换掉，我们发出去就收不回来）
    assert!(
        !emitted.contains("正在梳理您的需求"),
        "占位段被当成回答下发了：{emitted:?}"
    );
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

/// 单轮联调：**原样发的那条消息上游照常回答**（不依赖 `<|assistant|>` 尾巴）。
///
/// ── 为什么值得单独联调一次 ──────────────────────────────────
/// 单测只证明"我们组出来的文本没有标记"，证明不了"上游接受这种形态"。
/// 而这条改动的动机是用户实测反馈（客户端里显示出 `<|user|>` / `<|assistant|>`），
/// 一旦上游哪天开始要求那个尾巴，这里会是第一个报出来的地方。
///
/// 用**访客身份**跑：访客也能对话（本家零配置入口），因此不会往用户账号的
/// 对话列表里塞测试消息。
#[tokio::test]
#[ignore = "需要出网访问 chatglm.cn，本地联调用（cargo test -- --ignored）"]
async fn live_single_message_needs_no_marks() {
    let device_id = sign::uuid_v4();
    let (access, _refresh, _uid) = http::guest_access(&device_id, None)
        .await
        .expect("访客换证应当成功");

    let body = plan::build_chat_body(
        &json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "说三个字：你好呀" }] }] }),
        "glm-5.3-flash",
    );
    let sent = body["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert_eq!(sent, "说三个字：你好呀", "单轮必须原样发（实测：{sent:?}）");
    assert!(!sent.contains("<|"), "标记会显示在官方客户端里：{sent:?}");

    let raw = http::post_text(
        super::endpoints::PATH_ENGINE_STREAM,
        &access,
        &device_id,
        &body,
        None,
        "单轮联调",
    )
    .await
    .expect("不带任何标记的单条消息应当照常被接受");

    // 用差分状态机取正文（上游帧混着增量与全文快照，差分后才是能读的正文）
    use crate::server::core::protocol::chatglm_stream::ChatFromChatglmStream;
    let mut machine = ChatFromChatglmStream::new("glm-5.3-flash");
    let mut frames = machine.push(raw.as_bytes());
    frames.extend(machine.finish());
    let mut text = String::new();
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
            if let Some(content) = choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(serde_json::Value::as_str)
            {
                text.push_str(content);
            }
        }
    }
    println!("[live] 单轮（无标记）正文 = {text:?}");
    assert!(!text.is_empty(), "原样发的单条消息也应当有正文");
}

/// 任务中心联调：读 `center`，校验三档分类与余额口径。
///
/// ── 为什么这条只读 ──────────────────────────────────────────
/// `complete` 会**真实发放积分**（实测 +500 到账），跑一次就消耗掉一条一次性
/// 任务的额度。联调测试不该有这种副作用，所以这里只验读路径：
/// 接口通、分类切得对、`points_balance` 与余额口径一致。
///
/// ── 前置 ────────────────────────────────────────────────────
/// 需要一个**登录账号**的 access_token（访客没有任务中心，实测 401）。
/// 从环境变量读：`CHATGLM_LIVE_TOKEN` / `CHATGLM_LIVE_DEVICE`。未设置则跳过。
///
/// 跑法：
/// ```text
/// CHATGLM_LIVE_TOKEN=... CHATGLM_LIVE_DEVICE=... \
///   cargo test --lib chatglm_live -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "需要出网 + 登录账号令牌，本地联调用（cargo test -- --ignored）"]
async fn live_task_center_reads_categories() {
    let Ok(token) = std::env::var("CHATGLM_LIVE_TOKEN") else {
        println!("[live] 跳过：未设置 CHATGLM_LIVE_TOKEN");
        return;
    };
    let device_id = std::env::var("CHATGLM_LIVE_DEVICE").unwrap_or_else(|_| sign::uuid_v4());
    assert!(!token.trim().is_empty(), "令牌不能为空");

    let payload = http::get_json(
        super::endpoints::PATH_TASK_CENTER,
        token.trim(),
        &device_id,
        None,
        "任务中心联调",
    )
    .await
    .expect("任务中心读取应当成功（agent-api 要求带 User-Agent，egress 恒带 undici）");
    let result = http::check_envelope(&payload, "任务中心联调").expect("业务信封应当成功");

    let categories = result
        .get("categories")
        .and_then(serde_json::Value::as_array)
        .expect("应当返回 categories 数组");
    let codes: Vec<&str> = categories
        .iter()
        .filter_map(|category| category.get("code").and_then(serde_json::Value::as_str))
        .collect();
    println!("[live] 分类 = {codes:?}");
    assert!(codes.contains(&"newbie"), "应当有新手任务档：{codes:?}");
    assert!(codes.contains(&"daily"), "应当有每日任务档：{codes:?}");

    // 每条任务都必须有 task_id（click/complete 的唯一入参）
    for category in categories {
        for task in category
            .get("tasks")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            assert!(
                task.get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| !id.trim().is_empty()),
                "任务缺少 task_id：{task}"
            );
        }
    }

    // `points_balance` 是已折算的字符串（实测 "18644.05"），应能解析成正数
    let balance = result
        .pointer("/user_info/points_balance")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    println!("[live] points_balance = {balance:?}");
    assert!(
        balance.parse::<f64>().is_ok_and(|value| value > 0.0),
        "points_balance 应当是可解析的正数：{balance:?}"
    );
}
