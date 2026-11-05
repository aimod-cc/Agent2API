//! 上下文压缩路由配置：整块保存、按修订号防覆盖，请求读取同一份运行期快照。

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;

pub const KEY_COMPACT_ROUTES: &str = "compactModelRoutes";
pub const MAX_COMPACT_ROUTES: usize = 200;
pub const CLIENT_ZCODE: &str = "zcode";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactRoute {
    pub client: String,
    pub source: String,
    pub target: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactRoutes {
    pub enabled: bool,
    pub rules: Vec<CompactRoute>,
}

#[derive(Clone, Debug)]
pub struct CompactRoutesSnapshot {
    pub settings: Result<CompactRoutes, String>,
    pub revision: String,
}

impl Default for CompactRoutesSnapshot {
    fn default() -> Self { from_value(None) }
}

pub fn model_key(model: &str) -> String { model.trim().to_lowercase() }

pub fn validate(settings: &CompactRoutes) -> Result<(), String> {
    if settings.rules.len() > MAX_COMPACT_ROUTES {
        return Err(format!("压缩路由最多允许 {MAX_COMPACT_ROUTES} 条规则"));
    }
    let mut sources = HashSet::new();
    for rule in &settings.rules {
        if rule.client != CLIENT_ZCODE {
            return Err("压缩路由目前只支持 ZCode 客户端".into());
        }
        for model in [&rule.source, &rule.target] {
            if model.trim().is_empty() || model.trim() != model || model.chars().count() > 256
                || model.chars().any(char::is_control)
            {
                return Err("来源模型和压缩模型必须完整填写，模型 ID 不能超过 256 个字符或含控制字符".into());
            }
        }
        let source = model_key(&rule.source);
        if source == model_key(&rule.target) {
            return Err("来源模型和压缩模型不能相同".into());
        }
        if !sources.insert((rule.client.clone(), source)) {
            return Err(format!("同一客户端的来源模型不能重复：{}", rule.source));
        }
    }
    Ok(())
}

fn revision(value: Option<&Value>) -> String {
    let raw = value.cloned().unwrap_or(Value::Null);
    format!("{:x}", Sha256::digest(raw.to_string().as_bytes()))
}

fn from_value(value: Option<&Value>) -> CompactRoutesSnapshot {
    let settings = match value {
        None => Ok(CompactRoutes::default()),
        Some(value) => serde_json::from_value::<CompactRoutes>(value.clone())
            .map_err(|error| format!("压缩路由配置格式不正确：{error}"))
            .and_then(|settings| { validate(&settings)?; Ok(settings) }),
    };
    CompactRoutesSnapshot { settings, revision: revision(value) }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<CompactRoutesSnapshot> {
    Arc::new(from_value(raw.get(KEY_COMPACT_ROUTES)))
}

pub fn current() -> Arc<CompactRoutesSnapshot> {
    super::CONFIG.read().unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref().map(|config| config.compact_routes.clone()).unwrap_or_default()
}

pub fn save(settings: CompactRoutes, expected: &str) -> Result<(), GatewayError> {
    validate(&settings).map_err(GatewayError::bad_request)?;
    let value = serde_json::to_value(settings).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_field(KEY_COMPACT_ROUTES, |raw| {
        // 缺键时更新入口给 Null；它与读取端的初始修订号一致。
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "压缩路由已在其他位置修改，请重新读取后再保存；当前草稿未丢失"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("压缩路由保存失败，未应用本次修改"))
}
