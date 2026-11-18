use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::{self, prompt_rules, PromptSettings};
use crate::server::core::{client, degrade};
use crate::server::core::prompt::rules::{self, Content, Rule};
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn rules_view(settings: &PromptSettings) -> Vec<Value> {
    if settings.rules_configured {
        settings.rules.iter().map(|entry| prompt_rules::view_rule(&entry.rule, &entry.prompt)).collect()
    } else {
        prompt_rules::legacy_rules(settings).into_iter().filter_map(|rule| {
            let provider = rule.conditions.first()?.values.first()?;
            let prompt = settings.providers.get(provider)?;
            Some(prompt_rules::view_rule(&rule, prompt))
        }).collect()
    }
}

fn client_options() -> Vec<Value> {
    let mut options: Vec<_> = client::catalog::CLIENTS.iter()
        .map(|item| json!({ "id": item.id, "label": item.name })).collect();
    let snapshot = config::client_identification::current();
    if let Ok(settings) = &snapshot.settings {
        options.extend(settings.clients.iter().map(|item| json!({ "id": item.id, "label": item.name })));
    }
    options.push(json!({ "id": "other", "label": "未识别客户端" }));
    options
}

fn view(config: &config::RuntimeConfig, state: &ServerState, providers: Value) -> Value {
    let settings = config.prompt_settings();
    let fallback = prompt_rules::fallback(settings);
    let resolved = prompt_rules::resolved_fallback(settings);
    json!({
        "config": { "rules": rules_view(settings), "fallback": prompt_rules::view_content(&fallback, &resolved) },
        "revision": prompt_rules::revision(config.raw()),
        "providers": providers, "clients": client_options(),
        "models": crate::server::core::providers::catalog::advertised_model_ids(&state.store()),
        "maxRules": rules::MAX_RULES, "maxConditions": rules::MAX_CONDITIONS,
        "maxValues": rules::MAX_VALUES, "maxTextBytes": rules::MAX_TEXT_BYTES,
        "configError": settings.rules_error,
        "degradeActive": degrade::active(), "degradeUntilText": degrade::until_text(),
        "gateway": super::prompt::gateway_json(settings),
        "gatewayText": super::prompt::gateway_text_json(settings),
    })
}

pub async fn get_settings(State(state): State<ServerState>) -> Response {
    let providers = super::prompt::provider_options(&state).await;
    match tokio::task::spawn_blocking(move || {
        view(&config::current(), &state, providers)
    }).await {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    rules: Vec<Rule>,
    fallback: Content,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettings {
    config: Settings,
    revision: String,
}

pub async fn put_settings(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("系统提示词规则不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    match tokio::task::spawn_blocking(move || {
        let count = payload.config.rules.len();
        prompt_rules::save(payload.config.rules, payload.config.fallback, &payload.revision)?;
        crate::server::logging::log("[Config]", &format!("系统提示词已保存：{count} 条规则，仅对新请求生效"));
        Ok::<_, GatewayError>(())
    }).await {
        Ok(Ok(())) => get_settings(State(state)).await,
        Ok(Err(error)) => errors::management_error(error.status_code, error.message),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sample {
    provider: String,
    client: String,
    model: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewSettings {
    config: Settings,
    sample: Sample,
}

pub async fn preview_settings(body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<PreviewSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("命中测试内容不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    if let Err(error) = rules::validate_rules(&payload.config.rules).and_then(|_| rules::validate_content(&payload.config.fallback)) {
        return errors::management_error(400, error);
    }
    if payload.sample.provider.len() > 128 || payload.sample.client.len() > 128 || payload.sample.model.len() > 4096
        || [&payload.sample.provider, &payload.sample.client, &payload.sample.model].iter().any(|value| value.chars().any(char::is_control)) {
        return errors::management_error(400, "命中测试的请求条件无效或过长");
    }
    for (index, rule) in payload.config.rules.iter().enumerate() {
        if !rule.enabled || rule.conditions.is_empty() { continue; }
        let matchers = match rule.conditions.iter().map(rules::Matcher::compile).collect::<Result<Vec<_>, _>>() {
            Ok(matchers) => matchers,
            Err(error) => return errors::management_error(400, error),
        };
        let matches = |matcher: &rules::Matcher| matcher.matches(&payload.sample.provider, &payload.sample.client, &payload.sample.model);
        let hit = match rule.logic { rules::Logic::All => matchers.iter().all(matches), rules::Logic::Any => matchers.iter().any(matches) };
        if hit {
            return ok_json(json!({ "ruleId": rule.id, "index": index, "name": rule.name, "mode": rule.mode.as_str() }));
        }
    }
    ok_json(json!({ "ruleId": null, "index": null, "name": null, "mode": payload.config.fallback.mode.as_str() }))
}
