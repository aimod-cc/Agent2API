use super::{
    ApiKeyIdentity, NewRequestEntry, RequestQuery, RequestSource, RequestStats, Retention,
};
use crate::server::db::Db;
use serde_json::{json, Value};

fn temp_stats() -> (tempfile::TempDir, RequestStats) {
    crate::server::logging::process_started_at();
    let dir = tempfile::tempdir().expect("应当能创建临时目录");
    let db = Db::open(&dir.path().join("agent2api.db")).expect("应当能创建临时数据库");
    (dir, RequestStats::with_db(Some(db), Retention::default))
}

fn source(client: &str, key: &str) -> RequestSource {
    RequestSource {
        client: client.into(),
        api_key: ApiKeyIdentity::from_filter(key).expect("合法的密钥筛选标识"),
        ..Default::default()
    }
}

fn entry(id: &str, ts: i64) -> NewRequestEntry {
    let mut entry = NewRequestEntry::new("public-model", 200);
    entry.id = id.into();
    entry.ts = Some(ts);
    entry.provider = Some("test-provider".into());
    entry.upstream_model = "wire-model".into();
    entry.source = source("zcode", "key:a");
    entry.prompt_tokens = 8;
    entry.completion_tokens = 2;
    entry.total_tokens = 10;
    entry
}

fn ids(stats: &RequestStats, query: RequestQuery) -> Vec<String> {
    let page = stats.query_requests(&query);
    let rows = page["entries"].as_array().expect("请求记录应为数组");
    rows.iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect()
}

fn seed() -> (tempfile::TempDir, RequestStats, i64) {
    let (dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    stats.record(entry("ok", now));
    let mut test = entry("test", now);
    test.is_test = true;
    stats.record(test);
    stats.record_started(
        "running",
        now,
        "public-model",
        "public-model",
        "",
        &source("zcode", "key:a"),
        false,
    );
    let mut failed = entry("failed", now);
    failed.error = Some("回答中途断开".into());
    failed.source = source("codex", "env");
    stats.record(failed);
    let mut unknown = entry("unknown", now);
    unknown.source = RequestSource::default();
    unknown.provider = Some("other-provider".into());
    stats.record(unknown);
    (dir, stats, now)
}

#[test]
fn default_query_keeps_test_running_and_completed_rows() {
    let (_dir, stats, _) = seed();
    let page = stats.query_requests(&RequestQuery::default());
    assert_eq!(page["total"], 5);
    assert_eq!(page["matched"], 5);
    assert_eq!(page["running"], 1);
    assert_eq!(
        ids(&stats, RequestQuery::default()),
        ["unknown", "failed", "running", "test", "ok"]
    );
}

#[test]
fn client_filter_distinguishes_missing_identity_from_no_filter() {
    let (_dir, stats, _) = seed();
    for (client, expected) in [
        ("zcode", vec!["running", "test", "ok"]),
        ("codex", vec!["failed"]),
        ("", vec!["unknown"]),
    ] {
        assert_eq!(
            ids(
                &stats,
                RequestQuery {
                    client: Some(client.into()),
                    ..Default::default()
                }
            ),
            expected
        );
    }
}

#[test]
fn api_key_filters_distinguish_managed_environment_anonymous_and_unknown() {
    let (_dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    for (i, key) in ["key:a", "key:b", "env", "none", "unknown"]
        .iter()
        .enumerate()
    {
        let mut item = entry(key, now + i as i64);
        item.source = source("zcode", key);
        stats.record(item);
    }
    for key in ["key:a", "key:b", "env", "none", "unknown"] {
        assert_eq!(
            ids(
                &stats,
                RequestQuery {
                    api_key: Some(key.into()),
                    ..Default::default()
                }
            ),
            [key]
        );
    }
}

#[test]
fn production_and_completed_filters_work_independently_and_together() {
    let (_dir, stats, _) = seed();
    assert_eq!(
        ids(
            &stats,
            RequestQuery {
                production_only: true,
                ..Default::default()
            }
        ),
        ["unknown", "failed", "running", "ok"]
    );
    assert_eq!(
        ids(
            &stats,
            RequestQuery {
                completed_only: true,
                ..Default::default()
            }
        ),
        ["unknown", "failed", "test", "ok"]
    );
    let query = RequestQuery {
        production_only: true,
        completed_only: true,
        ..Default::default()
    };
    let page = stats.query_requests(&query);
    assert_eq!(page["total"], 5);
    assert_eq!(page["matched"], 3);
    assert_eq!(page["running"], 0);
    assert_eq!(ids(&stats, query), ["unknown", "failed", "ok"]);
}

#[test]
fn all_filters_compose_without_losing_the_new_fields() {
    let (_dir, stats, now) = seed();
    let query = RequestQuery {
        offset: 0,
        limit: Some(1),
        model: Some("wire-model".into()),
        provider: Some("test-provider".into()),
        client: Some("zcode".into()),
        api_key: Some("key:a".into()),
        production_only: true,
        completed_only: true,
        status: Some("ok".into()),
        start: Some(now),
        end: Some(now + 5),
    };
    let page = stats.query_requests(&query);
    assert_eq!(page["total"], 5);
    assert_eq!(page["matched"], 1);
    assert_eq!(page["running"], 0);
    assert_eq!(ids(&stats, query), ["ok"]);
}

#[test]
fn success_error_and_running_filters_match_the_displayed_state() {
    let (_dir, stats, _) = seed();
    for (status, expected) in [
        ("ok", vec!["unknown", "test", "ok"]),
        ("error", vec!["failed"]),
        ("running", vec!["running"]),
    ] {
        assert_eq!(
            ids(
                &stats,
                RequestQuery {
                    status: Some(status.into()),
                    ..Default::default()
                }
            ),
            expected
        );
    }
}

#[test]
fn model_filter_matches_both_public_and_upstream_names() {
    let (_dir, stats, _) = seed();
    let query = |model: &str| RequestQuery {
        model: Some(model.into()),
        completed_only: true,
        ..Default::default()
    };
    assert_eq!(
        ids(&stats, query("public-model")),
        ids(&stats, query("wire-model"))
    );
    assert!(ids(&stats, query("unrelated-model")).is_empty());
}

#[test]
fn time_range_is_start_inclusive_and_end_exclusive() {
    let (_dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    for (id, ts) in [
        ("before", now - 3),
        ("start", now - 2),
        ("inside", now - 1),
        ("end", now),
    ] {
        stats.record(entry(id, ts));
    }
    assert_eq!(
        ids(
            &stats,
            RequestQuery {
                start: Some(now - 2),
                end: Some(now),
                ..Default::default()
            }
        ),
        ["inside", "start"]
    );
}

#[test]
fn pagination_is_stable_for_equal_timestamps_and_preserves_counts() {
    let (_dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    for id in ["first", "second", "third"] {
        stats.record(entry(id, now));
    }
    assert_eq!(
        ids(
            &stats,
            RequestQuery {
                offset: 1,
                limit: Some(1),
                ..Default::default()
            }
        ),
        ["second"]
    );
    assert_eq!(
        ids(
            &stats,
            RequestQuery {
                limit: Some(0),
                ..Default::default()
            }
        ),
        ["third"]
    );
    let page = stats.query_requests(&RequestQuery {
        offset: 9,
        ..Default::default()
    });
    assert_eq!(page["entries"], json!([]));
    assert_eq!(page["matched"], 3);
}

#[test]
fn finishing_a_running_request_reuses_the_row_and_is_idempotent() {
    let (_dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    let item = entry("same-request", now);
    stats.record_started(
        &item.id,
        now,
        &item.model,
        &item.model,
        "",
        &item.source,
        false,
    );
    stats.record_started(
        &item.id,
        now,
        &item.model,
        &item.model,
        "",
        &item.source,
        false,
    );
    let started = stats.query_requests(&RequestQuery::default());
    assert_eq!(started["total"], 1);
    assert_eq!(started["running"], 1);
    stats.record(item.clone());
    stats.record(item);
    let page = stats.query_requests(&RequestQuery::default());
    assert_eq!(page["total"], 1);
    assert_eq!(page["running"], 0);
    assert_eq!(page["entries"][0]["totalTokens"], 10);
    let summary = stats.usage_summary("all", None, None);
    assert_eq!(summary["overview"]["requests"], 1);
    assert_eq!(summary["overview"]["tokens"], 10);
}

#[test]
fn model_tests_and_running_requests_do_not_inflate_usage_reports() {
    let (_dir, stats, _) = seed();
    let summary = stats.usage_summary("all", None, None);
    assert_eq!(summary["overview"]["requests"], 3);
    assert_eq!(summary["overview"]["successful"], 2);
    assert_eq!(summary["overview"]["tokens"], 20);
}

#[test]
fn failed_rows_clear_usage_but_keep_failure_and_first_response() {
    let (_dir, stats) = temp_stats();
    let now = crate::server::logging::now_ms();
    for (id, status, error) in [
        ("http-error", 429, None),
        ("stream-error", 200, Some("中途断流".to_string())),
    ] {
        let mut item = entry(id, now);
        item.status = status;
        item.error = error.clone();
        item.cache_read_tokens = 4;
        item.credit = Some(1.5);
        item.first_response_ms = Some(12);
        stats.record(item);
    }
    let page = stats.query_requests(&RequestQuery::default());
    for row in page["entries"].as_array().unwrap() {
        for field in [
            "promptTokens",
            "completionTokens",
            "totalTokens",
            "cacheReadTokens",
        ] {
            assert_eq!(row[field], 0);
        }
        assert!(row["credit"].is_null());
        assert_eq!(row["firstResponseMs"], 12);
    }
    assert_eq!(page["entries"][0]["error"], "中途断流");
}

#[test]
fn missing_database_returns_the_same_empty_query_shape() {
    let stats = RequestStats::with_db(None, Retention::default);
    let page: Value = stats.query_requests(&RequestQuery::default());
    assert_eq!(
        page,
        json!({"entries": [], "total": 0, "matched": 0, "running": 0})
    );
}
