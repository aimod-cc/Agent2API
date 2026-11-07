use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;

pub const KEY_VISION_COMPAT: &str = "visionCompatibility";
pub const MAX_TARGETS: usize = 8;
/// 每种方式最多能列多少个模型。
///
/// 旧版上限是 200 条「模型单独配置」，而旧版「全局方式」展开时会把目录里所有
/// 看不了图的模型都列进来，条数随提供商数量走，所以这里留得比 200 宽：上限只是为了
/// 挡住异常大的载荷，不该挡住正常展开出来的名单。
pub const MAX_MODELS: usize = 2000;

/// 处理方式。图片兼容只有这两种，所以每一块都是一个固定的设置块，不是可以增删的配置列表。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode { #[default] Assist, Reroute }

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self { Self::Assist => "assist", Self::Reroute => "reroute" }
    }
}

/// 旧版「模型单独配置」用的处理方式（多一档「不做兼容」）。
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

/// 名单里的一项：哪个提供商的哪个模型走这种方式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), model: model.into() }
    }

    fn matches(&self, provider: &str, model: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider) && self.model.eq_ignore_ascii_case(model)
    }
}

fn lists_model(list: &[ModelRef], provider: &str, model: &str) -> bool {
    list.iter().any(|entry| entry.matches(provider, model))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AssistSettings {
    /// 这些模型看不了图片时走辅助识图。空 = 这种方式不参与。
    pub models: Vec<ModelRef>,
    pub source: ModelSource,
    pub target: Option<VisionTarget>,
    /// 旧版用它记录保存当刻「工具服务里的视觉分析模型」，配合授权确认做变更校验。
    /// 现在「沿用」一律在请求时取当时的配置，这个字段只为兼容旧文件保留，不再读写。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_target: Option<VisionTarget>,
    pub reuse_results: bool,
}

impl Default for AssistSettings {
    fn default() -> Self {
        Self { models: Vec::new(), source: ModelSource::Shared, target: None, shared_target: None, reuse_results: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RerouteSettings {
    /// 这些模型的请求整体改交视觉模型。空 = 这种方式不参与。
    pub models: Vec<ModelRef>,
    pub prefer_current_route: bool,
    pub fallback: Fallback,
    /// 指定的接管模型与备选，按顺序尝试。
    pub targets: Vec<VisionTarget>,
}

impl Default for RerouteSettings {
    fn default() -> Self {
        Self { models: Vec::new(), prefer_current_route: true, fallback: Fallback::Stop, targets: Vec::new() }
    }
}

/// 旧版「模型单独配置」的一条。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOverride {
    pub provider: String,
    pub model: String,
    pub mode: OverrideMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VisionSettings {
    pub enabled: bool,
    pub assist: AssistSettings,
    pub reroute: RerouteSettings,
    pub unknown_capability: UnknownCapability,
    pub timeout_seconds: u64,
    /// 旧版顶层字段：全局二选一。读到就继续按旧语义生效，直到面板把它展开成名单。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// 旧版顶层字段：模型单独配置。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ModelOverride>,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            enabled: false, assist: AssistSettings::default(), reroute: RerouteSettings::default(),
            unknown_capability: UnknownCapability::TryOriginal, timeout_seconds: 30,
            mode: None, overrides: Vec::new(),
        }
    }
}

impl VisionSettings {
    /// 还是旧版形状：全局二选一 + 模型单独配置，两块的名单都还没展开。
    ///
    /// 旧配置里 `mode` 缺省也代表「辅助识图」，所以只要 `overrides` 非空就算旧版；
    /// 光有 `mode` 没有 `overrides` 同样是旧版（那正是旧的全局设置）。
    pub fn is_legacy(&self) -> bool {
        self.mode.is_some() || !self.overrides.is_empty()
    }

    /// 旧版语义：这个模型走哪种方式（`None` = 不做兼容）。不看 `enabled`，供展开名单时逐项判定。
    pub fn legacy_mode(&self, provider: &str, model: &str) -> Option<Mode> {
        match self.overrides.iter()
            .find(|entry| entry.provider.eq_ignore_ascii_case(provider) && entry.model.eq_ignore_ascii_case(model))
            .map(|entry| entry.mode) {
            Some(OverrideMode::Off) => None,
            Some(OverrideMode::Assist) => Some(Mode::Assist),
            Some(OverrideMode::Reroute) => Some(Mode::Reroute),
            None => Some(self.mode.unwrap_or_default()),
        }
    }

    /// 这个模型走哪种方式。两块的名单重叠时**辅助识图优先**（面板会把重叠直接指出来）。
    pub fn mode_for(&self, provider: &str, model: &str) -> Option<Mode> {
        if !self.enabled { return None; }
        if self.is_legacy() { return self.legacy_mode(provider, model); }
        if lists_model(&self.assist.models, provider, model) { Some(Mode::Assist) }
        else if lists_model(&self.reroute.models, provider, model) { Some(Mode::Reroute) }
        else { None }
    }

    pub fn uses(&self, mode: Mode) -> bool {
        if self.is_legacy() {
            return self.mode.unwrap_or_default() == mode
                || self.overrides.iter().any(|entry| matches!((entry.mode, mode),
                    (OverrideMode::Assist, Mode::Assist) | (OverrideMode::Reroute, Mode::Reroute)));
        }
        match mode {
            Mode::Assist => !self.assist.models.is_empty(),
            Mode::Reroute => !self.reroute.models.is_empty(),
        }
    }

    /// 把旧版字段清掉，只留展开后的名单。
    pub fn clear_legacy(&mut self) {
        self.mode = None;
        self.overrides.clear();
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

fn validate_model_list(label: &str, list: &[ModelRef]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for entry in list {
        if !valid_name(&entry.provider) || !valid_name(&entry.model) {
            return Err(format!("{label}的模型名单里有不完整的条目"));
        }
        if !seen.insert((entry.provider.to_lowercase(), entry.model.to_lowercase())) {
            return Err(format!("{label}的模型名单里同一个模型只能出现一次"));
        }
    }
    Ok(())
}

pub fn validate(settings: &VisionSettings) -> Result<(), String> {
    if !(5..=120).contains(&settings.timeout_seconds) {
        return Err("图片兼容最长等待必须在 5 到 120 秒之间".into());
    }
    if settings.assist.models.len() > MAX_MODELS || settings.reroute.models.len() > MAX_MODELS
        || settings.reroute.targets.len() > MAX_TARGETS {
        return Err(format!("图片兼容每种方式最多 {MAX_MODELS} 个模型、{MAX_TARGETS} 个备选视觉模型"));
    }
    validate_model_list("辅助识图", &settings.assist.models)?;
    validate_model_list("切换视觉模型", &settings.reroute.models)?;
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
            return Err("启用前请选择当前路由优先，或指定接管回答的视觉模型".into());
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
            return Err(GatewayError::with_status(409, "图片兼容已在其他位置修改，请重新读取后再改；本次改动未生效"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("图片兼容保存失败，未应用本次修改"))
}
