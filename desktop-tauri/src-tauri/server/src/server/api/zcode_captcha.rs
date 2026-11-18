//! 模型验证码按账号读取和提交；无参数概览只供展示，不能替提交选择账号。
//! 令牌只能由用户完成官方验证取得，与领取验证保持独立。
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::plugin_host::{self, wasm::verification};
use crate::server::core::providers::{adapter, zcode};
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

type ApiError = (i32, String);

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptchaQuery {
    account_id: Option<String>,
    provider: Option<String>,
}

/// 配置与库存的常规查询入口；仅 POST 的短通道避开公开账号/出口投影。
pub(super) fn account_provider(state: &ServerState, account_id: &str, requested: Option<&str>) -> Result<String, ApiError> {
    if account_id.is_empty() || account_id.len() > 512 || account_id.chars().any(char::is_control) {
        return Err((400, "请明确选择验证码所属账号".into()));
    }
    let record = state.store().route_account(account_id).ok_or_else(|| (404, "找不到该 ZCode 账号".into()))?;
    let provider = record["provider"].as_str().unwrap_or("");
    if zcode::region::Region::from_provider_id(provider).is_none() {
        return Err((400, "该账号不是 ZCode 账号".into()));
    }
    if requested.is_some_and(|requested| requested != provider) {
        return Err((400, "验证码提供商与账号真实归属不一致".into()));
    }
    if !plugin_host::serves_provider(provider) || plugin_host::host_for_provider(provider).is_none_or(|host| !host.ready()) {
        return Err((503, "ZCode 提供商执行体未启用或尚未就绪".into()));
    }
    Ok(provider.to_string())
}

fn config_text<'a>(value: &'a Value, key: &str, required: bool, limit: usize) -> Result<&'a str, ApiError> {
    value[key].as_str().filter(|text| text.len() <= limit && !text.chars().any(char::is_control)
        && (!required || !text.trim().is_empty()))
        .ok_or_else(|| (502, "验证码配置字段不完整或格式无效，请重新获取配置".into()))
}

/// WASM 只向前端投影公开配置白名单，绝不透传凭据或任意插件结果。
pub(super) fn project_config(value: &Value, provider: &str, account_id: &str) -> Result<Value, ApiError> {
    let enabled = value["enabled"].as_bool().ok_or_else(|| (502, "验证码配置缺少启用状态".into()))?;
    if value["provider"] != provider || value["accountId"] != account_id
        || value["engine"] != (if enabled { "aliyun" } else { "none" }) {
        return Err((502, "验证码配置与当前账号或验证组件不一致".into()));
    }
    let config_id = config_text(value, "configId", true, 256)?;
    let prefix = config_text(value, "prefix", enabled, 256)?;
    let scene_id = config_text(value, "sceneId", enabled, 256)?;
    let region = config_text(value, "region", enabled, 256)?;
    let skip = value["skipModelRequest"].as_bool().ok_or_else(|| (502, "验证码配置缺少模型验证规则".into()))?;
    Ok(json!({ "enabled": enabled, "engine": if enabled { "aliyun" } else { "none" },
        "provider": provider, "accountId": account_id, "configId": config_id,
        "prefix": prefix, "sceneId": scene_id, "region": region, "skipModelRequest": skip }))
}

fn project_stats(value: Value, provider: &str, account_id: &str) -> Result<Value, ApiError> {
    if value["provider"] != provider || value["accountId"] != account_id
        || value["ready"].as_u64().is_none() || !value["required"].is_boolean() {
        return Err((502, "验证码库存没有返回当前账号的有效状态".into()));
    }
    let mut fields = Map::new();
    for key in ["ready", "ttlMs", "minted", "consumed"] {
        if let Some(number) = value[key].as_u64() { fields.insert(key.into(), json!(number)); }
    }
    for key in ["required", "demandActive"] {
        if let Some(flag) = value[key].as_bool() { fields.insert(key.into(), json!(flag)); }
    }
    if let Some(at) = value["lastChallengeAt"].as_i64() { fields.insert("lastChallengeAt".into(), json!(at)); }
    if value["error"].as_str().is_some_and(|error| !error.is_empty()) {
        fields.insert("error".into(), json!("当前账号验证码配置不可用，请重新获取配置"));
    }
    fields.insert("config".into(), if value["config"].is_null() { Value::Null }
        else { project_config(&value["config"], provider, account_id)? });
    fields.insert("localGates".into(), json!([]));
    Ok(Value::Object(fields))
}

fn decorate(mut body: Value, provider: Option<&str>, account_id: Option<&str>, start_plan_accounts: usize) -> Value {
    if !body.is_object() { body = json!({ "ready": 0, "required": false, "error": "验证码库存状态无效" }); }
    let ready = body["ready"].as_u64().unwrap_or(0);
    let required = body["required"].as_bool().unwrap_or(false);
    let demand = body["demandActive"].as_bool().unwrap_or(false);
    let pool = config::captcha_pool_settings();
    let fields = body.as_object_mut().unwrap();
    fields.insert("provider".into(), json!(provider));
    fields.insert("accountId".into(), json!(account_id));
    fields.insert("captchaAccountId".into(), json!(account_id));
    fields.insert("target".into(), json!(pool.target));
    fields.insert("startPlanAccounts".into(), json!(start_plan_accounts));
    fields.insert("demandActive".into(), json!(demand));
    fields.insert("needsTokens".into(), json!(required && demand && start_plan_accounts > 0 && ready == 0));
    fields.insert("pollMs".into(), json!(pool.poll_ms));
    fields.insert("maxPerRound".into(), json!(pool.max_per_round));
    body
}

async fn account_stats(state: &ServerState, account_id: &str, requested: Option<&str>) -> Result<Value, ApiError> {
    let provider = account_provider(state, account_id, requested)?;
    let wasm = plugin_host::wasm::owns(&provider);
    let native = verification::legacy_native(&provider);
    if !wasm && !native { return Err((400, "当前执行体不支持 ZCode 验证码库存".into())); }
    let adapter = adapter::adapter_by_id(&provider).ok_or_else(|| (503, "验证码执行体不可用".into()))?;
    let result = adapter.admin_action(state.store(), account_id, "captcha.stats", &Value::Null).await
        .map_err(|error| (error.status_code, error.message))?;
    account_provider(state, account_id, Some(&provider))?;
    if plugin_host::wasm::owns(&provider) != wasm || verification::legacy_native(&provider) != native {
        return Err((409, "验证码执行体已变化，请重新读取当前账号状态".into()));
    }
    let result = if wasm { project_stats(result, &provider, account_id)? } else { result };
    let start_plan = state.store().route_account(account_id).is_some_and(|record| zcode::plan_of(&record) == zcode::PLAN_START);
    Ok(decorate(result, Some(&provider), Some(account_id), usize::from(start_plan)))
}

async fn overview(state: &ServerState) -> Value {
    let snapshot = state.store().routing_snapshot();
    let mut count = 0;
    let mut chosen = None;
    let mut fallback = None;
    for record in &snapshot.accounts {
        let provider = record["provider"].as_str().unwrap_or("");
        let id = record["id"].as_str().unwrap_or("");
        if id.is_empty() || record["enabled"] == false || zcode::region::Region::from_provider_id(provider).is_none() { continue; }
        fallback.get_or_insert_with(|| (provider.to_string(), id.to_string()));
        if zcode::plan_of(record) == zcode::PLAN_START {
            count += 1;
            chosen.get_or_insert_with(|| (provider.to_string(), id.to_string()));
        }
    }
    if let Some((provider, id)) = chosen.or(fallback) {
        let result = account_stats(state, &id, Some(&provider)).await
            .unwrap_or_else(|(_, message)| json!({ "ready": 0, "required": false, "error": message }));
        return decorate(result, Some(&provider), Some(&id), count);
    }
    decorate(json!({ "ready": 0, "required": false }), None, None, 0)
}

pub async fn get_captcha(State(state): State<ServerState>, Query(query): Query<CaptchaQuery>) -> Response {
    match query.account_id {
        Some(account) => match account_stats(&state, &account, query.provider.as_deref()).await {
            Ok(value) => ok_json(value), Err((status, message)) => management_error(status, message),
        },
        None if query.provider.is_some() => management_error(400, "查询指定提供商库存时必须选择账号"),
        None => ok_json(overview(&state).await),
    }
}

async fn push_bound(state: &ServerState, payload: &Value) -> Result<Value, ApiError> {
    let provider = payload["provider"].as_str().filter(|provider| zcode::region::Region::from_provider_id(provider).is_some())
        .ok_or_else(|| (400, "验证码提交必须明确提供商".into()))?;
    let account = payload["accountId"].as_str().filter(|id| !id.is_empty())
        .ok_or_else(|| (400, "验证码提交必须明确账号".into()))?;
    if payload["configId"].as_str().is_none_or(|id| id.trim().is_empty()) {
        return Err((400, "验证码提交缺少当次配置标识，请重新获取配置".into()));
    }
    // 不使用 account_provider_of/list_accounts：它们可能在构建公开视图时解析代理。
    // 这里的 provider 只是候选；可信调用会从无代理账号快照核对真实归属和凭据身份。
    let adapter = adapter::adapter_by_id(provider).ok_or_else(|| (503, "验证码执行体不可用".into()))?;
    let result = adapter.verification_action(state.store(), account, payload).await
        .map_err(|error| (error.status_code, error.message))?;
    let count = payload["tokens"].as_array().map_or(0, Vec::len) as u64;
    let accepted = result["accepted"].as_u64().filter(|accepted| *accepted <= count)
        .ok_or_else(|| (502, "验证码提交回执无效".into()))?;
    Ok(json!({ "accepted": accepted, "provider": provider, "accountId": account }))
}

async fn push_legacy(state: &ServerState, payload: &Value) -> Result<Value, ApiError> {
    // 无账号的旧请求无法辨别目标地区；只在当前 ZCode 执行体均为原生时兼容。
    if !verification::legacy_global_allowed() || zcode::region::Region::ALL.into_iter().any(|region| plugin_host::serves_provider(region.provider_id())
        && !verification::legacy_native(region.provider_id())) {
        return Err((400, "当前验证码提交需要 provider、accountId 和 configId，不能使用旧全局入口".into()));
    }
    let provider = zcode::region::Region::ALL.into_iter().map(|region| region.provider_id())
        .find(|provider| verification::legacy_native(provider)).ok_or_else(|| (503, "原生验证码执行体未安装或未启用".into()))?;
    let items = match payload.get("tokens") {
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return Err((400, "tokens 必须是数组".into())),
        None => vec![payload.clone()],
    };
    let adapter = adapter::adapter_by_id(provider).ok_or_else(|| (503, "原生验证码执行体不可用".into()))?;
    let result = adapter.verification_action(state.store(), "", &json!({ "tokens": items })).await
        .map_err(|error| (error.status_code, error.message))?;
    let accepted = result["accepted"].as_u64().unwrap_or(0);
    if accepted == 0 { return Err((400, "没有可入池的令牌（令牌为空或已提交）".into())); }
    // 仅旧原生客户端保留旧响应；WASM 回传从不在短通道之后等待库存查询。
    let mut response = overview(state).await;
    response["accepted"] = json!(accepted);
    Ok(response)
}

pub async fn push_captcha(State(state): State<ServerState>, body: Bytes) -> Response {
    if body.len() > verification::MAX_MESSAGE { return management_error(413, "验证码提交超过大小限制"); }
    let payload = match parse_body(&body) {
        Ok(value) if value.is_object() => value,
        Ok(_) => return management_error(400, "请求体必须是 JSON 对象"),
        Err(error) => return management_error(400, error.message),
    };
    let outcome = if ["provider", "accountId", "configId"].iter().any(|key| payload.get(*key).is_some()) {
        push_bound(&state, &payload).await
    } else { push_legacy(&state, &payload).await };
    match outcome {
        Ok(value) => ok_json(value), Err((status, message)) => management_error(status, message),
    }
}
