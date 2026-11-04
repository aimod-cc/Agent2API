//! 工具服务的「本机接入」：把 Skill 包 / MCP 配置写进本机客户端。
//!
//! 与 OmniProxy 桌面端安装器的目标一致，但落点复用本项目「Agent 配置」页已有的
//! 客户端路径表（`agents::resources::skill_roots` / MCP 配置文件），不另维护一份
//! 客户端清单：
//!   · Skill：写到客户端的用户级 skills 目录（`<root>/web-search/…`），
//!     包含 SKILL.md、skill.json、运行时脚本与 `.agent2api.json`（API Key）；
//!   · MCP：改写客户端 MCP 配置里本服务那一项（JSON / JSONC / TOML / YAML 由
//!     既有写入器处理），HTTP 形态填 url + Authorization，视觉分析还可以用
//!     本地桥接（stdio）形态以支持本地图片路径。
//!
//! 所有写入都走既有的预览 + 确认令牌（MCP）或原子写（Skill），失败不会留下
//! 半套配置。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::server::core::agents::{self, resources};

#[derive(Clone, Debug)]
pub struct InstallOutcome {
    pub client_id: String,
    pub label: String,
    pub ok: bool,
    pub path: String,
    pub error: Option<String>,
}

impl InstallOutcome {
    fn ok(client_id: &str, label: &str, path: impl Into<String>) -> Self {
        Self { client_id: client_id.into(), label: label.into(), ok: true, path: path.into(), error: None }
    }
    fn failed(client_id: &str, label: &str, error: impl Into<String>) -> Self {
        Self { client_id: client_id.into(), label: label.into(), ok: false, path: String::new(), error: Some(error.into()) }
    }
    pub fn to_json(&self) -> Value {
        json!({
            "clientId": self.client_id,
            "label": self.label,
            "ok": self.ok,
            "path": self.path,
            "error": self.error,
        })
    }
}

fn skill_dir(root: &Path, service: &str) -> PathBuf {
    root.join(super::package_name(service))
}

fn marker_is_ours(target: &Path, service: &str) -> bool {
    let Ok(Some(bytes)) = agents::files::read(&target.join("skill.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value.get("name").and_then(Value::as_str) == Some(super::package_name(service))
}

/* ─── Skill 目标 ───────────────────────────────────────────── */

pub fn skill_targets(service: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    for def in agents::CLIENTS {
        let roots = resources::skill_roots(def);
        if roots.is_empty() {
            continue;
        }
        let existing = roots.iter().map(|root| skill_dir(root, service)).find(|path| path.exists());
        let installed = existing.as_ref().is_some_and(|path| marker_is_ours(path, service));
        let path = existing.unwrap_or_else(|| skill_dir(&roots[0], service));
        rows.push(json!({
            "key": def.id,
            "label": def.name,
            "path": path.display().to_string(),
            "detected": crate::server::core::agents::paths::detect(def).detected,
            "installed": installed,
            "supported": true,
        }));
    }
    rows
}

pub fn install_skill(
    service: &str,
    variant: &str,
    base_url: &str,
    api_key: &str,
    client_ids: &[String],
) -> Vec<InstallOutcome> {
    let mut outcomes = Vec::new();
    if service != super::WEB_SEARCH_SERVICE_ID {
        return client_ids
            .iter()
            .map(|id| InstallOutcome::failed(id, id, "该服务没有 Skill 安装包"))
            .collect();
    }
    let files = super::build_skill_files(base_url, variant);
    let mut contents: Vec<(String, String)> = files
        .into_iter()
        .map(|file| (file.path.to_string(), file.content))
        .collect();
    contents.push((
        ".agent2api.json".into(),
        format!("{}\n", serde_json::to_string_pretty(&json!({ "api_key": api_key })).unwrap_or_default()),
    ));

    for client_id in client_ids {
        let Some(def) = agents::find(client_id) else {
            outcomes.push(InstallOutcome::failed(client_id, client_id, "未知客户端"));
            continue;
        };
        let roots = resources::skill_roots(def);
        if roots.is_empty() {
            outcomes.push(InstallOutcome::failed(client_id, def.name, "该客户端不支持 Skill"));
            continue;
        }
        // 已装过的那一份优先（保持路径稳定），否则装到该客户端的首选根目录
        let target = roots
            .iter()
            .map(|root| skill_dir(root, service))
            .find(|path| path.exists())
            .unwrap_or_else(|| skill_dir(&roots[0], service));
        match write_skill_pack(&target, &contents) {
            Ok(()) => outcomes.push(InstallOutcome::ok(client_id, def.name, target.display().to_string())),
            Err(error) => outcomes.push(InstallOutcome::failed(client_id, def.name, error)),
        }
    }
    outcomes
}

fn write_skill_pack(target: &Path, files: &[(String, String)]) -> Result<(), String> {
    resources::reject_links(target)?;
    if target.exists() && !target.is_dir() {
        return Err(format!("{} 已存在且不是目录", target.display()));
    }
    std::fs::create_dir_all(target).map_err(|error| format!("创建 Skill 目录失败：{error}"))?;
    for (relative, content) in files {
        let path = target.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("创建 Skill 子目录失败：{error}"))?;
        }
        agents::files::write_atomic(&path, content.as_bytes())?;
    }
    Ok(())
}

pub fn uninstall_skill(service: &str, client_ids: &[String]) -> Vec<InstallOutcome> {
    let mut outcomes = Vec::new();
    for client_id in client_ids {
        let Some(def) = agents::find(client_id) else {
            outcomes.push(InstallOutcome::failed(client_id, client_id, "未知客户端"));
            continue;
        };
        let mut removed = Vec::new();
        for root in resources::skill_roots(def) {
            let target = skill_dir(&root, service);
            if !target.exists() || !marker_is_ours(&target, service) {
                continue;
            }
            if std::fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
                outcomes.push(InstallOutcome::failed(client_id, def.name, "Skill 目录是链接，拒绝删除"));
                continue;
            }
            match std::fs::remove_dir_all(&target) {
                Ok(()) => removed.push(target.display().to_string()),
                Err(error) => outcomes.push(InstallOutcome::failed(client_id, def.name, format!("删除失败：{error}"))),
            }
        }
        if !removed.is_empty() {
            outcomes.push(InstallOutcome::ok(client_id, def.name, removed.join(", ")));
        } else if !outcomes.iter().any(|outcome| outcome.client_id == *client_id) {
            outcomes.push(InstallOutcome::failed(client_id, def.name, "未安装"));
        }
    }
    outcomes
}

/* ─── MCP 目标 ─────────────────────────────────────────────── */

fn server_name(service: &str) -> &'static str {
    super::package_name(service)
}

pub fn mcp_targets(service: &str) -> Vec<Value> {
    let name = server_name(service);
    let mut rows = Vec::new();
    for def in agents::CLIENTS {
        let Some(path) = resources::mcp_config_path(def) else {
            continue;
        };
        let installed = resources::snapshot(def, "mcp")
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("servers")
                    .and_then(Value::as_array)
                    .map(|servers| servers.iter().any(|server| server.get("name").and_then(Value::as_str) == Some(name)))
            })
            .unwrap_or(false);
        rows.push(json!({
            "key": def.id,
            "label": def.name,
            "path": path.display().to_string(),
            "detected": crate::server::core::agents::paths::detect(def).detected,
            "installed": installed,
            "supported": true,
        }));
    }
    rows
}

/// 视觉本地桥接脚本的落点（写一次、安装时复用）
fn ensure_bridge_script() -> Result<PathBuf, String> {
    let dir = crate::paths::config_dir().join("tools");
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建工具目录失败：{error}"))?;
    let path = dir.join("vision-bridge.js");
    let current = std::fs::read(&path).ok();
    if current.as_deref() != Some(super::VISION_BRIDGE_SCRIPT.as_bytes()) {
        agents::files::write_atomic(&path, super::VISION_BRIDGE_SCRIPT.as_bytes())?;
    }
    Ok(path)
}

fn timeout_entry(def: &agents::ClientDef, timeout: u64) -> Value {
    if timeout == 0 {
        return Value::Null;
    }
    let millis = timeout.saturating_mul(1000);
    match def.id {
        "zcode" => json!({ "timeoutMs": millis }),
        "claude-code" => json!({ "timeout": millis }),
        "codex" => json!({ "tool_timeout_sec": timeout }),
        _ => Value::Null,
    }
}

fn merge_entry(mut entry: Value, extra: Value) -> Value {
    if let (Some(object), Some(extra)) = (entry.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    entry
}

fn mcp_entry(
    def: &agents::ClientDef,
    service: &str,
    url: &str,
    api_key: &str,
    bridge: Option<&Path>,
    timeout: u64,
) -> Value {
    let authorization = format!("Bearer {api_key}");
    let extra = timeout_entry(def, timeout);
    let base = match bridge {
        Some(path) => {
            let bridge_path = path.display().to_string();
            let mut env = json!({
                "AGENT2API_VISION_ENDPOINT": url,
                "AGENT2API_VISION_KEY": api_key,
            });
            if timeout > 0 {
                env["AGENT2API_VISION_TIMEOUT_MS"] = json!(timeout.saturating_mul(1000) + 60_000);
            }
            match def.id {
                "opencode" => json!({ "type": "local", "command": ["node", bridge_path], "environment": env }),
                _ => json!({ "command": "node", "args": [bridge_path], "env": env }),
            }
        }
        None => match def.id {
            "opencode" => json!({
                "type": "remote",
                "url": url,
                "headers": { "Authorization": authorization },
            }),
            "gemini-cli" => json!({
                "httpUrl": url,
                "headers": { "Authorization": authorization },
            }),
            "codex" => json!({
                "url": url,
                "http_headers": { "Authorization": authorization },
            }),
            _ => json!({
                "type": "http",
                "url": url,
                "headers": { "Authorization": authorization },
            }),
        },
    };
    let _ = service;
    merge_entry(base, extra)
}

fn write_mcp_entry(def: &agents::ClientDef, name: &str, config: Value) -> Result<String, String> {
    let snapshot = resources::snapshot(def, "mcp")?;
    let revision = snapshot.get("revision").and_then(Value::as_str).unwrap_or("absent");
    let mut payload = json!({
        "kind": "mcp",
        "name": name,
        "config": config,
        "revision": revision,
    });
    let preview = resources::create_preview(def, &payload)?;
    if preview.get("hasChanges").and_then(Value::as_bool) == Some(false) {
        return Ok(resources::mcp_config_path(def).map(|path| path.display().to_string()).unwrap_or_default());
    }
    payload["token"] = preview.get("token").cloned().unwrap_or(Value::Null);
    resources::apply(def, &payload)?;
    Ok(resources::mcp_config_path(def).map(|path| path.display().to_string()).unwrap_or_default())
}

fn remove_mcp_entry(def: &agents::ClientDef, name: &str) -> Result<bool, String> {
    let snapshot = resources::snapshot(def, "mcp")?;
    let exists = snapshot
        .get("servers")
        .and_then(Value::as_array)
        .is_some_and(|servers| servers.iter().any(|server| server.get("name").and_then(Value::as_str) == Some(name)));
    if !exists {
        return Ok(false);
    }
    let revision = snapshot.get("revision").and_then(Value::as_str).unwrap_or("absent");
    let mut payload = json!({
        "kind": "mcp",
        "name": name,
        "delete": true,
        "revision": revision,
    });
    let preview = resources::create_preview(def, &payload)?;
    payload["token"] = preview.get("token").cloned().unwrap_or(Value::Null);
    resources::apply(def, &payload)?;
    Ok(true)
}

pub fn install_mcp(
    service: &str,
    mode: &str,
    base_url: &str,
    api_key: &str,
    client_ids: &[String],
) -> Vec<InstallOutcome> {
    let name = server_name(service);
    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        super::endpoint(service, "mcp")
    );
    let timeout = super::timeout_seconds(service);
    let bridge = if service == super::VISION_SERVICE_ID && mode == "bridge" {
        match ensure_bridge_script() {
            Ok(path) => Some(path),
            Err(error) => {
                return client_ids
                    .iter()
                    .map(|client_id| {
                        let label = agents::find(client_id).map(|def| def.name).unwrap_or(client_id.as_str());
                        InstallOutcome::failed(client_id, label, error.clone())
                    })
                    .collect();
            }
        }
    } else {
        None
    };

    let mut outcomes = Vec::new();
    for client_id in client_ids {
        let Some(def) = agents::find(client_id) else {
            outcomes.push(InstallOutcome::failed(client_id, client_id, "未知客户端"));
            continue;
        };
        if resources::mcp_config_path(def).is_none() {
            outcomes.push(InstallOutcome::failed(client_id, def.name, "该客户端不支持 MCP 配置写入"));
            continue;
        }
        let config = mcp_entry(def, service, &url, api_key, bridge.as_deref(), timeout);
        match write_mcp_entry(def, name, config) {
            Ok(path) => outcomes.push(InstallOutcome::ok(client_id, def.name, path)),
            Err(error) => outcomes.push(InstallOutcome::failed(client_id, def.name, error)),
        }
    }
    outcomes
}

pub fn uninstall_mcp(service: &str, client_ids: &[String]) -> Vec<InstallOutcome> {
    let name = server_name(service);
    let mut outcomes = Vec::new();
    for client_id in client_ids {
        let Some(def) = agents::find(client_id) else {
            outcomes.push(InstallOutcome::failed(client_id, client_id, "未知客户端"));
            continue;
        };
        match remove_mcp_entry(def, name) {
            Ok(true) => outcomes.push(InstallOutcome::ok(
                client_id,
                def.name,
                resources::mcp_config_path(def).map(|path| path.display().to_string()).unwrap_or_default(),
            )),
            Ok(false) => outcomes.push(InstallOutcome::failed(client_id, def.name, "未配置")),
            Err(error) => outcomes.push(InstallOutcome::failed(client_id, def.name, error)),
        }
    }
    outcomes
}
