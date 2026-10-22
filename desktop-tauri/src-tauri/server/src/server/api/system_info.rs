//! `GET /api/system-info` —— 设置页「版本信息」的一站式读数。
//!
//! ── 为什么合成一条端点，而不是让界面拼七八个现有接口 ───────────
//! 这个页面的用途是「出问题时截一张图 / 复制一段文本」：它必须**一次取全**，
//! 而且要能在别的接口都出错时仍然给出一份完整的环境快照。散着拼的话，
//! 界面得并发打 `/api/session`、`/api/storage`、`/api/update/status`、
//! `/api/panel/status`、`/api/config` 五条，任何一条失败就得在页面上留个
//! 「—」——而那个「—」恰恰可能是排障最需要的一项。这里在服务端一次算完，
//! 失败的分项各自降级成 null，界面只负责显示。
//!
//! ── 挂 protected ────────────────────────────────────────────
//! 响应里有数据目录、可执行文件路径、Key 数量、是否 fail-closed —— 都是
//! 管理面信息，与 `/api/storage` 同级。免鉴权会把部署结构暴露给任何能访问
//! 端口的人。
//!
//! ── 只报「有没有」，不报值 ──────────────────────────────────
//! Key 只给条数、GitHub 令牌只给 filled / origin、代理只给「是否指定出口」。
//! 这条纪律与 `/api/update/token` 一致：这个页面会被截图发到公开 issue 里，
//! 任何一处回显都会变成一次凭证泄露。
//!
//! ── 路径脱敏由后端做 ────────────────────────────────────────
//! `hideUsername` 走查询参数（`?hideUsername=1`），命中的路径在主目录前缀处
//! 换成 `~`。放后端是为了让界面「复制全部信息」拿到的也是脱敏后的文本 ——
//! 前端脱敏容易在剪贴板那条路上漏掉（见 `core::system_info` 的模块头）。
//! 开关本身是**纯展示偏好**，不落库：它只影响这一次请求的返回，刷新页面
//! 回到默认（显示完整路径）——「我这次要发给别人」是个一次性决定，记在
//! 配置里反而会让下次截图悄悄少了信息。

use std::path::Path;

use axum::extract::{Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::access;
use crate::server::config;
use crate::server::core::system_info;
use crate::server::core::update::{self, selfupdate, status_json, CURRENT_VERSION};
use crate::server::db;
use crate::server::http::ok_json;
use crate::server::ServerState;

/// 查询参数：`hideUsername=1` / `true` 时把主目录前缀替换成 `~`。
#[derive(Default, serde::Deserialize)]
pub struct Params {
    #[serde(rename = "hideUsername")]
    hide_username: Option<String>,
}

/// GET /api/system-info
pub async fn get_system_info(
    State(state): State<ServerState>,
    Query(params): Query<Params>,
) -> Response {
    let hide = matches!(
        params.hide_username.as_deref().map(str::trim),
        Some("1") | Some("true") | Some("yes") | Some("on")
    );
    let show = |path: &Path| -> String {
        if hide {
            system_info::mask_path(path)
        } else {
            path.to_string_lossy().to_string()
        }
    };

    let config_dir = config::config_dir();
    let db_file = match state.db() {
        Some(db) => db.file().to_path_buf(),
        None => config_dir.join(db::FILE_NAME),
    };
    let exe = std::env::current_exe().ok();
    let db_available = state.db().is_some();
    let db_bytes = std::fs::metadata(&db_file).map(|meta| meta.len()).unwrap_or(0);

    // 库里的实际 schema 版本：库不可用时为 null（不拿程序常量冒充 —— 那会让
    // 「库没打开」看起来像「版本对得上」）。
    let db_schema = if db_available {
        db::stored_schema_version(&db_file)
    } else {
        None
    };

    let snapshot = config::current();
    let accounts = state.store().list_accounts();
    let account_list = accounts
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let account_count = account_list.len();
    let enabled_accounts = account_list
        .iter()
        .filter(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(false))
        .count();
    let limited_accounts = account_list
        .iter()
        .filter(|item| is_rate_limited(item))
        .count();

    let in_flight: usize = state
        .upstream()
        .connections()
        .snapshot()
        .into_iter()
        .map(|(_, count)| count)
        .sum();

    let stats = state.request_stats().stats();
    let requests = stats.get("total").and_then(Value::as_u64).unwrap_or(0);
    let logs = crate::server::logging::store_ref()
        .map(|store| store.stats().total)
        .unwrap_or(0);

    let token_status = status_json();
    let has_update_proxy = config::update_proxy().is_some();

    ok_json(json!({
        "app": {
            // 编译期版本：桌面壳会用它自己的运行时 package_info 覆盖显示，
            // 其余形态这就是唯一真值（与 `/api/update/check` 的缺省口径一致）
            "version": CURRENT_VERSION,
            "channel": if config::update_beta() { "beta" } else { "stable" },
            "schemaVersion": db::schema::SCHEMA_VERSION,
            "dbSchemaVersion": db_schema,
            // 模拟的上游客户端身份：上游突然报 401 / 模型清单变少时，
            // 第一句要问的就是「你报的是哪个版本」
            "upstreamClientVersion": crate::server::core::endpoints::WORKBUDDY_CLIENT_VERSION,
            "upstreamCliVersion": crate::server::core::endpoints::WORKBUDDY_CLI_VERSION,
        },
        "runtime": {
            "deployment": system_info::deployment(),
            "container": system_info::in_container(),
            "os": system_info::os(),
            "osVersion": system_info::os_version(),
            "arch": system_info::arch_label(),
            "startedAt": system_info::started_at_ms(),
            "uptimeSeconds": system_info::uptime_seconds(),
            "pid": std::process::id(),
            "debugBuild": cfg!(debug_assertions),
            "verbose": crate::server::logging::is_verbose(),
        },
        "deployment": {
            "host": state.host.to_string(),
            "port": state.port,
            "panelPort": state.panel_port,
            "uiHosted": state.ui_dir().is_some(),
            "localPanel": state.local_panel,
            "configDir": show(&config_dir),
            "dbFile": show(&db_file),
            "dbBytes": db_bytes,
            "dbAvailable": db_available,
            "logFile": show(&crate::server::logging::log_file_path_public()),
            "exePath": exe.as_deref().map(show).unwrap_or_default(),
            "installerKind": update::installer_kind(),
            "selfUpdate": selfupdate::supported(),
            "backupAvailable": selfupdate::backup_available(),
        },
        "security": {
            "adminRegistered": access::admin_registered(),
            "panelAuth": access::panel_auth_enabled(),
            "apiKeyCount": snapshot.active_api_keys().len(),
            "allowNoKey": env_flag("AGENT2API_ALLOW_NO_KEY"),
            "v1FailClosed": access::v1_fail_closed(),
            "panelGate": access::panel_gate(),
            "githubTokenFilled": token_status.get("filled").and_then(Value::as_bool).unwrap_or(false),
            "githubTokenOrigin": token_status.get("origin").cloned().unwrap_or(Value::Null),
            // 只报「有没有指定出口」：代理地址里常带账号密码，截图会泄露
            "updateProxy": has_update_proxy,
        },
        "features": {
            "debugMode": snapshot.debug_mode(),
            "sanitizeFingerprints": snapshot.sanitize_fingerprints(),
            "cors": snapshot.cors_enabled(),
            "developerRoleCompat": snapshot.developer_role_compat(),
            "keepaliveStrip": config::strip_newline_keepalive(),
            "autoContinue": config::auto_continue(),
            "autoContinueMaxRounds": config::auto_continue_max_rounds(),
            "modelsDev": config::models_dev_enabled(),
            // 报当前这一端的开关（桌面壳 / headless 各读各的），与 /api/captcha
            // 的两个分端字段同源；旧前端只认这个布尔值，保持行为不变
            "captchaEnabled": snapshot.captcha_enabled(state.local_panel),
            "updateBeta": config::update_beta(),
        },
        "stats": {
            "accounts": account_count,
            "enabledAccounts": enabled_accounts,
            "limitedAccounts": limited_accounts,
            "models": state.models().count(),
            "inFlight": in_flight,
            "requests": requests,
            "logs": logs,
            "upgradePending": state.upgrade_pending(),
        },
    }))
}

/// 账号是否处于限流冷却（与前端 `isRateLimited` 同一判据：任一模型的
/// `resetAt` 晚于当前时刻）。
fn is_rate_limited(account: &Value) -> bool {
    let Some(limits) = account.get("rateLimits").and_then(Value::as_object) else {
        return false;
    };
    let now = crate::server::logging::now_ms();
    limits.values().any(|info| {
        info.get("resetAt")
            .and_then(Value::as_i64)
            .map(|reset| reset > now)
            .unwrap_or(false)
    })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}
