//! Antigravity 本机目录对应的当前登录账号只读识别。
//!
//! 只读取非凭证的身份展示字段（例如 OAuth `id_token` 的邮箱声明或 `state.vscdb` 里的
//! `userStatus` / `lastLoginUsername`），绝不读取或记录任何 `access_token` / `refresh_token`。
//! 同时对历史记录施以近期时间窗口约束，避免很久以前的旧记录被误归入当前切换后的账号。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::Value;

use super::antigravity::DiscoveredRoot;
use super::protobuf::{extract_user_status_email, is_valid_email};
use super::sqlite_reader::open_readonly_with_snapshot;

/// 近期窗口（2 小时）：只有落在这个时间窗内的新请求才自动绑定「客户端当前登录账号」，
/// 更早的历史记录或手动全量重扫出来的旧请求回退到目录来源简称，避免跨号切换后误归属。
pub const RECENT_ACCOUNT_WINDOW_MS: i64 = 2 * 60 * 60 * 1000;

const MAX_OAUTH_TOKEN_FILE_BYTES: u64 = 64 * 1024;
const FUTURE_CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;

/// 无法归属具体账号时的目录来源展示名（兜底文案）。
pub fn fallback_account_label(kind: &str) -> &'static str {
    match kind {
        "desktop" => "桌面版",
        "insiders" => "Insiders",
        "cli" => "CLI (agy)",
        "t3_acp" => "T3 Code ACP",
        "wsl" => "WSL 目录",
        _ => "本机直连",
    }
}

/// 结合目录类型、探测到的当前登录邮箱、AccountStore 中的备注名与请求时间，生成 `(account_id, account_name)`。
pub fn resolve_account_identity(
    root_kind: &str,
    detected_email: Option<&str>,
    email_names: &HashMap<String, String>,
    step_ts: i64,
    now_ms: i64,
) -> (String, String) {
    let within_window = now_ms.saturating_sub(step_ts) <= RECENT_ACCOUNT_WINDOW_MS
        && step_ts <= now_ms.saturating_add(FUTURE_CLOCK_SKEW_MS);
    if within_window {
        if let Some(email) = detected_email.map(str::trim).filter(|s| !s.is_empty()) {
            let lower = email.to_ascii_lowercase();
            let account_id = format!("local-direct:antigravity:{lower}");
            let account_name = email_names
                .get(&lower)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| email.to_string());
            return (account_id, account_name);
        }
    }

    let kind_slug = if root_kind.trim().is_empty() {
        "unknown"
    } else {
        root_kind.trim()
    };
    (
        format!("local-direct:antigravity:{kind_slug}"),
        fallback_account_label(root_kind).to_string(),
    )
}

/// 探测单个 Antigravity 数据根目录当前登录的账号邮箱（只读、零阻塞、不碰任何令牌本体）。
pub fn detect_root_email(root: &DiscoveredRoot) -> Option<String> {
    if !root.exists {
        return None;
    }
    let root_path = Path::new(&root.path);
    let token_file = root_path.join("antigravity-oauth-token");
    if let Some(email) = read_email_from_oauth_token_file(&token_file) {
        return Some(email);
    }
    for vscdb in candidate_vscdb_paths(root) {
        if let Some(email) = read_email_from_vscdb(&vscdb) {
            return Some(email);
        }
    }
    None
}

/// 从 `antigravity-oauth-token`（agy CLI 登录产物）的 `id_token` JWT 载荷中读取 `email`。
pub(super) fn read_email_from_oauth_token_file(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_OAUTH_TOKEN_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(text.trim()).ok()?;
    let id_token = json.get("id_token").and_then(Value::as_str)?;
    let payload_b64 = id_token.split('.').nth(1)?.trim_end_matches('=');
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload_b64))
        .ok()?;
    let claims: Value = serde_json::from_slice(&payload_bytes).ok()?;
    let email = claims.get("email").and_then(Value::as_str)?.trim();
    is_valid_email(email).then(|| email.to_string())
}

fn query_item_table_text(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    use rusqlite::{params, types::ValueRef, OptionalExtension};
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .ok()?;
    let result = stmt
        .query_row(params![key], |row| match row.get_ref(0)? {
            ValueRef::Text(bytes) => Ok(Some(String::from_utf8_lossy(bytes).into_owned())),
            ValueRef::Blob(bytes) => Ok(Some(String::from_utf8_lossy(bytes).into_owned())),
            _ => Ok(None),
        })
        .optional()
        .ok()
        .flatten()
        .flatten();
    result
}

fn normalize_plain_username(raw: &str) -> Option<String> {
    let unquoted = raw.trim().trim_matches('"').trim();
    if is_valid_email(unquoted) {
        Some(unquoted.to_string())
    } else {
        None
    }
}

/// 通过只读快照打开 VS Code / Antigravity `state.vscdb`，依次尝试多种公开账号展示键提取邮箱。
pub(super) fn read_email_from_vscdb(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    open_readonly_with_snapshot(path, |conn| {
        if let Some(raw_status) = query_item_table_text(conn, "antigravityUnifiedStateSync.userStatus") {
            if let Some(email) = extract_user_status_email(&raw_status) {
                return Ok(Some(email));
            }
        }
        if let Some(auth_status) = query_item_table_text(conn, "antigravityAuthStatus") {
            if let Ok(val) = serde_json::from_str::<Value>(auth_status.trim()) {
                if let Some(email) = val.get("email").and_then(Value::as_str) {
                    let trimmed = email.trim();
                    if is_valid_email(trimmed) {
                        return Ok(Some(trimmed.to_string()));
                    }
                }
            }
        }
        for key in [
            "agy.onboarding.lastLoginUsername",
            "jetski.onboarding.lastLoginUsername",
        ] {
            if let Some(raw_name) = query_item_table_text(conn, key) {
                if let Some(email) = normalize_plain_username(&raw_name) {
                    return Ok(Some(email));
                }
            }
        }
        Ok(None)
    })
    .ok()
    .flatten()
}

fn app_config_roots() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(appdata));
        }
        if let Some(home) = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|v| !v.is_empty())
        {
            dirs.push(PathBuf::from(home).join("AppData").join("Roaming"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support"),
            );
        }
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(xdg));
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".config"));
        }
    }
    dirs
}

fn candidate_vscdb_paths(root: &DiscoveredRoot) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let app_names: &[&str] = match root.kind.as_str() {
        "desktop" => &["Antigravity"],
        "insiders" => &["Antigravity - Insiders", "Antigravity Insiders"],
        "custom" => {
            let folder_name = Path::new(&root.path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if folder_name.contains("insiders") {
                &["Antigravity - Insiders", "Antigravity Insiders"]
            } else if folder_name == "antigravity" {
                &["Antigravity"]
            } else {
                &[]
            }
        }
        "wsl" => {
            let root_path = Path::new(&root.path);
            let folder_name = root_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !folder_name.contains("cli") {
                if let Some(gemini_dir) = root_path.parent() {
                    if let Some(wsl_home) = gemini_dir.parent() {
                        let names: &[&str] = if folder_name.contains("insiders") {
                            &["Antigravity - Insiders", "Antigravity Insiders"]
                        } else {
                            &["Antigravity"]
                        };
                        for app in names {
                            out.push(
                                wsl_home
                                    .join(".config")
                                    .join(app)
                                    .join("User")
                                    .join("globalStorage")
                                    .join("state.vscdb"),
                            );
                        }
                    }
                }
            }
            return out;
        }
        _ => return out,
    };

    for base in app_config_roots() {
        for app in app_names {
            out.push(
                base.join(app)
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb"),
            );
        }
    }
    out
}
