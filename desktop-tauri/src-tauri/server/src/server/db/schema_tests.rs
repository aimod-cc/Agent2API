use super::*;

fn version(conn: &Connection) -> i64 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn table_names(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn every_supported_schema_version_can_upgrade_without_losing_accounts() {
    for old in 1..=SCHEMA_VERSION {
        let conn = Connection::open_in_memory().unwrap();
        let mut cleaned = 0;
        for version in 1..=old {
            apply_version(&conn, version, &mut cleaned).unwrap();
        }
        conn.pragma_update(None, "user_version", old).unwrap();
        let data = r#"{"id":"kept","accessToken":"secret","future":{"keep":true}}"#;
        conn.execute(
            "INSERT INTO accounts(id,provider,priority,enabled,added_at,data) VALUES ('kept','workbuddy',7,1,0,?1)",
            [data],
        ).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION, "from={old}");
        let stored: String = conn
            .query_row("SELECT data FROM accounts WHERE id='kept'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, data, "from={old}");
        let tables = table_names(&conn);
        migrate(&conn).unwrap();
        assert_eq!(table_names(&conn), tables, "二次升级应为空操作：from={old}");
    }
}

#[test]
fn newer_database_is_rejected_before_any_schema_or_data_change() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE future_data(value TEXT); INSERT INTO future_data VALUES ('keep');",
    )
    .unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    let before = table_names(&conn);
    assert!(migrate(&conn).is_err());
    assert_eq!(version(&conn), SCHEMA_VERSION + 1);
    assert_eq!(table_names(&conn), before);
    assert_eq!(
        conn.query_row("SELECT value FROM future_data", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}

#[test]
fn migration_failure_does_not_advance_the_version_and_can_be_retried() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(V1_SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute_batch("ALTER TABLE requests RENAME TO held_requests;")
        .unwrap();
    assert!(migrate(&conn).is_err());
    assert_eq!(version(&conn), 1);
    assert!(conn.is_autocommit());
    conn.execute_batch("ALTER TABLE held_requests RENAME TO requests;")
        .unwrap();
    migrate(&conn).unwrap();
    assert_eq!(version(&conn), SCHEMA_VERSION);
}

#[test]
fn v36_migrates_local_direct_antigravity_requests_and_daily_provider_stats() {
    let conn = Connection::open_in_memory().unwrap();
    let mut cleaned = 0;
    for v in 1..=35 {
        apply_version(&conn, v, &mut cleaned).unwrap();
    }
    conn.pragma_update(None, "user_version", 35).unwrap();

    conn.execute(
        "INSERT INTO requests (id, ts, model, account_id, account_name, status, provider, client, origin) \
         VALUES ('req-ld-1', 1760000000000, 'gemini-4-neon', 'local-direct', '本机直连', 200, 'local-direct', 'Antigravity', 'local_direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO local_direct_refs (request_id, agent_id, root_kind, source_key, ts) \
         VALUES ('req-ld-1', 'antigravity', 'desktop', 'k-1', 1760000000000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO requests (id, ts, model, account_id, account_name, status, provider, client, origin) \
         VALUES ('req-ld-2', 1760000001000, 'gemini-4-neon', 'local-direct:antigravity:custom@example.com', '我的号', 200, 'local-direct', 'Antigravity', 'local_direct')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO local_direct_refs (request_id, agent_id, root_kind, source_key, ts) \
         VALUES ('req-ld-2', 'antigravity', 'cli', 'k-2', 1760000001000)",
        [],
    )
    .unwrap();

    let daily_stats = serde_json::json!([
        {"provider": "antigravity", "requests": 2, "successful": 2, "tokens": 100, "costUsd": 0.1, "unpricedRequests": 0},
        {"provider": "local-direct", "requests": 3, "successful": 3, "tokens": 250, "costUsd": 0.25, "unpricedRequests": 1}
    ])
    .to_string();
    conn.execute(
        "INSERT INTO request_daily (date, requests, successful, tokens, provider_stats) \
         VALUES ('2026-11-26', 5, 5, 350, ?1)",
        [daily_stats],
    )
    .unwrap();

    migrate(&conn).unwrap();
    assert_eq!(version(&conn), SCHEMA_VERSION);

    let (p1, aid1, aname1): (String, String, String) = conn
        .query_row(
            "SELECT provider, account_id, account_name FROM requests WHERE id='req-ld-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(p1, "antigravity");
    assert_eq!(aid1, "local-direct:antigravity:desktop");
    assert_eq!(aname1, "桌面版");

    let (p2, aid2, aname2): (String, String, String) = conn
        .query_row(
            "SELECT provider, account_id, account_name FROM requests WHERE id='req-ld-2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(p2, "antigravity");
    assert_eq!(aid2, "local-direct:antigravity:custom@example.com");
    assert_eq!(aname2, "我的号");

    let merged_raw: String = conn
        .query_row(
            "SELECT provider_stats FROM request_daily WHERE date='2026-11-26'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let merged: Vec<serde_json::Value> = serde_json::from_str(&merged_raw).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["provider"], "antigravity");
    assert_eq!(merged[0]["requests"], 5);
    assert_eq!(merged[0]["successful"], 5);
    assert_eq!(merged[0]["tokens"], 350);
    assert_eq!(merged[0]["unpricedRequests"], 1);
}
