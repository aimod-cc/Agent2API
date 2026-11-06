use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_tools: Option<bool>,
    pub supports_images: Option<bool>,
    pub supports_video: Option<bool>,
    pub supports_reasoning: Option<bool>,
    pub reasoning_levels: Vec<String>,
    pub effort: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeModels {
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
    pub subagent: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NativeConfig {
    pub schema: u32,
    pub source_id: String,
    pub entry: Value,
    pub settings: Value,
    pub model_entries: Value,
    pub manual_model_entries: Value,
    pub auth: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub provider: String,
    pub effort: String,
    pub enabled: bool,
    pub default: bool,
    pub note: String,
    pub models: Vec<Model>,
    pub claude_models: ClaudeModels,
    /// gateway / key / env / none / preserve
    pub auth_mode: String,
    pub api_key: String,
    pub env_key: String,
    pub gateway_key_id: String,
    pub updated_at: i64,
    pub native: NativeConfig,
}

impl Profile {
    pub fn new(id: &str, name: &str, protocol: &str, base: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            protocol: protocol.into(),
            base_url: base.into(),
            kind: "relay".into(),
            auth_mode: "preserve".into(),
            enabled: true,
            default: true,
            native: NativeConfig {
                schema: 2,
                source_id: id.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn view(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "kind": self.kind, "protocol": self.protocol,
            "baseUrl": self.base_url, "model": self.model, "provider": self.provider,
            "effort": self.effort, "enabled": self.enabled, "default": self.default,
            "note": self.note, "models": self.models, "claudeModels": self.claude_models,
            "authMode": self.auth_mode, "apiKey": "", "envKey": self.env_key, "gatewayKeyId": self.gateway_key_id,
            "hasApiKey": !self.api_key.is_empty(), "schemaVersion": self.native.schema,
            "sourceId": self.native.source_id, "updatedAt": self.updated_at,
        })
    }
}

pub fn active_profile(profiles: &[Profile]) -> Option<&Profile> {
    profiles
        .iter()
        .find(|item| item.enabled && item.default)
        .or_else(|| profiles.iter().find(|item| item.enabled))
}
