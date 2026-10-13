//! MonkeyCode 上游端点与请求常量（cookie 名 / 路径 / 请求头）。
//!
//! ── 上游长什么样（逆向来源 `Acankao/MonkeyCodeReverseEngineer`）──────
//! 单体 Web 应用，网页与 API 同域（`https://monkeycode-ai.com`，
//! `mvp/config.py` 的 `BASE_URL`）；国际站在 `monkeycode-ai.net`（见
//! `region.rs` 的模块头：`.net` 站点存在，但参考未覆盖其端点的实测）。
//!
//! ```text
//!   认证    GET  /api/v1/users/status      → {"code":0,"data":{"user":{...}}}
//!   目录    GET  /api/v1/users/models      → {"code":0,"data":{"models":[...]}}
//!   任务    POST /api/v1/users/tasks       （会话转发用；见 `task.rs`）
//!   任务流  WS   /api/v1/users/tasks/stream?id=&mode=（ACP 事件；见 `stream.rs`）
//! ```
//!
//! ── cookie 名（参考 `mvp/config.py` + `docs/02-auth/03-login-methods.md`）──
//! 普通用户 `monkeycode_ai_session`，团队管理员 `monkeycode_ai_team_session`；
//! TTL 30 天、HttpOnly + Secure + SameSite=Lax。本网关只接**普通用户**这一路
//! （团队登录是另一套端点与另一套账号体系，不在本期范围）。
//!
//! ── 鉴权形态：粘贴式（**无验证码 / 无短信 / 无 OAuth 窗口**）──────────
//! 本家的密码登录要 go-cap 验证码、OAuth 要百智云 SCaptcha + 短信 —— 两条都
//! 不适合网关代跑。参考 `docs/02-auth/03-login-methods.md` 的结论：从浏览器
//! 复制 session cookie 是最省事、最可靠的一条路，因此本网关的登录入口就是
//! 「粘贴 session」+ 一次 `GET /users/status` 校验（见 `login.rs`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use super::region::Region;

/// 普通用户的 session cookie 名（`mvp/config.py::SESSION_COOKIE_NAME`）
pub const SESSION_COOKIE_NAME: &str = "monkeycode_ai_session";

/// 团队管理员的 session cookie 名（参考里同源；本网关本期不接，仅登记以免
/// 将来有人把团队 cookie 当普通 cookie 用）
pub const TEAM_SESSION_COOKIE_NAME: &str = "monkeycode_ai_team_session";

/// 登录状态校验路径（`mvp/auth.py::check_status`，也是 `docs/05-api` 认证端点表里的那条）
pub const USER_STATUS_PATH: &str = "/api/v1/users/status";

/// 模型目录路径（`mvp/models.py::list_models`，`GET {base}/api/v1/users/models`）
pub const MODELS_PATH: &str = "/api/v1/users/models";

/// 任务列表路径（`discoverImageId` 用：从已有任务里取 `image.id`）
pub const TASKS_PATH: &str = "/api/v1/users/tasks";

/// 任务列表的查询串：`discoverImageId` 取最近 5 条任务（见 `login.rs`）
pub const TASKS_DISCOVER_QUERY: &str = "?page=1&size=5";

/// 任务创建路径（`task.rs::create_task` 用）
pub const TASK_CREATE_PATH: &str = "/api/v1/users/tasks";

/// 任务流 WebSocket 路径（`?id=<taskId>&mode=new|attach`，见 `task_stream_url`）
pub const TASK_STREAM_PATH: &str = "/api/v1/users/tasks/stream";

/// 建任务默认宿主机：公共主机（参考 `task-runner.ts::DEFAULT_HOST_ID`）。
///
/// 自建 / 私有云站点的宿主机名可能不是它，因此与 `BASE_URL` 同款留了环境变量
/// 覆盖入口（`host_id()`）。注意 `resource.life` 在公共主机上最大 3 小时
/// （`docs/protocol/llm-protocol-complete.md` §4.5），本家用的 1 小时在限额内。
pub const DEFAULT_HOST_ID: &str = "public_host";

/// session 有效期（`docs/02-auth/03-login-methods.md`：30 天硬限制，**不可续期**）
pub const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;

/// 出站 User-Agent（照抄客户端/参考实现的形态；上游对陌生 UA 会加风控）
pub const USER_AGENT: &str = "monkeycode-local-proxy";

/// `Cookie: monkeycode_ai_session=…` 请求头值
pub fn cookie_header(session: &str) -> String {
    format!("{SESSION_COOKIE_NAME}={session}")
}

/// 站内 JSON 请求的公共头：`Origin` / `Referer` 伪装成站点自身的前端请求
/// （参考 `mkHeaders`），加一个可辨识的 UA。
pub fn json_headers(region: Region) -> Vec<(String, String)> {
    let origin = region.site_origin();
    vec![
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Origin".to_string(), origin.to_string()),
        ("Referer".to_string(), format!("{origin}/")),
        ("Accept".to_string(), "application/json".to_string()),
    ]
}

/// 在公共头后追加鉴权 cookie（调用方还会传 body 时由 `send_raw` 自动补
/// `Content-Type`，这里不重复加）。
pub fn authed_headers(region: Region, session: &str) -> Vec<(String, String)> {
    let mut headers = json_headers(region);
    headers.push(("Cookie".to_string(), cookie_header(session)));
    headers
}

/// 状态校验的完整 URL
pub fn status_url(region: Region) -> String {
    format!("{}{USER_STATUS_PATH}", region.base_url())
}

/// 模型目录的完整 URL
pub fn models_url(region: Region) -> String {
    format!("{}{MODELS_PATH}", region.base_url())
}

/// 任务列表（用于发现 image_id）的完整 URL
pub fn tasks_discover_url(region: Region) -> String {
    format!("{}{TASKS_PATH}{TASKS_DISCOVER_QUERY}", region.base_url())
}

/// 建任务接口的完整 URL
pub fn task_create_url(region: Region) -> String {
    format!("{}{TASK_CREATE_PATH}", region.base_url())
}

/// 建任务用的宿主机名（`MONKEYCODE_HOST_ID` / `MONKEYCODE_INTL_HOST_ID` 可覆盖）
pub fn host_id(region: Region) -> String {
    region
        .env_override("HOST_ID")
        .unwrap_or_else(|| DEFAULT_HOST_ID.to_string())
}

/// 任务流 WebSocket 的完整 URL：`wss://<站点>/api/v1/users/tasks/stream?id=&mode=`。
///
/// scheme 按参考的 `httpToWs` 转换（https → wss、http → ws，自建 / 测试站点
/// 走后者）；`mode` 由调用方给（当前只有 `new`：本次请求新建任务轮次，
/// 见 `stream.rs` 的模块头）。
pub fn task_stream_url(region: Region, task_id: &str, mode: &str) -> String {
    let base = region.base_url();
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base
    };
    format!("{ws_base}{TASK_STREAM_PATH}?id={task_id}&mode={mode}")
}

/// WebSocket 握手头：站点自身前端发起的形态（参考 `browser-headers.ts::wsHeaders`
/// 的字段集）：Origin / Cookie / Accept-Language / Cache-Control / Pragma。
///
/// UA 用本家统一的出站 UA（`USER_AGENT`，与 REST 调用同一口径；参考用的是
/// 一份 Chrome UA，本网关有意保持「可辨识的客户端」这一既有选择）。
/// `Sec-WebSocket-Version` / `Sec-WebSocket-Key` 由 tokio-tungstenite 在握手时
/// 自动补，不在这里手写（手写反而可能与库的生成逻辑打架）。
pub fn ws_headers(region: Region, session: &str) -> Vec<(String, String)> {
    let origin = region.site_origin();
    vec![
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        ("Accept-Language".to_string(), "zh-CN,zh;q=0.9,en;q=0.8".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
        ("Pragma".to_string(), "no-cache".to_string()),
        ("Origin".to_string(), origin.to_string()),
        ("Cookie".to_string(), cookie_header(session)),
    ]
}

/// 接口类型 → 上游 CLI / Agent 名（`proxy/src/task-runner.ts` 的映射，逐字核实）。
///
/// 三种 `interface_type` 决定容器里装哪个 coding agent（`docs/03-llm/02-interface-types.md`）：
///   - `openai_chat`      → `opencode`（通用 OpenAI 兼容）
///   - `openai_responses` → `codex`
///   - `anthropic`        → `claude`
///
/// 未知 / 缺失的接口类型回落 `opencode`（与参考 `task-runner.ts` 的兜底一致）
/// —— 那是「最通用」的一档，不是随意的默认值。
pub fn cli_name_for(interface_type: &str) -> &'static str {
    match interface_type.trim().to_ascii_lowercase().as_str() {
        "openai_responses" => "codex",
        "anthropic" => "claude",
        _ => "opencode",
    }
}
