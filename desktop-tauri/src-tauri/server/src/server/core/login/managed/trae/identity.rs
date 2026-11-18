//! 本次真实GetUserInfo后的私有UID见证；完整secret/public/稳定身份必须与本轮完全对应。
//! 不Deserialize witness，不取回调资料/JWT uid/unknown，不以JSON账号代替真实来源。

use std::sync::{Arc, Weak};

use agent2api_provider_protocol::NormalizedAccount;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::super::{Call, RuntimeBinding, Session, State, TraeCallSource};
use super::{
    context::Variant,
    response::{Profile, SecretJson, Tokens},
    worker::Operation,
    ExchangeInput, ReceiverOwner,
};
use crate::server::core::plugin_host::wasm::managed::TraeExecution;
use crate::server::errors::GatewayError;

pub(super) struct Witness {
    source: TraeCallSource,
    session: Weak<Session>,
    runtime: Arc<RuntimeBinding>,
    receiver: Weak<ReceiverOwner>,
    execution: Weak<TraeExecution>,
    operation: Weak<Operation>,
    call: String,
    step: String,
    lease: String,
    flow: String,
    variant: Variant,
    client_id: &'static str,
    uid: String,
    identity: String,
    secret: [u8; 32],
    public: [u8; 32],
}

fn digest(value: &Value) -> [u8; 32] {
    let serialized = Zeroizing::new(value.to_string());
    Sha256::digest(serialized.as_bytes()).into()
}

/// 有限平铺secret兼容当前Trae凭据；私钥只进secret，不进public/授权URL/日志。
pub(super) fn account(
    input: &ExchangeInput,
    tokens: &Tokens,
    profile: &Profile,
) -> Result<SecretJson, GatewayError> {
    let context = &input.context.protocol;
    if !super::response::valid_token(tokens.access.as_str())
        || profile.uid.is_empty()
        || context.device_public.is_empty()
        || context.device_private.is_empty()
    {
        return Err(super::identity_error());
    }
    let variant = context.variant.as_str();
    let identity = format!("trae-{variant}-{}", profile.uid);
    if identity.len() > 512 {
        return Err(super::identity_error());
    }
    let nickname = profile.nickname.as_deref().unwrap_or("");
    let enterprise = profile.enterprise.as_deref().unwrap_or("");
    let name = if nickname.is_empty() {
        format!("Trae {variant}")
    } else {
        nickname.chars().take(100).collect()
    };
    let tail: String = tokens
        .access
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut result = SecretJson::new(json!({"account": {
        "identity": identity, "desktop": false,
        "secret": {
            "variant": variant, "domain": "trae.cn", "apiHost": "https://api.trae.cn",
            "authClientId": context.variant.client_id(), "uid": profile.uid,
            "accessToken": tokens.access.as_str(), "deviceId": context.device_id,
            "machineId": context.machine_id, "devicePublicKey": context.device_public,
            "devicePrivateKey": context.device_private.as_str(), "source": "web"
        },
        "public": {
            "name": name, "uid": profile.uid, "nickname": nickname.chars().take(100).collect::<String>(),
            "enterpriseId": enterprise, "variant": variant, "tokenTail": tail,
            "hasRefreshToken": tokens.refresh.is_some(), "canRefresh": tokens.refresh.is_some(), "hasAccessToken": true,
            "available": true, "desktop": false
        }
    }}));
    if let Some(refresh) = &tokens.refresh {
        result.value_mut()["account"]["secret"]["refreshToken"] = json!(refresh.as_str());
    }
    if let Some(at) = tokens.expires_at {
        if at <= crate::server::logging::now_ms() {
            return Err(super::identity_error());
        }
        result.value_mut()["account"]["secret"]["expiresAt"] = json!(at);
        result.value_mut()["account"]["public"]["expiresAt"] = json!(at);
        result.value_mut()["account"]["public"]["expiresAtMs"] = json!(at);
    }
    if let Some(nickname) = &profile.nickname {
        result.value_mut()["account"]["secret"]["nickname"] = json!(nickname);
    }
    if let Some(enterprise) = &profile.enterprise {
        result.value_mut()["account"]["secret"]["enterpriseId"] = json!(enterprise);
    }
    let account = &result.value()["account"];
    // 显示值不能包含整枚新票据/设备私钥；真实UID异常也不得向公开身份透出secret。
    let public = account["public"].to_string();
    for secret in [
        Some(tokens.access.as_str()),
        tokens.refresh.as_ref().map(|token| token.as_str()),
        Some(context.device_private.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        if public.contains(secret) || identity.contains(secret) {
            return Err(super::identity_error());
        }
    }
    shape(account)?;
    Ok(result)
}

pub(super) fn install(
    call: &Call,
    input: &ExchangeInput,
    operation: &Arc<Operation>,
    execution: &Arc<TraeExecution>,
    profile: &Profile,
    account: &Value,
) -> Result<(), GatewayError> {
    shape(account)?;
    let witness = Witness {
        source: call.trae_call_source()?,
        session: input.context.session.clone(),
        runtime: input.context.runtime.clone(),
        receiver: input.context.receiver.clone(),
        execution: Arc::downgrade(execution),
        operation: Arc::downgrade(operation),
        call: input.call.clone(),
        step: input.step.clone(),
        lease: input.lease.clone(),
        flow: call.flow().to_string(),
        variant: input.context.protocol.variant,
        client_id: input.context.protocol.variant.client_id(),
        uid: profile.uid.clone(),
        identity: account["identity"]
            .as_str()
            .ok_or_else(super::identity_error)?
            .to_string(),
        secret: digest(&account["secret"]),
        public: digest(&account["public"]),
    };
    execution.with_current(call, || {
        let mut state = call
            .session
            .state
            .try_lock()
            .map_err(|_| super::identity_error())?;
        if !super::exchange_current(&state, call, input, operation)
            || operation.gate().load(std::sync::atomic::Ordering::Acquire)
                != (super::EXCHANGE_SENT | super::PROFILE_SENT)
            || !operation.same_execution(execution)
        {
            return Err(super::identity_error());
        }
        state.trae.witness = Some(witness);
        Ok(())
    })
}

fn current(witness: &Witness, state: &State) -> bool {
    witness.source.matches_state(state)
        && super::live(state)
        && witness.runtime.identity != 0
        && witness.client_id == witness.variant.client_id()
        && state.active.as_deref() == Some(witness.call.as_str())
        && state.callback_step.as_deref() == Some(witness.step.as_str())
        && state.trae.claimed
        && state.trae.taken
        && state.trae.context.is_none()
        && state.trae.current.as_deref() == Some(witness.lease.as_str())
        && state.trae.published_step.as_deref() == Some(witness.step.as_str())
        && state
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &witness.runtime))
        && witness.session.upgrade().is_some_and(|session| {
            session.valid()
                && session.provider == "trae"
                && session.method_index == 0
                && session.flow == witness.flow
        })
        && state.trae.receiver.as_ref().is_some_and(|receiver| {
            !receiver.revoked.is_cancelled()
                && Weak::ptr_eq(&witness.receiver, &Arc::downgrade(receiver))
        })
        && state.step.as_ref().is_some_and(|step| {
            step.id == witness.step
                && step.status == "processing"
                && step.lease.as_deref() == Some(witness.lease.as_str())
                && step.browser.is_none()
        })
        && state.leases.get(&witness.lease).is_some_and(|lease| {
            lease.consumed
                && !lease.queued
                && lease.profile
                    == crate::server::core::plugin_host::wasm::managed_policy::TRAE_PROFILE
                && lease.step.as_deref() == Some(witness.step.as_str())
                && lease.official_state.is_none()
        })
        && witness.execution.upgrade().is_some()
        && witness
            .operation
            .upgrade()
            .is_some_and(|operation| operation.matches_state(state))
}

pub(in crate::server::core::login::managed) fn persistable(state: &State) -> bool {
    state
        .trae
        .witness
        .as_ref()
        .is_some_and(|witness| current(witness, state))
}

pub(in crate::server::core::login::managed) fn can_complete(
    call: &Call,
) -> Result<bool, GatewayError> {
    if !call.is_callback() || call.published() || !call.consumed_input() {
        return Ok(false);
    }
    let execution = call.trae_execution()?;
    execution.with_current(call, || {
        let state = call
            .session
            .state
            .try_lock()
            .map_err(|_| super::identity_error())?;
        Ok(state.trae.witness.as_ref().is_some_and(|witness| {
            witness.source.matches(call)
                && Weak::ptr_eq(&witness.session, &Arc::downgrade(&call.session))
                && current(witness, &state)
        }))
    })
}

pub(in crate::server::core::login::managed) fn validate_account(
    call: &Call,
    account: &NormalizedAccount,
) -> Result<(), GatewayError> {
    if !super::applies(call.provider(), call.method_index()) {
        return Ok(());
    }
    let value = SecretJson::new(json!({"secret": account.secret, "public": account.public,
        "identity": account.identity, "desktop": account.desktop}));
    validate_value(call, value.value())
}

pub(super) fn validate_value(call: &Call, account: &Value) -> Result<(), GatewayError> {
    shape(account)?;
    if !call.is_callback() || call.published() || !call.consumed_input() {
        return Err(super::identity_error());
    }
    let execution = call.trae_execution()?;
    execution.with_current(call, || {
        let state = call
            .session
            .state
            .try_lock()
            .map_err(|_| super::identity_error())?;
        let witness = state
            .trae
            .witness
            .as_ref()
            .ok_or_else(super::identity_error)?;
        if !witness.source.matches(call)
            || !current(witness, &state)
            || !Weak::ptr_eq(&witness.session, &Arc::downgrade(&call.session))
            || !Weak::ptr_eq(&witness.execution, &Arc::downgrade(&execution))
            || account["identity"].as_str() != Some(witness.identity.as_str())
            || account["secret"]["uid"].as_str() != Some(witness.uid.as_str())
            || account["public"]["uid"].as_str() != Some(witness.uid.as_str())
            || account["secret"]["variant"].as_str() != Some(witness.variant.as_str())
            || account["public"]["variant"].as_str() != Some(witness.variant.as_str())
            || account["secret"]["authClientId"].as_str() != Some(witness.client_id)
            || digest(&account["secret"]) != witness.secret
            || digest(&account["public"]) != witness.public
        {
            return Err(super::identity_error());
        }
        Ok(())
    })
}

fn shape(account: &Value) -> Result<(), GatewayError> {
    if account.as_object().is_none_or(|fields| {
        fields.len() != 4
            || fields
                .keys()
                .any(|key| !["identity", "secret", "public", "desktop"].contains(&key.as_str()))
    }) || account["desktop"] != false
        || !account["identity"].is_string()
    {
        return Err(super::identity_error());
    }
    let required = [
        "variant",
        "domain",
        "apiHost",
        "authClientId",
        "uid",
        "accessToken",
        "deviceId",
        "machineId",
        "devicePublicKey",
        "devicePrivateKey",
        "source",
    ];
    let optional = ["refreshToken", "expiresAt", "nickname", "enterpriseId"];
    let fields = account["secret"]
        .as_object()
        .ok_or_else(super::identity_error)?;
    if fields.len() > required.len() + optional.len()
        || required.iter().any(|key| !fields.contains_key(*key))
        || fields
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
        || required.iter().any(|key| {
            fields
                .get(*key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
        || fields["domain"] != "trae.cn"
        || fields["apiHost"] != "https://api.trae.cn"
        || fields["source"] != "web"
        || !fields["accessToken"]
            .as_str()
            .is_some_and(super::response::valid_token)
        || fields
            .get("refreshToken")
            .is_some_and(|value| !value.as_str().is_some_and(super::response::valid_token))
    {
        return Err(super::identity_error());
    }
    let public = account["public"]
        .as_object()
        .ok_or_else(super::identity_error)?;
    let public_fields = [
        "name",
        "uid",
        "nickname",
        "enterpriseId",
        "variant",
        "tokenTail",
        "hasRefreshToken",
        "canRefresh",
        "hasAccessToken",
        "available",
        "desktop",
        "expiresAt",
        "expiresAtMs",
    ];
    if public.len() > public_fields.len()
        || public
            .keys()
            .any(|key| !public_fields.contains(&key.as_str()))
        || Zeroizing::new(account["secret"].to_string()).len() > 64 * 1024
        || account["public"].to_string().len() > 8 * 1024
    {
        return Err(super::identity_error());
    }
    Ok(())
}
