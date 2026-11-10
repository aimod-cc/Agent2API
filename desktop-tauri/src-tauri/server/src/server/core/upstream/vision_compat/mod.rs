mod assist;
mod continuation;
mod cache;
mod content;
mod stream;
mod targets;
pub(super) use continuation::resume;
pub(super) use stream::rewrite;
pub(crate) use continuation::VisionContinuation;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::server::config::vision_compat::{self as config, Mode, VisionRule, VisionSettings, VisionTarget};
use crate::server::errors::GatewayError;
use super::{connections::ConnectionGuard, payload::ProviderContext, usage::TelemetrySnapshot, ForwardOutcome, InFlightGuard, RouteTarget, UpstreamService};

#[derive(Clone, Debug)]
pub(crate) struct VisionCall {
    pub snapshot: Box<TelemetrySnapshot>,
    pub started_at: i64,
    pub finished_at: i64,
    pub status: i64,
    pub error: Option<String>,
}

pub(super) struct RequestVision {
    pub settings: VisionSettings,
    pub revision: String,
    shared: Option<VisionTarget>,
    original_messages: usize,
    started: Instant,
    triggered: AtomicBool,
}

impl RequestVision {
    pub(super) fn capture(body: &Value) -> Result<Option<Arc<Self>>, GatewayError> {
        if !content::has_images(body) { return Ok(None); }
        let snapshot = config::current();
        let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
        if !settings.enabled || !(settings.uses(Mode::Assist) || settings.uses(Mode::Reroute)) { return Ok(None); }
        Ok(Some(Arc::new(Self { settings: settings.clone(), revision: snapshot.revision.clone(),
            shared: config::shared_target(), original_messages: body.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
            started: Instant::now(), triggered: AtomicBool::new(false) })))
    }

    fn remaining(&self, stream: bool) -> Duration {
        let timeouts = crate::server::config::timeout_settings();
        let request_ms = if stream { timeouts.headers_ms() } else { timeouts.body_ms() };
        Duration::from_secs(self.settings.timeout_seconds).min(
            Duration::from_millis(request_ms).saturating_sub(self.started.elapsed()))
    }
}

pub(super) fn handles_error(ctx: &ProviderContext<'_>, error: &GatewayError) -> bool {
    (ctx.routing.strict_vision || ctx.vision.as_ref().is_some_and(|vision|
        !vision.triggered.load(Ordering::Acquire) && ctx.telemetry.vision_enabled()))
        && stream::explicit_rejection(error)
}

pub(super) fn rejection_message(text: &str) -> bool { stream::rejection_text(text) }

pub(super) async fn before_target(service: &UpstreamService, ctx: &ProviderContext<'_>, target: &RouteTarget,
    slot: &mut Option<InFlightGuard>, connections: &mut ConnectionGuard) -> Result<Option<ForwardOutcome>, GatewayError> {
    let Some(vision) = &ctx.vision else { return Ok(None); };
    if ctx.native.is_some_and(|native| native.protocol == crate::server::core::protocol::native::WireProtocol::Anthropic || native.requires_native())
        && super::native_payload::eligible(ctx, &target.provider)?
    { return Ok(None); }
    if vision.triggered.load(Ordering::Acquire) { return Ok(None); }
    let model = targets::actual_model(ctx, target);
    // 按模型查出命中的那条规则：后面读到的视觉模型、备选、复用开关全都取这条规则的，
    // 而不是两块设置的 —— 同一种方式可以有多条规则，各自的目标可以不同。
    let rule = vision.settings.rule_for(&target.provider, &model);
    ctx.telemetry.set_vision_enabled(rule.is_some());
    let Some(rule) = rule else { return Ok(None); };
    match targets::supports_images(&target.provider, &model) {
        Some(true) => {
            ctx.telemetry.note_vision(json!({
                "mode": rule.mode.as_str(), "status": "skipped", "trigger": "capability",
                "sourceProvider": target.provider, "sourceModel": model,
                "targetProvider": "", "targetModel": "", "durationMs": 0,
                "imageCount": 0, "cachedImages": 0, "promptTokens": 0,
                "completionTokens": 0, "totalTokens": 0, "calls": [],
                "reason": "model-declares-image-support",
            }));
            Ok(None)
        }
        None if rule.unknown_capability.effective(vision.settings.unknown_capability)
            == config::UnknownCapability::TryOriginal => {
            ctx.telemetry.note_vision(json!({
                "mode": rule.mode.as_str(), "status": "skipped", "trigger": "capability",
                "sourceProvider": target.provider, "sourceModel": model,
                "targetProvider": "", "targetModel": "", "durationMs": 0,
                "imageCount": 0, "cachedImages": 0, "promptTokens": 0,
                "completionTokens": 0, "totalTokens": 0, "calls": [],
                "reason": "image-capability-unknown",
            }));
            Ok(None)
        }
        _ => recover(service, ctx, target, &rule, "capability", slot, connections).await.map(Some),
    }
}

pub(super) async fn forward(service: &UpstreamService, ctx: ProviderContext<'_>, slot: &mut Option<InFlightGuard>,
    connections: &mut ConnectionGuard) -> Result<ForwardOutcome, GatewayError> {
    if ctx.vision.is_none() {
        return super::provider_loop::forward_with_providers(service, ctx, slot, connections).await;
    }
    ctx.telemetry.set_vision_enabled(true);
    let mut provisional_slot = None;
    let result = super::provider_loop::forward_with_providers(service, ctx.clone(), &mut provisional_slot, connections).await;
    let result = match result {
        Ok(outcome) if ctx.telemetry.vision_enabled() && ctx.vision.as_ref().is_some_and(|vision| !vision.triggered.load(Ordering::Acquire))
            && !matches!(&outcome, ForwardOutcome::Native(response)
                if response.protocol == crate::server::core::protocol::native::WireProtocol::Anthropic)
            && !ctx.native.is_some_and(|native| native.requires_native()) => {
            stream::hold(outcome, ctx.telemetry).await
        }
        other => other,
    };
    let error = match result {
        Ok(outcome) => return Ok(super::provider_loop::attach_slot(outcome, slot, connections)),
        Err(error) => error,
    };
    if !handles_error(&ctx, &error) || ctx.telemetry.is_cancelled()
        || ctx.native.is_some_and(|native| native.requires_native()
            || native.protocol == crate::server::core::protocol::native::WireProtocol::Anthropic && error.native_response.is_some())
    { return Err(error); }
    let Some(target) = targets::from_snapshot(&ctx) else { return Err(error); };
    let model = targets::actual_model(&ctx, &target);
    let Some(rule) = ctx.vision.as_ref().and_then(|vision| vision.settings.rule_for(&target.provider, &model)) else { return Err(error); };
    ctx.telemetry.finish_last_attempt(Some(i64::from(error.status_code)), Some(&error.message));
    ctx.telemetry.prepare_vision_retry();
    let outcome = recover(service, &ctx, &target, &rule, "rejection", slot, connections).await?;
    Ok(super::provider_loop::attach_slot(outcome, slot, connections))
}

enum Prepared {
    Assist { body: Value, destination: targets::Destination },
    Rerouted(ForwardOutcome),
}

async fn recover(service: &UpstreamService, ctx: &ProviderContext<'_>, original: &RouteTarget, rule: &VisionRule,
    trigger: &str, slot: &mut Option<InFlightGuard>, connections: &mut ConnectionGuard) -> Result<ForwardOutcome, GatewayError> {
    let vision = ctx.vision.as_ref().ok_or_else(|| GatewayError::new("图片兼容上下文不存在"))?;
    if vision.triggered.swap(true, Ordering::AcqRel) {
        return Err(GatewayError::bad_request("本次图片兼容已经执行，未递归调用或改用其他处理方式").with_code("vision_recursion_blocked"));
    }
    let started = Instant::now();
    connections.rebind(None);
    let mut detail = json!({ "mode": rule.mode.as_str(), "trigger": trigger, "status": "processing",
        "sourceProvider": original.provider, "sourceModel": targets::actual_model(ctx, original),
        "targetProvider": "", "targetModel": "", "durationMs": 0, "cachedImages": 0, "imageCount": 0,
        "promptTokens": 0, "completionTokens": 0, "totalTokens": 0, "calls": [] });
    ctx.telemetry.note_vision(detail.clone());
    let budget = vision.remaining(ctx.stream);
    let result = {
        let work = async {
            match rule.mode {
                Mode::Assist => assist::run(service, ctx, original, vision, rule, &mut detail).await
                    .map(|(body, destination)| Prepared::Assist { body, destination }),
                Mode::Reroute => reroute(service, ctx, original, vision, rule, &mut detail, slot, connections).await.map(Prepared::Rerouted),
            }
        };
        if budget.is_zero() {
            drop(work);
            Err(timeout_error())
        } else if let Some(token) = ctx.telemetry.cancel_token() {
            tokio::select! {
                biased;
                _ = token.cancelled() => Err(super::cancellation::cancelled_error()),
                result = tokio::time::timeout(budget, work) => result.unwrap_or_else(|_| Err(timeout_error())),
            }
        } else { tokio::time::timeout(budget, work).await.unwrap_or_else(|_| Err(timeout_error())) }
    };
    if let Some(recorded) = ctx.telemetry.route_decision().and_then(|route| route.get("visionCompatibility").cloned()) {
        for key in ["calls", "promptTokens", "completionTokens", "totalTokens"] {
            if let Some(value) = recorded.get(key) { detail[key] = value.clone(); }
        }
    }
    detail["durationMs"] = json!(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
    detail["status"] = json!(if result.is_ok() { "completed" } else { "failed" });
    if let Err(error) = &result {
        detail["error"] = error.message.chars().take(500).collect::<String>().into();
        if error.code.as_deref() == Some("vision_compat_timeout") {
            ctx.telemetry.note_error(&error.message);
            if let Some(cancel) = ctx.telemetry.cancel_token() { cancel.cancel(); }
        }
    }
    ctx.telemetry.note_vision(detail);
    let outcome = match result? {
        Prepared::Assist { body, destination } => {
            continuation::remember(ctx, &body, &destination);
            ctx.telemetry.prepare_vision_retry();
            dispatch(service, ctx, &destination, &body, ctx.telemetry, ctx.stream,
                ctx.preserve_system_prompt, slot, connections).await?
        }
        Prepared::Rerouted(outcome) if !ctx.stream => match outcome {
            ForwardOutcome::Stream { stream, .. } => {
                let completion = super::aggregate::aggregate_frame_stream(Box::pin(stream), ctx.telemetry.clone(), Default::default()).await?;
                ForwardOutcome::Completion { body: completion.body }
            }
            other => other,
        },
        Prepared::Rerouted(outcome) => outcome,
    };
    Ok(stream::rewrite(outcome, ctx.body.get("model").and_then(Value::as_str).unwrap_or(""), service, ctx.telemetry.clone()))
}

fn timeout_error() -> GatewayError {
    GatewayError::with_status(504, "图片兼容处理超时，已停止；未删除图片或切换其他处理方式").with_code("vision_compat_timeout")
}

async fn dispatch(service: &UpstreamService, ctx: &ProviderContext<'_>, destination: &targets::Destination,
    body: &Value, telemetry: &Arc<super::usage::RequestTelemetry>, stream: bool, preserve_prompt: bool,
    slot: &mut Option<InFlightGuard>, connections: &mut ConnectionGuard) -> Result<ForwardOutcome, GatewayError> {
    let mut next = ctx.clone();
    next.body = body;
    next.native = None;
    next.routing = destination.routing.clone();
    next.vision = None;
    next.direct_provider = Some(destination.target.provider.clone());
    next.direct_model = Some(destination.target.model.clone());
    next.compact_routed = if preserve_prompt { false } else { ctx.compact_routed };
    next.title_routed = if preserve_prompt { false } else { ctx.title_routed };
    next.telemetry = telemetry;
    next.stream = stream;
    next.preserve_system_prompt = preserve_prompt;
    next.ignore_model_gate = false;
    next.pinned_account = None;
    telemetry.set_vision_enabled(true);
    let mut provisional_slot = None;
    let outcome = Box::pin(super::provider_loop::forward_with_providers(service, next, &mut provisional_slot, connections)).await?;
    let outcome = stream::hold(outcome, telemetry).await?;
    Ok(super::provider_loop::attach_slot(outcome, slot, connections))
}

async fn reroute(service: &UpstreamService, ctx: &ProviderContext<'_>, original: &RouteTarget, _vision: &RequestVision,
    rule: &VisionRule, detail: &mut Value, slot: &mut Option<InFlightGuard>, connections: &mut ConnectionGuard) -> Result<ForwardOutcome, GatewayError> {
    let (destinations, errors) = targets::reroute_targets(service, ctx, rule, original);
    detail["imageCount"] = content::collect(ctx.body)?.len().into();
    if destinations.is_empty() {
        return Err(GatewayError::with_status(503, format!("当前路由及指定目标中没有符合权限和路由配置的可用视觉模型；未改用辅助识图{}",
            errors.first().map(|error| format!("：{error}")).unwrap_or_default())).with_code("vision_target_unavailable"));
    }
    let mut last_error = None;
    for destination in destinations {
        if ctx.telemetry.is_cancelled() { return Err(super::cancellation::cancelled_error()); }
        detail["targetProvider"] = destination.target.provider.clone().into();
        detail["targetModel"] = destination.target.model.clone().into();
        ctx.telemetry.note_vision(detail.clone());
        let mut body = ctx.body.clone();
        body["model"] = destination.request_model.clone().into();
        if !destination.target.reasoning.is_empty() && !crate::server::core::model_rules::reasoning_is_off(&destination.target.reasoning) {
            body["reasoning_effort"] = destination.target.reasoning.clone().into();
        }
        match dispatch(service, ctx, &destination, &body, ctx.telemetry, true, ctx.preserve_system_prompt, slot, connections).await {
            Ok(outcome) => {
                continuation::remember(ctx, &body, &destination);
                return Ok(outcome);
            }
            Err(error) => {
                ctx.telemetry.finish_last_attempt(Some(i64::from(error.status_code)), Some(&error.message));
                ctx.telemetry.prepare_vision_retry();
                last_error = Some(error);
            }
        }
    }
    let error = last_error.unwrap_or_else(|| GatewayError::new("视觉模型不可用"));
    Err(GatewayError::with_status(error.status_code, format!("视觉模型处理失败：{}；未改用辅助识图或删除图片", error.message))
        .with_code("vision_reroute_failed"))
}
