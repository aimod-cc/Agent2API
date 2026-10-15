//! 软件更新路由（对照 server.mjs 749-773 行）。
//!
//!   GET  /api/update/check?current=1.0.0   检查新版本（返回里带最新版的发布说明）
//!   POST /api/update/download              开始下载 { url, name }
//!   GET  /api/update/progress              下载进度
//!   POST /api/update/cancel                取消下载
//!   GET  /api/update/proxy                 更新出网线路（null = 直连）
//!   POST /api/update/proxy                 换更新出网线路 { proxy: null | {…} }
//!   GET  /api/update/token                 GitHub 令牌状态 { filled, origin }（不回显本体）
//!   POST /api/update/token                 保存 / 清除 GitHub 令牌 { token: string | null }
//!
//! ── 容器内更新的三条（Linux headless / Docker 专有）──────────
//!   POST /api/update/install               校验并应用已下载的更新包（替换二进制与 ui）
//!   POST /api/update/rollback              回滚到上一版本（用 install 留下的备份）
//!   POST /api/update/restart               重启进程（容器由 restart 策略拉起）
//! 三条都要求 `AGENT2API_SELF_UPDATE=1`，否则 400；机制见 `core::update::selfupdate`。
//! `check` 的 `current` 缺省（网页端不传）时用服务端自己的版本号 —— 桌面壳一定
//! 会传（后端不知道自己被哪个壳打包，那个口径不能变），所以只有网页端走缺省。
//!
//! ── 路由挂法 ────────────────────────────────────────────────
//! Node 是 `path.startsWith('/api/update/')` 的前缀判定：命中前缀后逐条判
//! 方法与路径，都不匹配则落到最外层 404（**不带** `success` 信封的
//! `{"error":{"message":"Not found: <METHOD> <path>"}}`，实测如此）。
//! 所以这里用一条 `{*rest}` 通配入口 + dispatch：拆成独立 axum 路由会让
//! `/api/update/zzz` 这类未注册子路径落到全局兜底 —— 那其实也是同一个 404 形状，
//! 但 `DELETE /api/update/check` 这类「已注册路径 + 未注册方法」会变成 405 兜底，
//! 与 Node 的 404 分叉（见 http.rs 的 method_not_allowed 说明）。
//!
//! ── 错误信封 ────────────────────────────────────────────────
//! Node 的四条都在最外层大 try 里 → 失败走 errorPayload 的 **OpenAI 风格**
//! body，状态码取 UpdateError 自带的值（400 参数/域名非法、502 网络与 GitHub 侧）。
//! 实测：`{"url":"http://evil.com/a.exe"}` → 400「下载地址必须是 https」。
//! 后加的 `/releases` 沿用同一个信封（它同样返回 UpdateError）。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::proxies::{describe_account_proxy, normalize_account_proxy};
use crate::server::core::update::{set_token, status_json, UpdateError};
use crate::server::errors::{management_error, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// 更新错误 → 响应（OpenAI 风格 body，状态码保留）
fn update_error(error: UpdateError) -> Response {
    logging::log("[Update]", &format!("❌ {}", error.message));
    error.to_gateway_error().into_response()
}

/// `/api/update/{*rest}` 的入口（任意方法、任意子路径）。
pub async fn entry(State(state): State<ServerState>, request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let full_path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or("").to_string();
    let body = match axum::body::to_bytes(request.into_body(), crate::server::http::MAX_BODY_SIZE)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => return management_error(413, format!("请求体读取失败或过大: {error}")),
    };
    dispatch(&state, method, &full_path, &query, &body).await
}

/// 路径分发（判定顺序按既有四条 if 的写法顺延）
async fn dispatch(
    state: &ServerState,
    method: Method,
    full_path: &str,
    query: &str,
    body: &Bytes,
) -> Response {
    if method == Method::GET && full_path == "/api/update/check" {
        // `url.searchParams.get('current') || ''`：缺省是空串（不是 null）
        let current = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == "current")
            .map(|(_, value)| percent_decode(value))
            .unwrap_or_default();
        // 网页端（web_shim 注入的面板）不传版本号：它没有「壳打包的版本」这个
        // 概念，以服务端自己的编译期版本为准。桌面壳始终显式传值，行为不变。
        let current = if current.trim().is_empty() {
            crate::server::core::update::CURRENT_VERSION.to_string()
        } else {
            current
        };
        return match state.update().check(&current).await {
            Ok(data) => ok_json(data),
            Err(error) => update_error(error),
        };
    }

    if method == Method::GET && full_path == "/api/update/status" {
        // 最近一次「检查更新」的结果（定时任务与壳命令写入，前端 60 秒轮询
        // 读这里亮侧栏徽标，自己不再打 GitHub —— 匿名限额 60 次/小时）。
        // 本进程还没查过时是 {checked:false}，前端按「未知」处理，不亮标。
        return ok_json(state.update().last_check());
    }

    if method == Method::GET && full_path == "/api/update/proxy" {
        // 「软件更新」的出网线路（设置页更新面板里「检查更新」左侧的下拉读它）。
        // 返回与账号代理**同一个描述形态**（label / config / error）：
        // 前端据此取 listenerUid 当下拉值、取 label 显示、取 error 标红 ——
        // 三件事账号页都已经有现成口径，形状对齐就不用第二套解析。
        return ok_json(json!({
            "proxy": describe_account_proxy(config::update_proxy().as_ref()),
        }));
    }

    if method == Method::POST && full_path == "/api/update/proxy" {
        // 换线路：`{proxy: null}` = 直连，`{proxy: {source:'clash', listenerUid}}`
        // = 指定 Clash 出口。归一复用账号代理的 normalize_account_proxy
        // （custom 形状也顺带支持），非法值 400 —— 静默忽略会让界面显示
        // 「已切换」而值没存上，比报错难排查得多。
        let payload = match parse_body(body) {
            Ok(value) => value,
            Err(error) => return GatewayError::new(error.message).into_response(),
        };
        let raw = payload.get("proxy").cloned().unwrap_or(Value::Null);
        return match normalize_account_proxy(&raw) {
            Ok(normalized) => {
                // 「换」才算数：重复选中当前线路不该被当成换线路（否则就成了
                // 绕开失败冷却的口子，见下面 clear_check_cooldown 的说明）
                let changed = config::update_proxy() != normalized;
                let saved = config::set_update_proxy(normalized);
                if changed {
                    state.update().clear_check_cooldown();
                }
                // 落库失败仍返回新值（内存快照已生效，本次运行照走新线路），
                // saved 字段如实带上，调用方要提醒就用它
                ok_json(json!({
                    "proxy": describe_account_proxy(config::update_proxy().as_ref()),
                    "saved": saved,
                }))
            }
            Err(error) => {
                logging::log("[Update]", &format!("❌ 更新出网线路保存失败: {}", error.message));
                GatewayError::bad_request(error.message).into_response()
            }
        };
    }

    if method == Method::GET && full_path == "/api/update/token" {
        // 「更新设置」弹窗里 GitHub 令牌一栏的读数：**只报有没有、来自哪**，
        // 不回显本体 —— 界面保存后只显示「已填写」，这条 API 就是那三个字
        // 的唯一事实来源（回显 = 白加密）。加解密与状态口径见 token 模块。
        return ok_json(status_json());
    }

    if method == Method::POST && full_path == "/api/update/token" {
        // 保存 / 清除：`{token: "ghp_…"}` 保存、`{token: null}` 清除。
        // 缺 token 字段按请求错误处理而不是「清除」—— 清除必须显式传 null，
        // 免得某个调用方手滑发个 `{}` 就把令牌删了。校验在 token::set_token
        // （字符集白名单挡请求头注入），非法值 400。
        let payload = match parse_body(body) {
            Ok(value) => value,
            Err(error) => return GatewayError::new(error.message).into_response(),
        };
        let Some(raw) = payload.get("token").cloned() else {
            return GatewayError::bad_request("请求体缺少 token 字段（清除请传 null）").into_response();
        };
        if !raw.is_null() && raw.as_str().is_none() {
            return GatewayError::bad_request("token 必须是字符串或 null").into_response();
        }
        return match set_token(raw.as_str()) {
            Ok(update) => {
                // 令牌真的换了才清「检查更新」的失败冷却：配额桶按身份计（匿名按
                // 出口 IP、带令牌按用户），换了凭证旧冷却不再适用；重复粘贴同一个
                // 令牌不作数（token::stored_token 逐字比较，见 TokenUpdate 说明）
                if update.changed {
                    state.update().clear_check_cooldown();
                }
                // 落库失败仍返回新状态（内存已生效，本次运行照用），saved 如实带
                let mut status = status_json();
                status["saved"] = json!(update.saved);
                ok_json(status)
            }
            Err(message) => {
                logging::log("[Update]", &format!("❌ GitHub 令牌保存失败: {message}"));
                GatewayError::bad_request(message).into_response()
            }
        };
    }

    if method == Method::POST && full_path == "/api/update/download" {
        // 非法 JSON 在 Node 里是 JSON.parse 抛错 → errorPayload 500 proxy_error
        // （与 /api/auto-checkin 同一条路径），这里保持一致
        let payload = match parse_body(body) {
            Ok(value) => value,
            Err(error) => return GatewayError::new(error.message).into_response(),
        };
        let text = |key: &str| -> String {
            payload
                .get(key)
                .filter(|value| !value.is_null())
                .map(js_string)
                .unwrap_or_default()
        };
        return match state.update().start_download(&text("url"), &text("name")) {
            Ok(data) => ok_json(data),
            Err(error) => update_error(error),
        };
    }

    if method == Method::GET && full_path == "/api/update/progress" {
        return ok_json(state.update().get_progress());
    }

    if method == Method::POST && full_path == "/api/update/cancel" {
        return ok_json(state.update().cancel_download());
    }

    // ── 容器内更新（Linux headless / Docker）────────────────────
    // 三条都不接受请求体：装什么、回滚到哪儿、重启谁，全部由服务端自己决定
    // （更新包按固定名从下载目录取，路径不从请求里进 —— 见 core::update::mod）。
    // 界面目录取自 ServerState（headless 入口 set_ui_dir 装进去的），
    // 桌面包没有它（None）—— 桌面形态本来也不会走到这三条。

    if method == Method::POST && full_path == "/api/update/install" {
        let ui_dir = state.ui_dir().map(|path| path.as_path());
        return match state.update().install_downloaded(ui_dir).await {
            Ok(data) => ok_json(data),
            Err(error) => update_error(error),
        };
    }

    if method == Method::POST && full_path == "/api/update/rollback" {
        let ui_dir = state.ui_dir().map(|path| path.as_path());
        return match state.update().rollback(ui_dir) {
            Ok(data) => ok_json(data),
            Err(error) => update_error(error),
        };
    }

    if method == Method::POST && full_path == "/api/update/restart" {
        return match state.update().request_restart() {
            Ok(data) => ok_json(data),
            Err(error) => update_error(error),
        };
    }

    // 未命中任何一条 → 全局 404 的文案（Node 落到最外层 `sendJson(res, 404, …)`），
    // 注意这个 body **没有 success 字段**
    crate::server::errors::not_found_response(method.as_str(), full_path)
}

/// JS `String(value)`：非字符串也照转（手改的请求体里 name 可能是数字）。
/// 对象/数组形态只会来自构造出来的脏请求，这里给 JSON 文本即可 ——
/// 它最终会被 safe_file_name 清成安全文件名。
fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 查询串的百分号解码（`current=1.0.0-beta%2B1` 这类形态）。
/// 失败时原样返回（不是致命错误，交给版本比较处理）。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .ok()
                    .and_then(|text| u8::from_str_radix(text, 16).ok());
                match hex {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            // `+` 在查询串里代表空格（与 URLSearchParams 一致）
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
