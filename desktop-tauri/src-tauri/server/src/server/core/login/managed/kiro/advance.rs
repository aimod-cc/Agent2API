//! 只在原门户 AWS choice 回调 Call 已消费输入后推进一次，不重绑 listener。

use serde_json::Value;

use super::{invalid, lease, response, unavailable, Call, GatewayError, Outcome, Phase};
use crate::server::core::plugin_host::wasm::manifest::Options;

pub(in crate::server::core::login::managed) fn advance(
    call: &Call,
    params: &Value,
    options: &Options,
) -> Result<Value, GatewayError> {
    if params.as_object().is_none_or(|fields| {
        fields.len() != 2 || !fields.contains_key("leaseId") || !fields.contains_key("expected")
    }) || params["expected"].as_str() != Some(call.expected())
    {
        return Err(invalid());
    }
    let previous = params["leaseId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let declaration = options
        .manifest
        .managed_login
        .as_ref()
        .ok_or_else(unavailable)?;
    declaration
        .profile(
            call.provider(),
            call.method_index(),
            super::policy::KIRO_AWS_PROFILE,
        )
        .ok_or_else(|| GatewayError::with_status(403, "本次 Kiro 登录未声明固定 AWS 回调权限"))?;
    let new_id = super::super::nonce()?;
    let new_state = super::super::nonce()?;
    if new_id == previous || new_state == call.flow() || new_state == new_id {
        return Err(unavailable());
    }
    call.with_kiro_advance(|state, receipt| {
        if receipt.lease != previous
            || receipt.step != call.expected()
            || state.kiro.current.as_deref() != Some(previous)
        {
            return Err(unavailable());
        }
        let receiver = state
            .kiro
            .receiver
            .as_ref()
            .filter(|receiver| !receiver.cancel.is_cancelled())
            .ok_or_else(unavailable)?
            .clone();
        if !std::sync::Weak::ptr_eq(&receipt.receiver, &std::sync::Arc::downgrade(&receiver)) {
            return Err(unavailable());
        }
        let old = state.leases.get(previous).ok_or_else(unavailable)?;
        let binding = old.kiro.as_ref().ok_or_else(unavailable)?;
        if state.leases.len() != 1
            || state.kiro.retired.len() >= 2
            || binding.consumed_state.is_none()
            || binding.consumed_state.as_deref() == Some(call.flow())
        {
            return Err(unavailable());
        }
        if !old.consumed
            || old.queued
            || old.step.as_deref() != Some(call.expected())
            || binding.phase != Phase::Portal
            || binding.accepted != Some(Outcome::PortalAwsChoice)
            || binding.advanced
            || binding.consumed_state.as_deref() == Some(new_state.as_str())
        {
            return Err(unavailable());
        }
        let login_provider = receipt
            .login_provider
            .filter(|provider| matches!(*provider, "builderid" | "enterprise" | "internal"))
            .ok_or_else(unavailable)?;
        if binding.login_provider != Some(login_provider) {
            return Err(unavailable());
        }
        let old_challenge = binding
            .consumed_challenge
            .as_ref()
            .ok_or_else(unavailable)?
            .clone();
        let mut next = lease(&receiver, call.expected(), Phase::Aws, new_state.clone());
        let next_binding = next.kiro.as_mut().ok_or_else(unavailable)?;
        next_binding.advanced = true;
        // 仅保留不可逆 PKCE challenge 至下一页发布时核对，不保留旧verifier/code/state。
        next_binding.consumed_challenge = Some(old_challenge);
        next_binding.next_origin = Some(receipt.next_origin.ok_or_else(unavailable)?);
        next_binding.login_provider = Some(login_provider);
        let reply = response(
            &new_id,
            Phase::Aws,
            receiver.port,
            &new_state,
            call.session.expires_at,
        );
        // 旧租约的state/回调资格已消费；旧标识只进有界撤销列表，不留可重发载荷。
        let old = state.leases.remove(previous).ok_or_else(unavailable)?;
        state.kiro.retired.push(previous.into());
        state.kiro.current = Some(new_id.clone());
        state.callback_step = None;
        // expected仅作为本Call发布围栏，旧浏览器/lease与输入资格从此消失。
        state.step = Some(super::super::Step::preparing(call.expected().to_string()));
        state.leases.insert(new_id.clone(), next);
        drop(old);
        Ok(reply)
    })
}
