//! Kiro 专用 URL/参数校验。每阶段 host/path/state 来自原租约，不从回调反造来源。

use std::collections::BTreeMap;

use super::{invalid, unavailable, CallbackFacts, Lease, Outcome, Phase, Publication};
use crate::server::core::plugin_host::wasm::managed_policy as policy;
use crate::server::errors::GatewayError;

fn parse(raw: &str) -> Result<url::Url, GatewayError> {
    if raw.is_empty()
        || raw.len() > 8192
        || raw.contains('\\')
        || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(invalid());
    }
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(invalid());
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(invalid());
    }
    Ok(url)
}

fn query(url: &url::Url) -> Result<BTreeMap<String, String>, GatewayError> {
    if url.query().is_none() {
        return Err(invalid());
    }
    let mut fields = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if fields.len() >= 32
            || key.is_empty()
            || key.len() > 128
            || value.len() > 8192
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
            || key.contains('\u{fffd}')
            || value.contains('\u{fffd}')
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(fields)
}

fn required<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &str,
    max: usize,
) -> Result<&'a str, GatewayError> {
    fields
        .get(key)
        .map(String::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= max
                && !value
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace())
        })
        .ok_or_else(invalid)
}

fn allowed(fields: &BTreeMap<String, String>, names: &[&str]) -> Result<(), GatewayError> {
    if fields.keys().any(|key| !names.contains(&key.as_str())) {
        Err(invalid())
    } else {
        Ok(())
    }
}

pub(super) fn authorization(
    raw: &str,
    phase: Phase,
    port: u16,
    state: &str,
) -> Result<Publication, GatewayError> {
    let url = parse(raw)?;
    let origin = url.origin().ascii_serialization();
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || match phase {
            Phase::Portal => origin != policy::KIRO_PORTAL_ORIGIN,
            Phase::Aws => !policy::KIRO_AWS_ORIGINS.contains(&origin.as_str()),
        }
        || url.path()
            != match phase {
                Phase::Portal => "/signin",
                Phase::Aws => "/authorize",
            }
        || raw.split('?').next() != Some(format!("{origin}{}", url.path()).as_str())
    {
        return Err(invalid());
    }
    let fields = query(&url)?;
    if required(&fields, "state", 4096)? != state
        || required(&fields, "redirect_uri", 8192)? != phase.redirect(port)
        || required(&fields, "code_challenge_method", 32)? != "S256"
    {
        return Err(invalid());
    }
    let challenge = required(&fields, "code_challenge", 43)?;
    if challenge.len() != 43
        || !challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid());
    }
    let client_id = match phase {
        Phase::Portal => {
            allowed(
                &fields,
                &[
                    "state",
                    "redirect_uri",
                    "code_challenge",
                    "code_challenge_method",
                    "redirect_from",
                ],
            )?;
            if required(&fields, "redirect_from", 32)? != "KiroIDE" {
                return Err(invalid());
            }
            None
        }
        Phase::Aws => {
            allowed(
                &fields,
                &[
                    "response_type",
                    "client_id",
                    "redirect_uri",
                    "scopes",
                    "state",
                    "code_challenge",
                    "code_challenge_method",
                ],
            )?;
            if required(&fields, "response_type", 16)? != "code" {
                return Err(invalid());
            }
            let client_id = required(&fields, "client_id", 4096)?;
            required(&fields, "scopes", 8192)?;
            Some(zeroize::Zeroizing::new(client_id.to_string()))
        }
    };
    Ok(Publication {
        origin,
        challenge: zeroize::Zeroizing::new(challenge.to_string()),
        client_id,
    })
}

pub(super) fn callback(
    lease: &Lease,
    step: &str,
    raw: &str,
) -> Result<(Outcome, Option<&'static str>), GatewayError> {
    let facts = callback_facts(lease, step, raw)?;
    Ok((facts.outcome, facts.next_origin))
}

pub(super) fn callback_facts(
    lease: &Lease,
    step: &str,
    raw: &str,
) -> Result<CallbackFacts, GatewayError> {
    let binding = lease.kiro.as_ref().ok_or_else(invalid)?;
    let receiver = binding
        .receiver
        .upgrade()
        .filter(|receiver| !receiver.cancel.is_cancelled())
        .ok_or_else(unavailable)?;
    if lease.closed() || lease.step.as_deref() != Some(step) || binding.publication.is_none() {
        return Err(unavailable());
    }
    let url = parse(raw)?;
    let callbacks = binding.phase.callbacks(receiver.port);
    if !raw
        .split('?')
        .next()
        .is_some_and(|base| callbacks.iter().any(|callback| callback == base))
        || url.scheme() != "http"
        || url.port() != Some(receiver.port)
    {
        return Err(invalid());
    }
    let fields = query(&url)?;
    if Some(required(&fields, "state", 4096)?) != binding.oauth_state.as_deref()
        || binding.oauth_state != lease.official_state
    {
        return Err(invalid());
    }
    if fields.contains_key("error") {
        allowed(
            &fields,
            &[
                "state",
                "error",
                "error_description",
                "error_uri",
                "login_option",
            ],
        )?;
        required(&fields, "error", 256)?;
        if binding.phase == Phase::Aws && fields.contains_key("login_option") {
            return Err(invalid());
        }
        if let Some(option) = fields.get("login_option") {
            if !["google", "github", "builderid", "awsidc", "internal"].contains(&option.as_str()) {
                return Err(invalid());
            }
        }
        return Ok(CallbackFacts {
            outcome: Outcome::Refused,
            next_origin: None,
            login_provider: None,
            terminal: None,
        });
    }
    match binding.phase {
        Phase::Aws => {
            allowed(&fields, &["state", "code"])?;
            let code = required(&fields, "code", 4096)?;
            let publication = binding.publication.as_ref().ok_or_else(unavailable)?;
            let login_provider = binding.login_provider.ok_or_else(unavailable)?;
            let terminal = super::exchange::TerminalProof::from_aws(
                publication,
                receiver.port,
                code,
                login_provider,
            )?;
            Ok(CallbackFacts {
                outcome: Outcome::AwsCode,
                next_origin: None,
                login_provider: Some(login_provider),
                terminal: Some(terminal),
            })
        }
        Phase::Portal => match required(&fields, "login_option", 32)? {
            "google" | "github" => {
                allowed(&fields, &["state", "login_option", "code"])?;
                let code = required(&fields, "code", 4096)?;
                let option = required(&fields, "login_option", 32)?;
                let login_provider = if option == "google" {
                    "google"
                } else {
                    "github"
                };
                let publication = binding.publication.as_ref().ok_or_else(unavailable)?;
                let terminal = super::exchange::TerminalProof::from_social(
                    publication,
                    receiver.port,
                    url.path(),
                    option,
                    code,
                )?;
                Ok(CallbackFacts {
                    outcome: Outcome::SocialCode,
                    next_origin: None,
                    login_provider: Some(login_provider),
                    terminal: Some(terminal),
                })
            }
            "builderid" | "awsidc" | "internal" => {
                allowed(
                    &fields,
                    &["state", "login_option", "issuer_url", "idc_region"],
                )?;
                let region = required(&fields, "idc_region", 32)?;
                if crate::server::core::plugin_host::wasm::aws_oidc::origin(region).is_none() {
                    return Err(invalid());
                }
                let issuer = parse(required(&fields, "issuer_url", 4096)?)?;
                if issuer.scheme() != "https"
                    || issuer.host_str().is_none()
                    || issuer.port_or_known_default() != Some(443)
                    || issuer.query().is_some()
                {
                    return Err(invalid());
                }
                let next_origin = crate::server::core::plugin_host::wasm::aws_oidc::origin(region)
                    .ok_or_else(invalid)?;
                let login_provider = match required(&fields, "login_option", 32)? {
                    "builderid" => "builderid",
                    "awsidc" => "enterprise",
                    "internal" => "internal",
                    _ => return Err(invalid()),
                };
                Ok(CallbackFacts {
                    outcome: Outcome::PortalAwsChoice,
                    next_origin: Some(next_origin),
                    login_provider: Some(login_provider),
                    terminal: None,
                })
            }
            _ => Err(invalid()),
        },
    }
}
