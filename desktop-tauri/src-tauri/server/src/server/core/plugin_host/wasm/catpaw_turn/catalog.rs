//! 原 CatPaw 账号与冻结出口的本次模型选择见证；展示缓存与插件数字不代替原响应。
//! 仅原轮次 worker 在 round 前查询一次，返回物体不构成跨账号目录权限或续接许可。

use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{Map, Value};
use tokio::time::Instant;

use super::{input::PreparedInput, json_text, network, source};
use crate::server::core::{
    egress::managed::FrozenHttpClient,
    plugin_host::wasm::conversation::SourceRef,
};

use super::super::catpaw_account::Account as RowSource;

const URL: &str = "https://ai.catpaw.meituan.com/api/agent/maas/model-types";
const BODY: &str = r#"{"tenant":"CatDesk","scene":"CATX_APP","env":"EXTERNAL"}"#;
const MAX_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct Witness {
    source: SourceRef,
    constraints: Value,
    observed_at: i64,
}

impl Witness {
    pub(super) fn check(
        &self,
        source: &SourceRef,
        prepared: &PreparedInput,
    ) -> Result<(), proto::WireError> {
        if !self.source.same_source(source)
            || self.constraints != prepared.constraints
            || self.observed_at <= 0
        {
            return Err(unavailable());
        }
        source.check_current().map_err(|_| unavailable())
    }
}

fn static_model(name: &str, model_type: i64) -> bool {
    matches!(
        (name_key(name).as_str(), model_type),
        ("kimi-k3", 83) | ("glm-5-3-flash", 91)
    )
}

pub(super) async fn verify(
    original: &SourceRef,
    account: &RowSource,
    client: &FrozenHttpClient,
    prepared: &PreparedInput,
) -> Result<Witness, proto::WireError> {
    source::check(original, account)?;
    let deadline = original
        .deadline()
        .min(Instant::now() + Duration::from_secs(30));
    let request = network::prepare(client, account, network::Route::Catalog, BODY)?;
    source::inspect_account(original, account).await.map_err(|_| unavailable())?;
    // 原独立轮次 worker 持完整出口和账号；取消不把已经开始的目录请求改投另一来源。
    let bytes = tokio::time::timeout_at(deadline, async {
        let mut response = client
            .client()
            .map_err(|_| unavailable())?
            .execute(request.into_catalog()?)
            .await
            .map_err(|_| unavailable())?;
        if response.status() != reqwest::StatusCode::OK
            || response.url().as_str() != URL
            || response.headers().contains_key("location")
            || response
                .content_length()
                .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|size| size > MAX_BYTES)
            {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_, proto::WireError>(bytes)
    })
    .await
    .map_err(|_| unavailable())??;
    let text = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
    let response = json_text::parse(text, MAX_BYTES).map_err(|_| unavailable())?;
    let fields = response.as_object().ok_or_else(unavailable)?;
    if !matches!(fields.get("code").and_then(Value::as_i64), Some(0 | 200))
        || fields.get("error").is_some_and(|value| !value.is_null())
        || fields
            .get("success")
            .is_some_and(|value| value.as_bool() != Some(true))
    {
        return Err(unavailable());
    }
    let models = response["data"]
        .as_array()
        .filter(|models| !models.is_empty() && models.len() <= 512)
        .ok_or_else(unavailable)?;
    let material = original.material();
    let model = select(models, material.model())?;
    let model_type = model["modelTypeId"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(unavailable)?;
    let name = model["modelTypeName"]
        .as_str()
        .filter(|name| valid_text(name, 1024))
        .ok_or_else(unavailable)?;
    if prepared.constraints["model_type"].as_i64() != Some(model_type)
        || prepared.constraints["model_name"].as_str() != Some(name.trim())
    {
        return Err(unavailable());
    }
    let has_images = material.messages().iter().any(|message| message["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["type"] == "image_url")));
    if has_images && model.get("supportImage").is_some_and(|flag| !flag_value(flag).is_ok_and(|value| value)) { return Err(unavailable()); }
    if has_images && model.get("supportImage").is_none() && !static_model(name, model_type) { return Err(unavailable()); }
    let definitions = match model.get("parameterDefinitions") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(definitions))
            if definitions.len() <= 64 && definitions.iter().all(Value::is_object) =>
        {
            definitions.as_slice()
        }
        _ => return Err(unavailable()),
    };
    let parameters = material.parameters();
    let known = static_model(name, model_type);
    let effort = effort(parameters, model, definitions, known)?;
    let context = context(parameters, definitions, known && model_type == 91)?;
    if !same_optional(&prepared.constraints["effort"], effort.as_deref())
        || !same_optional(&prepared.constraints["context"], context.as_deref())
    {
        return Err(unavailable());
    }
    source::check(original, account)?;
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    let observed_at = crate::server::logging::now_ms();
    if observed_at <= 0 {
        return Err(unavailable());
    }
    Ok(Witness {
        source: original.clone(),
        constraints: prepared.constraints.clone(),
        observed_at,
    })
}

fn select<'a>(models: &'a [Value], requested: &Value) -> Result<&'a Value, proto::WireError> {
    let requested = match requested {
        Value::String(text) if valid_text(text, 1024) => text.trim().to_string(),
        Value::Number(value) => value
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(unavailable)?
            .to_string(),
        _ => return Err(unavailable()),
    };
    let key = name_key(&requested);
    let mut canonical = None;
    let mut alternate = None;
    for model in models {
        let fields = model.as_object().ok_or_else(unavailable)?;
        if fields
            .get("provider")
            .and_then(Value::as_str)
            .is_some_and(|provider| provider.eq_ignore_ascii_case("USER_CUSTOM"))
        {
            continue;
        }
        let extended = match fields.get("extendedInfo") {
            None | Some(Value::Null) => None,
            Some(Value::Object(fields)) => Some(fields),
            _ => return Err(unavailable()),
        };
        if let Some(flag) = extended
            .and_then(|fields| fields.get("isAuto"))
            .filter(|value| !value.is_null())
        {
            if flag_value(flag)? {
                continue;
            }
        }
        let Some(name) = fields
            .get("modelTypeName")
            .and_then(Value::as_str)
            .filter(|name| valid_text(name, 1024))
        else {
            continue;
        };
        let canonical_match = name_key(name) == key;
        let caption = extended
            .and_then(|fields| fields.get("modelCaptionZhCN"))
            .and_then(Value::as_str)
            .filter(|text| valid_text(text, 1024));
        let description = fields
            .get("description")
            .and_then(Value::as_str)
            .filter(|text| valid_text(text, 1024));
        let numeric = requested.parse::<i64>().ok().is_some_and(|id| {
            id > 0 && fields.get("modelTypeId").and_then(Value::as_i64) == Some(id)
        });
        let alternative_match = numeric
            || caption
                .or(description)
                .is_some_and(|name| name_key(name) == key);
        if canonical_match {
            unique(&mut canonical, model)?;
        }
        if alternative_match {
            unique(&mut alternate, model)?;
        }
    }
    canonical.or(alternate).ok_or_else(unavailable)
}

fn unique<'a>(slot: &mut Option<&'a Value>, value: &'a Value) -> Result<(), proto::WireError> {
    if slot.is_some_and(|previous| previous != value) {
        return Err(unavailable());
    }
    *slot = Some(value);
    Ok(())
}

fn effort(
    parameters: &Map<String, Value>,
    model: &Value,
    definitions: &[Value],
    known: bool,
) -> Result<Option<String>, proto::WireError> {
    let mut requested = None;
    for name in ["reasoning_effort", "reasoningEffort", "effort"] {
        let Some(value) = parameters.get(name).filter(|value| !value.is_null()) else {
            continue;
        };
        let text = value
            .as_str()
            .filter(|text| valid_text(text, 64))
            .ok_or_else(unavailable)?;
        agree(&mut requested, text.trim().to_ascii_lowercase())?;
    }
    let Some(requested) = requested else {
        return Ok(None);
    };
    if let Some(value) = model
        .get("supportThinking")
        .filter(|value| !value.is_null())
    {
        if !flag_value(value)? {
            return Err(unavailable());
        }
    }
    let definition = definition(definitions, "effort")?;
    if let Some(values) = definition
        .and_then(|value| value.get("values"))
        .filter(|value| !value.is_null())
    {
        let values = values
            .as_array()
            .filter(|values| values.len() <= 64)
            .ok_or_else(unavailable)?;
        if !values.is_empty() {
            let mut found = None;
            for value in values {
                let text = value["value"]
                    .as_str()
                    .filter(|text| valid_text(text, 64))
                    .ok_or_else(unavailable)?;
                if text.eq_ignore_ascii_case(&requested) {
                    agree(&mut found, text.to_string())?;
                }
            }
            return found.map(Some).ok_or_else(unavailable);
        }
    }
    // 原静态事实只适用于本次远程目录仍明确返回同一数字ID的两种已记录模型。
    if known && matches!(requested.as_str(), "low" | "high" | "max") {
        Ok(Some(requested))
    } else {
        Err(unavailable())
    }
}

fn context(
    parameters: &Map<String, Value>,
    definitions: &[Value],
    known_context: bool,
) -> Result<Option<String>, proto::WireError> {
    let mut requested = None;
    for name in [
        "context_window",
        "contextWindow",
        "context_length",
        "contextLength",
    ] {
        if let Some(value) = parameters.get(name).filter(|value| !value.is_null()) {
            agree(&mut requested, context_value(value)?)?;
        }
    }
    for (name, path) in [
        ("model_params", "/context"),
        ("modelParams", "/context"),
        ("request_context", "/modelParams/declarativeParams/context"),
        ("requestContext", "/modelParams/declarativeParams/context"),
    ] {
        if let Some(value) = parameters
            .get(name)
            .and_then(|value| value.pointer(path))
            .filter(|value| !value.is_null())
        {
            agree(&mut requested, context_value(value)?)?;
        }
    }
    let definition = definition(definitions, "context")?;
    let mut supported = std::collections::BTreeSet::new();
    if let Some(definition) = definition {
        let values = definition["values"]
            .as_array()
            .filter(|values| !values.is_empty() && values.len() <= 64)
            .ok_or_else(unavailable)?;
        for value in values {
            supported.insert(context_value(&value["value"])?);
        }
        if requested.is_none() {
            if let Some(value) = definition
                .get("defaultValue")
                .filter(|value| !value.is_null())
            {
                requested = Some(context_value(value)?);
            }
        }
    } else if known_context {
        supported.extend(["204800", "512000", "1024000"].map(str::to_string));
        if requested.is_none() {
            requested = Some("1024000".to_string());
        }
    }
    let Some(requested) = requested else {
        return Ok(None);
    };
    if !supported.contains(&requested) {
        return Err(unavailable());
    }
    Ok(Some(requested))
}

fn definition<'a>(values: &'a [Value], id: &str) -> Result<Option<&'a Value>, proto::WireError> {
    let mut matches = values.iter().filter(|value| value["id"] == id);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(unavailable());
    }
    Ok(first)
}

fn context_value(value: &Value) -> Result<String, proto::WireError> {
    let text = match value {
        Value::String(text) if valid_text(text, 64) => text.trim().to_ascii_lowercase(),
        Value::Number(number) => number
            .as_i64()
            .filter(|number| *number > 0)
            .ok_or_else(unavailable)?
            .to_string(),
        _ => return Err(unavailable()),
    };
    let text = match text.as_str() {
        "200k" => "204800",
        "500k" => "512000",
        "1m" | "1024k" => "1024000",
        other => other,
    };
    text.parse::<i64>()
        .ok()
        .filter(|number| *number > 0)
        .map(|number| number.to_string())
        .ok_or_else(unavailable)
}

fn flag_value(value: &Value) -> Result<bool, proto::WireError> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Number(value) if value.as_i64() == Some(0) => Ok(false),
        Value::Number(value) if value.as_i64() == Some(1) => Ok(true),
        Value::String(value) if value == "true" || value == "1" => Ok(true),
        Value::String(value) if value == "false" || value == "0" => Ok(false),
        _ => Err(unavailable()),
    }
}

fn agree(slot: &mut Option<String>, value: String) -> Result<(), proto::WireError> {
    if slot.as_ref().is_some_and(|previous| previous != &value) {
        return Err(unavailable());
    }
    *slot = Some(value);
    Ok(())
}
fn same_optional(value: &Value, expected: Option<&str>) -> bool {
    match expected {
        None => value.is_null(),
        Some(text) => value.as_str() == Some(text),
    }
}
fn valid_text(text: &str, maximum: usize) -> bool {
    !text.trim().is_empty() && text.len() <= maximum && !text.chars().any(char::is_control)
}
fn name_key(text: &str) -> String {
    text.trim()
        .chars()
        .map(|ch| {
            if ch.is_whitespace() || matches!(ch, '_' | '.') {
                '-'
            } else {
                ch.to_ascii_lowercase()
            }
        })
        .collect()
}
fn unavailable() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw本次原账号目录未确认请求模型、数字ID或档位，未用展示缓存和插件参数代替",
        "catpaw_model_source_unconfirmed",
    )
}
