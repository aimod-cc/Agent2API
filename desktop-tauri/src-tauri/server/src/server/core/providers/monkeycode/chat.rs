//! MonkeyCode 会话转发入口（装配层）：建任务 → 连任务流 → 翻译成 OpenAI 流。
//!
//! ── 这一层解决什么问题 ──────────────────────────────────────
//! 一次客户端请求在上游是**多步会话**（`adapter.rs` 的 `is_stateful` 为 true，
//! 编排层分流到这里）：
//! ```text
//!   ① credentials 快照（session + image_id；缺 image_id 直接报可读错误）
//!   ② 模型名 → 目录条目（upstreamId / cliName）        models::resolve_task_model
//!   ③ OpenAI messages → prompt（system 单列）          task::build_prompt
//!   ④ POST /api/v1/users/tasks 建任务                  task::create_task
//!   ⑤ 连 wss://…/tasks/stream?id=&mode=new             stream::connect
//!   ⑥ 发 auto-approve + user-input                     stream::start_task
//!   ⑦ 消费 ACP 事件 → 本网关的 OpenAI 兼容流            stream::drive_stream
//! ```
//! 四 ~ 六步失败时**还没有给客户端任何字节**，因此按普通网关错误返回（带状态码，
//! 编排层照常换账号 / 透传）；七步之后头已发出，错误只能进流内（见 `stream.rs`）。
//!
//! ── 分工（哪些判断不属于这里）───────────────────────────────
//!   账号选路、限额冷却、provider 轮询、telemetry 记账都在编排层
//!   （`upstream::provider_loop`）；单请求内每一步的协议细节在 `task.rs` /
//!   `stream.rs`。本文件只做装配与两个小决策：**流式还是聚合**、**哪些失败
//!   在返回前发生**。
//!
//! ── 非流式怎么处理（与 catpaw / kuku 同一取舍）────────────────
//! `stream=false` 时仍走同一条任务流（事件流是上游唯一产出通道），由
//! `stream::drive_aggregate` 把同一份翻译状态聚合成完整 `chat.completion`。
//! 与流式路径唯一的差别是错误可以带状态码返回（没下发过头），因此额度 / 鉴权
//! 这类错误在非流式下仍能走到编排层的账号轮换；流式下只能在流内收尾。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；持锁不跨 await
//! （本文件不持任何锁，账号读取是一次同步快照）。

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use tokio_stream::wrappers::ReceiverStream;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::region::Region;
use super::{credentials, models, stream, task};

/// 转发入口（`adapter.rs::forward_conversation` 转调）。
pub async fn run_chat(
    store: &AccountStore,
    region: Region,
    account_id: &str,
    body: &Value,
    proxy: Option<ResolvedProxy>,
    stream_requested: bool,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<ForwardOutcome, GatewayError> {
    let record = store.monkeycode_account_record(region, account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            401,
            format!(
                "MonkeyCode {} 账号 {account_id} 不存在（请重新添加）",
                region.label()
            ),
        ));
    }
    let credentials = credentials::from_record(record.as_ref())?;
    if !credentials.ready_for_task() {
        return Err(GatewayError::with_status(
            400,
            format!(
                "MonkeyCode {} 账号缺少创建任务所需的 image_id（VM 镜像 UUID）：\
                 请在账号页编辑该账号填入（浏览器登录后，DevTools → Network → \
                 POST /api/v1/users/tasks 请求里的 image_id），或先在官网跑一个任务让网关自动发现",
                region.label()
            ),
        ));
    }
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if requested.is_empty() {
        return Err(GatewayError::bad_request("请求缺少 model"));
    }
    let target = models::resolve_task_model(region, &requested).ok_or_else(|| {
        GatewayError::with_status(
            503,
            format!(
                "MonkeyCode 模型目录里找不到模型「{requested}」：请在模型管理页刷新清单后重试"
            ),
        )
    })?;
    let prompt = task::build_prompt(body.get("messages"))?;
    logging::verbose(
        "[MonkeyCode]",
        &format!(
            "会话转发 model={} → upstream={} cli={} stream={} account={} region={}",
            requested,
            target.model,
            target.cli_name,
            stream_requested,
            if account_id.is_empty() { "(队首账号)" } else { account_id },
            region.id(),
        ),
    );
    // ① 建任务（失败在返回前：错误带状态码，编排层照常换账号 / 透传）
    let task_id = task::create_task(
        region,
        &credentials,
        &target,
        &prompt,
        proxy.as_ref(),
        telemetry,
    )
    .await?;
    // ② 连任务流并发出起始交互（同上，失败在返回前）
    let mut connection = stream::connect(region, &credentials, &task_id, proxy.as_ref()).await?;
    stream::start_task(&mut connection, &prompt.content).await?;
    let translator = stream::Translator::new(
        format!("chatcmpl-{task_id}"),
        requested,
        include_usage(body),
    );
    if stream_requested {
        // 流式：后台任务喂 mpsc，主路径返回 ReceiverStream（与 CatPaw / Kuku 同结构）。
        // 客户端断开 → 发送失败 → 后台任务停止消费并关闭 WS。
        let (sender, receiver) =
            tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
        let telemetry = telemetry.clone();
        crate::spawn_task(async move {
            stream::drive_stream(connection, translator, sender, &telemetry).await;
        });
        Ok(ForwardOutcome::Stream {
            status: 200,
            stream: Box::new(ReceiverStream::new(receiver)),
        })
    } else {
        let body = stream::drive_aggregate(connection, translator, telemetry).await?;
        Ok(ForwardOutcome::Completion { body })
    }
}

/// `stream_options.include_usage === true`（只认布尔真值，与 catpaw 同口径：
/// 字符串 / 数字不算，少一帧比多一帧更容易被客户端发现）。
fn include_usage(body: &Value) -> bool {
    body.pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
