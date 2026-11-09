//! 标题生成路由配置：把客户端自动生成的标题请求改写到指定的低成本模型。
//!
//! ── 为什么单独一层，而不是并进压缩路由 ────────────────────────
//! 压缩路由按「客户端 + 来源模型」映射，一条压缩请求对应一条规则；标题生成
//! 是按**客户端**识别的（同一个客户端只会用固定提示词要标题），且目标模型只有
//! 一个。两者的配置形状、校验规则、失败语义都不同，混在一张表里会让
//! 「这条规则为什么没命中」变得难解释。
//!
//! ── 识别依据 ──────────────────────────────────────────────────
//! 标题请求都是「固定 system 提示词 + 一段待总结内容」，正文里带客户端特有的
//! 措辞。判据放在 `core::title_routing`，这里只管配置的形状与持久化。
//!
//! ── 与压缩路由同一套持久化约定 ────────────────────────────────
//! 整块保存、按修订号防覆盖、请求读同一份运行期快照。

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;

pub const KEY_TITLE_ROUTES: &str = "titleModelRoutes";
/// 与压缩路由一样留一个宽松上限：挡住异常大的载荷，不该挡住正常配置。
pub const MAX_CLIENTS: usize = 32;
/// 规则条数上限：标题生成是「小请求分流」，正常也就一两条，32 条足够宽松。
pub const MAX_RULES: usize = 32;

/// 支持识别的客户端。新增一家要同时在 `core::title_routing` 里加判据，
/// 否则界面能勾、请求永远不命中。
pub const CLIENT_ZCODE: &str = "zcode";
pub const CLIENT_CLAUDE_CODE: &str = "claude-code";
pub const CLIENT_CODEX: &str = "codex";

pub const CLIENTS: &[(&str, &str)] = &[
    (CLIENT_ZCODE, "ZCode"),
    (CLIENT_CLAUDE_CODE, "Claude Code"),
    (CLIENT_CODEX, "Codex"),
];

/// 标题模型不可用时怎么办。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// 退回客户端原模型重试一次（默认）：标题请求失败会直接影响会话列表的显示。
    #[default]
    Original,
    /// 明确报错，不改用原模型。
    Error,
}

impl Fallback {
    /// 用于日志与诊断；序列化本身走 serde 的 snake_case。
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self { Self::Original => "original", Self::Error => "error" }
    }
}

/// 标题生成的一条规则：认哪些客户端、改成哪只模型、失败怎么办。
///
/// 为什么从「一套模型管所有客户端」改成逐条规则：客户端可以不止一家想分流，
/// 而且各家的标题提示词长度不同，值得配不同的模型（便宜的那只给 Codex，
/// 长一点的给 Claude Code）。逐条规则也让「为什么这条请求改写了」能落到具体一行。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleRule {
    /// 这条规则是否生效。总开关关掉时整块都不生效，这里管的是单条。
    #[serde(default)]
    pub enabled: bool,
    /// 参与识别的客户端 id。空 = 这条规则不生效（等同关闭）。
    pub clients: Vec<String>,
    /// 标题模型。空串 = 未配置。
    pub model: String,
    pub fallback: Fallback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleRoutes {
    pub enabled: bool,
    /// 按列表顺序匹配，第一条命中的生效。
    pub rules: Vec<TitleRule>,
}

impl Default for TitleRoutes {
    fn default() -> Self { Self { enabled: true, rules: Vec::new() } }
}

/// 读时形状：同时认新形状（`rules`）与旧形状（`clients` + `model` + `fallback`）。
///
/// 旧形状是 2026-11 之前那一版「一套模型管所有客户端」。这里只做读时折叠
/// （旧配置 → 一条规则），写回去一律是新形状 —— 于是不需要单独的迁移步骤，
/// 用户下次保存就换过来了。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTitleRoutes {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    rules: Option<Vec<TitleRule>>,
    #[serde(default)]
    clients: Option<Vec<String>>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    fallback: Option<Fallback>,
}

impl<'de> Deserialize<'de> for TitleRoutes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawTitleRoutes::deserialize(deserializer)?;
        Ok(match raw.rules {
            Some(rules) => Self { enabled: raw.enabled, rules },
            // 旧形状：折叠成一条规则
            None if raw.clients.is_some() || raw.model.is_some() => Self {
                enabled: raw.enabled,
                rules: vec![TitleRule {
                    enabled: true,
                    clients: raw.clients.unwrap_or_default(),
                    model: raw.model.unwrap_or_default(),
                    fallback: raw.fallback.unwrap_or_default(),
                }],
            },
            None => Self { enabled: raw.enabled, rules: Vec::new() },
        })
    }
}

#[derive(Clone, Debug)]
pub struct TitleRoutesSnapshot {
    pub settings: Result<TitleRoutes, String>,
    pub revision: String,
}

impl Default for TitleRoutesSnapshot {
    fn default() -> Self { from_value(None) }
}

pub fn model_key(model: &str) -> String { model.trim().to_lowercase() }

pub fn client_label(id: &str) -> Option<&'static str> {
    CLIENTS.iter().find(|(key, _)| *key == id).map(|(_, label)| *label)
}

pub fn validate(settings: &TitleRoutes) -> Result<(), String> {
    if settings.rules.len() > MAX_RULES {
        return Err(format!("标题生成路由最多支持 {MAX_RULES} 条规则"));
    }
    for (index, rule) in settings.rules.iter().enumerate() {
        let at = index + 1;
        if rule.clients.len() > MAX_CLIENTS {
            return Err(format!("标题生成路由第 {at} 条规则最多支持 {MAX_CLIENTS} 个客户端"));
        }
        let mut seen = HashSet::new();
        for client in &rule.clients {
            if client_label(client).is_none() {
                return Err(format!("标题生成路由不支持客户端「{client}」"));
            }
            if !seen.insert(client.as_str()) {
                return Err(format!("第 {at} 条规则里「{}」重复出现", client_label(client).unwrap_or(client)));
            }
        }
        // 只有真的启用时才要求配全：关掉以后保留半截配置不该变成非法文件。
        let live = settings.enabled && rule.enabled;
        if live && rule.clients.is_empty() {
            return Err(format!("启用标题生成路由第 {at} 条规则前请先选择客户端"));
        }
        let model = &rule.model;
        if model.trim().is_empty() {
            if live { return Err(format!("启用标题生成路由第 {at} 条规则前请先选择标题模型")); }
            continue;
        }
        if model.trim() != model || model.chars().count() > 256 || model.chars().any(char::is_control) {
            return Err(format!("第 {at} 条规则的标题模型 ID 不能超过 256 个字符、不能含控制字符或首尾空格"));
        }
    }
    Ok(())
}

/// 这条规则此刻是否真的会命中：自己开着、勾了客户端、配了模型。
fn effective(rule: &TitleRule) -> bool {
    rule.enabled && !rule.clients.is_empty() && !rule.model.trim().is_empty()
}

fn revision(value: Option<&Value>) -> String {
    let raw = value.cloned().unwrap_or(Value::Null);
    format!("{:x}", Sha256::digest(raw.to_string().as_bytes()))
}

fn from_value(value: Option<&Value>) -> TitleRoutesSnapshot {
    let settings = match value {
        None => Ok(TitleRoutes::default()),
        Some(value) => serde_json::from_value::<TitleRoutes>(value.clone())
            .map_err(|error| format!("标题生成路由配置格式不正确：{error}"))
            .and_then(|settings| { validate(&settings)?; Ok(settings) }),
    };
    TitleRoutesSnapshot { settings, revision: revision(value) }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<TitleRoutesSnapshot> {
    Arc::new(from_value(raw.get(KEY_TITLE_ROUTES)))
}

pub fn current() -> Arc<TitleRoutesSnapshot> {
    super::CONFIG.read().unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref().map(|config| config.title_routes.clone()).unwrap_or_default()
}

/// 当前生效的配置：总开关打开、且至少有一条真会命中的规则。
pub fn active() -> Option<TitleRoutes> {
    let snapshot = current();
    let settings = snapshot.settings.as_ref().ok()?;
    if !settings.enabled || !settings.rules.iter().any(effective) { return None; }
    Some(settings.clone())
}

/// 这次请求该走哪条规则：按列表顺序取第一条命中的 —— 顺序即优先级。
pub fn match_rule(settings: &TitleRoutes, client: &str) -> Option<TitleRule> {
    settings.rules.iter()
        .filter(|rule| effective(rule))
        .find(|rule| rule.clients.iter().any(|id| id == client))
        .cloned()
}

pub fn save(settings: TitleRoutes, expected: &str) -> Result<(), GatewayError> {
    validate(&settings).map_err(GatewayError::bad_request)?;
    let value = serde_json::to_value(settings).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_field(KEY_TITLE_ROUTES, |raw| {
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "标题生成路由已在其他位置修改，请重新读取后再保存；当前草稿未丢失"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("标题生成路由保存失败，未应用本次修改"))
}
