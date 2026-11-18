//! 原Kiro terminal回调的私有兑换证据和冻结候选正文；不是发送或登录成功回执。
//! 原publication/code只由真实claim产生；候选clientSecret没有独立注册回执，不称上游确认。

use std::sync::{Arc, Weak};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use zeroize::{Zeroize, Zeroizing};

use super::super::call::KiroCallSource;
use super::super::{Call, RuntimeBinding, State};
use super::{Outcome, Phase, Publication, Receipt};
use crate::server::errors::GatewayError;

const LIMIT: usize = 128 * 1024;
const SOCIAL_TOKEN: &str = "https://prod.us-east-1.auth.desktop.kiro.dev/oauth/token";
const SOCIAL_ORIGIN: &str = "https://app.kiro.dev";
const AWS_ORIGINS: &[&str] = crate::server::core::plugin_host::wasm::aws_oidc::ORIGINS;

pub(in crate::server::core::login::managed) fn invalid() -> GatewayError {
    GatewayError::with_status(
        409,
        "Kiro候选换码正文与本次真实回调不一致或已消费，未发送或重放授权码",
    )
    .with_code("kiro_exchange_source_invalid")
}

/// 无Clone/serde/公开构造入口；同一proof对象同时由原Receipt强持、原Binding弱见证。
pub(super) struct TerminalProof {
    outcome: Outcome,
    code: Zeroizing<String>,
    challenge: Zeroizing<String>,
    authorization_origin: String,
    login_provider: &'static str,
    client_id: Option<Zeroizing<String>>,
    redirect: Zeroizing<String>,
    endpoint: String,
}

impl TerminalProof {
    pub(super) fn from_social(
        publication: &Publication,
        port: u16,
        path: &str,
        option: &str,
        code: &str,
    ) -> Result<Self, GatewayError> {
        if publication.origin != SOCIAL_ORIGIN
            || publication.client_id.is_some()
            || !matches!(path, "/signin/callback" | "/oauth/callback")
            || !matches!(option, "google" | "github")
            || port == 0
        {
            return Err(invalid());
        }
        let login_provider = match option {
            "google" => "google",
            "github" => "github",
            _ => return Err(invalid()),
        };
        Ok(Self {
            outcome: Outcome::SocialCode,
            code: Zeroizing::new(code.to_string()),
            challenge: Zeroizing::new(publication.challenge.to_string()),
            authorization_origin: publication.origin.clone(),
            login_provider,
            client_id: None,
            redirect: Zeroizing::new(format!(
                "http://localhost:{port}{path}?login_option={option}"
            )),
            endpoint: SOCIAL_TOKEN.to_string(),
        })
    }
    pub(super) fn from_aws(
        publication: &Publication,
        port: u16,
        code: &str,
        login_provider: &'static str,
    ) -> Result<Self, GatewayError> {
        if !AWS_ORIGINS.contains(&publication.origin.as_str())
            || port == 0
            || !matches!(login_provider, "builderid" | "enterprise" | "internal")
        {
            return Err(invalid());
        }
        let client_id = publication
            .client_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        Ok(Self {
            outcome: Outcome::AwsCode,
            code: Zeroizing::new(code.to_string()),
            challenge: Zeroizing::new(publication.challenge.to_string()),
            authorization_origin: publication.origin.clone(),
            login_provider,
            client_id: Some(Zeroizing::new(client_id.to_string())),
            redirect: Zeroizing::new(format!("http://127.0.0.1:{port}/oauth/callback")),
            endpoint: format!("{}/token", publication.origin),
        })
    }
}

impl Receipt {
    pub(in crate::server::core::login::managed) fn exchange_current(
        &self,
        state: &State,
        call: &Call,
    ) -> bool {
        self.call
            .as_ref()
            .is_some_and(|id| id.as_str() == call.id())
            && self.step == call.expected()
            && self.matches_exchange_state(state)
    }

    pub(in crate::server::core::login::managed) fn matches_exchange_state(
        &self,
        state: &State,
    ) -> bool {
        let Some(call_id) = self.call.as_ref() else {
            return false;
        };
        let Some(proof) = &self.terminal else {
            return false;
        };
        matches!(self.outcome, Outcome::SocialCode | Outcome::AwsCode)
            && proof.outcome == self.outcome
            && self.login_provider == Some(proof.login_provider)
            && state.active.as_deref() == Some(call_id.as_str())
            && state.callback_step.as_deref() == Some(self.step.as_str())
            && state.kiro.current.as_deref() == Some(self.lease.as_str())
            && state.kiro.receiver.as_ref().is_some_and(|receiver| {
                !receiver.cancel.is_cancelled()
                    && Weak::ptr_eq(&self.receiver, &Arc::downgrade(receiver))
            })
            && state.step.as_ref().is_some_and(|step| {
                step.id == self.step
                    && step.status == "processing"
                    && step.browser.is_none()
                    && step.lease.as_deref() == Some(self.lease.as_str())
            })
            && state.leases.get(&self.lease).is_some_and(|lease| {
                lease.consumed
                    && !lease.queued
                    && lease.step.as_deref() == Some(self.step.as_str())
                    && lease.official_state.is_none()
                    && lease.kiro.as_ref().is_some_and(|binding| {
                        binding.accepted == Some(self.outcome)
                            && Weak::ptr_eq(&binding.receiver, &self.receiver)
                            && binding.login_provider == Some(proof.login_provider)
                            && binding.oauth_state.is_none()
                            && binding.publication.is_none()
                            && binding.terminal.as_ref().is_some_and(|original| {
                                Weak::ptr_eq(original, &Arc::downgrade(proof))
                            })
                            && match binding.phase {
                                Phase::Portal => {
                                    self.outcome == Outcome::SocialCode
                                        && lease.profile == super::policy::KIRO_PORTAL_PROFILE
                                        && proof.client_id.is_none()
                                        && proof.authorization_origin == SOCIAL_ORIGIN
                                        && matches!(proof.login_provider, "google" | "github")
                                }
                                Phase::Aws => {
                                    self.outcome == Outcome::AwsCode
                                        && lease.profile == super::policy::KIRO_AWS_PROFILE
                                        && proof.client_id.is_some()
                                        && matches!(
                                            proof.login_provider,
                                            "builderid" | "enterprise" | "internal"
                                        )
                                        && binding.next_origin
                                            == Some(proof.authorization_origin.as_str())
                                }
                            }
                    })
            })
    }
}

pub(in crate::server::core::login::managed) struct Prepared {
    receipt: Arc<Receipt>,
    proof: Arc<TerminalProof>,
    body: Zeroizing<Vec<u8>>,
    verifier: Zeroizing<String>,
    client_secret: Option<Zeroizing<String>>,
    deadline: Instant,
    runtime: Arc<RuntimeBinding>,
    consumed: std::sync::atomic::AtomicBool,
}

/// 原Call的一次准备结果，不Deserialize/Clone，也不携带可从自由JSON恢复的权限。
pub(crate) struct KiroExchangeInput {
    source: KiroCallSource,
    prepared: Arc<Prepared>,
}

/// 本地最终准入的一次消费，不代表服务收到/兑换成功；完整输入归唯一worker继续拥有。
pub(crate) struct KiroExchangePermit {
    input: KiroExchangeInput,
}

struct Candidate(Value);
impl Drop for Candidate {
    fn drop(&mut self) {
        fn erase(value: &mut Value) {
            match value {
                Value::String(text) => text.zeroize(),
                Value::Object(fields) => {
                    for value in fields.values_mut() {
                        erase(value);
                    }
                }
                Value::Array(items) => {
                    for value in items {
                        erase(value);
                    }
                }
                _ => {}
            }
        }
        erase(&mut self.0);
    }
}

fn text<'a>(
    fields: &'a serde_json::Map<String, Value>,
    name: &str,
    limit: usize,
) -> Result<&'a str, GatewayError> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= limit
                && !value
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace())
        })
        .ok_or_else(invalid)
}

impl Prepared {
    pub(in crate::server::core::login::managed) fn from_candidate(
        receipt: Arc<Receipt>,
        params: &Value,
        runtime: Arc<RuntimeBinding>,
        deadline: Instant,
    ) -> Result<Arc<Self>, GatewayError> {
        let raw = params
            .as_str()
            .filter(|raw| !raw.is_empty() && raw.len() <= LIMIT)
            .ok_or_else(invalid)?;
        let candidate =
            Candidate(
                crate::server::core::plugin_host::wasm::managed_task::declaration::parse_object::<
                    Value,
                >(raw.as_bytes(), LIMIT)
                .map_err(|_| invalid())?,
            );
        let fields = candidate.0.as_object().ok_or_else(invalid)?;
        let proof = receipt.terminal.as_ref().ok_or_else(invalid)?.clone();
        let expected: &[&str] = match proof.outcome {
            Outcome::SocialCode => &["code", "code_verifier", "redirect_uri"],
            Outcome::AwsCode => &[
                "clientId",
                "clientSecret",
                "grantType",
                "redirectUri",
                "code",
                "codeVerifier",
            ],
            _ => return Err(invalid()),
        };
        if fields.len() != expected.len()
            || fields.keys().any(|key| !expected.contains(&key.as_str()))
            || text(fields, "code", 4096)? != proof.code.as_str()
            || Instant::now() >= deadline
        {
            return Err(invalid());
        }
        let (verifier_name, redirect_name) = match proof.outcome {
            Outcome::SocialCode => ("code_verifier", "redirect_uri"),
            Outcome::AwsCode => ("codeVerifier", "redirectUri"),
            _ => return Err(invalid()),
        };
        let verifier = text(fields, verifier_name, 128)?;
        if verifier.len() < 43
            || !verifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
            || URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
                != proof.challenge.as_str()
            || text(fields, redirect_name, 8192)? != proof.redirect.as_str()
        {
            return Err(invalid());
        }
        let client_secret = if proof.outcome == Outcome::AwsCode {
            if text(fields, "clientId", 4096)?
                != proof.client_id.as_ref().ok_or_else(invalid)?.as_str()
                || text(fields, "grantType", 32)? != "authorization_code"
            {
                return Err(invalid());
            }
            Some(Zeroizing::new(
                text(fields, "clientSecret", 8192)?.to_string(),
            ))
        } else {
            None
        };
        Ok(Arc::new(Self {
            receipt,
            proof,
            body: Zeroizing::new(raw.as_bytes().to_vec()),
            verifier: Zeroizing::new(verifier.to_string()),
            client_secret,
            deadline,
            runtime,
            consumed: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    pub(in crate::server::core::login::managed) fn receipt(&self) -> &Arc<Receipt> {
        &self.receipt
    }
    pub(in crate::server::core::login::managed) fn runtime(&self) -> &Arc<RuntimeBinding> {
        &self.runtime
    }
    pub(in crate::server::core::login::managed) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(in crate::server::core::login::managed) fn consumed(&self) -> bool {
        self.consumed.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(in crate::server::core::login::managed) fn consume(&self) -> Result<(), GatewayError> {
        self.consumed
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(|_| invalid())?;
        Ok(())
    }
}

impl KiroExchangeInput {
    pub(in crate::server::core::login::managed) fn new(
        source: KiroCallSource,
        prepared: Arc<Prepared>,
    ) -> Self {
        Self { source, prepared }
    }
    pub(in crate::server::core::login::managed) fn matches_state(&self, state: &State) -> bool {
        self.prepared.consumed() && self.source.matches_state(state, &self.prepared)
    }
    pub(in crate::server::core::login::managed) fn belongs_to_receipt(
        &self,
        receipt: &Receipt,
    ) -> bool {
        std::ptr::eq(self.prepared.receipt.as_ref(), receipt)
    }
    pub(crate) fn source(&self) -> &KiroCallSource {
        &self.source
    }
    pub(crate) fn method(&self) -> &'static str {
        "POST"
    }
    pub(crate) fn endpoint(&self) -> &str {
        self.prepared.proof.endpoint.as_str()
    }
    pub(crate) fn body(&self) -> &[u8] {
        self.prepared.body.as_slice()
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.prepared.deadline
    }
    pub(crate) fn runtime_identity(&self) -> usize {
        self.prepared.runtime.identity
    }
    pub(crate) fn runtime_generation(&self) -> u64 {
        self.prepared.runtime.generation
    }
    pub(crate) fn verifier(&self) -> &str {
        self.prepared.verifier.as_str()
    }
    pub(crate) fn client_secret_candidate(&self) -> Option<&str> {
        self.prepared
            .client_secret
            .as_ref()
            .map(|secret| secret.as_str())
    }
    pub(crate) fn code(&self) -> &str {
        self.prepared.proof.code.as_str()
    }
    pub(crate) fn challenge(&self) -> &str {
        self.prepared.proof.challenge.as_str()
    }
    pub(crate) fn login_provider(&self) -> &str {
        self.prepared.proof.login_provider
    }
    pub(crate) fn authorization_origin(&self) -> &str {
        self.prepared.proof.authorization_origin.as_str()
    }
    pub(crate) fn redirect_uri(&self) -> &str {
        self.prepared.proof.redirect.as_str()
    }
    pub(crate) fn client_id(&self) -> Option<&str> {
        self.prepared.proof.client_id.as_ref().map(|id| id.as_str())
    }
    /// 必须在原Host/Runtime门内调用；内部再进原Session短门并一次CAS，门内不发请求。
    pub(crate) fn consume(self, call: &Call) -> Result<KiroExchangePermit, GatewayError> {
        self.source.with_current(call, || self.prepared.consume())?;
        Ok(KiroExchangePermit { input: self })
    }
}
impl KiroExchangePermit {
    pub(crate) fn input(&self) -> &KiroExchangeInput {
        &self.input
    }
    pub(crate) fn into_input(self) -> KiroExchangeInput {
        self.input
    }
}
