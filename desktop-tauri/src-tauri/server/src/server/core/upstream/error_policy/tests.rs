use super::*;
use crate::server::config::error_policy::{ErrorPolicySnapshot, ErrorSettings};

fn snapshot(enabled: bool) -> ErrorPolicySnapshot {
    let mut settings = ErrorSettings::default();
    settings.rules.iter_mut().find(|rule| rule.kind == ErrorKind::Transient).unwrap().retry_unconfirmed = enabled;
    ErrorPolicySnapshot { settings, configured: true, legacy: false }
}

fn uncertain(ordinary: bool) -> GatewayError {
    let mut error = GatewayError::with_status(502, "connection closed before message completed")
        .with_code("upstream_submission_unconfirmed");
    error.unconfirmed_inference = ordinary;
    error
}

#[test]
fn unconfirmed_inference_is_opt_in_and_requires_host_evidence() {
    for enabled in [false, true] {
        for ordinary in [false, true] {
            let decision = for_error_with(&snapshot(enabled), "custom-test", "test-model", &uncertain(ordinary), false);
            assert_eq!(decision.retries(), enabled && ordinary);
            assert_eq!(decision.hard_stop, !(enabled && ordinary));
            assert_eq!(decision.unconfirmed_replay, enabled && ordinary);
            assert_eq!(decision.rule.action, ErrorAction::RetryThenSwitch);
            assert_eq!(decision.stop_reason.is_some(), !(enabled && ordinary));
        }
    }
}

#[test]
fn provider_model_and_business_code_conditions_still_apply() {
    let mut policy = snapshot(false);
    let mut rule = policy.settings.rules.iter().find(|rule| rule.kind == ErrorKind::Transient).unwrap().clone();
    rule.id = "transport-opt-in".into(); rule.provider = "custom-test".into(); rule.model = "test-model".into();
    rule.retry_unconfirmed = true;
    policy.settings.overrides.push(rule);
    assert!(for_error_with(&policy, "custom-test", "test-model", &uncertain(true), false).retries());
    assert!(!for_error_with(&policy, "custom-other", "test-model", &uncertain(true), false).retries());
    assert!(!for_error_with(&policy, "custom-test", "other-model", &uncertain(true), false).retries());
    policy.settings.overrides[0].code = "502".into();
    assert!(!for_error_with(&policy, "custom-test", "test-model", &uncertain(true), false).retries());
}

#[test]
fn unrelated_stops_and_explicit_stop_are_never_relaxed() {
    let policy = snapshot(true);
    assert!(for_error_with(&policy, "custom-test", "test-model", &uncertain(true), true).hard_stop);
    for code in ["request_replay_blocked", "cancelled", "permission_denied", "runtime_stream_interrupted", "network_cleanup_unconfirmed"] {
        let mut error = uncertain(true); error.code = Some(code.into());
        assert!(for_error_with(&policy, "custom-test", "test-model", &error, false).hard_stop, "{code}");
    }
    let remote = json!({"code": "upstream_submission_unconfirmed"});
    let class = UpstreamErrorClass::Fatal { status: 502, message: "remote".into(), upstream_code: None };
    assert!(decide_with(&policy, "custom-test", "test-model", &class, 502, &remote, false, false).hard_stop);
}

#[test]
fn existing_settings_default_to_no_unconfirmed_replay() {
    let mut value = serde_json::to_value(ErrorSettings::default()).unwrap();
    for rule in value["rules"].as_array_mut().unwrap() { rule.as_object_mut().unwrap().remove("retryUnconfirmed"); }
    let settings: ErrorSettings = serde_json::from_value(value).unwrap();
    settings.validate().unwrap();
    assert!(settings.rules.iter().all(|rule| !rule.retry_unconfirmed));
    let mut invalid = settings;
    invalid.rules.iter_mut().find(|rule| rule.kind == ErrorKind::Auth).unwrap().retry_unconfirmed = true;
    assert!(invalid.validate().is_err());
}
