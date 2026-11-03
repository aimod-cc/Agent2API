use serde_json::Value;
use std::collections::HashSet;

use super::{ClientDef, Profile};

pub fn profile(def: &ClientDef, profile: &Profile) -> Result<(), String> {
    if profile.id.is_empty() || profile.id.len() > 200 || profile.id.chars().any(char::is_control) {
        return Err("方案 ID 无效".into());
    }
    if profile.name.is_empty() || profile.name.len() > 300 {
        return Err("请填写方案名称".into());
    }
    if !def.writable {
        return Err(def.note.into());
    }
    if !def.protocols.contains(&profile.protocol.as_str()) {
        return Err("这个客户端不支持所选协议".into());
    }
    if !matches!(
        profile.kind.as_str(),
        "gateway" | "official" | "relay" | "local"
    ) {
        return Err("配置来源无效".into());
    }
    if profile.kind == "gateway" && !def.gateway {
        return Err("这个客户端不能接入本网关".into());
    }
    let inherited_official = profile.base_url.is_empty()
        && profile.auth_mode == "preserve"
        && !profile.native.settings.is_null();
    if !inherited_official {
        let url = url::Url::parse(&profile.base_url)
            .map_err(|_| "请填写完整的 http 或 https 接口地址")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("接口地址必须是 http 或 https，且不能包含账号、密码或片段".into());
        }
        if profile.kind == "gateway"
            && !matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            )
        {
            return Err("本网关方案必须指向网关所在机器的回环地址".into());
        }
        if profile.protocol == "gemini"
            && (url.path().trim_end_matches('/').ends_with("/v1beta")
                || url.path().contains(":generateContent"))
        {
            return Err("Gemini 请填写服务根地址，客户端会自行追加 API 版本和请求路径".into());
        }
        if profile.protocol != "gemini"
            && def.id != "workbuddy"
            && ["/chat/completions", "/responses", "/messages"]
                .iter()
                .any(|suffix| url.path().ends_with(suffix))
        {
            return Err("请填写接口基址，不要附加 messages、responses 或 chat/completions".into());
        }
    }
    if !matches!(
        profile.auth_mode.as_str(),
        "gateway" | "key" | "env" | "none" | "preserve"
    ) {
        return Err("认证方式无效".into());
    }
    if profile.auth_mode == "gateway" && profile.kind != "gateway" {
        return Err("本网关认证不能用于其他服务".into());
    }
    if profile.auth_mode == "key" && profile.api_key.trim().is_empty() {
        return Err("请填写 API Key".into());
    }
    if profile.api_key.contains(['\r', '\n']) {
        return Err("API Key 不能包含换行".into());
    }
    if profile.auth_mode == "env" && !def.env_key {
        return Err("这个客户端不支持本页的环境变量认证方式".into());
    }
    if profile.auth_mode == "env" && !valid_env(&profile.env_key) {
        return Err("请填写有效的环境变量名".into());
    }
    if def.id == "zed" && !matches!(profile.auth_mode.as_str(), "preserve" | "none") {
        return Err("请在 Zed 中配置 API Key，本页不会写入系统凭证".into());
    }
    if def.models {
        if profile.models.is_empty() || profile.models.len() > 200 {
            return Err("请配置至少一个模型，最多 200 个".into());
        }
        let mut ids = HashSet::new();
        for model in &profile.models {
            if model.id.trim().is_empty()
                || model.id.len() > 400
                || model.id.chars().any(char::is_control)
                || !ids.insert(&model.id)
            {
                return Err("模型 ID 不能为空或重复".into());
            }
            if model
                .context_window
                .is_some_and(|value| value == 0 || value > 100_000_000)
                || model
                    .max_output_tokens
                    .is_some_and(|value| value == 0 || value > 100_000_000)
            {
                return Err("模型 Token 上限必须是 1 到 1 亿之间的整数".into());
            }
            if matches!(def.id, "zed" | "kimi-code" | "crush") && model.context_window.is_none() {
                return Err("这个客户端的自定义模型必须填写上下文窗口".into());
            }
            if def.id == "crush"
                && (model.max_output_tokens.is_none()
                    || model.supports_images.is_none()
                    || model.supports_reasoning.is_none())
            {
                return Err(
                    "Crush 自定义模型需要填写输出上限、图片与思考能力，不能猜测这些必要字段".into(),
                );
            }
            if !model.effort.is_empty()
                && model.effort != "default"
                && !model.reasoning_levels.contains(&model.effort)
            {
                return Err(format!("模型 {} 未声明所选思考等级", model.id));
            }
        }
        if def.default_model && !profile.models.iter().any(|model| model.id == profile.model) {
            return Err("默认模型必须是模型列表中的一项".into());
        }
    } else if def.default_model && profile.model.is_empty() && !inherited_official {
        return Err("请选择或填写模型 ID".into());
    }
    if !profile.effort.is_empty() && profile.effort != "default" {
        if !def.effort {
            return Err("这个客户端未开放思考等级编辑".into());
        }
        let levels: &[&str] = match def.id {
            "codex" => &["none", "minimal", "low", "medium", "high", "xhigh"],
            "claude-code" => &["low", "medium", "high", "xhigh", "max", "auto"],
            "dsh" => &["off", "none", "minimal", "low", "medium", "high", "xhigh", "max"],
            _ => &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
        };
        if !levels.contains(&profile.effort.as_str()) {
            return Err("这个客户端不支持所选思考等级".into());
        }
    }
    Ok(())
}

pub fn valid_env(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub fn credential(profile: &Profile) -> Result<Option<String>, String> {
    match profile.auth_mode.as_str() {
        "key" => Ok(Some(profile.api_key.clone())),
        "gateway" => {
            let snapshot = crate::server::config::current();
            let active = snapshot.active_api_keys();
            if active.is_empty() {
                return Err("网关未配置可用 API Key，请先在「网关 Key」页创建并启用".into());
            }
            let entries = crate::server::core::api_keys::entries_from(&snapshot.raw());
            let key = if profile.gateway_key_id == "env" {
                crate::server::config::parse::env_api_key_value()
            } else if profile.gateway_key_id.is_empty() {
                active.first().cloned()
            } else {
                entries
                    .iter()
                    .find(|key| {
                        key.is_usable() && key.id == profile.gateway_key_id && active.contains(&key.key)
                    })
                    .map(|key| key.key.clone())
            };
            let key = key.ok_or("所选网关 Key 已删除、停用或无法解密")?;
            if let Some(entry) = entries.iter().find(|entry| entry.key == key) {
                let mut wanted = if profile.models.is_empty() {
                    vec![profile.model.as_str()]
                } else {
                    profile.models.iter().map(|m| m.id.as_str()).collect()
                };
                wanted.extend([
                    profile.claude_models.opus.as_str(),
                    profile.claude_models.sonnet.as_str(),
                    profile.claude_models.haiku.as_str(),
                    profile.claude_models.subagent.as_str(),
                ]);
                if !entry.allowed_models.is_empty()
                    && wanted.iter().filter(|id| !id.is_empty()).any(|id| {
                        !entry
                            .allowed_models
                            .iter()
                            .any(|allowed| allowed.eq_ignore_ascii_case(id))
                    })
                {
                    return Err("所选网关 Key 不允许使用方案中的全部模型".into());
                }
            }
            Ok(Some(key))
        }
        "none" => Ok(Some(String::new())),
        "preserve" => {
            if !profile.api_key.is_empty() {
                Ok(Some(profile.api_key.clone()))
            } else {
                Ok(None)
            }
        }
        "env" => Ok(None),
        _ => Err("认证方式无效".into()),
    }
}

pub fn require_gateway_url(profile: &Profile, port: u16) -> Result<(), String> {
    if profile.kind == "gateway"
        && !super::is_gateway(&profile.base_url, &format!("http://127.0.0.1:{port}"))
    {
        return Err("本网关方案的地址或端口与当前网关不一致，请重新选择本网关来源".into());
    }
    Ok(())
}

pub fn value_object(value: &mut Value) -> Result<&mut serde_json::Map<String, Value>, String> {
    value
        .as_object_mut()
        .ok_or_else(|| "配置结构与客户端原生对象不一致，拒绝覆盖".into())
}
