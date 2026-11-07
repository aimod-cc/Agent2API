use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::super::{
    clients::Plan,
    dsh_document::{self, PatchDocument},
    dsh_patch::{self, Composition, MCP_PACKAGE},
    files, paths, ClientDef, Profile,
};

pub fn config_path(def: &ClientDef) -> Result<PathBuf, String> {
    let detection = paths::detect(def);
    if detection.unsupported {
        return Err(detection
            .note
            .unwrap_or_else(|| "DSH 配置目标尚未确定".into()));
    }
    detection
        .resource_path
        .ok_or_else(|| "请先在 Agent 配置页选择 DSH 运行方式，再安装 MCP".into())
}

pub fn read(path: &Path) -> Result<Value, String> {
    let composition = Composition::load(path)?;
    let servers = dsh_patch::mcp_entries(&composition)?
        .into_iter()
        .map(|(id, config)| {
            let name = config
                .get("serverName")
                .and_then(Value::as_str)
                .unwrap_or("");
            let endpoint = config
                .get("url")
                .or_else(|| config.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let editable =
                composition.editable(&id, path).is_ok() && !dsh_document::dynamic(&config);
            json!({ "name": name, "endpoint": endpoint, "config": config, "editable": editable,
            "deletable": editable && !composition.inherited.contains_key(&id) })
        })
        .collect::<Vec<_>>();
    Ok(
        json!({ "path": path, "exists": path.is_file(), "servers": servers, "revision": revision(&composition) }),
    )
}

fn revision(composition: &Composition) -> String {
    let inputs = composition.inputs.iter().map(|input| json!({ "path": input.path, "revision": super::revision(input.before.as_deref()) })).collect::<Vec<_>>();
    super::revision(Some(json!({ "inputs": inputs, "entries": composition.entries.iter().map(|(id, entry)| (id, &entry.value)).collect::<std::collections::BTreeMap<_, _>>() }).to_string().as_bytes()))
}

pub fn plan(def: &ClientDef, payload: &Value) -> Result<Plan, String> {
    let path = config_path(def)?;
    let composition = Composition::load(&path)?;
    if payload.get("revision").and_then(Value::as_str) != Some(revision(&composition).as_str()) {
        return Err("DSH 插件配置或继承设置已变化，请刷新后重试".into());
    }
    let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-')
    {
        return Err("DSH MCP 名称必须是 1～32 位字母、数字、下划线或连字符".into());
    }
    let delete = payload.get("delete").and_then(Value::as_bool) == Some(true);
    let existing = dsh_patch::mcp_entries(&composition)?
        .into_iter()
        .find(|(_, config)| config.get("serverName").and_then(Value::as_str) == Some(name));
    let mut change = files::Change::load(path.clone())?;
    super::reject_links(&path)?;
    let original = files::text(change.before.as_deref())?;
    let text = if let Some((id, _)) = existing {
        composition.editable(&id, &path)?;
        if delete {
            dsh_patch::remove_entry(&original, &id)?
        } else {
            dsh_patch::update_config(
                &original,
                &id,
                &validate(name, payload.get("config").ok_or("缺少 MCP 配置")?)?,
            )?
        }
    } else {
        if delete {
            return Err("MCP 服务不存在".into());
        }
        let id = format!("agent2api-mcp-{name}");
        if composition.entries.contains_key(&id) {
            return Err("DSH 已有同名插件标识，请先在客户端处理冲突".into());
        }
        let config = validate(name, payload.get("config").ok_or("缺少 MCP 配置")?)?;
        PatchDocument::parse(&original)?
            .append(&json!({ "insert": [{ "id": id, "name": MCP_PACKAGE, "config": config }] }))?
    };
    let planned = Composition::preview(&path, &text)?;
    let actual = dsh_patch::mcp_entries(&planned)?
        .into_iter()
        .find(|(_, config)| config.get("serverName").and_then(Value::as_str) == Some(name));
    if delete && actual.is_some() {
        return Err("此 MCP 仍由继承的插件组合提供，不能通过删除本地覆盖来卸载".into());
    }
    if !delete
        && actual.as_ref().map(|(_, config)| config)
            != Some(&validate(
                name,
                payload.get("config").ok_or("缺少 MCP 配置")?,
            )?)
    {
        return Err("DSH 叠加后的 MCP 配置与预期不一致，已停止写入".into());
    }
    change.after = Some(text.into_bytes());
    let mut changes = vec![change];
    composition.guards(&mut changes);
    for change in &changes {
        change.check()?;
    }
    Ok(Plan {
        changes,
        warnings: vec![
            "MCP 只写入所选 DSH profile，其他模型、插件与运行方式保持不变。".into(),
            "配置文件校验通过不代表 MCP 已连接；请在 DSH 中确认工具，必要时重启所选运行方式。"
                .into(),
        ],
        profile: Profile {
            id: format!("mcp:{name}"),
            kind: "resource".into(),
            ..Default::default()
        },
        operation: "resource".into(),
    })
}

pub fn validate(name: &str, value: &Value) -> Result<Value, String> {
    let object = value.as_object().ok_or("DSH MCP 配置必须是对象")?;
    if value.to_string().len() > 256 * 1024 || dsh_document::dynamic(value) {
        return Err("DSH MCP 配置过大或包含动态表达式".into());
    }
    if object
        .get("serverName")
        .and_then(Value::as_str)
        .is_some_and(|value| value != name)
    {
        return Err("DSH MCP 的 serverName 必须与服务名称一致".into());
    }
    let transport = object
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("");
    match transport {
        "stdio" => {
            if !object
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err("DSH 本地 MCP 需要非空 command".into());
            }
            if object.get("args").is_some_and(|value| {
                !value
                    .as_array()
                    .is_some_and(|args| args.iter().all(Value::is_string))
            }) {
                return Err("DSH MCP args 必须是字符串数组".into());
            }
            string_map(object.get("env"), "env")?;
            if object.get("cwd").is_some_and(|value| !value.is_string()) {
                return Err("DSH MCP cwd 必须是字符串".into());
            }
        }
        "streamable-http" => {
            let valid = object
                .get("url")
                .and_then(Value::as_str)
                .and_then(|value| url::Url::parse(value).ok())
                .is_some_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                });
            if !valid {
                return Err("DSH MCP 需要有效的 HTTP(S) 地址".into());
            }
            string_map(object.get("headers"), "headers")?;
        }
        _ => return Err("DSH MCP transport 必须是 stdio 或 streamable-http".into()),
    }
    if object
        .get("toolCallTimeoutMs")
        .is_some_and(|value| !value.as_u64().is_some_and(|value| value > 0))
    {
        return Err("DSH MCP toolCallTimeoutMs 必须是正整数".into());
    }
    if object
        .get("failOnStartupError")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("DSH MCP failOnStartupError 必须是布尔值".into());
    }
    let mut config = value.clone();
    config["serverName"] = json!(name);
    Ok(config)
}

fn string_map(value: Option<&Value>, name: &str) -> Result<(), String> {
    if value.is_some_and(|value| {
        !value
            .as_object()
            .is_some_and(|map| map.values().all(Value::is_string))
    }) {
        return Err(format!("DSH MCP {name} 必须是字符串键值表"));
    }
    Ok(())
}
