//! Magpie signin.go / chatgpt_api.go 的 loopback 授权与手动回调。
//! 只在本进程等待回调；poll 检查本地状态，不对上游发设备码轮询。
//! Pending 中的 code/verifier/nonce 不落盘，完成、取消或超时均释放监听端口。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol::{LoginCallback, LoginPoll, LoginStart};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::server::core::account_store::AccountStore;
use crate::server::core::login::LoginCallbackResult;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account, host, http, oidc, AUTHORIZE_URL, DYNAMIC_CLIENT, ID, RESOURCE, SCOPES};

const LOGIN_TTL_MS: i64 = 5 * 60 * 1000;
const MAX_FLOWS: usize = 16;
pub(super) const MAX_CALLBACK_BYTES: usize = 16 * 1024;

pub(super) struct Flow {
    pub id: String,
    pub redirect: String,
    verifier: String,
    nonce: String,
    host_id: String,
    registration: Option<account::Credentials>,
    pub stop: CancellationToken,
    client: reqwest::Client,
    direct: bool,
    pub deadline: i64,
    state: tokio::sync::Mutex<Progress>,
}

#[derive(Clone)]
enum Progress {
    Waiting,
    Exchanging,
    Done(agent2api_provider_protocol::NormalizedAccount),
    Failed(GatewayError),
}

fn flows() -> &'static Mutex<HashMap<String, Arc<Flow>>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Arc<Flow>>>> = OnceLock::new();
    FLOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup(id: &str) -> Result<Arc<Flow>, GatewayError> {
    flows()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| GatewayError::with_status(404, "ChatGPT 登录任务不存在或已过期，请重新发起"))
}

pub(super) fn metadata(store: &AccountStore) -> Value {
    let mut choices = vec![json!({ "value": "new", "label": "添加新的 ChatGPT 账号或工作区" })];
    let snapshot = store.list_accounts();
    for row in snapshot
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("provider").and_then(Value::as_str) == Some(ID))
    {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        let name = row.get("name").and_then(Value::as_str).unwrap_or("ChatGPT");
        choices.push(
            json!({ "value": id, "label": format!("重新授权：{}", account::safe_text(name)) }),
        );
    }
    json!({ "methods": [{
        "methodIndex": 0, "type": "oauth", "label": "使用 ChatGPT 登录",
        "manualCallback": true, "callbackInput": "url", "validatesCallback": true,
        "browserMode": "external", "localOnly": false,
        "instructions": "在系统浏览器授权。如果回调页面无法打开，请粘贴地址栏里的完整回调地址。",
        "prompts": [{ "key": "accountId", "type": "select", "label": "登录账号",
            "default": "new", "options": choices }]
    }] })
}

pub(super) async fn start(options: Value) -> Result<LoginStart, GatewayError> {
    if options
        .get("methodIndex")
        .is_some_and(|index| index.as_u64() != Some(0))
    {
        return Err(GatewayError::bad_request("不支持的 ChatGPT 登录方式"));
    }
    let requested = options
        .pointer("/inputs/accountId")
        .and_then(Value::as_str)
        .unwrap_or("new");
    let store = account::shared()?;
    let registration = if requested.is_empty() || requested == "new" {
        None
    } else {
        Some(account::load(&store, requested)?.1)
    };
    let proxy = if options.get("proxy").is_some() {
        http::proxy_from_options(options.get("proxy"))?
    } else if registration.is_some() {
        account::proxy(&store, requested)?
    } else {
        None
    };
    let client = http::client(proxy.as_ref())?;
    let host_id = host::get()?;
    let state = host::random_token(24)?;
    let verifier = host::random_token(48)?;
    let nonce = host::random_token(24)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 1455)).await {
        Ok(listener) => listener,
        Err(_) => TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| GatewayError::with_status(503, "无法打开 ChatGPT 本机回调端口"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|_| GatewayError::new("无法取得 ChatGPT 回调端口"))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/auth/callback");
    let mut url =
        url::Url::parse(AUTHORIZE_URL).map_err(|_| GatewayError::new("ChatGPT 授权地址无效"))?;
    {
        let client_id = registration
            .as_ref()
            .map(|credentials| credentials.client_id.as_str())
            .unwrap_or(DYNAMIC_CLIENT);
        let mut query = url.query_pairs_mut();
        query
            .append_pair("client_id", client_id)
            .append_pair("ext_agent_host_id", &host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect)
            .append_pair("resource", RESOURCE)
            .append_pair("scope", SCOPES)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        if registration.is_none() {
            query.append_pair("agent_name_hint", "Agent2API");
        }
    }
    let flow = Arc::new(Flow {
        id: state.clone(),
        redirect,
        verifier,
        nonce,
        host_id,
        registration,
        client,
        direct: proxy.is_none(),
        deadline: logging::now_ms() + LOGIN_TTL_MS,
        stop: CancellationToken::new(),
        state: tokio::sync::Mutex::new(Progress::Waiting),
    });
    {
        let mut table = flows()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        table.retain(|_, existing| {
            if existing.deadline <= logging::now_ms() {
                existing.stop.cancel();
                false
            } else {
                true
            }
        });
        if table.len() >= MAX_FLOWS {
            return Err(GatewayError::with_status(
                429,
                "ChatGPT 登录任务过多，请先完成或取消已有任务",
            ));
        }
        table.insert(state.clone(), flow.clone());
    }
    super::listener::spawn(flow.clone(), listener);
    let expiry_id = state.clone();
    let expiry_stop = flow.stop.clone();
    crate::spawn_task(async move {
        tokio::select! {
            _ = expiry_stop.cancelled() => {},
            _ = tokio::time::sleep(Duration::from_millis(LOGIN_TTL_MS as u64)) => cancel(&expiry_id),
        }
    });
    Ok(LoginStart {
        flow: state, url: url.to_string(), poll_interval_ms: 1000,
        instructions: "请在系统浏览器完成 ChatGPT 授权。远程访问时，复制无法打开的回调页面完整地址并粘贴回来。".to_string(),
        external_browser: true, allowed_hosts: vec!["auth.openai.com".to_string(), "chatgpt.com".to_string()],
    })
}

pub(super) fn cancel(id: &str) {
    if let Some(flow) = flows()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id)
    {
        flow.stop.cancel();
    }
}

fn retry() -> LoginCallbackResult {
    LoginCallbackResult {
        poll: LoginPoll::Pending {
            hint: Some("回调地址与本次 ChatGPT 登录不匹配，请重新复制完整地址".to_string()),
        },
        next_url: None,
        retryable: true,
    }
}

enum Callback {
    Code { code: String, client_id: String },
    Error(String),
}

fn parse_callback(flow: &Flow, raw: &str) -> Option<Callback> {
    if raw.len() > MAX_CALLBACK_BYTES || raw.bytes().any(|ch| ch < b' ' || ch == 0x7f) {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    let redirect = url::Url::parse(&flow.redirect).ok()?;
    if url.scheme() != redirect.scheme()
        || url.host_str() != redirect.host_str()
        || url.port_or_known_default() != redirect.port_or_known_default()
        || url.path() != redirect.path()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut fields = HashMap::new();
    for (key, value) in url.query_pairs() {
        if matches!(key.as_ref(), "code" | "state" | "client_id" | "error")
            && fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return None;
        }
    }
    if fields.get("state").map(String::as_str) != Some(flow.id.as_str()) {
        return None;
    }
    // 成功与 error 回调都在 state/地址归属通过之后处理，误贴不能杀死当前任务。
    if let Some(error) = fields.get("error").filter(|value| !value.is_empty()) {
        if fields.get("code").is_some_and(|code| !code.is_empty()) {
            return None;
        }
        return Some(Callback::Error(super::error::message(400, error)));
    }
    let code = fields
        .get("code")
        .filter(|code| !code.is_empty() && code.len() <= 8192)?
        .clone();
    let client_id = match &flow.registration {
        Some(registered) => {
            if fields
                .get("client_id")
                .is_some_and(|value| value != &registered.client_id)
            {
                return None;
            }
            registered.client_id.clone()
        }
        None => fields
            .get("client_id")
            .filter(|value| {
                !value.is_empty() && value.len() <= 512 && value.as_str() != DYNAMIC_CLIENT
            })?
            .clone(),
    };
    Some(Callback::Code { code, client_id })
}

pub(super) async fn callback(input: LoginCallback) -> Result<LoginCallbackResult, GatewayError> {
    let flow = lookup(&input.flow)?;
    receive(&flow, input.url.trim()).await
}

pub(super) async fn receive(
    flow: &Arc<Flow>,
    raw: &str,
) -> Result<LoginCallbackResult, GatewayError> {
    if flow.stop.is_cancelled() || flow.deadline <= logging::now_ms() {
        return Err(GatewayError::bad_request(
            "ChatGPT 登录已取消或超时，请重新发起",
        ));
    }
    let Some(callback) = parse_callback(flow, raw) else {
        return Ok(retry());
    };
    let mut progress = flow.state.lock().await;
    if !matches!(*progress, Progress::Waiting) {
        return Ok(as_poll(&progress)?.into());
    }
    match callback {
        Callback::Error(message) => {
            *progress = Progress::Failed(GatewayError::bad_request(message.clone()));
            Ok(LoginPoll::Failed { message }.into())
        }
        Callback::Code { code, client_id } => {
            *progress = Progress::Exchanging;
            let flow = flow.clone();
            crate::spawn_task(async move {
                let result = tokio::select! {
                    biased;
                    _ = flow.stop.cancelled() => return,
                    result = exchange(&flow, &code, &client_id) => result,
                };
                if flow.stop.is_cancelled() {
                    return;
                }
                let mut progress = flow.state.lock().await;
                *progress = match result {
                    Ok(account) => Progress::Done(account),
                    Err(error) => Progress::Failed(login_error(&flow, error)),
                };
            });
            Ok(LoginPoll::Pending {
                hint: Some("已收到浏览器授权，正在向 OpenAI 换取登录凭证…".to_string()),
            }
            .into())
        }
    }
}

async fn exchange(
    flow: &Flow,
    code: &str,
    client_id: &str,
) -> Result<agent2api_provider_protocol::NormalizedAccount, GatewayError> {
    let tokens = http::tokens(
        &flow.client,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", &flow.verifier),
            ("redirect_uri", &flow.redirect),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    if tokens.id_token.is_empty() {
        return Err(GatewayError::with_status(502, "OpenAI 未返回签名身份凭证"));
    }
    let subject = flow
        .registration
        .as_ref()
        .map(|credentials| credentials.subject.as_str());
    let claims = oidc::verify(
        &flow.client,
        &tokens.id_token,
        client_id,
        Some(&flow.nonce),
        subject,
    )
    .await?;
    let mut credentials = account::Credentials::empty(client_id.to_string(), flow.host_id.clone());
    credentials.apply_tokens(tokens)?;
    credentials.apply_identity(&claims)?;
    credentials.normalized()
}

fn login_error(flow: &Flow, mut error: GatewayError) -> GatewayError {
    if let Some(diagnostic) = &mut error.login_diagnostic {
        diagnostic.route = Some(if flow.direct { "direct" } else { "proxy" }.to_string());
        if flow.direct && error.status_code == 403 {
            error.message.push_str("\n当前软件使用直连，不会跟随系统浏览器的代理；如需使用系统代理，可在“备注与代理”中点击“填入系统代理”。");
        }
    }
    logging::log_event("error", "auth", &error.message, Some(json!({
        "provider": ID, "stage": error.login_diagnostic.as_ref().map(|diagnostic| &diagnostic.stage),
        "status": error.status_code, "code": error.code,
        "requestId": error.login_diagnostic.as_ref().and_then(|diagnostic| diagnostic.request_id.as_ref()),
    })));
    error
}

fn as_poll(progress: &Progress) -> Result<LoginPoll, GatewayError> {
    Ok(match progress {
        Progress::Waiting => LoginPoll::Pending {
            hint: Some("等待浏览器完成 ChatGPT 授权".to_string()),
        },
        Progress::Exchanging => LoginPoll::Pending {
            hint: Some("已收到浏览器授权，正在向 OpenAI 换取登录凭证…".to_string()),
        },
        Progress::Done(account) => LoginPoll::Done {
            account: account.clone(),
        },
        Progress::Failed(error) => return Err(error.clone()),
    })
}

pub(super) async fn poll(id: &str) -> Result<LoginPoll, GatewayError> {
    let flow = lookup(id)?;
    if flow.stop.is_cancelled() || flow.deadline <= logging::now_ms() {
        return Ok(LoginPoll::Failed {
            message: "ChatGPT 登录已取消或超时，请重新发起".to_string(),
        });
    }
    let progress = flow.state.lock().await;
    as_poll(&progress)
}
