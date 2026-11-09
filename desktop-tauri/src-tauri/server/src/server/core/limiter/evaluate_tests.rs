use super::*;
use serde_json::json;

fn source(id: &str, remaining: f64, models: &[&str]) -> QuotaSource {
    serde_json::from_value(json!({
        "id": id, "label": id, "remaining": remaining, "total": 100,
        "used": 100.0 - remaining, "models": models,
    }))
    .unwrap()
}

fn condition(id: &str, models: &[&str]) -> Condition {
    Condition {
        join: Join::Or,
        source_id: id.into(),
        source: None,
        models: models.iter().map(|model| (*model).into()).collect(),
        metric: Metric::Remaining,
        comparison: Comparison::Lte,
        threshold: 10.0,
        window: None,
    }
}

fn facts(sources: Vec<QuotaSource>) -> sources::Facts {
    sources::Facts::from([(
        "account".into(),
        sources::SourceFacts {
            sources,
            ..Default::default()
        },
    )])
}

fn group(conditions: Vec<Condition>) -> Group {
    Group {
        enabled: true,
        action: Action::Skip,
        target: Target::Models,
        conditions,
    }
}

#[test]
fn comparisons_respect_exact_threshold_boundaries() {
    let mut c = condition("daily", &[]);
    for (comparison, expected) in [
        (Comparison::Lt, false),
        (Comparison::Lte, true),
        (Comparison::Gte, true),
    ] {
        c.comparison = comparison;
        assert_eq!(compare(10.0, &c), expected);
    }
}

#[test]
fn missing_nonfinite_expired_and_other_channel_quotas_do_not_block() {
    let account = json!({"id":"account","quotaChannel":"paid"});
    let c = condition("daily", &[]);
    assert!(
        !read(
            &account,
            &c,
            &sources::Facts::new(),
            &counter::Facts::new(),
            1000
        )
        .hit
    );
    for index in 0..4 {
        let mut quota = source("daily", 0.0, &[]);
        match index {
            0 => quota.remaining = Some(f64::NAN),
            1 => quota.ends_at = 1000,
            2 => quota.channel = "free".into(),
            3 => quota.starts_at = 1001,
            _ => unreachable!(),
        }
        let value = read(
            &account,
            &c,
            &facts(vec![quota]),
            &counter::Facts::new(),
            1000,
        );
        assert!(!value.hit, "case={index}");
    }
}

#[test]
fn only_matching_and_branches_contribute_blocked_models() {
    let account = json!({"id":"account"});
    let facts = facts(vec![
        source("a", 0.0, &[]),
        source("b", 90.0, &[]),
        source("c", 0.0, &[]),
    ]);
    let mut second = condition("b", &["model-b"]);
    second.join = Join::And;
    let group = group(vec![
        condition("a", &["model-a"]),
        second,
        condition("c", &["model-c"]),
    ]);
    let result = state(&account, &group, &facts, &counter::Facts::new(), 1000);
    assert!(result.hit);
    assert!(!result.account);
    assert_eq!(result.models, ["model-c"]);
}

#[test]
fn disabled_groups_do_not_block_and_account_targets_block_all_models() {
    let account = json!({"id":"account"});
    let facts = facts(vec![source("daily", 0.0, &[])]);
    let mut group = group(vec![condition("daily", &["model-a"])]);
    group.enabled = false;
    assert!(!state(&account, &group, &facts, &counter::Facts::new(), 1000).hit);
    group.enabled = true;
    group.target = Target::Account;
    assert!(state(&account, &group, &facts, &counter::Facts::new(), 1000).account);
}

#[test]
fn local_counts_from_the_previous_window_do_not_block_the_new_window() {
    let account = json!({"id":"account"});
    let mut c = condition("local", &[]);
    c.metric = Metric::Tokens;
    c.comparison = Comparison::Gte;
    c.window = Some(Window {
        reset: Reset::Fixed,
        seconds: 60,
        ..Default::default()
    });
    let mut count = counter::Count::default();
    count.tokens = 100;
    count.start = 0;
    let counts = counter::Facts::from([(("account".into(), counter::key(&c)), count)]);
    assert!(read(&account, &c, &sources::Facts::new(), &counts, 59_999).hit);
    assert!(!read(&account, &c, &sources::Facts::new(), &counts, 60_000).hit);
}

#[test]
fn invalid_limits_and_out_of_scope_models_are_rejected() {
    let base = json!([{"conditions":[{"sourceId":"local","metric":"tokens","comparison":"gte","threshold":10,"window":{"reset":"fixed","seconds":60}}]}]);
    assert!(groups::normalize_groups(&base).is_ok());
    for threshold in [
        json!(0),
        json!(-1),
        json!(0.5),
        json!(9_007_199_254_740_992u64),
    ] {
        let mut value = base.clone();
        value[0]["conditions"][0]["threshold"] = threshold;
        assert!(groups::normalize_groups(&value).is_err());
    }
    let mut value = base.clone();
    value[0]["conditions"][0]["window"]["seconds"] = json!(59);
    assert!(groups::normalize_groups(&value).is_err());
    value = base;
    value[0]["conditions"][0]["models"] = json!(["outside"]);
    value[0]["conditions"][0]["source"] =
        json!({"id":"local","label":"local","models":["allowed"]});
    assert!(groups::normalize_groups(&value).is_err());
}
