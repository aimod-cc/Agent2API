use super::super::{types::ClaudeModels, Profile};
use super::{remove, set, string};
use serde_json::{json, Value};

const MODEL_KEYS: &[&str] = &[
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];

pub fn read(value: &Value) -> Result<Vec<Profile>, String> {
    if !value.is_object() {
        return Err("Claude Code settings.json 必须是对象".into());
    }
    let env = value.get("env").cloned().unwrap_or_else(|| json!({}));
    if !env.is_object() {
        return Err("Claude Code env 必须是对象".into());
    }
    let mut profile = Profile::new(
        "claude-current",
        "Claude Code 当前配置",
        "anthropic",
        &string(&env, "ANTHROPIC_BASE_URL"),
    );
    profile.model = string(&env, "ANTHROPIC_MODEL");
    if profile.model.is_empty() {
        profile.model = string(value, "model");
    }
    profile.effort = string(&env, "CLAUDE_CODE_EFFORT_LEVEL");
    if profile.effort.is_empty() {
        profile.effort = string(value, "effortLevel");
    }
    profile.api_key = string(&env, "ANTHROPIC_AUTH_TOKEN");
    if profile.api_key.is_empty() {
        profile.api_key = string(&env, "ANTHROPIC_API_KEY");
    }
    profile.claude_models = ClaudeModels {
        opus: string(&env, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
        sonnet: string(&env, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
        haiku: string(&env, "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        subagent: string(&env, "CLAUDE_CODE_SUBAGENT_MODEL"),
    };
    profile.models = super::claude_models::read(value);
    if !profile.model.is_empty() && !profile.models.iter().any(|model| model.id == profile.model) {
        profile.models.push(super::super::Model { id: profile.model.clone(), name: profile.model.clone(), ..Default::default() });
    }
    let mut settings = json!({ "env": {} });
    for key in MODEL_KEYS.iter().copied().chain([
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_EFFORT_LEVEL",
    ]) {
        if let Some(item) = env.get(key) {
            set(&mut settings, &["env", key], item.clone())?;
        }
    }
    for key in ["model", "effortLevel", "modelPicker", "modelSettings"] {
        if let Some(item) = value.get(key) {
            set(&mut settings, &[key], item.clone())?;
        }
    }
    if settings.get("env").and_then(Value::as_object).is_some_and(|env| env.is_empty())
        && settings.get("model").is_none() && settings.get("effortLevel").is_none() && profile.models.is_empty()
    { return Ok(Vec::new()); }
    profile.native.settings = settings;
    Ok(vec![profile])
}

pub fn write(
    value: &mut Value,
    profile: &Profile,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if profile.base_url.is_empty() && profile.auth_mode == "preserve" {
        remove(value, &["env", "ANTHROPIC_BASE_URL"]);
    } else {
        set(
            value,
            &["env", "ANTHROPIC_BASE_URL"],
            json!(profile.base_url),
        )?;
    }
    if profile.auth_mode == "preserve" && !profile.native.settings.is_null() {
        for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            if let Some(item) = profile.native.settings.pointer(&format!("/env/{key}")) {
                set(value, &["env", key], item.clone())?;
            } else {
                remove(value, &["env", key]);
            }
        }
    } else if let Some(key) = super::super::validate::credential(profile)? {
        let field = if profile.auth_mode == "key" {
            "ANTHROPIC_API_KEY"
        } else {
            "ANTHROPIC_AUTH_TOKEN"
        };
        let other = if field == "ANTHROPIC_API_KEY" {
            "ANTHROPIC_AUTH_TOKEN"
        } else {
            "ANTHROPIC_API_KEY"
        };
        set(value, &["env", field], json!(key))?;
        set(value, &["env", other], json!(""))?;
    }
    for (key, model) in [
        ("ANTHROPIC_MODEL", &profile.model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", &profile.claude_models.opus),
        (
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            &profile.claude_models.sonnet,
        ),
        (
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            &profile.claude_models.haiku,
        ),
        (
            "CLAUDE_CODE_SUBAGENT_MODEL",
            &profile.claude_models.subagent,
        ),
    ] {
        if model.is_empty() {
            if key == "ANTHROPIC_MODEL" { remove(value, &["env", key]); }
            continue;
        } else {
            set(value, &["env", key], json!(model))?;
        }
        let display_key = format!("{key}_NAME");
        if value.pointer(&format!("/env/{display_key}")).is_some() {
            remove(value, &["env", &display_key]);
        }
    }
    if profile.model.is_empty() {
        remove(value, &["model"]);
    } else {
        set(value, &["model"], json!(profile.model))?;
    }
    let per_model_effort = profile.models.iter().any(|model| !model.effort.is_empty() && model.effort != "default");
    if per_model_effort || profile.effort.is_empty() || profile.effort == "default" {
        remove(value, &["env", "CLAUDE_CODE_EFFORT_LEVEL"]);
        remove(value, &["effortLevel"]);
        if per_model_effort && !profile.effort.is_empty() {
            warnings.push("已改用逐模型思考等级，不再写入覆盖全部模型的旧全局档位。".into());
        }
    } else {
        set(
            value,
            &["env", "CLAUDE_CODE_EFFORT_LEVEL"],
            json!(profile.effort),
        )?;
        if matches!(profile.effort.as_str(), "auto" | "max") {
            remove(value, &["effortLevel"]);
        } else {
            set(value, &["effortLevel"], json!(profile.effort))?;
        }
    }
    super::claude_models::write(value, profile)?;
    if !profile.models.is_empty() {
        warnings.push("Claude Code 多模型菜单需要 2.1.242 或更新版本，逐模型思考设置需要 2.1.251 或更新版本；上下文和输入能力仍由客户端识别，不写入虚假的逐模型能力字段。".into());
    }
    if !profile.models.is_empty() && value.get("availableModels").is_some() {
        warnings.push("Claude Code 已有模型允许名单，可能隐藏本次选择的模型；本次保留原有名单，不会放宽客户端限制。".into());
    }
    warnings.push("保留权限、Hooks、MCP 等设置；模型别名映射只修改填写的原生模型字段。".into());
    Ok(())
}
