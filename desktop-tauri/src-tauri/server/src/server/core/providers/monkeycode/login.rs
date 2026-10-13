//! MonkeyCode 粘贴式登录：**校验 + 归一化**（没有验证码 / 短信 / OAuth 窗口）。
//!
//! ── 链路（一步校验 + 一次 best-effort 发现）────────────────────
//! ```text
//! GET {站点}/api/v1/users/status   Cookie: monkeycode_ai_session=…
//!   → {"code":0,"data":{"user":{"id":"uuid","subscription_level":"pro"}}}
//! GET {站点}/api/v1/users/tasks?page=1&size=5
//!   → data.tasks[].image.id           （best-effort 发现 image_id）
//! ```
//! 校验端点取 `GET /api/v1/users/status` —— 参考 `mvp/auth.py::check_status`
//! 与 `docs/05-api/01-endpoint-catalog.md` 都把它列为「登录状态检查」的权威端点
//! （200 + `code:0` 即已登录）。
//!
//! ── 为什么 image_id 在这里发现而不是要求用户填 ──────────────────
//! 参考 `discoverImageId`（`proxy/src/admin-login.ts`）从**已有任务列表**的
//! `task.image.id` 里取镜像 UUID。老用户一般至少有一个历史任务，能自动拿到；
//! 新用户取不到 —— 此时**不阻断添加**，把 `imageId` 留空，由界面提示手动填写
//! （转发入口 `chat.rs` 会给出「缺少 image_id」的可读错误）。这比在添加时就拒绝
//! 一个「登录态明明有效」的账号友好得多。
//!
//! ── 归一化做什么 ────────────────────────────────────────────
//! 用户从 DevTools 复制的东西形态很杂：可能是裸值、可能是
//! `monkeycode_ai_session=xxx`、也可能是整行 `Cookie: monkeycode_ai_session=xxx;`
//! 或带引号。这里统一剥成裸值，避免把一整行当 cookie 值发给上游（那会稳定 401）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::client;
use super::endpoints;
use super::region::Region;

/// session 长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_SESSION_LENGTH: usize = 8192;

/// 归一化粘贴的 session：剥掉 `Cookie:` / `monkeycode_ai_session=` / `;` / 引号
/// 与所有空白，返回裸 cookie 值。
pub fn normalize_session(raw: &str) -> String {
    let mut text = raw.trim();
    // 整行 Cookie 头形态：`Cookie: monkeycode_ai_session=xxx;`
    if let Some(rest) = text
        .strip_prefix("Cookie:")
        .or_else(|| text.strip_prefix("cookie:"))
    {
        text = rest.trim();
    }
    // `monkeycode_ai_session=xxx` 形态（取 `=` 之后的第一个分号前的内容）
    if let Some(rest) = text.strip_prefix(endpoints::SESSION_COOKIE_NAME) {
        if let Some(value) = rest.strip_prefix('=') {
            text = value.trim();
        }
    }
    // 取第一个分号之前的内容（Cookie 串里可能带着别的 cookie）
    if let Some((head, _)) = text.split_once(';') {
        text = head.trim();
    }
    // 去掉成对的引号
    let text = text.trim_matches(|ch| ch == '"' || ch == '\'').trim();
    // 去掉任何残留的空白（cookie 值不含空白）
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 从用户对象里取展示名的候选字段（缺失时返回空串）
fn display_name_of(user: &Value) -> String {
    for key in ["name", "nickname", "display_name", "email"] {
        let value = user
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// best-effort 从已有任务列表发现 image_id（`discoverImageId` 的同款逻辑）。
///
/// 失败（无任务 / 网络错误 / 格式不认识）一律返回 None —— 这是**增强**而不是
/// 前置条件，不能因为它失败就判整个登录失败。
pub async fn discover_image_id(region: Region, session: &str) -> Option<String> {
    let url = endpoints::tasks_discover_url(region);
    let path = url
        .strip_prefix(&region.base_url())
        .unwrap_or(endpoints::TASKS_PATH);
    let payload = client::get_json(region, path, session, "任务列表查询")
        .await
        .ok()?;
    let data = payload.get("data").unwrap_or(&payload);
    let tasks = data.get("tasks").and_then(Value::as_array)?;
    for task in tasks {
        let id = task
            .pointer("/image/id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

/// 用粘贴的 session 校验登录态，返回可直接交给落账号入口的凭证对象：
/// `{session, imageId, userId, name}`。
///
/// `provided_image_id`：用户在表单里填的 image_id（可空）。非空时优先用它，
/// 否则做一次 best-effort 自动发现。
pub async fn verify_session(
    region: Region,
    raw_session: &str,
    provided_image_id: Option<&str>,
) -> Result<Value, GatewayError> {
    let session = normalize_session(raw_session);
    if session.is_empty() {
        return Err(GatewayError::with_status(400, "请粘贴 MonkeyCode 的 session cookie"));
    }
    if session.chars().count() > MAX_SESSION_LENGTH {
        return Err(GatewayError::with_status(400, "session 过长，请确认粘贴的是 cookie 值"));
    }
    let payload = client::request(
        "GET",
        region,
        endpoints::USER_STATUS_PATH,
        &session,
        None,
        "登录状态校验",
    )
    .await?;
    if client::business_code(&payload) != Some(client::SUCCESS_CODE) {
        let upstream = client::upstream_message(&payload);
        let message = if upstream.is_empty() {
            "MonkeyCode 登录态校验失败：请确认 session 有效且站点选对（国内 .com / 国际 .net）".to_string()
        } else {
            format!("MonkeyCode 登录态校验失败：{upstream}")
        };
        logging::log("[Login]", &format!("❌ {message}"));
        return Err(GatewayError::with_status(401, message));
    }
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    // user 可能落在 `data.user`（`docs/05-api` 的形态），也可能是 `data` 本身
    // （`mvp/auth.py` 的 `data.get("data", data)` 兜底）—— 两种都认。
    let user = data.get("user").cloned().unwrap_or_else(|| data.clone());
    let user_id = user
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let name = display_name_of(&user);
    let provided = provided_image_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let image_id = match provided {
        Some(value) => value.to_string(),
        None => discover_image_id(region, &session).await.unwrap_or_default(),
    };
    logging::log(
        "[Login]",
        &format!(
            "✅ MonkeyCode {} 登录态校验通过（user {}，image_id {}）",
            region.label(),
            if user_id.is_empty() { "缺失" } else { &user_id },
            if image_id.is_empty() { "未发现（需手动填）" } else { "已就绪" }
        ),
    );
    Ok(json!({
        "session": session,
        "imageId": image_id,
        "userId": user_id,
        "name": name,
    }))
}
