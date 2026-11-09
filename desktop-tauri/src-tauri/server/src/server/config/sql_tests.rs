use super::*;
use serde_json::json;

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    schema::migrate(&conn).unwrap();
    conn
}

#[test]
fn saving_settings_removes_deleted_settings_but_preserves_reserved_state() {
    let conn = database();
    for key in schema::RESERVED_KV_KEYS {
        conn.execute("INSERT INTO kv(key,value) VALUES (?1,'\"keep\"')", [key])
            .unwrap();
    }
    conn.execute(
        "INSERT INTO kv(key,value) VALUES ('old-setting','true')",
        [],
    )
    .unwrap();
    let raw = json!({"new-setting":42}).as_object().unwrap().clone();
    write_conn(&conn, &raw).unwrap();
    assert_eq!(read_conn(&conn).unwrap(), raw);
    for key in schema::RESERVED_KV_KEYS {
        let value: String = conn
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "\"keep\"", "{key}");
    }
}

#[test]
fn reserved_key_collision_rolls_back_all_setting_changes() {
    let conn = database();
    conn.execute("INSERT INTO kv(key,value) VALUES ('original','true')", [])
        .unwrap();
    let mut raw = json!({"a-new-setting":42}).as_object().unwrap().clone();
    raw.insert("panelAdmin".into(), json!({"username":"overwrite"}));
    assert!(write_conn(&conn, &raw).is_err());
    assert_eq!(
        read_conn(&conn).unwrap(),
        json!({"original":true}).as_object().unwrap().clone()
    );
    assert!(conn.is_autocommit());
}

#[test]
fn partial_write_failure_does_not_leave_half_saved_configuration() {
    let conn = database();
    conn.execute_batch("INSERT INTO kv(key,value) VALUES ('original','true'); CREATE TRIGGER reject_setting BEFORE INSERT ON kv WHEN NEW.key='b-fail' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    let raw = json!({"a-first":1,"b-fail":2}).as_object().unwrap().clone();
    assert!(write_conn(&conn, &raw).is_err());
    assert_eq!(
        read_conn(&conn).unwrap(),
        json!({"original":true}).as_object().unwrap().clone()
    );
}

#[test]
fn migration_data_and_completion_marker_commit_together() {
    let conn = database();
    conn.execute_batch("INSERT INTO kv(key,value) VALUES ('existing','true'); CREATE TRIGGER reject_marker BEFORE INSERT ON kv WHEN NEW.key='configMigrated' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    let raw = json!({"imported":1}).as_object().unwrap().clone();
    assert!(import_conn(&conn, &raw, KEY_CONFIG_MIGRATED).is_err());
    assert_eq!(
        read_conn(&conn).unwrap(),
        json!({"existing":true}).as_object().unwrap().clone()
    );
    assert!(!marker_present_conn(&conn, KEY_CONFIG_MIGRATED).unwrap());
    conn.execute_batch("DROP TRIGGER reject_marker;").unwrap();
    import_conn(&conn, &raw, KEY_CONFIG_MIGRATED).unwrap();
    assert!(marker_present_conn(&conn, KEY_CONFIG_MIGRATED).unwrap());
    assert_eq!(
        read_conn(&conn).unwrap(),
        json!({"existing":true,"imported":1})
            .as_object()
            .unwrap()
            .clone()
    );
}
