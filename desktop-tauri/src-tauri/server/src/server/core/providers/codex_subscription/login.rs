//! Magpie codex_device_signin.go 的设备授权流程：验证码、间隔、15 分钟截止与取消。
//! 仅访问 auth.openai.com 的公开 Codex OAuth；不使用 ChatGPT API 动态客户端注册。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent2api_provider_protocol::{LoginPoll, LoginStart, NormalizedAccount};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::server::core::account_store::{plugin_accounts, AccountStore};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account::Tokens, http, ID};

pub const LOGIN_TTL_MS: u64 = 15 * 60 * 1000;
const MAX_FLOWS: usize = 32;

struct Flow {
    cancel: CancellationToken,
    deadline: Instant,
    result: Mutex<LoginPoll>,
}

static FLOWS: OnceLock<Mutex<HashMap<String, Arc<Flow>>>> = OnceLock::new();

fn flows() -> &'static Mutex<HashMap<String, Arc<Flow>>> {
    FLOWS.get_or_init(Mutex::default)
}

pub(super) fn metadata() -> Value {
    json!({
        "instructions": "使用 ChatGPT 订阅的 Codex 授权，凭据仅保存到本网关，不修改本机 Codex 登录文件。",
        "methods": [
            { "methodIndex": 0, "type": "oauth", "label": "Codex 设备授权", "prompts": [],
              "browserMode": "external", "manualCallback": false,
              "instructions": "打开官方授权页面并输入本次验证码；需要在 ChatGPT 安全设置或工作区权限中允许设备授权。" },
            { "methodIndex": 1, "type": "api", "label": "导入 Codex 登录凭据",
              "instructions": "粘贴你主动提供的 Codex auth.json；请勿与其他正在刷新的客户端长期共用同一份凭据。这里不是 OpenAI API Key。",
              "prompts": [{ "key": "credentials", "type": "text", "label": "Codex 凭据 JSON",
                  "placeholder": "粘贴 auth.json 的完整内容", "secret": true, "optional": false }] }
        ]
    })
}

fn random_flow() -> Result<String, GatewayError> {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| GatewayError::with_status(500, "无法创建安全的 Codex 登录标识"))?;
    Ok(format!(
        "codex-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(super) async fn start(options: Value) -> Result<LoginStart, GatewayError> {
    if options
        .get("methodIndex")
        .is_some_and(|value| value.as_u64() != Some(0))
    {
        return Err(GatewayError::bad_request("导入凭据请使用添加账号入口"));
    }
    if options
        .get("inputs")
        .is_some_and(|value| value.as_object().is_none_or(|fields| !fields.is_empty()))
    {
        return Err(GatewayError::bad_request(
            "Codex 设备授权不接收额外凭据输入",
        ));
    }
    let proxy = ResolvedProxy::from_json(options.get("proxy").unwrap_or(&Value::Null))
        .map_err(|_| GatewayError::bad_request("Codex 登录代理不可用，未回退直连"))?;
    let flow_id = random_flow()?;
    {
        let mut table = flows().lock().unwrap_or_else(|error| error.into_inner());
        table.retain(|_, flow| {
            let keep = Instant::now() < flow.deadline && !flow.cancel.is_cancelled();
            if !keep {
                flow.cancel.cancel();
            }
            keep
        });
        if table.len() >= MAX_FLOWS {
            return Err(GatewayError::with_status(
                429,
                "进行中的 Codex 登录过多，请完成或取消旧登录后重试",
            ));
        }
    }
    let request = http::with_headers(
        http::client(proxy.as_ref())?
            .post(format!("{}/api/accounts/deviceauth/usercode", http::ISSUER))
            .json(&json!({ "client_id": http::CLIENT_ID })),
        &http::client_headers(),
    );
    let reply = http::request(request, 1024 * 1024, "获取设备验证码").await?;
    if reply.status == 404 {
        return Err(GatewayError::bad_request(
            "当前账号或工作区未开放 Codex 设备授权，请在 ChatGPT 中开启，或主动导入已有 Codex 凭据",
        ));
    }
    if !(200..300).contains(&reply.status) {
        return Err(http::response_error("获取设备验证码", &reply));
    }
    let device_id = super::account::text(&reply.body, &["device_auth_id"]).to_string();
    let code = super::account::text(&reply.body, &["user_code", "usercode"]).to_string();
    if device_id.is_empty()
        || device_id.len() > 4096
        || code.is_empty()
        || code.len() > 128
        || code.chars().any(char::is_control)
    {
        return Err(GatewayError::with_status(
            502,
            "Codex 没有返回有效的设备验证码",
        ));
    }
    let seconds = reply
        .body
        .get("interval")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        })
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let interval = Duration::from_secs(seconds.min(LOGIN_TTL_MS / 1000));
    let cancel = CancellationToken::new();
    let flow = Arc::new(Flow {
        cancel: cancel.clone(),
        deadline: Instant::now() + Duration::from_millis(LOGIN_TTL_MS),
        result: Mutex::new(LoginPoll::Pending {
            hint: Some(format!("请在 ChatGPT 官方页面输入验证码：{code}")),
        }),
    });
    {
        let mut table = flows().lock().unwrap_or_else(|error| error.into_inner());
        if table.len() >= MAX_FLOWS {
            return Err(GatewayError::with_status(
                429,
                "进行中的 Codex 登录过多，请稍后重试",
            ));
        }
        table.insert(flow_id.clone(), flow.clone());
    }
    let poll_flow = flow.clone();
    let poll_code = code.clone();
    crate::spawn_task(async move {
        let deadline = tokio::time::Instant::from_std(poll_flow.deadline);
        let result = tokio::select! {
            biased;
            _ = poll_flow.cancel.cancelled() => return,
            _ = tokio::time::sleep_until(deadline) => Err(GatewayError::bad_request("Codex 设备验证码已过期，请重新发起授权")),
            result = complete(&poll_flow, &device_id, &poll_code, interval, proxy.as_ref()) => result,
        };
        if poll_flow.cancel.is_cancelled() {
            return;
        }
        let mut slot = poll_flow
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *slot = match result {
            Ok(account) => LoginPoll::Done { account },
            Err(error) => LoginPoll::Failed {
                message: error.message,
            },
        };
    });
    // 即使管理端关闭且未再轮询，也会释放临时令牌与流程表，不长期持有完成结果。
    let cleanup_id = flow_id.clone();
    crate::spawn_task(async move {
        tokio::select! {
            biased;
            _ = flow.cancel.cancelled() => return,
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(flow.deadline)) => {},
        }
        if let Some(flow) = flows()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&cleanup_id)
        {
            flow.cancel.cancel();
        }
    });
    Ok(LoginStart { flow: flow_id, url: format!("{}/codex/device", http::ISSUER),
        instructions: format!("请在 ChatGPT 官方页面输入验证码：{code}\n验证码 15 分钟有效。仅继续你自己在本网关发起的授权；完成后返回这里。"),
        poll_interval_ms: interval.as_millis().min(u64::MAX as u128) as u64,
        external_browser: true, allowed_hosts: vec!["auth.openai.com".into(), "chatgpt.com".into()] })
}

async fn complete(
    flow: &Flow,
    device_id: &str,
    code: &str,
    interval: Duration,
    proxy: Option<&ResolvedProxy>,
) -> Result<NormalizedAccount, GatewayError> {
    let mut misses = 0;
    loop {
        tokio::select! {
            biased;
            _ = flow.cancel.cancelled() => return Err(GatewayError::with_status(499, "Codex 登录已取消")),
            _ = tokio::time::sleep(interval) => {},
        }
        let request = http::with_headers(
            http::client(proxy)?
                .post(format!("{}/api/accounts/deviceauth/token", http::ISSUER))
                .json(&json!({ "device_auth_id": device_id, "user_code": code })),
            &http::client_headers(),
        );
        let reply = match http::request(request, 1024 * 1024, "查询授权进度").await {
            Ok(reply) => reply,
            Err(_) if misses < 4 => {
                misses += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        misses = 0;
        if matches!(reply.status, 403 | 404) {
            continue;
        }
        if !(200..300).contains(&reply.status) {
            return Err(http::response_error("设备授权", &reply));
        }
        let code = super::account::text(&reply.body, &["authorization_code"]);
        let verifier = super::account::text(&reply.body, &["code_verifier"]);
        if code.is_empty() || verifier.is_empty() {
            return Err(GatewayError::with_status(
                502,
                "Codex 授权响应缺少换取凭据所需的信息",
            ));
        }
        let redirect = format!("{}/deviceauth/callback", http::ISSUER);
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("client_id", http::CLIENT_ID)
            .append_pair("code", code)
            .append_pair("code_verifier", verifier)
            .append_pair("redirect_uri", &redirect)
            .finish();
        let request = http::with_headers(
            http::client(proxy)?
                .post(http::TOKEN_URL)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form),
            &http::client_headers(),
        );
        let reply = http::request(request, 1024 * 1024, "换取登录凭据").await?;
        if !(200..300).contains(&reply.status) {
            return Err(http::response_error("换取登录凭据", &reply));
        }
        let tokens = Tokens::read(&reply.body)?;
        if tokens.id_token.is_empty()
            || tokens.refresh.is_empty()
            || tokens.user_id.is_empty() && tokens.email.is_empty()
        {
            return Err(GatewayError::with_status(
                502,
                "Codex 没有返回完整的订阅登录凭据，请重新授权",
            ));
        }
        let mut secret = tokens.secret();
        secret["lastRefreshAt"] = json!(logging::now_ms());
        return Ok(NormalizedAccount {
            secret,
            public: tokens.public(None, false),
            identity: tokens.identity(),
            desktop: false,
        });
    }
}

pub(super) fn poll(id: &str) -> Result<LoginPoll, GatewayError> {
    let flow = flows()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| GatewayError::with_status(404, "Codex 登录流程不存在、已取消或已过期"))?;
    if flow.cancel.is_cancelled() || Instant::now() >= flow.deadline {
        return Ok(LoginPoll::Failed {
            message: "Codex 登录已取消或过期，请重新授权".into(),
        });
    }
    let result = flow
        .result
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    Ok(result)
}

pub(super) fn cancel(id: &str) {
    if let Some(flow) = flows()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id)
    {
        flow.cancel.cancel();
    }
}

pub(super) fn persist(
    account: &NormalizedAccount,
    proxy: Option<&Value>,
) -> Result<String, GatewayError> {
    let store = plugin_accounts::shared()
        .ok_or_else(|| GatewayError::with_status(503, "账号库尚未就绪"))?;
    persist_to(&store, account, proxy)
}

fn persist_to(
    store: &AccountStore,
    account: &NormalizedAccount,
    proxy: Option<&Value>,
) -> Result<String, GatewayError> {
    let tokens = Tokens::read(&account.secret)?;
    if account.identity != tokens.identity() || account.desktop {
        return Err(GatewayError::bad_request(
            "Codex 授权账号信息不一致，未保存",
        ));
    }
    let public = tokens.public(account.public.get("name").and_then(Value::as_str), false);
    let name = public
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Codex 订阅");
    let (saved, _) = store
        .plugin_upsert_account_with_source_and_proxy(
            &account.identity,
            ID,
            name,
            &account.secret,
            &public,
            false,
            "builtin",
            proxy,
        )
        .map_err(|_| {
            GatewayError::with_status(500, "Codex 登录凭据与代理保存失败，请检查账号库后重新授权")
        })?;
    saved
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| GatewayError::with_status(500, "Codex 账号保存后没有返回账号标识"))
}
