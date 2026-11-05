use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::client_identification;
use crate::server::core::client::{self, catalog, ClientEvidence, ClientSettings};
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::{logging, ServerState};

fn view() -> Result<Value, GatewayError> {
    let snapshot = client_identification::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    Ok(json!({
        "config": settings, "revision": snapshot.revision,
        "builtins": catalog::CLIENTS,
        "headers": ["user-agent", "x-title", "x-client-type", "x-cerebras-3rd-party-integration", catalog::DECLARATION_HEADER],
        "declarationHeader": catalog::DECLARATION_HEADER,
        "maxRules": catalog::MAX_RULES, "maxClients": catalog::MAX_CLIENTS,
    }))
}

fn response(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(error.status_code, error.message),
    }
}

pub async fn get_settings() -> Response { response(view()) }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettings {
    config: ClientSettings,
    revision: String,
}

pub async fn put_settings(body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("客户端识别设置不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return response(Err(error)),
    };
    match tokio::task::spawn_blocking(move || {
        let count = payload.config.rules.len();
        client_identification::save(payload.config, &payload.revision)?;
        logging::log("[Config]", &format!("客户端识别设置已保存：{count} 条规则，仅对新请求生效"));
        view()
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
