//! 通知与告警的三条端点。
//!
//! ```text
//! GET  /api/notify         当前配置（URL 已遮 token）+ 可选事件清单
//! PUT  /api/notify         写配置（全量覆盖：enabled / events / channels）
//! POST /api/notify/test    给已配渠道发一条测试消息（同步等结果）
//! ```
//!
//! ── 为什么是「全量覆盖」而不是逐字段打补丁 ──────────────────
//! 渠道是个**数组**（可增删改序），逐字段补丁要额外定义「怎么引用第几条」
//! 与「删哪条」两套语义 —— 那是自找的复杂度。界面本来就持有完整表单，
//! 保存时整份提交最直接。与 `/api/custom-providers/update` 同一处置。
//!
//! ── 写配置后必须做的两件事 ──────────────────────────────────
//!   1. `notify::refresh_cache()` —— 刷新那个「是否可发送」的内存镜像
//!      （高频路径短路的依据，见 core::notify 的说明）；
//!   2. `dedupe::reset()` —— 清空去重台账。用户刚刚改配置（多半就是发现
//!      之前没收到消息），让被去重掉的事件立刻能重新发出，符合他的意图。
//!
//! ── 测试端点是**同步等到结果**的（与 notify() 的异步相反）────────
//! 用户点「发送测试」就是想知道通不通；异步返回一个「已排队」对他毫无价值。
//! 所以这里直接 await 投递结果，并把每个渠道的成功 / 失败原因如实回给界面
//! —— 失败原因（HTTP 状态 + 对方返回的正文）是用户唯一能据此排查的线索。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::notify::{self, dedupe, event, NotifyConfig};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

/// 响应体：配置 + 可选事件（界面一次拿到全部渲染所需的东西）
fn notify_json() -> Value {
    let kinds: Vec<Value> = event::available_kinds()
        .into_iter()
        .map(|(id, label, description)| json!({ "id": id, "label": label, "description": description }))
        .collect();
    json!({
        "config": notify::public_config(),
        "kinds": kinds,
    })
}

/// 判断一个提交上来的「遮过的 URL」是否就是旧值的遮罩形态：
/// 前缀（含 `****` 之前的部分）相同、尾 4 位相同 → 是同一个地址的遮罩。
///
/// 两者形状对不上时返回 false（那就当成真的新地址，照常校验）——
/// 宁可让用户重填，也不把 A 渠道的 token 安到 B 渠道上。
fn same_masked_shape(old_url: &str, submitted: &str) -> bool {
    let (Some(old_head), Some(new_head)) = (old_url.split("****").next(), submitted.split("****").next())
    else {
        return false;
    };
    if old_head != new_head || old_head.is_empty() {
        return false;
    }
    // 尾 4 位也要一致（遮罩保留了它，就是给这种时候对的）
    let tail = |text: &str| text.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>();
    tail(old_url) == tail(submitted)
}

/// GET /api/notify
pub async fn get_notify(State(_state): State<ServerState>) -> Response {
    ok_json(notify_json())
}

/// PUT /api/notify —— body `{enabled, events, channels:[{name,type,url}]}`
pub async fn put_notify(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    if !payload.is_object() {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    }
    // 解析走与读配置**同一套** parse（形状不对逐字段兜底），避免「写进去的
    // 形状」与「读出来的形状」两套口径 —— 那正是将来最难看的一类不一致。
    let mut parsed = NotifyConfig::parse(&payload);

    // ── 遮过的 URL 要还原成原值 ─────────────────────────────
    // 界面读回来的是 `…/hook/****1234`（后端 public_config 遮的）。用户没动过
    // 那一栏时会把遮过的值原样提交回来 —— 直接存下去等于**把真地址改成了一串
    // 星号**，渠道从此失效。这里按位置对回去：同一序号的旧渠道若与提交值
    // 形状一致（除尾 4 位外全同），就还原成旧的真值。
    //
    // 为什么按**序号**而不是按 URL 文本匹配：遮过的值无法反查（那正是遮的目的），
    // 而「第几个渠道」是界面与后端天然共享的信息（数组顺序即界面顺序）。
    let existing = NotifyConfig::load();
    for (index, channel) in parsed.channels.iter_mut().enumerate() {
        if !channel.url.contains("****") {
            continue;
        }
        if let Some(old) = existing.channels.get(index) {
            if same_masked_shape(&old.url, &channel.url) {
                channel.url = old.url.clone();
            }
        }
    }

    // ── 校验：两条硬规则 ──
    // ① 渠道 URL 必须是 http(s)：其它 scheme（file:// / 各种伪协议）
    //    在 reqwest 里会以奇怪的错误失败，早点拦下来给一句人话；
    // ② 开了总开关却一个渠道都没有 —— 那是一条都发不出去的静默失效，
    //    用户会在「出事了却没收到」时才发现。明确拒绝。
    if parsed.enabled && parsed.channels.is_empty() {
        return errors::management_error(400, "启用通知前请先至少配置一个渠道");
    }
    for channel in &parsed.channels {
        let url = channel.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return errors::management_error(
                400,
                format!("渠道「{}」的地址必须以 http:// 或 https:// 开头", channel.name),
            );
        }
    }

    if !config::update_raw_field(notify::config::CONFIG_KEY, parsed.to_value()) {
        // 写盘失败：内存快照已更新（与其它 setter 同款语义），本次运行内仍生效
        notify::log("⚠️  通知设置写入失败，本次运行内仍生效");
    }
    notify::refresh_cache();
    // 改完配置让之前被去重的事件能重新发出（理由见模块头）
    dedupe::reset();
    notify::log(if parsed.enabled {
        "通知已启用"
    } else {
        "通知已停用"
    });
    ok_json(notify_json())
}

/// POST /api/notify/test —— body 可选 `{index}`（只测第几条）
pub async fn post_notify_test(State(_state): State<ServerState>, body: Bytes) -> Response {
    // body 允许为空（不指定就全测）；空体不当错误
    let index = if body.is_empty() {
        None
    } else {
        match parse_body(&body) {
            Ok(value) => value
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            Err(error) => return errors::management_error(400, error.message),
        }
    };
    match notify::notify_test(index).await {
        Ok(results) => ok_json(json!({ "results": results })),
        Err(message) => errors::management_error(400, message),
    }
}
