use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use agent2api_provider_protocol as proto;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prompt {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub options: Vec<OptionItem>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OptionItem { pub value: String, pub label: String }

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Interaction {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub fields: Vec<Prompt>,
    #[serde(default)]
    pub captcha: Option<Geetest>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Geetest {
    pub gt: String,
    pub challenge: String,
    pub success: bool,
    pub new_captcha: bool,
}

#[derive(Default)]
pub(super) struct Interactions {
    pending: Mutex<HashMap<String, Interaction>>,
}

impl Interactions {
    pub fn remove(&self, flow: &str) {
        if let Ok(mut pending) = self.pending.lock() { pending.remove(flow); }
    }
    pub fn remember(&self, flow: &str, value: &Value, options: &super::manifest::Options) -> Result<(), proto::WireError> {
        let Some(value) = value.get("interaction").filter(|value| !value.is_null()) else {
            if value.get("interaction").is_some() || value["state"] == "done" || value["state"] == "failed" { self.remove(flow); }
            return Ok(());
        };
        if !options.allows("login.interaction.v1") { return Err(super::error("插件未声明交互式登录能力")); }
        let interaction: Interaction = serde_json::from_value(value.clone()).map_err(|_| super::errors::protocol())?;
        if interaction.id.is_empty() || interaction.id.len() > 128 || interaction.id.contains('\0') || interaction.title.len() > 256
            || interaction.message.len() > 8192 || interaction.fields.len() > 16 {
            return Err(super::error("登录交互声明超过限制"));
        }
        let mut keys = std::collections::HashSet::new();
        for field in &interaction.fields {
            if field.key.is_empty() || field.key.len() > 64 || !field.key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                || matches!(field.key.as_str(), "__proto__" | "constructor" | "prototype") || !keys.insert(&field.key)
                || !matches!(field.kind.as_str(), "text" | "select") || field.label.len() > 256
                || field.options.len() > 64 || field.kind == "select" && field.options.is_empty()
                || field.options.iter().any(|item| item.value.len() > 1024 || item.label.len() > 256) {
                return Err(super::error("登录交互字段无效"));
            }
        }
        if let Some(captcha) = &interaction.captcha {
            if !options.allows("login.captcha.geetest.v1") || captcha.gt.is_empty() || captcha.gt.len() > 128
                || captcha.challenge.is_empty() || captcha.challenge.len() > 256 { return Err(super::error("登录验证码声明无效")); }
        }
        if interaction.captcha.is_some() && !interaction.fields.iter().all(|field| field.key != "captcha") {
            return Err(super::error("登录输入字段不能与验证码结果重名"));
        }
        let mut pending = self.pending.lock().map_err(|_| super::error("登录交互不可用"))?;
        if pending.len() >= 64 && !pending.contains_key(flow) { return Err(super::error("登录交互数量超过上限")); }
        pending.insert(flow.into(), interaction);
        Ok(())
    }
    pub fn accepted(&self, flow: &str, value: &Value) {
        if let Ok(mut pending) = self.pending.lock() {
            if pending.get(flow).is_some_and(|interaction| value["interaction"].as_str() == Some(interaction.id.as_str())) {
                pending.remove(flow);
            }
        }
    }

    pub fn snapshot(&self, flow: &str) -> Option<Value> {
        self.pending.lock().ok()?.get(flow).and_then(|interaction| serde_json::to_value(interaction).ok())
    }
    pub fn check(&self, flow: &str, params: &Value) -> Result<(), proto::WireError> {
        let guard = self.pending.lock().map_err(|_| super::error("登录交互不可用"))?;
        let interaction = guard.get(flow).ok_or_else(|| super::error("登录交互已过期"))?;
        if params["interaction"].as_str() != Some(interaction.id.as_str()) { return Err(super::error("登录步骤已变化，请重新读取")); }
        let inputs: BTreeMap<String, String> = serde_json::from_value(params["inputs"].clone()).map_err(|_| super::error("登录输入必须是文本字段"))?;
        if inputs.iter().any(|(key, value)| value.len() > 8192 || !interaction.fields.iter().any(|field| &field.key == key)) {
            return Err(super::error("登录输入包含未申报字段或超过限制"));
        }
        for field in &interaction.fields {
            let value = inputs.get(&field.key).map(String::as_str).unwrap_or("");
            if !field.optional && value.trim().is_empty() || field.kind == "select" && !value.is_empty()
                && !field.options.iter().any(|option| option.value == value) { return Err(super::error("请完整填写当前登录步骤")); }
        }
        if let Some(captcha) = &interaction.captcha {
            let reply = &params["captcha"];
            if reply["geetest_challenge"].as_str().is_none_or(|value| value.is_empty() || value.len() > 512 || !value.starts_with(&captcha.challenge))
                || ["geetest_validate", "geetest_seccode"].iter().any(|key| reply[*key].as_str().is_none_or(|value| value.is_empty() || value.len() > 1024)) {
                return Err(super::error("请完成当前步骤的人机验证"));
            }
        } else if params.get("captcha").is_some_and(|value| !value.is_null()) { return Err(super::error("当前步骤不接受验证码结果")); }
        Ok(())
    }
}
