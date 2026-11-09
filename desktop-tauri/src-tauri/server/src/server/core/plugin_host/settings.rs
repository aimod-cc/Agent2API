//! 插件通道与检查计划默认跟随软件；安装策略仍由插件单独保存。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::config::{self, UpdateChannels};

const KEY: &str = "pluginUpdates";
pub const MIN_INTERVAL: i64 = config::INTERVAL_MIN_MINUTES;
pub const MAX_INTERVAL: i64 = config::INTERVAL_MAX_MINUTES;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SettingsMode {
    #[default]
    Follow,
    Custom,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginSettings {
    pub channel_mode: SettingsMode,
    pub check_mode: SettingsMode,
    pub auto_check: bool,
    pub interval: i64,
    pub stable: bool,
    pub beta: bool,
    pub auto_update: bool,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            channel_mode: SettingsMode::Follow,
            check_mode: SettingsMode::Follow,
            auto_check: true,
            interval: config::DEFAULT_UPDATE_CHECK_MINUTES,
            stable: true,
            beta: false,
            auto_update: true,
        }
    }
}

impl PluginSettings {
    pub fn channels(self) -> UpdateChannels {
        UpdateChannels {
            stable: self.stable,
            beta: self.beta,
        }
    }

    fn effective(mut self) -> Self {
        if self.channel_mode == SettingsMode::Follow {
            let channels = config::update_channels();
            self.stable = channels.stable;
            self.beta = channels.beta;
        }
        if self.check_mode == SettingsMode::Follow {
            let task = config::scheduled_settings().update_check;
            self.auto_check = task.enabled;
            self.interval = task.interval;
        }
        self
    }
}

fn stored() -> PluginSettings {
    let value = config::with_raw(|raw| raw.get(KEY).cloned())
        .flatten()
        .unwrap_or(Value::Null);
    let mut settings = serde_json::from_value::<PluginSettings>(value.clone()).unwrap_or_default();
    let defaults = PluginSettings::default();
    if !(MIN_INTERVAL..=MAX_INTERVAL).contains(&settings.interval) {
        settings.interval = defaults.interval;
    }
    if !settings.stable && !settings.beta {
        settings.stable = true;
    }
    // 旧结构没有继承标记；只保留非默认的自定义值，默认值改为跟随软件。
    if value.get("channelMode").is_none()
        && (settings.beta != defaults.beta || settings.stable != defaults.stable)
    {
        settings.channel_mode = SettingsMode::Custom;
    }
    if value.get("checkMode").is_none()
        && (settings.auto_check != defaults.auto_check || settings.interval != defaults.interval)
    {
        settings.check_mode = SettingsMode::Custom;
    }
    settings
}

pub fn current() -> PluginSettings {
    stored().effective()
}

pub fn notify_changed(before: PluginSettings) {
    let next = current();
    let channel_changed = next.channels() != before.channels();
    if next.interval != before.interval {
        super::catalog::reschedule(next.interval * 60_000);
    }
    if channel_changed {
        super::catalog::invalidate_channel();
    }
    if next != before {
        if channel_changed
            || (!before.auto_check && next.auto_check)
            || (!before.auto_update && next.auto_update)
        {
            super::catalog::schedule_now();
        } else {
            super::catalog::wake();
        }
    }
}

pub fn configure(patch: &Value) -> Result<Value, String> {
    let object = patch
        .as_object()
        .ok_or_else(|| "插件设置必须是 JSON 对象".to_string())?;
    if object.is_empty() {
        return Err("没有需要更新的插件设置".to_string());
    }
    let before = current();
    let mut next = stored();
    let mode = |key: &str| -> Result<Option<SettingsMode>, String> {
        object
            .get(key)
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|_| format!("{key} 必须是 follow 或 custom"))
            })
            .transpose()
    };
    let channel_mode = mode("channelMode")?;
    let check_mode = mode("checkMode")?;
    let channel_patch = object.contains_key("stable") || object.contains_key("beta");
    let check_patch = object.contains_key("autoCheck") || object.contains_key("interval");
    if channel_mode == Some(SettingsMode::Follow) && channel_patch
        || check_mode == Some(SettingsMode::Follow) && check_patch
    {
        return Err("跟随软件时不能同时指定独立的通道或检查计划".to_string());
    }
    if channel_mode == Some(SettingsMode::Custom) || channel_patch {
        if next.channel_mode == SettingsMode::Follow {
            next.stable = before.stable;
            next.beta = before.beta;
        }
        next.channel_mode = SettingsMode::Custom;
    }
    if check_mode == Some(SettingsMode::Custom) || check_patch {
        if next.check_mode == SettingsMode::Follow {
            next.auto_check = before.auto_check;
            next.interval = before.interval;
        }
        next.check_mode = SettingsMode::Custom;
    }
    if let Some(mode) = channel_mode {
        next.channel_mode = mode;
    }
    if let Some(mode) = check_mode {
        next.check_mode = mode;
    }
    for (key, value) in object {
        match key.as_str() {
            "channelMode" | "checkMode" => continue,
            "interval" => {
                next.interval = value
                    .as_i64()
                    .filter(|value| (MIN_INTERVAL..=MAX_INTERVAL).contains(value))
                    .ok_or_else(|| {
                        format!("检查间隔必须是 {MIN_INTERVAL} ~ {MAX_INTERVAL} 分钟")
                    })?;
            }
            _ => {
                let field = match key.as_str() {
                    "autoCheck" => &mut next.auto_check,
                    "stable" => &mut next.stable,
                    "beta" => &mut next.beta,
                    "autoUpdate" => &mut next.auto_update,
                    _ => return Err(format!("未知的插件设置：{key}")),
                };
                *field = value
                    .as_bool()
                    .ok_or_else(|| format!("{key} 必须为 true / false"))?;
            }
        }
    }
    if !next.stable && !next.beta {
        return Err("至少需要关注正式版或 beta 版中的一个通道".to_string());
    }
    let value = serde_json::to_value(next).map_err(|error| error.to_string())?;
    let saved = config::update_raw_field(KEY, value);
    notify_changed(before);
    let mut result = json!(current());
    result["saved"] = json!(saved);
    Ok(result)
}
