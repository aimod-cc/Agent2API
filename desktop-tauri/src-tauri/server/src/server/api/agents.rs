//! 「Agent」页的管理接口：本机 AI 客户端的配置接管。
//!
//!   GET  /api/agents              全部客户端的静态信息 + 用户配置 + 安装状态
//!   POST /api/agents/profiles     保存一个客户端的配置列表（整份）
//!   POST /api/agents/apply        把某套配置写进客户端（单配置整段覆写 /
//!                                 多配置只改 enabled），写前自动备份
//!   POST /api/agents/preview      生成写入预览（不落盘）
//!   POST /api/agents/restore      按最近一次备份还原客户端文件
//!   POST /api/agents/import       从客户端现有文件里读回一套配置（首次使用）
//!
//! 契约形状（前端 `agent-page.tsx` 直接消费）：
//!   GET  → `{ clients: [{ id, name, …, installed, profiles: [...], viaGateway }] }`
//!   写操作 → 同一份 `{ clients }`（就地替换，页面不必再拉一次）
//!
//! 为什么写操作返回全量而不是单条：这一页的列表与左栏状态点都依赖
//! 「哪套生效」，返回单条会让前端自己拼状态、两处算出不同的结论。与
//! `api::proxies` 的代理池同一口径（那里也是写操作返回整份列表）。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::agents::{self, apply, Profile};
use crate::server::ServerState;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

/// 把一套 Profile 的字段从请求体里读出来（缺字段用默认值，非法值不致命）。
fn profile_from(body: &Value) -> Option<Profile> {
    let id = body.get("id").and_then(Value::as_str).unwrap_or("").trim();
    if id.is_empty() {
        return None;
    }
    Some(Profile {
        id: id.to_string(),
        name: body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .trim()
            .to_string(),
        kind: body
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("relay")
            .to_string(),
        protocol: body
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("openai_chat")
            .to_string(),
        base_url: body
            .get("baseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        provider: body
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        effort: body
            .get("effort")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string(),
        enabled: body.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        default: body.get("default").and_then(Value::as_bool).unwrap_or(false),
        note: body
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// 一个客户端的完整视图：静态定义 + 安装状态 + 用户配置 + 派生读数。
///
/// ── 两份配置合起来展示（照 cc-switch 的 import_default_config）──
/// 我们自己存的（kv `agentProfiles`）是**用户编辑过的**，优先；
/// 客户端文件里读出来的是**它现在实际在用的**。两者都给前端：
///   `profiles`  我们存的（可编辑、可切换）
///   `live`      从文件读出来的（只读事实，带 source: "live"）
/// 只在没有任何我们存的条目时，才把 live 当 `profiles` 展示 —— 否则首次
/// 打开这一页每个客户端都是「无配置」，用户还得手工抄一遍已有地址。
fn client_payload(def: &agents::ClientDef) -> Value {
    let installed = agents::is_installed(def);
    let profiles = agents::load_client(def.id);
    let (live, read_error) = if installed {
        agents::discover::read_live(def)
    } else {
        (Vec::new(), None)
    };

    // 展示用：没有自己的配置就拿 live 顶上
    let shown = if profiles.is_empty() && !live.is_empty() {
        &live
    } else {
        &profiles
    };

    let mut view = agents::client_view(def, installed);
    if let Some(object) = view.as_object_mut() {
        object.insert(
            "profiles".into(),
            Value::Array(shown.iter().map(agents::profile_view).collect()),
        );
        // live 单独给一份：前端用来显示「客户端现在指向谁」，与可编辑列表分开
        object.insert(
            "live".into(),
            Value::Array(
                live.iter()
                    .map(|item| {
                        let mut view = agents::profile_view(item);
                        if let Some(object) = view.as_object_mut() {
                            object.insert("source".into(), json!("live"));
                        }
                        view
                    })
                    .collect(),
            ),
        );
        object.insert("profilesAreLive".into(), json!(profiles.is_empty() && !live.is_empty()));
        object.insert("readError".into(), json!(read_error));
        object.insert("viaGateway".into(), json!(agents::via_gateway(shown)));
        object.insert(
            "activeId".into(),
            json!(agents::active_profile(shown).map(|item| item.id.clone())),
        );
        // 配置文件的绝对路径：前端「复制路径 / 打开所在目录」用得上
        object.insert(
            "resolvedPath".into(),
            json!(agents::resolve_path(def.config_path)
                .map(|path| path.display().to_string())
                .unwrap_or_default()),
        );
    }
    view
}

/// 本机网关地址。端口取自 `ServerState.port`（`AGENT2API_PROXY_PORT` /
/// 设置里的 `proxyPort` / 构建形态默认值，三级解析的结果），不能写死 ——
/// 用户换过端口时，写进客户端的地址必须跟着变，否则接管后请求全打到空端口。
fn gateway_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// 整页快照（所有写操作都返回它）。
fn snapshot(port: u16) -> Value {
    json!({
        "clients": agents::CLIENTS.iter().map(client_payload).collect::<Vec<_>>(),
        "gateway": {
            "baseUrl": gateway_base_url(port),
            "hint": "本机网关地址，随「网络代理」页的端口变化",
        },
    })
}

/// GET /api/agents
pub async fn list_agents(State(state): State<ServerState>) -> Response {
    ok_json(snapshot(state.port))
}

/// POST /api/agents/profiles（保存配置列表，不写客户端文件）
pub async fn save_profiles(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let client_id = payload.get("clientId").and_then(Value::as_str).unwrap_or("").trim();
    if client_id.is_empty() {
        return management_error(400, "缺少 clientId");
    }
    let Some(def) = agents::find(client_id) else {
        return management_error(404, format!("未知客户端：{client_id}"));
    };
    let Some(items) = payload.get("profiles").and_then(Value::as_array) else {
        return management_error(400, "profiles 必须是数组");
    };
    let mut profiles: Vec<Profile> = items.iter().filter_map(profile_from).collect();
    if profiles.is_empty() {
        return management_error(400, "至少要有一套配置");
    }
    // 单配置客户端：保证只有一条生效（前端已经保证了，这里再兜一层 ——
    // 手写请求或旧版本前端不该能写出「两条同时生效」的非法状态）
    if !def.multi {
        let mut seen = false;
        for item in profiles.iter_mut() {
            if item.enabled && !seen {
                seen = true;
            } else if item.enabled {
                item.enabled = false;
            }
        }
        if !seen {
            if let Some(first) = profiles.first_mut() {
                first.enabled = true;
            }
        }
        let default_id = profiles
            .iter()
            .find(|item| item.enabled)
            .map(|item| item.id.clone());
        for item in profiles.iter_mut() {
            item.default = Some(item.id.clone()) == default_id;
        }
    }
    match agents::save_client(client_id, &profiles) {
        Ok(()) => ok_json(snapshot(state.port)),
        Err(message) => management_error(500, message),
    }
}

/// POST /api/agents/apply（写进客户端文件）
///
/// body: `{ clientId, profileId? }`。不传 profileId = 按当前生效那套写。
pub async fn apply_profile(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let client_id = payload.get("clientId").and_then(Value::as_str).unwrap_or("").trim();
    if client_id.is_empty() {
        return management_error(400, "缺少 clientId");
    }
    let Some(def) = agents::find(client_id) else {
        return management_error(404, format!("未知客户端：{client_id}"));
    };
    let profiles = agents::load_client(client_id);
    let wanted = payload.get("profileId").and_then(Value::as_str).unwrap_or("");
    let profile = if wanted.is_empty() {
        agents::active_profile(&profiles).cloned()
    } else {
        profiles.iter().find(|item| item.id == wanted).cloned()
    };
    let Some(profile) = profile else {
        return management_error(400, "没有找到要写入的配置");
    };
    let Some(path) = agents::resolve_path(def.config_path) else {
        return management_error(400, "无法解析配置路径（没有用户主目录？）");
    };

    // 写前备份：只在「这次要改动客户端文件」时才备 —— 多配置的开关翻转也备，
    // 因为 enabled 的改动同样可能让客户端行为变化
    let backup_path = apply::backup(def.id, &path);

    let result = if def.multi {
        apply::apply_multi(def, &path, &profiles)
    } else {
        apply::apply_single(def, &path, &profile)
    };

    match result {
        Ok(()) => {
            let mut data = snapshot(state.port);
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "applied".into(),
                    json!({
                        "clientId": def.id,
                        "profileId": profile.id,
                        "path": path.display().to_string(),
                        "backup": backup_path.map(|item| item.display().to_string()),
                        "needsRestart": def.needs_restart,
                    }),
                );
            }
            ok_json(data)
        }
        Err(message) => management_error(400, message),
    }
}

/// POST /api/agents/preview（写入预览，不落盘）
pub async fn preview_profile(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let client_id = payload.get("clientId").and_then(Value::as_str).unwrap_or("").trim();
    let Some(def) = agents::find(client_id) else {
        return management_error(404, format!("未知客户端：{client_id}"));
    };
    // 先在我们存的列表里找；找不到再退回客户端文件里的 live 条目 ——
    // 首次打开这一页时列表就是 live 的，此时预览也该能出（它正是「原样
    // 写回去长什么样」的答案，对用户判断有没有副作用有用）。
    let profiles = agents::load_client(client_id);
    let wanted = payload.get("profileId").and_then(Value::as_str).unwrap_or("");
    let profile = if wanted.is_empty() {
        agents::active_profile(&profiles).cloned()
    } else {
        profiles.iter().find(|item| item.id == wanted).cloned()
    }
    .or_else(|| {
        let (live, _) = agents::discover::read_live(def);
        if wanted.is_empty() {
            agents::active_profile(&live).cloned()
        } else {
            live.into_iter().find(|item| item.id == wanted)
        }
    });
    let Some(profile) = profile else {
        return management_error(400, "没有找到要预览的配置");
    };
    let Some(path) = agents::resolve_path(def.config_path) else {
        return management_error(400, "无法解析配置路径");
    };
    match apply::preview(def, &path, &profile) {
        Ok(data) => ok_json(data),
        Err(message) => management_error(400, message),
    }
}

/// POST /api/agents/restore（按最近一次备份还原）
pub async fn restore_client(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let client_id = payload.get("clientId").and_then(Value::as_str).unwrap_or("").trim();
    let Some(def) = agents::find(client_id) else {
        return management_error(404, format!("未知客户端：{client_id}"));
    };
    let Some(path) = agents::resolve_path(def.config_path) else {
        return management_error(400, "无法解析配置路径");
    };
    match apply::restore_latest(def.id, &path) {
        Ok(source) => {
            let mut data = snapshot(state.port);
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "restored".into(),
                    json!({
                        "clientId": def.id,
                        "path": path.display().to_string(),
                        "from": source.display().to_string(),
                    }),
                );
            }
            ok_json(data)
        }
        Err(message) => management_error(400, message),
    }
}
/// POST /api/agents/import（把客户端文件里的现有配置收进「我的配置」）
///
/// 两种用法：
///   · 只传 `{ clientId }`        → 读出全部，返回但不落盘（前端预览用）
///   · 带 `{ clientId, adopt: true }` → 收进我们自己的列表（写 kv），之后
///     就能编辑 / 切换 / 写回
///
/// 为什么要有「收进来」这一步：客户端文件里的配置是**只读事实**，它没有我们
/// 的 id，也不该被我们直接改。用户想把它变成可编辑的一条，得先落进 kv ——
/// 这一步就是 cc-switch 的 `import_default_config` 在界面上对应的动作。
pub async fn import_client(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(_) => return management_error(400, "请求内容不是有效 JSON"),
    };
    let client_id = payload.get("clientId").and_then(Value::as_str).unwrap_or("").trim();
    let Some(def) = agents::find(client_id) else {
        return management_error(404, format!("未知客户端：{client_id}"));
    };
    let (live, read_error) = agents::discover::read_live(def);
    if live.is_empty() {
        return ok_json(json!({
            "profiles": [],
            "readError": read_error,
        }));
    }

    let adopt = payload.get("adopt").and_then(Value::as_bool).unwrap_or(false);
    if !adopt {
        return ok_json(json!({
            "profiles": live.iter().map(agents::profile_view).collect::<Vec<_>>(),
            "readError": Value::Null,
        }));
    }

    // 收进 kv：已有同 id 的条目就跳过（不覆盖用户改过的），其余追加。
    // 单配置客户端只收一条（文件里也只有一套生效），多配置全收。
    let existing = agents::load_client(def.id);
    let mut merged = existing.clone();
    let mut added = 0usize;
    for item in &live {
        if merged.iter().any(|entry| entry.id == item.id) {
            continue;
        }
        merged.push(item.clone());
        added += 1;
    }
    if !def.multi && merged.len() > 1 {
        // 单配置：只保留第一条（= 当前生效那套），其余不导入
        let first = merged.remove(0);
        merged = vec![first];
    }
    if added > 0 {
        if let Err(message) = agents::save_client(def.id, &merged) {
            return management_error(500, message);
        }
    }

    let mut data = snapshot(state.port);
    if let Some(object) = data.as_object_mut() {
        object.insert("imported".into(), json!({ "clientId": def.id, "count": added }));
    }
    ok_json(data)
}
