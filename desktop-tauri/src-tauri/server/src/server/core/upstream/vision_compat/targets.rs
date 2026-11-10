use serde_json::Value;

use crate::server::config::vision_compat::{VisionRule, VisionTarget};
use crate::server::core::{capability, key_scope, providers::catalog, routing, routing_groups, routing_groups::RequestNeeds};
use crate::server::errors::GatewayError;
use super::super::{payload::ProviderContext, route_context::RequestRouting, RouteTarget, UpstreamService};

#[derive(Clone)]
pub(super) struct Destination {
    pub target: VisionTarget,
    pub request_model: String,
    pub routing: RequestRouting,
}

pub(super) fn actual_model(ctx: &ProviderContext<'_>, target: &RouteTarget) -> String {
    ctx.direct_model.clone().or_else(|| target.account.as_ref().and_then(routing_groups::candidate_model).map(str::to_string))
        .unwrap_or_else(|| mapped_model(ctx, target))
}

fn mapped_model(ctx: &ProviderContext<'_>, target: &RouteTarget) -> String {
    let requested = ctx.body.get("model").and_then(Value::as_str).unwrap_or("");
    if crate::server::core::custom_providers::is_custom_provider_id(&target.provider) {
        crate::server::core::providers::custom::forward::cooldown_model(&target.provider, requested)
    } else { catalog::wire_target_for_provider(requested, &target.provider, target.account.as_ref()).model }
}

pub(super) fn supports_images(provider: &str, model: &str) -> Option<bool> {
    routing_groups::catalog_model(provider, model).and_then(|item| capability::images_for_channel(provider, &item))
}

fn compatible(target: &VisionTarget, needs: &RequestNeeds) -> bool {
    let Some(item) = routing_groups::catalog_model(&target.provider, &target.model) else { return false; };
    capability::images_for_channel(&target.provider, &item) == Some(true)
        && (!needs.tools || item.get("supportsToolCall").and_then(Value::as_bool) == Some(true))
        && (!needs.video || item.get("supportsVideo").and_then(Value::as_bool) == Some(true))
}

pub(super) fn resolve(service: &UpstreamService, ctx: &ProviderContext<'_>, target: VisionTarget,
    request_model: &str, needs: &RequestNeeds) -> Result<Destination, GatewayError> {
    // 指定模型是全局配置；只按目标自己的路由选账号，不继承来源路由的参与名单。
    let routing = RequestRouting::capture_request(request_model, &service.store, &serde_json::json!({}), ctx.key_scope, false)?;
    resolve_in_route(service, ctx, target, request_model, needs, &routing)
}

fn resolve_in_route(service: &UpstreamService, ctx: &ProviderContext<'_>, mut target: VisionTarget,
    request_model: &str, needs: &RequestNeeds, routing: &RequestRouting) -> Result<Destination, GatewayError> {
    if !key_scope::allows_provider(ctx.key_scope, &target.provider) {
        return Err(GatewayError::with_status(403, "当前网关 Key 不允许所选视觉提供商").with_code("vision_target_forbidden"));
    }
    let permission_model = routing.group.as_ref().map(|group| group.model.as_str()).unwrap_or(request_model);
    if !key_scope::allows_model(ctx.key_scope, permission_model)
        || !key_scope::allows_model(ctx.key_scope, &target.model) {
        return Err(GatewayError::with_status(403, "当前网关 Key 不允许所选视觉模型").with_code("vision_target_forbidden"));
    }
    if !compatible(&target, needs) {
        return Err(GatewayError::bad_request("所选视觉模型未确认支持本次请求需要的图片、工具或视频能力，请检查模型管理")
            .with_code("vision_target_incompatible"));
    }
    if routing.group.is_none() {
        // 与保存校验同一判据：目标按上游真实 id 存储，别名启用即可达
        if !catalog::provider_serves_model(&target.provider, &target.model) {
            return Err(GatewayError::with_status(404, "所选视觉模型已停用或不在当前提供商目录中").with_code("vision_target_unavailable"));
        }
        let actual = if crate::server::core::custom_providers::is_custom_provider_id(&target.provider) {
            crate::server::core::providers::custom::forward::cooldown_model(&target.provider, request_model)
        } else { catalog::wire_target_for_provider(request_model, &target.provider, None).model };
        if target.reasoning.is_empty() {
            target.reasoning = if crate::server::core::custom_providers::is_custom_provider_id(&target.provider) {
                crate::server::core::custom_providers::wire_model_for(&target.provider, request_model).1
            } else { catalog::wire_target_for_provider(request_model, &target.provider, None).reasoning }.unwrap_or_default();
        }
        if !actual.eq_ignore_ascii_case(&target.model) {
            return Err(GatewayError::with_status(409, "视觉模型映射已经改变，请重新保存图片兼容配置").with_code("vision_target_changed"));
        }
    }
    let routing = routing.for_vision_target(&service.store, &target.provider, &target.model,
        ctx.key_scope, ctx.pinned_account, None, Some(needs))?;
    Ok(Destination { target, request_model: request_model.to_string(), routing })
}

pub(super) fn original(service: &UpstreamService, ctx: &ProviderContext<'_>, original: &RouteTarget) -> Result<Destination, GatewayError> {
    let request_model = ctx.body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    let mut target = VisionTarget { provider: original.provider.clone(), model: actual_model(ctx, original), reasoning: String::new() };
    if ctx.routing.group.is_none() {
        target.reasoning = if crate::server::core::custom_providers::is_custom_provider_id(&target.provider) {
            crate::server::core::custom_providers::wire_model_for(&target.provider, &request_model).1
        } else { catalog::wire_target_for_provider(&request_model, &target.provider, original.account.as_ref()).reasoning }.unwrap_or_default();
    }
    let routing = ctx.routing.for_vision_target(&service.store, &target.provider, &target.model,
        ctx.key_scope, ctx.pinned_account, original.account.as_ref().map(routing_groups::attempt_key).as_deref(), None)?;
    Ok(Destination { target, request_model, routing })
}

pub(super) fn reroute_targets(service: &UpstreamService, ctx: &ProviderContext<'_>, rule: &VisionRule,
    failed: &RouteTarget) -> (Vec<Destination>, Vec<String>) {
    let mut result = Vec::new();
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let failed_model = actual_model(ctx, failed);
    let requested = ctx.body.get("model").and_then(Value::as_str).unwrap_or("");
    let needs = RequestNeeds::from_body(ctx.body);
    let mut add = |target: VisionTarget, request_model: &str, current_route: bool| {
        if target.provider == failed.provider && target.model.eq_ignore_ascii_case(&failed_model) { return; }
        let destination = if current_route {
            resolve_in_route(service, ctx, target, request_model, &needs, &ctx.routing)
        } else {
            resolve(service, ctx, target, request_model, &needs)
        };
        match destination {
            Ok(destination) => {
                if seen.insert((destination.target.provider.clone(), destination.target.model.to_lowercase(), destination.routing.cache_identity())) {
                    result.push(destination);
                }
            }
            Err(error) => errors.push(error.message),
        }
    };
    if rule.prefer_current_route {
        if ctx.routing.group.is_some() {
            for account in &ctx.routing.group_projection.accounts {
                let provider = routing::provider_of(account);
                let Some(model) = routing_groups::candidate_model(account) else { continue; };
                if supports_images(provider, model) != Some(true) { continue; }
                add(VisionTarget { provider: provider.to_string(), model: model.to_string(), reasoning: String::new() }, requested, true);
            }
        } else {
            for provider in ctx.routing.providers(catalog::forwarding_providers(requested)) {
                let model = if crate::server::core::custom_providers::is_custom_provider_id(&provider) {
                    crate::server::core::providers::custom::forward::cooldown_model(&provider, requested)
                } else { catalog::wire_target_for_provider(requested, &provider, None).model };
                if supports_images(&provider, &model) != Some(true) { continue; }
                add(VisionTarget { provider, model, reasoning: String::new() }, requested, true);
            }
        }
    }
    if rule.fallback == crate::server::config::vision_compat::Fallback::Designated {
        for target in &rule.targets { add(target.clone(), &target.model, false); }
    }
    (result, errors)
}

pub(super) fn from_snapshot(ctx: &ProviderContext<'_>) -> Option<RouteTarget> {
    let snapshot = ctx.telemetry.snapshot();
    let provider = snapshot.provider?;
    let mut account = serde_json::json!({ "id": snapshot.account_id, "provider": provider, "routingModel": snapshot.upstream_model });
    if let Some(candidate) = ctx.routing.group_projection.accounts.iter().find(|candidate|
        routing::account_id(candidate) == Some(snapshot.account_id.as_str()) && routing::provider_of(candidate) == provider
            && routing_groups::candidate_model(candidate) == Some(snapshot.upstream_model.as_str())) {
        account = candidate.clone();
    }
    Some(RouteTarget { provider, account_id: (!snapshot.account_id.is_empty()).then_some(snapshot.account_id),
        account: Some(account), proxy: None, priority: None, proxy_notice: None, decision: snapshot.route })
}
