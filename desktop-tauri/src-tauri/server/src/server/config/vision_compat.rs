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

/// 名单并集：投影时把同方式多条规则的模型合起来，同一个模型只留一次。
fn union_models<'a>(models: impl Iterator<Item = &'a ModelRef>) -> Vec<ModelRef> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for entry in models {
        if seen.insert((entry.provider.to_lowercase(), entry.model.to_lowercase())) {
            result.push(entry.clone());
        }
    }
    result
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

/// 一条图片兼容规则：哪些模型看不了图、怎么接。
///
/// ── 为什么从「两块固定设置」改成逐条规则 ──────────────────────
/// 原来两种方式各是一块固定设置：各带一份模型名单、各自只认一个视觉模型。
/// 于是「GLM-4.7 用这只读图、Kimi-K2 整次交给那只回答」这种配置表达不出来 ——
/// 想给某只模型单独换视觉模型，只能把整块的目标一起换掉。
///
/// ── 两组的字段挂在同一条上 ────────────────────────────────────
/// `mode` 决定用哪一组：
///   · `Mode::Assist` 用 `source` / `target` / `reuse_results`；
///   · `Mode::Reroute` 用 `prefer_current_route` / `fallback` / `targets`。
/// 另一组字段照样存着（校验时按 mode 只查用到的那组），用户在弹窗里把方式改回来时
/// 之前填的那些还在。
///
/// ── 与旧形状的关系 ────────────────────────────────────────────
/// `rules` 是权威形状；`assist` / `reroute` 两块随之降级成**投影**（见
/// `project_blocks`）：手机端与旧界面还在按两块读写，投影让它们继续看到真实配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct VisionRule {
    pub enabled: bool,
    /// 这些模型看不了图时走这条规则。空 = 这条规则不生效。
    pub models: Vec<ModelRef>,
    pub mode: Mode,
    /// 辅助识图：视觉模型从哪来（沿用工具服务的共享模型 / 这条规则自己指定）。
    pub source: ModelSource,
    /// 辅助识图：用哪只模型读图。
    pub target: Option<VisionTarget>,
    /// 辅助识图：复用相同图片的识读结果。
    pub reuse_results: bool,
    /// 整体接管：优先使用当前路由范围内的视觉模型。
    pub prefer_current_route: bool,
    pub fallback: Fallback,
    /// 整体接管：指定接管模型与备选，按顺序尝试。
    pub targets: Vec<VisionTarget>,
}

impl Default for VisionRule {
    /// 手写而不是 derive：`reuse_results` / `prefer_current_route` 的默认值是 **true**
    /// （与两块设置的默认值一致），derive 会全给 false，等于悄悄改掉既有行为。
    fn default() -> Self {
        Self {
            enabled: true, models: Vec::new(), mode: Mode::Assist, source: ModelSource::Shared,
            target: None, reuse_results: true, prefer_current_route: true, fallback: Fallback::Stop,
            targets: Vec::new(),
        }
    }
}

impl VisionRule {
    /// 从「辅助识图」那一块造一条规则：旧形状换算与保存时的投影共用同一份口径。
    pub fn from_assist(assist: &AssistSettings) -> Self {
        Self {
            enabled: !assist.models.is_empty(), models: assist.models.clone(), mode: Mode::Assist,
            source: assist.source, target: assist.target.clone(), reuse_results: assist.reuse_results,
            ..Self::default()
        }
    }

    /// 从「整体接管」那一块造一条规则。
    pub fn from_reroute(reroute: &RerouteSettings) -> Self {
        Self {
            enabled: !reroute.models.is_empty(), models: reroute.models.clone(), mode: Mode::Reroute,
            prefer_current_route: reroute.prefer_current_route, fallback: reroute.fallback,
            targets: reroute.targets.clone(), ..Self::default()
        }
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
    // 旧配置缺少开关时仍按关闭读取；只有整块未配置才采用新的默认值。
    #[serde(default)]
    pub enabled: bool,
    /// 权威形状：按列表顺序匹配，第一条命中的生效。
    #[serde(default)]
    pub rules: Vec<VisionRule>,
    /// 投影（不是配置源）：手机端与旧界面按这一块读写，见 `project_blocks`。
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
            enabled: true, rules: Vec::new(), assist: AssistSettings::default(), reroute: RerouteSettings::default(),
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

    /// 这个模型命中的那条规则（顺序即优先级）；`None` = 不做图片兼容。
    ///
    /// 返回所有权而不是引用：还没有规则（旧配置）时要按旧语义**现造**一条等价规则，
    /// 那条规则并不存在于配置里。
    ///
    /// 三种形状都照顾到，请求路径只认这一个入口：规则列表 → 旧版全局方式（+ 模型单独
    /// 配置）→ 还没折成规则的两块名单。
    pub fn rule_for(&self, provider: &str, model: &str) -> Option<VisionRule> {
        if !self.enabled { return None; }
        if !self.rules.is_empty() {
            return self.rules.iter()
                .filter(|rule| rule.enabled && !rule.models.is_empty())
                .find(|rule| lists_model(&rule.models, provider, model))
                .cloned();
        }
        if self.is_legacy() {
            let assist = || VisionRule {
                models: vec![ModelRef::new(provider, model)], mode: Mode::Assist,
                source: self.assist.source, target: self.assist.target.clone(),
                reuse_results: self.assist.reuse_results, ..VisionRule::default()
            };
            let reroute = || VisionRule {
                models: vec![ModelRef::new(provider, model)], mode: Mode::Reroute,
                prefer_current_route: self.reroute.prefer_current_route, fallback: self.reroute.fallback,
                targets: self.reroute.targets.clone(), ..VisionRule::default()
            };
            return self.legacy_mode(provider, model).map(|mode| match mode {
                Mode::Assist => assist(), Mode::Reroute => reroute(),
            });
        }
        // 还没折成规则的两块名单：辅助识图优先（与旧口径一致）。
        if lists_model(&self.assist.models, provider, model) {
            return Some(VisionRule {
                models: vec![ModelRef::new(provider, model)], ..VisionRule::from_assist(&self.assist)
            });
        }
        if lists_model(&self.reroute.models, provider, model) {
            return Some(VisionRule {
                models: vec![ModelRef::new(provider, model)], ..VisionRule::from_reroute(&self.reroute)
            });
        }
        None
    }

    /// 这个模型走哪种方式。
    pub fn mode_for(&self, provider: &str, model: &str) -> Option<Mode> {
        self.rule_for(provider, model).map(|rule| rule.mode)
    }

    /// 两块设置折成规则列表（旧配置迁移用）。只在还没有规则时生效；
    /// 名单为空的那一块不折 —— 旧口径里「名单空 = 这种方式不参与」。
    pub fn fold_blocks(&mut self) {
        if !self.rules.is_empty() { return; }
        if !self.assist.models.is_empty() { self.rules.push(VisionRule::from_assist(&self.assist)); }
        if !self.reroute.models.is_empty() { self.rules.push(VisionRule::from_reroute(&self.reroute)); }
    }

    /// 规则 → 两块设置（投影）。手机端与旧界面按两块读写，这里让它们看到真实配置：
    /// 名单取**同方式所有规则的并集**，其余字段取该方式第一条规则。
    pub fn project_blocks(&mut self) {
        let assists: Vec<&VisionRule> = self.rules.iter().filter(|rule| rule.mode == Mode::Assist).collect();
        let reroutes: Vec<&VisionRule> = self.rules.iter().filter(|rule| rule.mode == Mode::Reroute).collect();
        let first_assist = assists.first().copied();
        let first_reroute = reroutes.first().copied();
        self.assist = AssistSettings {
            models: union_models(assists.iter().flat_map(|rule| rule.models.iter())),
            source: first_assist.map(|rule| rule.source).unwrap_or_default(),
            target: first_assist.and_then(|rule| rule.target.clone()),
            shared_target: None,
            reuse_results: first_assist.map(|rule| rule.reuse_results).unwrap_or(true),
        };
        self.reroute = RerouteSettings {
            models: union_models(reroutes.iter().flat_map(|rule| rule.models.iter())),
            prefer_current_route: first_reroute.map(|rule| rule.prefer_current_route).unwrap_or(true),
            fallback: first_reroute.map(|rule| rule.fallback).unwrap_or_default(),
            targets: first_reroute.map(|rule| rule.targets.clone()).unwrap_or_default(),
        };
    }

    /// 这种方式此刻有没有在用（接口用它决定要不要校验视觉模型是否可用）。
    pub fn uses(&self, mode: Mode) -> bool {
        if self.is_legacy() {
            return self.mode.unwrap_or_default() == mode
                || self.overrides.iter().any(|entry| matches!((entry.mode, mode),
                    (OverrideMode::Assist, Mode::Assist) | (OverrideMode::Reroute, Mode::Reroute)));
        }
        if !self.rules.is_empty() {
            return self.rules.iter().any(|rule| rule.enabled && !rule.models.is_empty() && rule.mode == mode);
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
    // 规则列表（权威形状）：逐条查名单、目标，以及「启用了就得配全」。
    let listed: usize = settings.rules.iter().map(|rule| rule.models.len()).sum();
    if listed > MAX_MODELS {
        return Err(format!("图片兼容最多 {MAX_MODELS} 个模型"));
    }
    for (index, rule) in settings.rules.iter().enumerate() {
        let at = index + 1;
        validate_model_list(&format!("第 {at} 条规则"), &rule.models)?;
        if rule.targets.len() > MAX_TARGETS {
            return Err(format!("第 {at} 条规则最多 {MAX_TARGETS} 个备选视觉模型"));
        }
        match rule.mode {
            Mode::Assist => { if let Some(target) = &rule.target { validate_target(target)?; } }
            Mode::Reroute => {
                let mut targets = HashSet::new();
                for target in &rule.targets {
                    validate_target(target)?;
                    if !targets.insert((target.provider.to_lowercase(), target.model.to_lowercase())) {
                        return Err(format!("第 {at} 条规则的备选视觉模型不能重复"));
                    }
                }
            }
        }
        // 只有真的会用上的规则才要求配全：停用的、总开关关掉的，允许留着半截。
        if !settings.enabled || !rule.enabled || rule.models.is_empty() { continue; }
        match rule.mode {
            Mode::Assist => {
                if rule.source == ModelSource::Custom && rule.target.is_none() {
                    return Err(format!("第 {at} 条规则选了「指定视觉模型」，请先选一只"));
                }
            }
            Mode::Reroute => {
                let designated = rule.fallback == Fallback::Designated && !rule.targets.is_empty();
                if !rule.prefer_current_route && !designated {
                    return Err(format!("第 {at} 条规则请选择「优先使用当前路由内的视觉模型」，或指定接管回答的视觉模型"));
                }
                if rule.fallback == Fallback::Designated && rule.targets.is_empty() {
                    return Err(format!("第 {at} 条规则选了「指定备选模型」，请至少添加一个视觉模型"));
                }
            }
        }
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
    super::try_update_raw_field(KEY_VISION_COMPAT, |raw| {
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "图片兼容已在其他位置修改，请重新读取后再改；本次改动未生效"));
        }
        let previous = from_value(Some(raw)).settings.unwrap_or_default();
        let mut next = settings.clone();
        // 旧界面（手机端）发来的还是两块设置，没有 rules：折回规则列表。
        //
        // 同一种方式已经有多条规则时两块表达不了，直接拒掉并指路 —— 让它照样存下去，
        // 等于用两条规则覆盖掉其余的，那是静默丢配置。
        if next.rules.is_empty() {
            let count = |mode: Mode| previous.rules.iter().filter(|rule| rule.mode == mode).count();
            if count(Mode::Assist) > 1 || count(Mode::Reroute) > 1 {
                return Err(GatewayError::bad_request(
                    "图片兼容已改为按规则配置；这一种方式下有多条规则时，请到「路由 → 图片兼容」里修改"));
            }
            next.fold_blocks();
        }
        validate(&next).map_err(GatewayError::bad_request)?;
        // 两块设置降级成投影：手机端与旧界面还在按它读。
        next.project_blocks();
        *raw = serde_json::to_value(&next).map_err(|error| GatewayError::new(error.to_string()))?;
        Ok(())
    }, || GatewayError::new("图片兼容保存失败，未应用本次修改"))
}
