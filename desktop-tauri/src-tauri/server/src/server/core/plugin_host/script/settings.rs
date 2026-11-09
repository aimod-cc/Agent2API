use serde_json::{json, Value};

pub async fn configure(payload: Value) -> Result<Value, String> {
    let id = payload["id"].as_str().ok_or("缺少插件包标识")?;
    let before = super::storage::read()?;
    let package = before.packages.iter().find(|package| package.id == id).ok_or("脚本插件不存在")?;
    if payload.get("options").is_none() && payload.get("providerOptions").is_none() {
        return Ok(json!({ "options": package.options, "providerOptions": package.provider_options,
            "schema": package.settings_schema }));
    }
    if super::operations::operation()["active"] == true { return Err("已有脚本插件任务正在执行".into()); }
    let mut after = before.clone();
    let next = after.packages.iter_mut().find(|package| package.id == id).ok_or("脚本插件不存在")?;
    if let Some(options) = payload.get("options") {
        if !options.is_object() || serde_json::to_vec(options).map_err(|_| "插件参数无法编码")?.len() > 256 * 1024 {
            return Err("插件参数必须是不超过 256 KiB 的 JSON 对象".into());
        }
        next.options = options.clone();
    }
    if let Some(options) = payload.get("providerOptions") {
        let map = options.as_object().ok_or("提供商开关必须是对象")?;
        for (provider, values) in map {
            if !next.providers.contains(provider) { return Err("开关不属于当前插件的提供商".into()); }
            let values = values.as_object().ok_or("提供商开关必须是对象")?;
            let definitions = next.settings_schema["providerOptions"][provider].as_array();
            for (key, value) in values {
                if !value.is_boolean() || !definitions.is_some_and(|items| items.iter().any(|item| item["key"].as_str() == Some(key))) {
                    return Err("插件没有声明该开关".into());
                }
            }
        }
        next.provider_options = options.clone();
    }
    let options_changed = package.options != next.options;
    if !options_changed {
        let _mutation = crate::server::core::plugin_host::operations::mutation_guard().await;
        let current = super::storage::read()?;
        if super::operations::operation()["active"] == true
            || serde_json::to_value(&current).map_err(|error| error.to_string())? != serde_json::to_value(&before).map_err(|error| error.to_string())?
        {
            return Err("插件配置已被其他操作修改，请重新操作".into());
        }
        let host = super::runtime::host().filter(|_| package.enabled);
        if let Some(host) = &host {
            next.settings_schema = host.call("script.settings", json!({ "package": id, "providerOptions": next.provider_options }),
                std::time::Duration::from_secs(10)).await?;
        }
        if let Err(error) = super::storage::save(&after) {
            if let Some(host) = host {
                let _ = host.call("script.settings", json!({ "package": id, "providerOptions": package.provider_options }),
                    std::time::Duration::from_secs(10)).await;
            }
            return Err(error);
        }
        crate::server::core::providers::catalog::invalidate();
    } else {
        super::operations::begin("settings", json!({ "id": id, "name": package.name, "version": package.version }))?;
        crate::spawn_task(async move {
            let result = async {
                let executable = if after.packages.iter().any(|package| package.enabled) { super::bun::ensure().await? }
                    else { std::path::PathBuf::new() };
                super::operations::switch(before, after, executable, None).await
            }.await;
            super::operations::finished(result);
        });
        return Ok(json!({ "operation": super::operations::operation() }));
    }
    Ok(json!({ "saved": true }))
}
