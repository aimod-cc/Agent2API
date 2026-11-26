use std::collections::HashMap;

use base64::Engine;
use rusqlite::Connection;
use serde_json::json;

use crate::server::core::account_store::{AccountStore, ANTIGRAVITY_PROVIDER_ID};
use crate::server::db::Db;
use crate::server::request_stats::{NewRequestEntry, RequestQuery, RequestSource, RequestStats, Retention};

use super::antigravity::{discover_roots, scan_discovered_roots, scan_discovered_roots_with_accounts, DiscoveredRoot};
use super::antigravity_account::{
    detect_root_email, read_email_from_oauth_token_file, read_email_from_vscdb,
    resolve_account_identity, RECENT_ACCOUNT_WINDOW_MS,
};
use super::preview::build_live_preview;
use super::protobuf::{encode_field_bytes, encode_field_varint, strip_gemini_deployment_suffix};

fn mock_root(path: &std::path::Path) -> Vec<DiscoveredRoot> {
    vec![DiscoveredRoot {
        path: path.to_string_lossy().to_string(),
        kind: "custom".to_string(),
        kind_label: "自定义目录".to_string(),
        custom: true,
        exists: true,
    }]
}

fn encode_timestamp_msg(secs: i64, nanos: i64) -> Vec<u8> {
    let mut out = Vec::new();
    encode_field_varint(1, secs as u64, &mut out);
    if nanos > 0 {
        encode_field_varint(2, nanos as u64, &mut out);
    }
    out
}

fn encode_gen_metadata_msg(model: &str, last_step_idx: i64) -> Vec<u8> {
    let mut kv = Vec::new();
    encode_field_bytes(1, b"last_step_index", &mut kv);
    encode_field_bytes(2, last_step_idx.to_string().as_bytes(), &mut kv);

    let mut inner = Vec::new();
    encode_field_bytes(19, model.as_bytes(), &mut inner);
    encode_field_bytes(20, &kv, &mut inner);

    let mut outer = Vec::new();
    encode_field_bytes(1, &inner, &mut outer);
    outer
}

fn encode_step_metadata_msg(
    began_secs: i64,
    first_secs: i64,
    ended_secs: i64,
    uncached: i64,
    output: i64,
    cache_write: i64,
    cache_read: i64,
    response_id: &str,
    gen_idx: i64,
) -> Vec<u8> {
    let mut out = Vec::new();
    encode_field_bytes(1, &encode_timestamp_msg(began_secs, 0), &mut out);
    encode_field_bytes(6, &encode_timestamp_msg(first_secs, 0), &mut out);
    encode_field_bytes(7, &encode_timestamp_msg(ended_secs, 0), &mut out);

    let mut usage = Vec::new();
    encode_field_varint(2, uncached as u64, &mut usage);
    encode_field_varint(3, output as u64, &mut usage);
    encode_field_varint(4, cache_write as u64, &mut usage);
    encode_field_varint(5, cache_read as u64, &mut usage);
    if !response_id.is_empty() {
        encode_field_bytes(11, response_id.as_bytes(), &mut usage);
    }
    encode_field_bytes(9, &usage, &mut out);

    let mut gen_ref = Vec::new();
    encode_field_varint(3, gen_idx as u64, &mut gen_ref);
    encode_field_bytes(20, &gen_ref, &mut out);
    out
}

fn temp_stats() -> (tempfile::TempDir, RequestStats) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("agent2api.db")).unwrap();
    let stats = RequestStats::with_db(Some(db), Retention::default);
    (dir, stats)
}

#[test]
fn strips_gemini_deployment_suffix_only() {
    assert_eq!(strip_gemini_deployment_suffix("gemini-3.8-flash-n"), "gemini-3.8-flash");
    assert_eq!(strip_gemini_deployment_suffix("claude-opus-4-6-thinking"), "claude-opus-4-6-thinking");
    assert_eq!(strip_gemini_deployment_suffix("gemini-2.5-pro"), "gemini-2.5-pro");
}

#[test]
fn desktop_db_scan_collapses_response_id_dedups_gateway_and_previews_without_raw_table() {
    let (_stats_dir, stats) = temp_stats();
    let ag_dir = tempfile::tempdir().unwrap();
    let root = ag_dir.path();
    std::fs::create_dir_all(root.join("conversations")).unwrap();
    let logs_dir = root.join("brain").join("conv-1").join(".system_generated").join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();

    // Summary DB
    let sum_conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
    sum_conn
        .execute_batch(
            "CREATE TABLE conversation_summaries (
                conversation_id TEXT,
                title TEXT,
                preview TEXT,
                workspace_uris TEXT,
                last_modified_time INTEGER
            );
            INSERT INTO conversation_summaries VALUES
                ('conv-1', 'Fix login bug', 'Hello', '[\"file:///d:/Code/demo\"]', 1760000000);",
        )
        .unwrap();

    // Conversation DB with two steps sharing resp-1 plus one separate step resp-2
    let db_path = root.join("conversations").join("conv-1.db");
    let conv_conn = Connection::open(&db_path).unwrap();
    conv_conn
        .execute_batch(
            "CREATE TABLE gen_metadata (idx INTEGER, data BLOB);
             CREATE TABLE steps (idx INTEGER, metadata BLOB);",
        )
        .unwrap();
    let gm0 = encode_gen_metadata_msg("gemini-3.8-flash-n", 0);
    let gm1 = encode_gen_metadata_msg("claude-sonnet-4-6", 2);
    conv_conn
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (0, ?1), (1, ?2)", rusqlite::params![gm0, gm1])
        .unwrap();

    let t0 = 1_760_000_000i64;
    let s1 = encode_step_metadata_msg(t0, t0 + 1, t0 + 5, 100, 50, 10, 200, "resp-1", 0);
    let s2 = encode_step_metadata_msg(t0 + 1, t0 + 2, t0 + 6, 100, 80, 10, 200, "resp-1", 0);
    let s3 = encode_step_metadata_msg(t0 + 300, t0 + 301, t0 + 310, 300, 120, 0, 500, "resp-2", 1);
    conv_conn
        .execute("INSERT INTO steps(idx, metadata) VALUES (1, ?1), (2, ?2), (3, ?3)", rusqlite::params![s1, s2, s3])
        .unwrap();
    drop(conv_conn);
    drop(sum_conn);

    // Matching transcript_full.jsonl for conv-1
    let lines = [
        json!({"step_index": 0, "type": "CORTEX_STEP_TYPE_USER_INPUT", "content": "Help fix login"}),
        json!({"step_index": 1, "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE", "content": "Checking auth.rs", "thinking": "Need to inspect auth"}),
        json!({"step_index": 2, "type": "CORTEX_STEP_TYPE_USER_INPUT", "content": "Looks good, proceed"}),
        json!({"step_index": 3, "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE", "content": "Updated auth handler", "tool_calls": [{"id": "tc1", "name": "edit_file", "arguments": "{\"path\":\"auth.rs\"}"}]}),
    ];
    let transcript_text: String = lines.iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(logs_dir.join("transcript_full.jsonl"), transcript_text).unwrap();

    // Seed a gateway request matching step 3 tokens/model/timestamp to test gateway dedup
    let mut gw = NewRequestEntry::new("claude-sonnet-4-6", 200);
    gw.id = "gw-dup-1".into();
    gw.ts = Some((t0 + 305) * 1000);
    gw.prompt_tokens = 800; // 300 uncached + 500 cache_read
    gw.completion_tokens = 120;
    gw.total_tokens = 920;
    gw.cache_read_tokens = 500;
    gw.provider = Some("workbuddy".into());
    gw.source = RequestSource {
        client: "Antigravity".into(),
        ..Default::default()
    };
    stats.record(gw);

    let roots = mock_root(root);
    let states = stats.local_direct_file_states("antigravity");
    let batch = scan_discovered_roots(roots.clone(), &states, false);
    assert_eq!(batch.items.len(), 2);

    let report = stats.ingest_local_direct(batch.items, &batch.file_updates).unwrap();
    assert_eq!(report.inserted, 1);
    assert_eq!(report.deduped_gateway, 1);

    // Re-running incremental scan does not duplicate anything
    let states_after = stats.local_direct_file_states("antigravity");
    let batch2 = scan_discovered_roots(roots.clone(), &states_after, false);
    assert!(batch2.items.is_empty());

    // Query local_direct logs
    let page = stats.query_requests(&RequestQuery {
        origin: Some("local_direct".into()),
        ..Default::default()
    });
    assert_eq!(page["total"], 2);
    assert_eq!(page["matched"], 1);
    let entry = &page["entries"][0];
    assert_eq!(entry["provider"], "antigravity");
    assert_eq!(entry["model"], "gemini-3.8-flash");
    assert_eq!(entry["promptTokens"], 310); // 100 uncached + 200 cache_read + 10 cache_write
    assert_eq!(entry["completionTokens"], 80); // max collapsed output across resp-1 steps
    assert_eq!(entry["origin"], "local_direct");

    // Verify request_raw table remains empty
    let stats_json = stats.stats();
    assert_eq!(stats_json["downstreamRaw"], 0);

    // Verify live preview reads transcript on demand
    let req_id = entry["id"].as_str().unwrap();
    let reference = stats.local_direct_ref_by_request(None, Some(req_id)).unwrap();
    let preview = build_live_preview(&reference).unwrap();
    assert_eq!(preview["liveLocal"], true);
    let req_body: serde_json::Value =
        serde_json::from_str(preview["requestBody"].as_str().unwrap()).unwrap();
    let resp_body: serde_json::Value =
        serde_json::from_str(preview["responseBody"].as_str().unwrap()).unwrap();
    assert_eq!(req_body["messages"][0]["content"], "Help fix login");
    assert_eq!(resp_body["choices"][0]["message"]["content"], "Checking auth.rs");
    assert_eq!(resp_body["choices"][0]["message"]["reasoning_content"], "Need to inspect auth");

    // Deleting request log keeps file watermark so unchanged db won't resurrect deleted log
    let deleted = stats.clear_requests(
        &RequestQuery {
            origin: Some("local_direct".into()),
            ..Default::default()
        },
        false,
    ).unwrap();
    assert_eq!(deleted.deleted, 1);
    let states_kept = stats.local_direct_file_states("antigravity");
    let batch3 = scan_discovered_roots(roots, &states_kept, false);
    assert!(batch3.items.is_empty());

    // Usage summary supports origin filter ('all', 'gateway', 'local_direct')
    let summary_gw = stats.usage_summary("all", None, None, None, Some("gateway"));
    assert_eq!(summary_gw["overview"]["requests"], 1);
    let summary_ld = stats.usage_summary("all", None, None, None, Some("local_direct"));
    assert_eq!(summary_ld["overview"]["requests"], 0);
}

#[test]
fn agy_cli_transcript_fallback_extracts_tokens_when_db_lacks_usage() {
    let ag_dir = tempfile::tempdir().unwrap();
    let root = ag_dir.path();
    std::fs::create_dir_all(root.join("conversations")).unwrap();
    let logs_dir = root.join("brain").join("cli-1").join(".system_generated").join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();

    // DB without step usage, only gen_metadata model
    let conv_conn = Connection::open(root.join("conversations").join("cli-1.db")).unwrap();
    conv_conn
        .execute_batch(
            "CREATE TABLE gen_metadata (idx INTEGER, data BLOB);
             CREATE TABLE steps (idx INTEGER, metadata BLOB);",
        )
        .unwrap();
    let gm0 = encode_gen_metadata_msg("gemini-2.5-pro", 0);
    conv_conn
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (0, ?1)", [gm0])
        .unwrap();
    drop(conv_conn);

    let line = json!({
        "step_index": 2,
        "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
        "created_at": "2026-11-20T10:00:00Z",
        "content": "Done!",
        "input_tokens": 420,
        "cache_read_tokens": 80,
        "output_tokens": 60
    });
    std::fs::write(logs_dir.join("transcript_full.jsonl"), format!("{line}\n")).unwrap();

    let roots = mock_root(root);
    let batch = scan_discovered_roots(roots, &Default::default(), false);
    assert_eq!(batch.items.len(), 1);
    let item = &batch.items[0];
    assert_eq!(item.entry.model, "gemini-2.5-pro");
    assert_eq!(item.entry.prompt_tokens, 500);
    assert_eq!(item.entry.cache_read_tokens, 80);
    assert_eq!(item.entry.completion_tokens, 60);
    assert_eq!(item.entry.total_tokens, 560);
}

#[test]
fn extracts_email_from_oauth_token_jwt_and_vscdb_sources() {
    let dir = tempfile::tempdir().unwrap();

    // 1. antigravity-oauth-token JWT id_token
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"sub":"123","email":"cli.user@example.com"}"#);
    let jwt = format!("eyJhbGciOiJSUzI1NiJ9.{claims}.sig");
    let tok_path = dir.path().join("antigravity-oauth-token");
    std::fs::write(
        &tok_path,
        json!({ "token": "ignored-secret", "id_token": jwt }).to_string(),
    )
    .unwrap();
    assert_eq!(
        read_email_from_oauth_token_file(&tok_path).as_deref(),
        Some("cli.user@example.com")
    );

    // 2. state.vscdb with nested base64 protobuf userStatus
    let mut inner_pb = Vec::new();
    encode_field_bytes(3, b"desktop.user@example.com", &mut inner_pb);
    encode_field_bytes(7, b"desktop.user@example.com", &mut inner_pb);
    let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner_pb);

    let mut f2_msg = Vec::new();
    encode_field_bytes(1, inner_b64.as_bytes(), &mut f2_msg);

    let mut f1_msg = Vec::new();
    encode_field_bytes(1, b"userStatusSentinelKey", &mut f1_msg);
    encode_field_bytes(2, &f2_msg, &mut f1_msg);

    let mut outer_pb = Vec::new();
    encode_field_bytes(1, &f1_msg, &mut outer_pb);
    let outer_b64 = base64::engine::general_purpose::STANDARD.encode(&outer_pb);

    let vscdb_status = dir.path().join("status.vscdb");
    let conn = Connection::open(&vscdb_status).unwrap();
    conn.execute_batch("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")
        .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        rusqlite::params!["antigravityUnifiedStateSync.userStatus", outer_b64],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        read_email_from_vscdb(&vscdb_status).as_deref(),
        Some("desktop.user@example.com")
    );

    // 3. state.vscdb fallback to antigravityAuthStatus JSON and lastLoginUsername
    let vscdb_auth = dir.path().join("auth.vscdb");
    let conn = Connection::open(&vscdb_auth).unwrap();
    conn.execute_batch("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")
        .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        rusqlite::params!["antigravityAuthStatus", r#"{"email":"legacy@example.com"}"#],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        read_email_from_vscdb(&vscdb_auth).as_deref(),
        Some("legacy@example.com")
    );

    let vscdb_user = dir.path().join("lastlogin.vscdb");
    let conn = Connection::open(&vscdb_user).unwrap();
    conn.execute_batch("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")
        .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        rusqlite::params!["agy.onboarding.lastLoginUsername", "\"quoted@example.com\""],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        read_email_from_vscdb(&vscdb_user).as_deref(),
        Some("quoted@example.com")
    );
}

#[test]
fn resolves_recent_account_with_alias_and_falls_back_for_old_steps() {
    let mut names = HashMap::new();
    names.insert("user@example.com".to_string(), "主力谷歌号".to_string());
    let now = 1_800_000_000_000i64;

    // 近期请求使用邮箱 + 备注名映射
    let (id_recent, name_recent) = resolve_account_identity(
        "desktop",
        Some("User@Example.com"),
        &names,
        now - 60_000,
        now,
    );
    assert_eq!(id_recent, "local-direct:antigravity:user@example.com");
    assert_eq!(name_recent, "主力谷歌号");

    // 无备注映射时直接用邮箱
    let (id_plain, name_plain) = resolve_account_identity(
        "cli",
        Some("other@example.com"),
        &names,
        now - 60_000,
        now,
    );
    assert_eq!(id_plain, "local-direct:antigravity:other@example.com");
    assert_eq!(name_plain, "other@example.com");

    // 超过近期窗口的旧历史不套用当前登录账号，回 fallback
    let (id_old, name_old) = resolve_account_identity(
        "desktop",
        Some("user@example.com"),
        &names,
        now - RECENT_ACCOUNT_WINDOW_MS - 1_000,
        now,
    );
    assert_eq!(id_old, "local-direct:antigravity:desktop");
    assert_eq!(name_old, "桌面版");

    // 验证 AccountStore::antigravity_email_names + scan_discovered_roots_with_accounts 端到端串联
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("agent2api.db")).unwrap();
    let account_json = json!({
        "id": "ag-test-1",
        "provider": ANTIGRAVITY_PROVIDER_ID,
        "name": "测试Antigravity备注",
        "priority": 1,
        "enabled": true,
        "addedAt": 0,
        "pluginPublic": {
            "email": "cli.alias@example.com"
        },
        "refreshToken": "1//dummy-refresh"
    })
    .to_string();
    db.with_mut(|conn| {
        conn.execute(
            "INSERT INTO accounts(id, provider, priority, enabled, added_at, data) \
             VALUES ('ag-test-1', ?1, 1, 1, 0, ?2)",
            rusqlite::params![ANTIGRAVITY_PROVIDER_ID, account_json],
        )
    })
    .unwrap()
    .unwrap();
    let store = AccountStore::with_db(Some(db));

    let cli_root = dir.path().join("antigravity-cli");
    std::fs::create_dir_all(cli_root.join("conversations")).unwrap();
    let logs_dir = cli_root.join("brain").join("cli-acc").join(".system_generated").join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"email":"cli.alias@example.com"}"#);
    std::fs::write(
        cli_root.join("antigravity-oauth-token"),
        json!({ "id_token": format!("h.{claims}.s") }).to_string(),
    )
    .unwrap();
    let recent_iso = chrono::Utc::now().to_rfc3339();
    std::fs::write(
        logs_dir.join("transcript_full.jsonl"),
        format!(
            "{}\n",
            json!({
                "step_index": 1,
                "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
                "created_at": recent_iso,
                "model": "gemini-4-neon",
                "input_tokens": 100,
                "output_tokens": 50
            })
        ),
    )
    .unwrap();

    let roots = vec![DiscoveredRoot {
        path: cli_root.to_string_lossy().to_string(),
        kind: "cli".to_string(),
        kind_label: "Antigravity CLI (agy)".to_string(),
        custom: true,
        exists: true,
    }];
    let account_names = store.antigravity_email_names();
    let batch = scan_discovered_roots_with_accounts(roots, &Default::default(), false, &account_names);
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].entry.provider.as_deref(), Some("antigravity"));
    assert_eq!(batch.items[0].entry.account_id, "local-direct:antigravity:cli.alias@example.com");
    assert_eq!(batch.items[0].entry.account_name, "测试Antigravity备注");
}

#[test]
fn detect_local_antigravity_accounts_smoke() {
    let roots = discover_roots(&[]);
    for root in roots.into_iter().filter(|r| matches!(r.kind.as_str(), "desktop" | "cli")) {
        if let Some(email) = detect_root_email(&root) {
            assert!(email.contains('@'), "root {} email should look valid", root.kind);
        }
    }
}
