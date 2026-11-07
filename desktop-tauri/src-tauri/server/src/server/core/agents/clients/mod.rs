mod claude;
mod claude_models;
mod cline;
mod codex;
mod codex_models;
mod crush;
mod crush_models;
mod delete;
mod dsh;
mod dsh_config;
mod edit;
mod gemini;
mod kimi;
mod opencode;
mod readonly;
mod workbuddy;
mod zcode;
mod zcode_models;
mod zed;

use serde_json::{json, Value};
use std::path::Path;

use super::{
    files::{self, Change},
    ClientDef, Model, Profile,
};

pub struct Plan {
    pub changes: Vec<Change>,
    pub warnings: Vec<String>,
    pub profile: Profile,
    pub operation: String,
}

pub fn read(def: &ClientDef, path: &Path, value: &Value) -> Result<Vec<Profile>, String> {
    match def.id {
        "claude-code" => claude::read(value),
        "codex" => codex::read(path, value),
        "gemini-cli" => gemini::read(path, value),
        "opencode" => opencode::read(value),
        "workbuddy" => workbuddy::read(value),
        "zcode" => zcode::read(value),
        "cline" => cline::read(path, value),
        "kimi-code" => kimi::read(path, value),
        "zed" => zed::read(value),
        "crush" => crush::read_effective(path, value),
        "dsh" => dsh::read(path, value),
        "qoder" | "cursor-cli" => readonly::read(def, value),
        _ => Err("没有经过验证的客户端适配器".into()),
    }
}

pub fn plan(
    def: &ClientDef,
    path: &Path,
    profile: &Profile,
    operation: &str,
    port: u16,
) -> Result<Plan, String> {
    super::validate::profile(def, profile)?;
    super::validate::require_gateway_url(profile, port)?;
    if !def.writable {
        return Err(def.note.into());
    }
    let detection = super::paths::detect(def);
    if detection.unsupported {
        return Err(detection.note.unwrap_or_else(|| def.note.into()));
    }
    if def.id == "claude-code" && !profile.models.is_empty() {
        let version = detection.version.split('.').take(3).map(|part| part.parse::<u32>().ok()).collect::<Option<Vec<_>>>();
        if let Some(version) = version.filter(|version| version.len() == 3) {
            if version.as_slice() < [2, 1, 242].as_slice() {
                return Err("当前 Claude Code 版本不支持多模型菜单，请先升级到 2.1.242 或更新版本".into());
            }
            if profile.models.iter().any(|model| !model.effort.is_empty()) && version.as_slice() < [2, 1, 251].as_slice() {
                return Err("当前 Claude Code 版本不支持逐模型思考设置，请先升级到 2.1.251 或更新版本".into());
            }
        }
    }
    if def.id == "dsh" && super::dsh_paths::is_patch(path) {
        return dsh_config::plan(path, profile, operation);
    }
    let mut change = Change::load(path.to_path_buf())?;
    let original = files::text(change.before.as_deref())?;
    let mut value = if def.id == "workbuddy" && original.trim().is_empty() {
        json!([])
    } else {
        files::parse(def.format, &original)?
    };
    let mut warnings = vec![def.note.into()];
    let mut extra = Vec::new();
    match def.id {
        "claude-code" => claude::write(&mut value, profile, &mut warnings)?,
        "codex" => extra = codex::write(path, &mut value, profile, operation, &mut warnings)?,
        "gemini-cli" => extra = gemini::write(path, &mut value, profile, &mut warnings)?,
        "opencode" => opencode::write(&mut value, profile, operation)?,
        "workbuddy" => workbuddy::write(&mut value, profile, &mut warnings)?,
        "zcode" => zcode::write(&mut value, profile)?,
        "cline" => extra = cline::write(path, &mut value, profile)?,
        "kimi-code" => kimi::write(path, &mut value, profile, operation, &mut warnings)?,
        "zed" => zed::write(&mut value, profile, operation, &mut warnings)?,
        "crush" => extra = crush::write(path, &mut value, profile, operation)?,
        "dsh" => extra = dsh::write(path, &mut value, profile, operation, &mut warnings)?,
        _ => return Err("这个客户端当前仅支持读取".into()),
    }
    let mut writable_fields = super::model_config::declared_fields(def, path);
    if def.id == "kimi-code" && profile.protocol != "anthropic" {
        if let Some(fields) = writable_fields.as_array_mut() { fields.retain(|field| field != "maxOutputTokens"); }
    }
    let unrepresented = profile.models.iter().any(|model| [
        ("contextWindow", model.context_window.is_some()), ("maxOutputTokens", model.max_output_tokens.is_some()),
        ("supportsTools", model.supports_tools.is_some()), ("supportsImages", model.supports_images.is_some()),
        ("supportsVideo", model.supports_video.is_some()), ("supportsReasoning", model.supports_reasoning.is_some()),
    ].iter().any(|(key, present)| *present && !writable_fields.as_array().is_some_and(|fields| fields.iter().any(|field| field == key))));
    if unrepresented {
        warnings.push("模型能力来自网关目录；此客户端没有对应配置项的能力不会写入，其余字段按模型分别保存。".into());
    }
    change.after = Some(if def.id == "workbuddy" && original.trim().is_empty() {
        serde_json::to_vec_pretty(&value).map_err(|e| format!("生成模型数组失败: {e}"))?
    } else {
        files::render(def.format, &original, &value)?
    });
    let mut changes = vec![change];
    changes.extend(extra);
    for change in &changes {
        change.check()?;
    }
    let mut plan = Plan {
        changes,
        warnings,
        profile: profile.clone(),
        operation: operation.into(),
    };
    edit::sync_cached_config(def, path, &mut plan, profile)?;
    Ok(plan)
}

pub fn equivalent(def: &ClientDef, path: &Path, saved: &Profile, live: &Profile) -> bool {
    let fields = super::model_config::declared_fields(def, path);
    let supports = |key: &str| fields.as_array().is_some_and(|fields| fields.iter().any(|field| field == key))
        && !(def.id == "kimi-code" && key == "maxOutputTokens" && saved.protocol != "anthropic");
    let capability_matches = |key: &str, expected: Option<bool>, current: Option<bool>| {
        !supports(key) || expected.is_none() || (def.id == "kimi-code" && expected == Some(false)) || expected == current
    };
    let per_model_effort = saved.models.iter().any(|model| !model.effort.is_empty() && model.effort != "default");
    let base = |url: &str| {
        url.trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .to_string()
    };
    if base(&saved.base_url) != base(&live.base_url) || saved.protocol != live.protocol {
        return false;
    }
    if def.toggle && saved.enabled != live.enabled {
        return false;
    }
    if def.behavior == "single" && saved.model != live.model {
        return false;
    }
    if matches!(def.id, "zcode" | "opencode" | "kimi-code" | "zed" | "crush" | "dsh") {
        let target = native_id(saved);
        if !live.native.source_id.is_empty() && live.native.source_id != target {
            return false;
        }
    }
    if def.models {
        let mut a = saved.models.clone();
        let mut b = live.models.clone();
        a.sort_by(|a, b| a.id.cmp(&b.id));
        b.sort_by(|a, b| a.id.cmp(&b.id));
        if a.iter().any(|expected| {
            b.iter()
                .find(|current| current.id == expected.id)
                .is_none_or(|current| {
                    (!expected.name.is_empty()
                        && def.id != "zcode"
                        && expected.name != current.name)
                        || (supports("contextWindow") && expected.context_window.is_some_and(|value| Some(value) != current.context_window))
                        || (supports("maxOutputTokens") && expected.max_output_tokens.is_some_and(|value| Some(value) != current.max_output_tokens))
                        || (def.effort && !matches!(def.id, "cline" | "dsh") && !(def.id == "kimi-code" && super::paths::kimi_legacy(path))
                            && expected.effort != current.effort)
                        || !capability_matches("supportsTools", expected.supports_tools, current.supports_tools)
                        || !capability_matches("supportsImages", expected.supports_images, current.supports_images)
                        || !capability_matches("supportsVideo", expected.supports_video, current.supports_video)
                        || !capability_matches("supportsReasoning", expected.supports_reasoning, current.supports_reasoning)
                })
        }) {
            return false;
        }
    } else if saved.model != live.model {
        return false;
    }
    if def.id == "claude-code" && [(&saved.claude_models.opus, &live.claude_models.opus),
        (&saved.claude_models.sonnet, &live.claude_models.sonnet), (&saved.claude_models.haiku, &live.claude_models.haiku),
        (&saved.claude_models.subagent, &live.claude_models.subagent)].iter().any(|(saved, live)| !saved.is_empty() && saved != live) {
        return false;
    }
    if !saved.effort.is_empty() && saved.effort != "default" && saved.effort != live.effort
        && !(matches!(def.id, "claude-code" | "codex") && per_model_effort) {
        return false;
    }
    match super::validate::credential(saved) {
        Ok(Some(key)) if def.id != "zed" && key != live.api_key => return false,
        Err(_) => return false,
        _ => {}
    }
    if saved.auth_mode == "env" && saved.env_key != live.env_key {
        return false;
    }
    true
}

pub use edit::{preserve_selection, remove_models};

pub fn read_dsh(path: &Path) -> Result<Vec<Profile>, String> { dsh_config::read(path) }

pub fn delete_plan(def: &ClientDef, path: &Path, profile: &Profile) -> Result<Plan, String> {
    if def.id == "dsh" && super::dsh_paths::is_patch(path) { return dsh_config::plan(path, profile, "native-delete"); }
    delete::plan(def, path, profile)
}

pub fn native_id(profile: &Profile) -> String {
    if !profile.native.source_id.is_empty() {
        return profile.native.source_id.clone();
    }
    format!("agent2api-{}", profile.id)
}

pub fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").into()
}

pub fn set(value: &mut Value, path: &[&str], item: Value) -> Result<(), String> {
    let (key, parents) = path.split_last().ok_or("配置路径不能为空")?;
    let mut cursor = value;
    for parent in parents {
        let object = cursor
            .as_object_mut()
            .ok_or_else(|| format!("配置字段 {parent} 不是对象，拒绝覆盖"))?;
        cursor = object
            .entry((*parent).to_string())
            .or_insert_with(|| json!({}));
    }
    cursor
        .as_object_mut()
        .ok_or_else(|| format!("配置字段 {key} 的父级不是对象"))?
        .insert((*key).into(), item);
    Ok(())
}

pub fn remove(value: &mut Value, path: &[&str]) {
    let Some((key, parents)) = path.split_last() else {
        return;
    };
    let mut cursor = value;
    for parent in parents {
        let Some(next) = cursor.get_mut(parent) else {
            return;
        };
        cursor = next;
    }
    if let Some(object) = cursor.as_object_mut() {
        object.remove(*key);
    }
}

pub fn merge(existing: Option<&Value>, imported: &Value) -> Value {
    let mut result = existing
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (result.as_object_mut(), imported.as_object()) {
        for (key, value) in source {
            if value.is_object() {
                let child = merge(target.get(key), value);
                target.insert(key.clone(), child);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    result
}

pub fn native_entry(existing: Option<&Value>, imported: &Value) -> Result<Value, String> {
    if existing.is_some_and(|value| !value.is_object()) {
        return Err("客户端条目不是原生对象，拒绝覆盖".into());
    }
    Ok(existing
        .or_else(|| imported.is_object().then_some(imported))
        .cloned()
        .unwrap_or_else(|| json!({})))
}

pub fn apply_credential(value: &mut Value, path: &[&str], profile: &Profile) -> Result<(), String> {
    if let Some(key) = super::validate::credential(profile)? {
        set(value, path, json!(key))?;
    } else if profile.auth_mode == "preserve" && profile.native.source_id.is_empty() {
        return Err("新配置没有可保留的认证，请选择 API Key、环境变量或无需认证".into());
    }
    Ok(())
}

pub fn model(id: &str, value: &Value) -> Model {
    Model {
        id: id.into(),
        name: string(value, "name"),
        ..Default::default()
    }
}

pub fn trim_chat_url(base: &str) -> String {
    base.trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .into()
}
