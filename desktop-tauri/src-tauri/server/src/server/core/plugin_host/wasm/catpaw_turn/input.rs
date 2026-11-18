//! 固定 prepare/start 输入，只验证本轮材料，不从输入取得账号、出口或执行事实。
mod history;
pub(super) mod material;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{error, json_text};

pub(super) const SOURCE: &str = "CatX";
pub(super) const MODE: &str = "CATX_APP";
pub(super) const PERMISSION: &str = "unsafeBypassPermissions";
pub(super) const TOOL_VERSION: &str = "2.0.2";
pub(super) const MAX_MESSAGES: usize = 512;
const MAX_DIRECTIVES: usize = 2 * 1024 * 1024;

pub(super) struct PreparedInput {
    pub(super) constraints: Value,
    pub(super) message_count: usize,
    pub(super) continuation: bool,
    pub(super) revision: u64,
}

impl PreparedInput {
    pub(super) fn parse(params: &Value, client_stream: bool) -> Result<Self, proto::WireError> {
        json_text::shape(params, 3 * 1024 * 1024)?;
        json_text::fields(
            params,
            &["constraints", "message_count"],
            &["constraints", "message_count"],
        )?;
        let constraints = &params["constraints"];
        json_text::fields(
            constraints,
            &[
                "model_type",
                "model_name",
                "effort",
                "context",
                "tool_configs",
                "tool_choice",
                "parallel_requested",
                "client_stream",
            ],
            &[
                "model_type",
                "model_name",
                "effort",
                "context",
                "tool_configs",
                "tool_choice",
                "parallel_requested",
                "client_stream",
            ],
        )?;
        let model = constraints["model_type"]
            .as_i64()
            .filter(|value| *value > 0)
            .ok_or_else(error::invalid)?;
        let name = constraints["model_name"]
            .as_str()
            .filter(|value| json_text::identifier(value, 1024))
            .ok_or_else(error::invalid)?;
        let effort = optional_text(constraints.get("effort"), 64)?;
        let context = optional_text(constraints.get("context"), 64)?;
        if context
            .as_deref()
            .is_some_and(|text| text.parse::<u64>().ok().is_none_or(|value| value == 0))
        {
            return Err(error::invalid());
        }
        if constraints["client_stream"].as_bool() != Some(client_stream)
            || !matches!(
                constraints["parallel_requested"],
                Value::Null | Value::Bool(_)
            )
        {
            return Err(error::invalid());
        }
        tools(&constraints["tool_configs"], &constraints["tool_choice"])?;
        let message_count = params["message_count"]
            .as_u64()
            .filter(|count| (1..=MAX_MESSAGES as u64).contains(count))
            .ok_or_else(error::invalid)? as usize;
        Ok(Self {
            constraints: json!({ "model_type": model, "model_name": name,
            "effort": effort, "context": context, "tool_configs": constraints["tool_configs"],
            "tool_choice": constraints["tool_choice"], "parallel_requested": constraints["parallel_requested"],
            "client_stream": client_stream }),
            message_count,
            continuation: false,
            revision: 0,
        })
    }
}

fn optional_text(
    value: Option<&Value>,
    maximum: usize,
) -> Result<Option<String>, proto::WireError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if json_text::identifier(text, maximum) => Ok(Some(text.clone())),
        _ => Err(error::invalid()),
    }
}

fn tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn tools(configs: &Value, choice: &Value) -> Result<(), proto::WireError> {
    json_text::shape(configs, 2 * 1024 * 1024)?;
    let configs = configs
        .as_array()
        .filter(|items| items.len() <= 256)
        .ok_or_else(error::invalid)?;
    let mut names = std::collections::BTreeSet::new();
    for config in configs {
        json_text::fields(
            config,
            &["name", "enable", "fromClient", "inputSchema", "description"],
            &["name", "enable", "fromClient", "inputSchema"],
        )?;
        let name = config["name"]
            .as_str()
            .filter(|value| tool_name(value))
            .ok_or_else(error::invalid)?;
        if !names.insert(name)
            || config["enable"] != true
            || config["fromClient"] != true
            || !config["inputSchema"].is_object()
        {
            return Err(error::invalid());
        }
        if let Some(value) = config.get("description") {
            if value
                .as_str()
                .is_none_or(|text| text.chars().count() > 8000)
            {
                return Err(error::invalid());
            }
        }
        json_text::shape(config, 256 * 1024)?;
    }
    match choice {
        Value::String(value) if matches!(value.as_str(), "auto" | "none" | "required") => {
            if value == "none" && !configs.is_empty() || value == "required" && configs.is_empty() {
                return Err(error::invalid());
            }
        }
        Value::Object(_) => {
            json_text::fields(choice, &["type", "function"], &["type", "function"])?;
            json_text::fields(&choice["function"], &["name"], &["name"])?;
            let name = choice["function"]["name"]
                .as_str()
                .ok_or_else(error::invalid)?;
            if choice["type"] != "function" || configs.len() != 1 || !names.contains(name) {
                return Err(error::invalid());
            }
        }
        _ => return Err(error::invalid()),
    }
    Ok(())
}

pub(super) struct Correlation {
    pub(super) conversation: String,
    pub(super) turn: String,
    pub(super) messages: Vec<String>,
    pub(super) response: String,
    pub(super) created: i64,
}

pub(super) struct StartInput {
    pub(super) round: String,
    pub(super) turn: String,
    pub(super) history: String,
    pub(super) directives: String,
}

impl StartInput {
    pub(super) fn parse(
        params: &Value,
        prepared: &PreparedInput,
        ids: &Correlation,
        handle: &str,
    ) -> Result<Self, proto::WireError> {
        json_text::fields(
            params,
            &[
                "handle",
                "round_json",
                "turn_json",
                "history_json",
                "request_context_json",
                "session_revision",
            ],
            &[
                "handle",
                "round_json",
                "turn_json",
                "history_json",
                "request_context_json",
                "session_revision",
            ],
        )?;
        if params["handle"].as_str() != Some(handle)
            || params["session_revision"].as_u64() != Some(prepared.revision)
        {
            return Err(error::invalid());
        }
        let text = |field: &str| {
            params[field]
                .as_str()
                .filter(|text| !text.is_empty())
                .ok_or_else(error::invalid)
        };
        let round = if prepared.continuation {
            if !params["round_json"].is_null() { return Err(error::invalid()); }
            ""
        } else { text("round_json")? };
        let turn = text("turn_json")?;
        let history = text("history_json")?;
        let directives = text("request_context_json")?;
        let bytes = [round.len(), turn.len(), history.len(), directives.len()]
            .into_iter()
            .try_fold(0usize, |total, bytes| total.checked_add(bytes))
            .ok_or_else(error::budget)?;
        if bytes > 24 * 1024 * 1024 {
            return Err(error::budget());
        }
        let history_view = json_text::parse(history, json_text::MAX_BYTES)?;
        history::check(&history_view, prepared, ids)?;
        let directives_view = json_text::parse(directives, MAX_DIRECTIVES)?;
        json_text::fields(
            &directives_view,
            &["system_prompt", "rules_message"],
            &["system_prompt", "rules_message"],
        )?;
        for name in ["system_prompt", "rules_message"] {
            match &directives_view[name] {
                Value::Null => {}
                Value::String(text) if !text.is_empty() && text.len() <= 1024 * 1024 => {}
                _ => return Err(error::invalid()),
            }
        }
        if !prepared.continuation {
            let round_view = json_text::parse(round, json_text::MAX_BYTES)?;
            check_round(&round_view, prepared, ids, &history_view, &directives_view)?;
        }
        let turn_view = json_text::parse(turn, json_text::MAX_BYTES)?;
        check_turn(&turn_view, prepared, ids, &history_view, &directives_view)?;
        Ok(Self {
            round: round.to_string(),
            turn: turn.to_string(),
            history: history.to_string(),
            directives: directives.to_string(),
        })
    }
}

fn common(
    value: &Value,
    prepared: &PreparedInput,
    ids: &Correlation,
    directives: &Value,
) -> Result<(), proto::WireError> {
    if value["conversationId"].as_str() != Some(ids.conversation.as_str())
        || value["source"] != SOURCE
        || value["mode"] != MODE
        || value["permissionMode"] != PERMISSION
        || value["toolVersion"] != TOOL_VERSION
        || value["modelType"] != prepared.constraints["model_type"]
    {
        return Err(error::invalid());
    }
    let system = &directives["system_prompt"];
    if system.is_null() {
        if value.get("systemPromptContext").is_some() {
            return Err(error::invalid());
        }
    } else {
        json_text::fields(
            &value["systemPromptContext"],
            &["systemPromptOverride"],
            &["systemPromptOverride"],
        )?;
        if &value["systemPromptContext"]["systemPromptOverride"] != system {
            return Err(error::invalid());
        }
    }
    let rules = &directives["rules_message"];
    if rules.is_null() {
        if value.get("rulesMessage").is_some() {
            return Err(error::invalid());
        }
    } else if &value["rulesMessage"] != rules {
        return Err(error::invalid());
    }
    Ok(())
}

fn check_round(
    value: &Value,
    prepared: &PreparedInput,
    ids: &Correlation,
    history: &Value,
    directives: &Value,
) -> Result<(), proto::WireError> {
    json_text::fields(
        value,
        &[
            "conversationId",
            "source",
            "messages",
            "modelType",
            "mode",
            "permissionMode",
            "toolVersion",
            "systemPromptContext",
            "rulesMessage",
            "requestContext",
        ],
        &[
            "conversationId",
            "source",
            "messages",
            "modelType",
            "mode",
            "permissionMode",
            "toolVersion",
        ],
    )?;
    common(value, prepared, ids, directives)?;
    if &value["messages"] != history {
        return Err(error::invalid());
    }
    let mut expected = serde_json::Map::new();
    for field in ["effort", "context"] {
        if !prepared.constraints[field].is_null() {
            expected.insert(field.to_string(), prepared.constraints[field].clone());
        }
    }
    if expected.is_empty() {
        if value.get("requestContext").is_some() {
            return Err(error::invalid());
        }
    } else {
        json_text::fields(&value["requestContext"], &["modelParams"], &["modelParams"])?;
        json_text::fields(
            &value["requestContext"]["modelParams"],
            &["declarativeParams"],
            &["declarativeParams"],
        )?;
        if value["requestContext"]["modelParams"]["declarativeParams"] != Value::Object(expected) {
            return Err(error::invalid());
        }
    }
    Ok(())
}

fn check_turn(
    value: &Value,
    prepared: &PreparedInput,
    ids: &Correlation,
    history: &Value,
    directives: &Value,
) -> Result<(), proto::WireError> {
    json_text::fields(
        value,
        &[
            "conversationId",
            "turnRequestId",
            "source",
            "action",
            "message",
            "modelType",
            "mode",
            "permissionMode",
            "toolVersion",
            "toolConfigs",
            "availableTools",
            "systemPromptContext",
            "rulesMessage",
        ],
        &[
            "conversationId",
            "turnRequestId",
            "source",
            "action",
            "message",
            "modelType",
            "mode",
            "permissionMode",
            "toolVersion",
            "toolConfigs",
            "availableTools",
        ],
    )?;
    common(value, prepared, ids, directives)?;
    let last = history
        .as_array()
        .and_then(|items| items.last())
        .ok_or_else(error::invalid)?;
    let available: Vec<Value> = prepared.constraints["tool_configs"]
        .as_array()
        .ok_or_else(error::invalid)?
        .iter()
        .map(|tool| tool["name"].clone())
        .collect();
    if value["turnRequestId"].as_str() != Some(ids.turn.as_str())
        || value["action"] != "turn"
        || &value["message"] != last
        || value["toolConfigs"] != prepared.constraints["tool_configs"]
        || value["availableTools"] != Value::Array(available)
    {
        return Err(error::invalid());
    }
    Ok(())
}
