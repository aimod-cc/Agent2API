use std::sync::Arc;

use serde_json::Value;

use crate::server::config::{compact_routes, title_routes};
use crate::server::core::{blocking, compact_routing, title_routing};
use crate::server::core::key_scope::KeyScope;
use crate::server::errors::GatewayError;
use crate::server::request_stats::{RequestSource, RequestStats};
use crate::server::ServerState;

/// 一次请求可能命中的特殊路由（压缩 / 标题）。两者都在模型解析这一步改写
/// 模型，但判据互不重叠，且**一条请求最多命中一层**（见下面 `resolve_chat_model`）。
#[derive(Clone, Copy, Default)]
pub(super) struct SpecialRoute {
    pub compact: Option<&'static str>,
    pub title: Option<&'static str>,
}

impl SpecialRoute {
    pub(super) fn none() -> Self { Self::default() }
    pub(super) fn compact(client: Option<&'static str>) -> Self { Self { compact: client, title: None } }
    pub(super) fn title(client: Option<&'static str>) -> Self { Self { compact: None, title: client } }
    fn any(self) -> bool { self.compact.is_some() || self.title.is_some() }
}

pub(super) async fn resolve_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
) -> Result<String, GatewayError> {
    resolve_client_model(state, payload, scope, SpecialRoute::none()).await
}

pub(super) async fn resolve_client_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
    special: SpecialRoute,
) -> Result<String, GatewayError> {
    let state = state.clone();
    let scope = scope.cloned();
    let mut body = std::mem::take(payload);
    let (body, result) = blocking::run(move || {
        let result = resolve_chat_model(&state, &mut body, scope.as_ref(), special);
        (body, result)
    }).await?;
    *payload = body;
    result
}

fn resolve_chat_model(
    state: &ServerState,
    body: &mut Value,
    scope: Option<&KeyScope>,
    special: SpecialRoute,
) -> Result<String, GatewayError> {
    // 先校验来源模型，不能用目标的权限替来源放行；同时清除下游伪造的内部标记。
    let source = super::pipeline::resolve_model(state, body, scope)?;
    if !special.any() { return Ok(source); }
    // 压缩优先：它的模板里也含 "summary of the conversation" 一类措辞，
    // 先判压缩可以保证一条压缩请求不会被标题路由抢走。
    if let Some(client) = special.compact {
        return rewrite_compact(state, body, scope, client, &source);
    }
    let client = special.title.expect("special.any() 已保证至少一层");
    rewrite_title(state, body, scope, client, &source)
}

fn rewrite_compact(
    state: &ServerState,
    body: &mut Value,
    scope: Option<&KeyScope>,
    client: &str,
    source: &str,
) -> Result<String, GatewayError> {
    let snapshot = compact_routes::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    if !settings.enabled { return Ok(source.to_string()); }
    let Some(rule) = settings.rules.iter().find(|rule| rule.enabled && rule.client == client
        && compact_routes::model_key(&rule.source) == compact_routes::model_key(source)) else { return Ok(source.to_string()); };

    // Key 的提供商权限原本在转发层检查；来源被换掉以后仍须通过这道门，不能只验证 B。
    validate_compact_access(state, source, scope)?;
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
    compact_routing::mark_routed(body, source, &target, client);
    crate::server::logging::log("[Compact]", &format!("ZCode 上下文压缩：{source} → {target}"));
    Ok(target)
}

/// 标题生成改写。
///
/// 与压缩路由的两处不同：
///   · 失败时按配置决定是否退回原模型 —— 标题请求失败会让客户端的会话列表
///     显示不出名字，代价比多花一次调用大；
///   · 退回原模型时必须把已改写的 body 还原，否则客户端会拿到一个它没要的模型。
fn rewrite_title(
    state: &ServerState,
    body: &mut Value,
    scope: Option<&KeyScope>,
    client: &str,
    source: &str,
) -> Result<String, GatewayError> {
    let Some(settings) = title_routes::active() else { return Ok(source.to_string()); };
    // 按列表顺序取第一条认这个客户端的规则 —— 顺序即优先级；一条都没命中就原样转发。
    let Some(rule) = title_routes::match_rule(&settings, client) else { return Ok(source.to_string()); };
    let target_model = rule.model.trim().to_string();
    if target_model.is_empty() { return Ok(source.to_string()); }

    // 目标模型同样要过 Key 权限与目录校验。
    let mut target_body = serde_json::json!({ "model": target_model });
    let target = match super::pipeline::resolve_model(state, &mut target_body, scope) {
        Ok(target) => target,
        Err(mut error) => {
            if rule.fallback == title_routes::Fallback::Error {
                error.message = format!("标题生成路由 {} → {} 无法执行：{}", source, target_model, error.message);
                return Err(error);
            }
            crate::server::logging::log("[Title]", &format!(
                "标题模型 {target_model} 不可用（{}），本次退回原模型 {source}", error.message));
            return Ok(source.to_string());
        }
    };
    if title_routes::model_key(&target) == title_routes::model_key(source) { return Ok(source.to_string()); }

    if let Some(object) = body.as_object_mut() {
        object.insert("model".into(), target.clone().into());
        // 标题请求不需要原模型的路由组与思考等级：它们描述的是原模型的语义。
        object.remove("_wb_route_group_id");
        object.remove("_wb_route_continue");
        if let Some(group) = target_body.get("_wb_route_group_id") {
            object.insert("_wb_route_group_id".into(), group.clone());
        }
    }
    compact_routing::parameters::clear_source_reasoning(body);
    title_routing::mark_routed(body, source, &target, client);
    crate::server::logging::log("[Title]", &format!(
        "{} 标题生成：{source} → {target}", title_routes::client_label(client).unwrap_or(client)));
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
