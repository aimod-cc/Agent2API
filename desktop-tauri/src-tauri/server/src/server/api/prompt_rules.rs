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
        "upstreamModels": crate::server::core::providers::catalog::upstream_model_ids(&state.store()),
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
    #[serde(default)]
    upstream_model: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewSettings {
    config: Settings,
    sample: Sample,
}

fn resolve_preview_upstream_model(provider: &str, model: &str, explicit_upstream: &str) -> String {
    let explicit = explicit_upstream.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let requested = model.trim();
    if requested.is_empty() {
        return String::new();
    }
    let provider = provider.trim();
    if !provider.is_empty() {
        if let Some(group_model) = config::routing_profiles()
            .groups
            .values()
            .find(|group| group.model.eq_ignore_ascii_case(requested))
            .and_then(|group| {
                group
                    .members
                    .iter()
                    .find(|member| member.included && member.provider.eq_ignore_ascii_case(provider))
            })
            .and_then(|member| {
                let raw = member.model.trim();
                if raw.is_empty() {
                    return None;
                }
                Some(
                    crate::server::core::routing_groups::catalog_model(&member.provider, raw)
                        .map(|item| crate::server::core::models::model_id(&item))
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| raw.to_string()),
                )
            })
        {
            return group_model;
        }
    }
    crate::server::core::providers::catalog::effective_upstream_model(requested, provider, None)
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
    if payload.sample.provider.len() > 128 || payload.sample.client.len() > 128
        || payload.sample.model.len() > 4096 || payload.sample.upstream_model.len() > 4096
        || [&payload.sample.provider, &payload.sample.client, &payload.sample.model, &payload.sample.upstream_model]
            .iter().any(|value| value.chars().any(char::is_control)) {
        return errors::management_error(400, "命中测试的请求条件无效或过长");
    }
    let upstream_model = resolve_preview_upstream_model(
        &payload.sample.provider,
        &payload.sample.model,
        &payload.sample.upstream_model,
    );
    for (index, rule) in payload.config.rules.iter().enumerate() {
        if !rule.enabled || rule.conditions.is_empty() { continue; }
        let matchers = match rule.conditions.iter().map(rules::Matcher::compile).collect::<Result<Vec<_>, _>>() {
            Ok(matchers) => matchers,
            Err(error) => return errors::management_error(400, error),
        };
        let matches = |matcher: &rules::Matcher| {
            matcher.matches(
                &payload.sample.provider,
                &payload.sample.client,
                &payload.sample.model,
                &upstream_model,
            )
        };
        let hit = match rule.logic { rules::Logic::All => matchers.iter().all(matches), rules::Logic::Any => matchers.iter().any(matches) };
        if hit {
            return ok_json(json!({
                "ruleId": rule.id,
                "index": index,
                "name": rule.name,
                "mode": rule.mode.as_str(),
                "upstreamModel": upstream_model,
            }));
        }
    }
    ok_json(json!({
        "ruleId": null,
        "index": null,
        "name": null,
        "mode": payload.config.fallback.mode.as_str(),
        "upstreamModel": upstream_model,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn preview_json(payload: Value) -> (axum::http::StatusCode, Value) {
        let response = preview_settings(Bytes::from(payload.to_string())).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn preview_matches_upstream_model_and_preserves_legacy_sample_shape() {
        let config = json!({
            "rules": [
                {
                    "id": "r-upstream",
                    "name": "DeepSeek 上游",
                    "enabled": true,
                    "logic": "all",
                    "conditions": [
                        { "id": "c1", "field": "upstream_model", "op": "contains", "values": ["deepseek"] }
                    ],
                    "mode": "custom",
                    "file": "",
                    "text": "deepseek prompt"
                },
                {
                    "id": "r-client",
                    "name": "Haiku 客户端",
                    "enabled": true,
                    "logic": "all",
                    "conditions": [
                        { "id": "c2", "field": "model", "op": "contains", "values": ["claude-haiku"] }
                    ],
                    "mode": "append",
                    "file": "",
                    "text": "haiku prompt"
                }
            ],
            "fallback": { "mode": "passthrough", "file": "", "text": "" }
        });

        // 显式指定映射后的上游模型：命中 r-upstream
        let (status, body) = preview_json(json!({
            "config": config,
            "sample": {
                "provider": "workbuddy",
                "client": "claude-code",
                "model": "claude-haiku-4-5",
                "upstreamModel": "deepseek-v4.1-flash"
            }
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["data"]["ruleId"], "r-upstream");
        assert_eq!(body["data"]["mode"], "custom");
        assert_eq!(body["data"]["upstreamModel"], "deepseek-v4.1-flash");

        // 省略 upstreamModel（向后兼容旧前端）：无映射时自动回落为请求名，命中 r-client
        let (status, body) = preview_json(json!({
            "config": config,
            "sample": {
                "provider": "workbuddy",
                "client": "claude-code",
                "model": "claude-haiku-4-5"
            }
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["data"]["ruleId"], "r-client");
        assert_eq!(body["data"]["mode"], "append");
        assert_eq!(body["data"]["upstreamModel"], "claude-haiku-4-5");
    }
}
