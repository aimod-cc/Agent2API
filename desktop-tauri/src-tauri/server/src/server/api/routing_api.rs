//! GET/PUT /api/routing + GET /api/routing/queue —— 账号路由的设置与**队列可见化**。
//!
//! ── /api/routing（与 /api/timeouts 同一模式）─────────────────
//! 独立端点、允许部分字段、返回生效后的全量值、无副作用（不重启进程）。
//! 保存后对下一个请求立即生效（转发层逐请求读 `config::routing_settings()`
//! 内存快照，不读盘）。
//!
//! ── /api/routing/queue（队列可见化，账号页面板的数据源）────────
//! 回答「按当前策略，这个模型的候选账号会怎么排、谁被跳过、为什么」。
//! **排序实现与转发选路是同一个函数**（`routing::plan_route`）—— 转发那次
//! 怎么排，这里就怎么画；两处若各写一份，界面标的顺序迟早和实际转发分叉
//! （这正是「候选队列可见化」要消灭的那类问题）。
//!
//! 与 `/api/session` 的 `routedAccountId` 的关系：那个字段只回答「队首是谁」
//! （★ 的数据源，保持不变），本端点给的是**整张清单 + 原因句** —— 面板要
//! 展示每一位，顺带把被跳过者也摆出来（禁用 / 冷却 / 并发满 / 限制器 /
//! 无凭证各有各的原因，用户不用再猜）。
//!
//! `model` 查询参数可省：省略（或空）时按「不限模型」的全局队列展开
//! （不收窄到「提供该模型的家」）—— 面板在没有模型上下文时也有得画。

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{self, RoutingPatch, RoutingSettings};
use crate::server::core::providers::router;
use crate::server::core::routing;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

/// GET /api/routing
pub async fn get_routing(State(_state): State<ServerState>) -> Response {
    ok_json(routing_json(config::routing_settings()))
}

/// PUT /api/routing —— body 各键全部可选（不给 = 不动）
pub async fn put_routing(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let mut patch = RoutingPatch::default();
    if let Some(value) = object.get(config::KEY_ROUTING_STRATEGY) {
        let Some(text) = value.as_str() else {
            return errors::management_error(400, "strategy 必须是字符串");
        };
        let known = [
            config::ROUTING_STRATEGY_ORDER,
            config::ROUTING_STRATEGY_ROTATE,
        ];
        if !known.contains(&text) {
            return errors::management_error(
                400,
                format!("未知的路由策略「{text}」（只支持 {}）", known.join(" / ")),
            );
        }
        patch.strategy = Some(config::RouteStrategy::from_id(text));
    }
    if let Some(value) = object.get(config::KEY_ROUTING_STICKY) {
        let Some(text) = value.as_str() else {
            return errors::management_error(400, "sticky 必须是字符串");
        };
        let known = [
            config::STICKY_AUTO,
            config::STICKY_SESSION,
            config::STICKY_TURN,
            config::STICKY_OFF,
        ];
        if !known.contains(&text) {
            return errors::management_error(
                400,
                format!("未知的会话粘性档位「{text}」（只支持 {}）", known.join(" / ")),
            );
        }
        patch.sticky = Some(config::StickyMode::from_id(text));
    }
    let bool_field = |key: &str| -> Result<Option<bool>, Response> {
        match object.get(key) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(errors::management_error(400, format!("{key} 必须是布尔值"))),
        }
    };
    let failover_backoff = match bool_field(config::KEY_ROUTING_FAILOVER_BACKOFF) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let sink = match bool_field(config::KEY_ROUTING_SINK) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let breaker_enabled = match bool_field(config::KEY_ROUTING_BREAKER_ENABLED) {
        Ok(value) => value,
        Err(response) => return response,
    };
    patch.failover_backoff = failover_backoff;
    patch.sink = sink;
    patch.breaker_enabled = breaker_enabled;
    let int_field = |key: &str, min: i64, max: i64| -> Result<Option<i64>, Response> {
        match object.get(key) {
            None => Ok(None),
            Some(value) => match value.as_i64() {
                Some(count) if (min..=max).contains(&count) => Ok(Some(count)),
                _ => Err(errors::management_error(
                    400,
                    format!("{key} 必须是 {min}~{max} 的整数"),
                )),
            },
        }
    };
    patch.breaker_threshold = match int_field(
        config::KEY_ROUTING_BREAKER_THRESHOLD,
        config::ROUTING_BREAKER_MIN_THRESHOLD,
        config::ROUTING_BREAKER_MAX_THRESHOLD,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    patch.breaker_timeout_seconds = match int_field(
        config::KEY_ROUTING_BREAKER_TIMEOUT,
        config::ROUTING_BREAKER_MIN_TIMEOUT,
        config::ROUTING_BREAKER_MAX_TIMEOUT,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    patch.breaker_success_threshold = match int_field(
        config::KEY_ROUTING_BREAKER_SUCCESS,
        config::ROUTING_BREAKER_MIN_SUCCESS,
        config::ROUTING_BREAKER_MAX_SUCCESS,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !config::set_routing(patch) {
        return errors::management_error(500, "路由设置落盘失败");
    }
    ok_json(routing_json(config::routing_settings()))
}

fn routing_json(settings: RoutingSettings) -> Value {
    json!({
        config::KEY_ROUTING_STRATEGY: settings.strategy.as_str(),
        config::KEY_ROUTING_STICKY: settings.sticky.as_str(),
        config::KEY_ROUTING_FAILOVER_BACKOFF: settings.failover_backoff,
        config::KEY_ROUTING_SINK: settings.sink,
        config::KEY_ROUTING_BREAKER_ENABLED: settings.breaker_enabled,
        config::KEY_ROUTING_BREAKER_THRESHOLD: settings.breaker_threshold,
        config::KEY_ROUTING_BREAKER_TIMEOUT: settings.breaker_timeout_seconds,
        config::KEY_ROUTING_BREAKER_SUCCESS: settings.breaker_success_threshold,
    })
}

/// GET /api/routing/queue?model=
pub async fn get_routing_queue(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let model = params
        .get("model")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let snapshot = config::current();
    let accounts_value = state.store().list_accounts();
    let accounts = routing::accounts_of(&accounts_value);
    // 候选收窄：点名了模型 → 只留「清单里有它的家」（与转发同一张家清单，
    // `route_for_forward`）；没点名 → 全部账号。与 session.rs 的 ★ 推算同一取材。
    let providers: Vec<String> = match &model {
        Some(model) => router::route_for_forward(model),
        None => Vec::new(),
    };
    let scoped: Vec<Value> = match model.as_deref() {
        Some(_) if providers.is_empty() => accounts.clone(),
        Some(_) => accounts
            .iter()
            .filter(|account| providers.contains(&routing::provider_of(account).to_string()))
            .cloned()
            .collect(),
        None => accounts.clone(),
    };
    let now = crate::server::logging::now_ms();
    let counts: HashMap<String, usize> = state
        .upstream()
        .connections()
        .snapshot()
        .into_iter()
        .collect();
    let keys = routing::CooldownKeys::new(model.as_deref().unwrap_or(""));
    // 限制器软跳过与转发的选路同一判据（被拦的账号在面板上也是「跳过」，
    // 原因写明是限制器而不是限流 —— 两回事）
    let facts = crate::server::core::usage_records::balance_facts();
    let token_facts = crate::server::core::limiter::token_facts();
    let limiter_blocked: HashSet<String> = scoped
        .iter()
        .filter(|account| {
            crate::server::core::usage_records::balance_blocked(account, &facts)
                || crate::server::core::limiter::token_skip_blocked(account, &token_facts, now)
        })
        .filter_map(|account| routing::account_id(account).map(str::to_string))
        .collect();
    let settings = config::routing_settings();
    // 面板只**看**当前状态，不做粘性查找、不前进轮转游标 —— 预演不该刷新
    // 任何会话的计时，也不该消耗下一次轮转
    let (sunk, resting, open) = state.upstream().route_queue_facts(now);
    let meta = routing::PlanMeta {
        strategy: settings.strategy,
        sunk: &sunk,
        resting: &resting,
        open: &open,
        limiter_blocked: &limiter_blocked,
        cursor: None,
        sticky_account: None,
    };
    let plan = routing::plan_route(&scoped, &keys, &counts, &[], now, &meta);
    let entry_json = |entry: &routing::PlanEntry| {
        json!({
            "id": entry.id,
            "name": entry.name,
            "provider": entry.provider,
            "providerLabel": crate::server::core::providers::kind_from_id(&entry.provider)
                .map(|kind| crate::server::core::providers::meta(kind).label.to_string())
                .unwrap_or_else(|| entry.provider.clone()),
            "state": entry.state,
            "reason": entry.reason,
            "resetAt": if entry.reset_at > 0 { Value::from(entry.reset_at) } else { Value::Null },
            "priority": entry.account.get("priority").cloned().unwrap_or(Value::Null),
        })
    };
    ok_json(json!({
        "strategy": settings.strategy.as_str(),
        "sticky": settings.sticky.as_str(),
        "model": model.clone().map(Value::String).unwrap_or(Value::Null),
        "lastRequestModel": snapshot.last_request_model(),
        "pickedId": plan.ordered.first().map(|entry| entry.id.clone()).unwrap_or_default(),
        "ordered": plan.ordered.iter().map(entry_json).collect::<Vec<_>>(),
        "blocked": plan.blocked.iter().map(entry_json).collect::<Vec<_>>(),
    }))
}
