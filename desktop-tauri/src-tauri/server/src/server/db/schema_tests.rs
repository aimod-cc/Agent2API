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
