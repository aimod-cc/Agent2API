use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use super::storage::Registry;
use crate::server::core::account_store::plugin_accounts;
use crate::server::core::plugin_host;

fn state() -> &'static Mutex<Value> {
    static STATE: OnceLock<Mutex<Value>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(json!({ "active": false })))
}

pub fn operation() -> Value {
    state()
        .lock()
        .map(|value| value.clone())
        .unwrap_or(Value::Null)
}

pub fn stage(stage: &str, message: &str) {
    if let Ok(mut value) = state().lock() {
        value["stage"] = json!(stage);
        value["message"] = json!(message);
        value["received"] = json!(0);
    }
}

pub fn download_progress(received: usize) {
    if let Ok(mut value) = state().lock() {
        value["received"] = json!(received);
    }
}

pub(super) fn begin(action: &str, target: Value) -> Result<(), String> {
    let mut value = state().lock().map_err(|_| "脚本插件任务不可用")?;
    if value["active"] == true {
        return Err("已有脚本插件任务正在执行".into());
    }
    *value = json!({ "active": true, "action": action, "target": target, "stage": "starting", "received": 0,
        "startedAt": crate::server::logging::now_ms(), "error": null });
    Ok(())
}

pub(super) fn finished(result: Result<(), String>) {
    super::icons::wake(false);
    // 装了／更了／卸了之后可更新状态必须重算：自动检查的间隔是 6 小时，不强制的话
    // 刚更新完的插件会顶着一小时前的旧结论继续显示「可更新」（点进去才发现没有）。
    super::updates::wake(true);
    if let Ok(mut value) = state().lock() {
        value["active"] = json!(false);
        value["updatedAt"] = json!(crate::server::logging::now_ms());
        value["stage"] = json!(if result.is_ok() { "done" } else { "failed" });
        value["error"] = json!(result.err());
    }
}

pub fn install(
    name: String, version: String, trusted: bool, expected_id: Option<String>,
) -> Result<Value, String> {
    if !trusted {
        return Err("安装脚本插件前必须确认信任来源；脚本并非操作系统沙箱".into());
    }
    let source = super::source::InstallSource::parse(&name, &version)?;
    if let Some(id) = &expected_id {
        if !super::storage::read()?.packages.iter().any(|package| &package.id == id) {
            return Err("待更新的脚本插件不存在".into());
        }
    }
    let mut target = match &source {
        super::source::InstallSource::Local { path } => json!({ "source": format!("file:{}", path.display()) }),
        super::source::InstallSource::Npm { name, version } => json!({
            "id": super::packages::package_id(name), "name": name, "source": name, "version": version,
        }),
        super::source::InstallSource::Magpie { name, version } => json!({
            "id": super::packages::package_id(name), "name": name, "source": format!("magpie:{name}"), "version": version,
        }),
        super::source::InstallSource::Github { repository, reference } => json!({
            "source": format!("github:{repository}"), "version": reference,
        }),
    };
    if let Some(id) = &expected_id { target["id"] = json!(id); }
    begin("install", target)?;
    crate::spawn_task(async move {
        let result = async {
            let executable = super::bun::ensure().await?;
            let mut package = source.download().await?;
            super::dependencies::install(&package, &executable).await?;
            if expected_id.as_ref().is_some_and(|id| id != &package.id) {
                return Err("下载的脚本插件包名与待更新插件不一致，未替换原插件".into());
            }
            if let Ok(mut value) = state().lock() {
                value["target"]["id"] = json!(package.id);
                value["target"]["name"] = json!(package.name);
                value["target"]["version"] = json!(package.version);
            }
            let before = super::storage::read()?;
            let mut after = before.clone();
            if let Some(previous) = after.packages.iter_mut().find(|item| item.id == package.id) {
                package.enabled = previous.enabled;
                package.installed_at = previous.installed_at;
                package.providers = previous.providers.clone();
                package.options = previous.options.clone();
                package.provider_options = previous.provider_options.clone();
                package.settings_schema = previous.settings_schema.clone();
                package.provider_aliases = previous.provider_aliases.clone();
                package.disabled_providers = previous.disabled_providers.clone();
                *previous = package;
            } else {
                if let Some(aliases) = after.identities.get(&package.id) {
                    package.provider_aliases = aliases.clone();
                    package.providers = aliases.values().cloned().collect();
                }
                if after.packages.len() >= 64 {
                    return Err("最多安装 64 个脚本插件".into());
                }
                after.packages.push(package);
            }
            switch(before, after, executable, None).await
        }
        .await;
        finished(result);
    });
    Ok(operation())
}

pub fn change(
    id: String,
    action: String,
    enabled: Option<bool>,
    provider: Option<String>,
) -> Result<Value, String> {
    let before = super::storage::read()?;
    let mut after = before.clone();
    let package = after
        .packages
        .iter_mut()
        .find(|package| package.id == id)
        .ok_or("脚本插件不存在")?;
    let target = json!({ "id": package.id, "name": package.name, "version": package.version });
    match action.as_str() {
        "reload" => {
            if !package.enabled { return Err("插件已停用，请先启用后再重新加载".into()); }
        }
        "enable" => package.enabled = enabled.ok_or("缺少启停状态")?,
        "checkin" => return Err("签到及附带任务已默认允许，请在签到中心管理执行计划".into()),
        "provider" => {
            let provider = provider
                .filter(|provider| package.providers.contains(provider))
                .ok_or("提供商不属于该脚本插件")?;
            package.disabled_providers.retain(|id| id != &provider);
            if !enabled.ok_or("缺少启停状态")? {
                package.disabled_providers.push(provider);
            }
        }
        "remove" => {
            let aliases = if package.provider_aliases.is_empty() {
                package.providers.iter().map(|id| (id.clone(), id.clone())).collect()
            } else { package.provider_aliases.clone() };
            if !aliases.is_empty() { after.identities.insert(id.clone(), aliases); }
            after.packages.retain(|package| package.id != id);
        },
        _ => return Err("未知的脚本插件操作".into()),
    }
    begin(&action, target)?;
    crate::spawn_task(async move {
        let result = async {
            let executable = if after.packages.iter().any(|package| package.enabled) {
                super::bun::ensure().await?
            } else {
                std::path::PathBuf::new()
            };
            switch(before, after, executable, (action == "reload").then_some(id.as_str())).await
        }
        .await;
        finished(result);
    });
    Ok(operation())
}

pub(super) async fn switch(
    mut before: Registry,
    mut after: Registry,
    executable: std::path::PathBuf,
    retry: Option<&str>,
) -> Result<(), String> {
    let store = plugin_accounts::shared().ok_or("账号库尚未就绪")?;
    let _mutation = plugin_host::operations::mutation_guard().await;
    if let Some(id) = retry {
        let current = super::storage::read()?;
        if !current.packages.iter().any(|package| package.id == id && package.enabled) {
            return Err("插件已停用或移除，未重新加载".into());
        }
        if plugin_host::startup::status(id).is_some_and(|status| status.phase.pending()) {
            return Err("插件仍在加载，请等待当前启动完成".into());
        }
        if plugin_host::startup::status(id).is_some_and(|status| status.phase == plugin_host::startup::Phase::Ready) {
            return Ok(());
        }
        before = current.clone();
        after = current;
    }
    let current = super::storage::read()?;
    if serde_json::to_value(&current).map_err(|error| error.to_string())? != serde_json::to_value(&before).map_err(|error| error.to_string())? {
        return Err("插件配置已被其他操作修改，请重新操作".into());
    }
    let mut ids = super::runtime::all_ids(&before);
    ids.extend(super::runtime::all_ids(&after));
    let hosts: Vec<_> = super::runtime::host().into_iter().collect();
    stage("waiting", "等待脚本插件的在途请求和登录完成");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let guard = loop {
        if let Some(guard) = plugin_host::registry::SwitchGuard::begin(&hosts, ids.clone()) {
            break guard;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("脚本插件仍有在途任务，未中断请求；请稍后重试".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let before_ids = ids.clone();
    stage("loading", "正在加载脚本插件");
    let old_executable = hosts.first().map(|host| host.options.exe.clone());
    plugin_host::runtime::shutdown_packs(&[super::HOST_ID.to_string()]).await?;
    super::transport::clear();
    guard.clear_adapters();
    let result = async {
        let prepared = super::runtime::prepare(&store, &mut after, executable).await?;
        if let Some(failed) = after
            .packages
            .iter()
            .find(|package| package.enabled && package.error.is_some())
        {
            return Err(failed
                .error
                .clone()
                .unwrap_or_else(|| "脚本插件加载失败".into()));
        }
        super::storage::save(&after)?;
        if let Err(error) = super::runtime::activate(&after, prepared) {
            super::storage::save(&before)?;
            return Err(error);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        stage("restoring", "正在恢复原有脚本插件");
        plugin_host::runtime::shutdown_packs(&[super::HOST_ID.to_string()]).await?;
        super::transport::clear();
        for id in super::runtime::all_ids(&after).difference(&before_ids) {
            plugin_host::registry::remove(id);
        }
        super::runtime::clear_owners();
        guard.clear_adapters();
        if let Some(executable) = old_executable {
            let mut restored = before;
            if let Err(recovery) =
                super::runtime::start_registry(&store, &mut restored, executable).await
            {
                return Err(format!("{error}；恢复原脚本插件失败：{recovery}"));
            }
        }
        return Err(format!("{error}；原安装记录已保留"));
    }
    drop(guard);
    crate::server::core::providers::catalog::invalidate();
    crate::server::core::checkin_schedules::extend_plugin_providers_once();
    plugin_host::schedules::wake();
    Ok(())
}
