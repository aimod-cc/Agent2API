use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{absolute_env, app_data, content, home, invalid, object};

mod refresh;

pub(super) use refresh::invoke;

pub(super) fn selected() -> Result<std::path::PathBuf, proto::WireError> {
    if let Some(root) = absolute_env("RACCOON_BOX_AGENT_CONFIG_DIR")
        .or_else(|| absolute_env("BOX_AGENT_CONFIG_DIR"))
    {
        return Ok(root.join("auth.json"));
    }
    if let Some(root) = absolute_env("BOX_AGENT_HOME") {
        return Ok(root.join("config/auth.json"));
    }
    let home = home()?;
    let mut roots = vec![app_data()?.join("office-raccoon")];
    #[cfg(windows)]
    roots.push(home.join(".office-raccoon"));
    let marker = ".raccoon-lab-client.json";
    if roots
        .first()
        .is_some_and(|root| root.is_dir() && !root.join(marker).exists())
    {
        return Ok(home.join(".box-agent/config/auth.json"));
    }
    for root in roots {
        if let Ok(value) = object(&root.join(marker), 4096) {
            if value["application"] == "@raccoon/office-desktop" && value["version"] == 1 {
                return Ok(root.join("agent/config/auth.json"));
            }
        }
    }
    Ok(home.join(".box-agent/config/auth.json"))
}

pub(super) fn read() -> Result<Value, proto::WireError> {
    let path = selected()?;
    let value = object(&path, 256 * 1024)?;
    content(&path, project(&value)?)
}

fn project(value: &Value) -> Result<Value, proto::WireError> {
    let groups: &[&[&str]] = &[
        &["accessToken", "access_token", "token", "auth_token"],
        &["refreshToken", "refresh_token"],
        &["expiresAt", "tokenExpiresAt"],
        &["userId"],
        &["officeIdentity", "office_identity"],
        &["officeOrgName", "office_org_name"],
        &["officeOrgRole", "office_org_role"],
    ];
    let present = |value: &Value| {
        !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())
    };
    let mut result = serde_json::json!({});
    for aliases in groups {
        // 同字段的别名也按顶层优先：不能让嵌套accessToken盖过顶层access_token。
        let selected = aliases
            .iter()
            .find_map(|key| value.get(*key).filter(|value| present(value)))
            .or_else(|| {
                aliases
                    .iter()
                    .find_map(|key| value["auth"].get(*key).filter(|value| present(value)))
            });
        if let Some(value) = selected {
            result[aliases[0]] = value.clone();
        }
    }
    if result["accessToken"]
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(invalid());
    }
    Ok(result)
}
