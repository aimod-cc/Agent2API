use std::sync::Arc;

use serde_json::Value;

use crate::server::config::compact_routes;
use crate::server::core::{blocking, compact_routing};
use crate::server::core::key_scope::KeyScope;
use crate::server::errors::GatewayError;
use crate::server::request_stats::{RequestSource, RequestStats};
use crate::server::ServerState;

pub(super) async fn resolve_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
) -> Result<String, GatewayError> {
    resolve_client_model(state, payload, scope, None).await
}

pub(super) async fn resolve_client_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
    compact_client: Option<&'static str>,
) -> Result<String, GatewayError> {
    let state = state.clone();
    let scope = scope.cloned();
    let mut body = std::mem::take(payload);
    let (body, result) = blocking::run(move || {
        let result = resolve_chat_model(&state, &mut body, scope.as_ref(), compact_client);
        (body, result)
    }).await?;
    *payload = body;
    result
}

fn resolve_chat_model(
    state: &ServerState,
    body: &mut Value,
    scope: Option<&KeyScope>,
    compact_client: Option<&str>,
) -> Result<String, GatewayError> {
    // 先校验来源模型，不能用目标的权限替来源放行；同时清除下游伪造的内部标记。
    let source = super::pipeline::resolve_model(state, body, scope)?;
    let Some(client) = compact_client else { return Ok(source); };
    let snapshot = compact_routes::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    if !settings.enabled { return Ok(source); }
    let Some(rule) = settings.rules.iter().find(|rule| rule.enabled && rule.client == client
        && compact_routes::model_key(&rule.source) == compact_routes::model_key(&source)) else { return Ok(source); };

    // Key 的提供商权限原本在转发层检查；来源被换掉以后仍须通过这道门，不能只验证 B。
    validate_compact_access(state, &source, scope)?;
    validate_compact_access(state, &rule.target, scope)?;
    // 用同一套模型目录及 Key 权限检查目标；不递归调用本入口，A→B 不会接着套 B→C。
    let mut target_body = serde_json::json!({ "model": rule.target });
    let target = super::pipeline::resolve_model(state, &mut target_body, scope).map_err(|mut error| {
        error.message = format!("压缩路由 {} → {} 无法执行：{}；未回退原模型", source, rule.target, error.message);
        error
    })?;
    if let Some(object) = body.as_object_mut() {
        object.insert("model".into(), target.clone().into());
        object.remove("_wb_route_group_id");
        object.remove("_wb_route_continue");
        if let Some(group) = target_body.get("_wb_route_group_id") {
            object.insert("_wb_route_group_id".into(), group.clone());
        }
    }
    compact_routing::parameters::clear_source_reasoning(body);
    compact_routing::mark_routed(body, &source, &target, client);
    crate::server::logging::log("[Compact]", &format!("ZCode 上下文压缩：{source} → {target}"));
    Ok(target)
}

fn validate_compact_access(state: &ServerState, model: &str, scope: Option<&KeyScope>) -> Result<(), GatewayError> {
    let models = crate::server::core::providers::catalog::models_response(state.store(), scope);
    if models.get("data").and_then(Value::as_array).is_some_and(|items| items.iter().any(|item|
        item.get("id").and_then(Value::as_str).is_some_and(|id| compact_routes::model_key(id) == compact_routes::model_key(model))
            && item.get("kind").and_then(Value::as_str) != Some("image"))) {
        return Ok(());
    }
    Err(GatewayError::with_status(404, format!("压缩路由中的模型「{model}」当前不可用，或不在这把网关 Key 允许的模型与提供商范围内；未回退原模型"))
        .with_code("model_not_found"))
}

pub(super) async fn record_started(
    stats: Arc<RequestStats>,
    id: &str,
    ts: i64,
    model: &str,
    client_model: &str,
    reasoning: &str,
    source: &RequestSource,
    is_test: bool,
) {
    // 进行中行写入与调用方创建断线守卫之间不能留下可取消的等待窗口。
    blocking::in_place(|| {
        stats.record_started(id, ts, model, client_model, reasoning, source, is_test);
    });
}
