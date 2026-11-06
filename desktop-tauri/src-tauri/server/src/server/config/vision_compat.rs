use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;

pub const KEY_VISION_COMPAT: &str = "visionCompatibility";
pub const MAX_TARGETS: usize = 8;
pub const MAX_OVERRIDES: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode { #[default] Assist, Reroute }

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self { Self::Assist => "assist", Self::Reroute => "reroute" }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode { #[default] Assist, Reroute, Off }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource { #[default] Shared, Custom }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback { #[default] Stop, Designated }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownCapability { #[default] TryOriginal, Compat }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisionTarget {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub reasoning: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AssistSettings {
    pub source: ModelSource,
    pub target: Option<VisionTarget>,
    pub shared_target: Option<VisionTarget>,
    pub reuse_results: bool,
}

impl Default for AssistSettings {
    fn default() -> Self { Self { source: ModelSource::Shared, target: None, shared_target: None, reuse_results: true } }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RerouteSettings {
    pub prefer_current_route: bool,
    pub fallback: Fallback,
    pub targets: Vec<VisionTarget>,
}

impl Default for RerouteSettings {
    fn default() -> Self {
        Self { prefer_current_route: true, fallback: Fallback::Stop, targets: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOverride {
    pub provider: String,
    pub model: String,
    pub mode: OverrideMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct VisionSettings {
    pub enabled: bool,
    pub mode: Mode,
    pub assist: AssistSettings,
    pub reroute: RerouteSettings,
    pub overrides: Vec<ModelOverride>,
    pub unknown_capability: UnknownCapability,
    pub timeout_seconds: u64,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            enabled: false, mode: Mode::Assist, assist: AssistSettings::default(),
            reroute: RerouteSettings::default(), overrides: Vec::new(),
            unknown_capability: UnknownCapability::TryOriginal, timeout_seconds: 30,
        }
    }
}

impl VisionSettings {
    pub fn mode_for(&self, provider: &str, model: &str) -> Option<Mode> {
        if !self.enabled { return None; }
        match self.overrides.iter().find(|entry| entry.provider.eq_ignore_ascii_case(provider)
            && entry.model.eq_ignore_ascii_case(model)).map(|entry| entry.mode) {
            Some(OverrideMode::Off) => None,
            Some(OverrideMode::Assist) => Some(Mode::Assist),
            Some(OverrideMode::Reroute) => Some(Mode::Reroute),
            None => Some(self.mode),
        }
    }

    pub fn uses(&self, mode: Mode) -> bool {
        self.mode == mode || self.overrides.iter().any(|entry| matches!((entry.mode, mode),
            (OverrideMode::Assist, Mode::Assist) | (OverrideMode::Reroute, Mode::Reroute)))
    }
}

#[derive(Clone, Debug)]
pub struct VisionSnapshot {
    pub settings: Result<VisionSettings, String>,
    pub revision: String,
}

impl Default for VisionSnapshot {
    fn default() -> Self { from_value(None) }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 256
        && !value.chars().any(char::is_control)
}

pub fn validate_target(target: &VisionTarget) -> Result<(), String> {
    if !valid_name(&target.provider) || !valid_name(&target.model) {
        return Err("请选择完整的提供商与模型，名称不能超过 256 个字符或含控制字符".into());
    }
    if !target.reasoning.is_empty() && !crate::server::core::model_rules::REASONING_LEVELS.contains(&target.reasoning.as_str()) {
        return Err("图片兼容的思考等级无效".into());
    }
    Ok(())
}

pub fn validate(settings: &VisionSettings) -> Result<(), String> {
    if !(5..=120).contains(&settings.timeout_seconds) {
        return Err("图片兼容最长等待必须在 5 到 120 秒之间".into());
    }
    if settings.reroute.targets.len() > MAX_TARGETS || settings.overrides.len() > MAX_OVERRIDES {
        return Err(format!("图片兼容最多允许 {MAX_TARGETS} 个备选模型和 {MAX_OVERRIDES} 条模型单独配置"));
    }
    if let Some(target) = &settings.assist.target { validate_target(target)?; }
    if let Some(target) = &settings.assist.shared_target { validate_target(target)?; }
    let mut targets = HashSet::new();
    for target in &settings.reroute.targets {
        validate_target(target)?;
        if !targets.insert((target.provider.to_lowercase(), target.model.to_lowercase())) {
            return Err("备选视觉模型不能重复".into());
        }
    }
    let mut overrides = HashSet::new();
    for entry in &settings.overrides {
        if !valid_name(&entry.provider) || !valid_name(&entry.model) {
            return Err("模型单独配置的提供商和模型不完整".into());
        }
        if !overrides.insert((entry.provider.to_lowercase(), entry.model.to_lowercase())) {
            return Err("同一提供商的同一模型不能重复配置".into());
        }
    }
    if settings.enabled && settings.uses(Mode::Assist)
        && settings.assist.source == ModelSource::Custom && settings.assist.target.is_none() {
        return Err("启用前请先选择辅助识图的视觉模型".into());
    }
    if settings.enabled && settings.uses(Mode::Reroute) {
        let designated = settings.reroute.fallback == Fallback::Designated && !settings.reroute.targets.is_empty();
        if !settings.reroute.prefer_current_route && !designated {
            return Err("启用前请选择当前路由优先，或配置指定的备选视觉模型".into());
        }
        if settings.reroute.fallback == Fallback::Designated && settings.reroute.targets.is_empty() {
            return Err("已选择指定备选模型，请至少添加一个视觉模型".into());
        }
    }
    Ok(())
}

fn revision(value: Option<&Value>) -> String {
    format!("{:x}", Sha256::digest(value.cloned().unwrap_or(Value::Null).to_string().as_bytes()))
}

fn from_value(value: Option<&Value>) -> VisionSnapshot {
    let settings = match value {
        None => Ok(VisionSettings::default()),
        Some(value) => serde_json::from_value::<VisionSettings>(value.clone())
            .map_err(|error| format!("图片兼容配置格式不正确：{error}"))
            .and_then(|settings| { validate(&settings)?; Ok(settings) }),
    };
    VisionSnapshot { settings, revision: revision(value) }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<VisionSnapshot> {
    Arc::new(from_value(raw.get(KEY_VISION_COMPAT)))
}

pub fn current() -> Arc<VisionSnapshot> {
    super::CONFIG.read().unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref().map(|config| config.vision_compat.clone()).unwrap_or_default()
}

pub fn shared_target() -> Option<VisionTarget> {
    let config = crate::server::core::tool_services::load().vision?;
    Some(VisionTarget { provider: config.provider, model: config.upstream_model, reasoning: config.reasoning })
}

pub fn save(settings: VisionSettings, expected: &str) -> Result<(), GatewayError> {
    validate(&settings).map_err(GatewayError::bad_request)?;
    let value = serde_json::to_value(settings).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_field(KEY_VISION_COMPAT, |raw| {
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "图片兼容已在其他位置修改，请重新读取后再保存；当前草稿未丢失"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("图片兼容保存失败，未应用本次修改"))
}
