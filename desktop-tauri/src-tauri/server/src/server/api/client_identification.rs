use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::client_identification;
use crate::server::core::client::{self, catalog, ClientEvidence, ClientSettings};
use crate::server::core::custom_providers;
use crate::server::core::providers::{self, catalog as provider_catalog};
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::{logging, ServerState};

fn view(state: &ServerState) -> Result<Value, GatewayError> {
    let snapshot = client_identification::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    // 已添加供应商（与模型管理左栏同源）：有可用账号的内置家 + 自定义家，
    // 供应商归属规则的第一列从这里选
    let mut providers: Vec<Value> = provider_catalog::managed_manifests(state.store()).into_iter()
        .map(|(id, _)| json!({ "id": id, "label": providers::label_of(&id), "custom": false }))
        .collect();
    providers.extend(custom_providers::list().iter().filter_map(|owner| {
        let id = owner.get("id").and_then(Value::as_str)?.to_string();
        let name = owner.get("name").and_then(Value::as_str).unwrap_or(&id);
        Some(json!({ "id": id, "label": name, "custom": true }))
    }));
    Ok(json!({
        "config": settings, "revision": snapshot.revision,
        "builtins": catalog::CLIENTS,
        "providers": providers,
        "headers": ["user-agent", "x-title", "x-client-type", "x-cerebras-3rd-party-integration", catalog::DECLARATION_HEADER],
        "declarationHeader": catalog::DECLARATION_HEADER,
        "maxRules": catalog::MAX_RULES, "maxClients": catalog::MAX_CLIENTS,
        "maxProviderRules": catalog::MAX_PROVIDER_RULES,
    }))
}

fn response(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(error.status_code, error.message),
    }
}

pub async fn get_settings(State(state): State<ServerState>) -> Response { response(view(&state)) }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettings {
    config: ClientSettings,
    revision: String,
}

pub async fn put_settings(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("客户端识别设置不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return response(Err(error)),
    };
    match tokio::task::spawn_blocking(move || {
        let count = payload.config.provider_rules.len();
        client_identification::save(payload.config, &payload.revision)?;
        logging::log("[Config]", &format!("客户端识别设置已保存：{count} 条供应商归属规则，仅对新请求生效"));
        view(&state)
    }).await {
        Ok(result) => response(result),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewSettings {
    config: ClientSettings,
    #[serde(default)]
    rule_id: String,
    #[serde(default)]
    sample: Option<ClientEvidence>,
}

fn preview(state: &ServerState, payload: PreviewSettings) -> Result<Value, GatewayError> {
    catalog::validate(&payload.config).map_err(GatewayError::bad_request)?;
    if !payload.rule_id.is_empty() && !payload.config.rules.iter().any(|rule| rule.id == payload.rule_id) {
        return Err(GatewayError::bad_request("待预览的识别规则不存在"));
    }
    // 先限制扫描窗口，再解码证据；大日志库不因一次预览做全库扫描。
    let db = state.db().ok_or_else(|| GatewayError::new("数据库不可用，无法预览识别规则"))?;
    let records = db.with(|conn| {
        let mut stmt = conn.prepare("SELECT id, ts, client, client_evidence FROM \
            (SELECT row_id, id, ts, client, client_evidence FROM requests ORDER BY row_id DESC LIMIT 500) \
            WHERE client_evidence <> '{}' ORDER BY row_id DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }).ok_or_else(|| GatewayError::new("读取近期请求失败"))?
        .map_err(|error| GatewayError::new(error.to_string()))?;
    let chosen = payload.config.rules.iter().find(|rule| rule.id == payload.rule_id);
    let preview_one = |old: &str, evidence: ClientEvidence| {
        let matches = chosen.is_none_or(|rule| client::rule_matches(rule, &evidence));
        let (value, next) = client::identify(evidence, &payload.config);
        let applied = chosen.is_none_or(|rule| next.rule_id == rule.id);
        (matches, applied, json!({
            "before": old, "client": value, "clientLabel": next.client_name,
            "evidence": next, "ruleMatches": matches, "ruleApplied": applied,
        }))
    };
    let mut samples = Vec::new();
    let mut scanned = 0;
    let mut matched = 0;
    let mut applied = 0;
    for (id, ts, old, text) in records {
        let Some(evidence) = serde_json::from_str::<ClientEvidence>(&text).ok().filter(|item| !item.method.is_empty()) else { continue; };
        scanned += 1;
        let (matches, effective, mut row) = preview_one(&old, evidence.normalized());
        if !matches { continue; }
        matched += 1;
        applied += usize::from(effective);
        if samples.len() < 20 {
            row["id"] = json!(id);
            row["ts"] = json!(ts);
            samples.push(row);
        }
    }
    let sample = payload.sample.map(|evidence| preview_one("", evidence.normalized()).2);
    Ok(json!({ "scanned": scanned, "matched": matched, "applied": applied, "window": 500, "samples": samples, "sample": sample }))
}

pub async fn preview_settings(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<PreviewSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("识别预览内容不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return response(Err(error)),
    };
    match tokio::task::spawn_blocking(move || preview(&state, payload)).await {
        Ok(result) => response(result),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}
