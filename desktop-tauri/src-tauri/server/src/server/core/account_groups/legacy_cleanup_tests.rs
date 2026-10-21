use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;
use serde_json::{json, Value};

use super::legacy::disable_legacy_group_keys_once;

static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent2api-account-groups-legacy-{label}-{}-{sequence}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn connection(&self) -> Connection {
        let connection = Connection::open(&self.0).expect("临时库应当能打开");
        connection
            .execute_batch("CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);")
            .expect("kv 表应当能创建");
        connection
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", self.0.display(), suffix));
        }
    }
}

fn set_api_keys(connection: &Connection, entries: &Value) {
    connection
        .execute(
            "INSERT INTO kv(key, value) VALUES ('apiKeys', ?1)",
            [entries.to_string()],
        )
        .expect("apiKeys 应当能写入");
}

fn api_keys(connection: &Connection) -> Value {
    let text: String = connection
        .query_row("SELECT value FROM kv WHERE key = 'apiKeys'", [], |row| {
            row.get(0)
        })
        .expect("apiKeys 应当存在");
    serde_json::from_str(&text).expect("apiKeys 应当是合法 JSON")
}

fn marker_exists(connection: &Connection) -> bool {
    connection
        .query_row(
            "SELECT COUNT(*) FROM kv WHERE key = 'accountGroupsLegacyCleared'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

fn without_enabled(entries: &Value) -> Value {
    let mut copy = entries.clone();
    for item in copy.as_array_mut().into_iter().flatten() {
        if let Some(object) = item.as_object_mut() {
            object.remove("enabled");
        }
    }
    copy
}

#[test]
fn disables_only_enabled_legacy_group_entries_and_preserves_all_other_fields() {
    let db = TempDb::new("fields");
    let connection = db.connection();
    let original = json!([
        {
            "id": "group-active",
            "name": "组 Key",
            "key": "enc1:opaque-group",
            "enabled": true,
            "keyUnreadable": true,
            "routeMode": "account_group",
            "accountGroupId": "old-group"
        },
        {
            "id": "group-missing",
            "name": "缺省启用",
            "key": "plain-group-key",
            "routeMode": "account_group",
            "accountGroupId": "old-group"
        },
        {
            "id": "group-disabled",
            "name": "已经停用",
            "key": "enc1:already-disabled",
            "enabled": false,
            "routeMode": "account_group",
            "accountGroupId": "old-group"
        },
        {
            "id": "normal",
            "name": "普通 Key",
            "key": "enc1:normal",
            "enabled": true,
            "allowedProviders": ["workbuddy"]
        }
    ]);
    set_api_keys(&connection, &original);

    assert!(disable_legacy_group_keys_once(&connection).unwrap());
    let after = api_keys(&connection);
    assert_eq!(after[0]["enabled"], false);
    assert_eq!(after[1]["enabled"], false);
    assert_eq!(after[2]["enabled"], false);
    assert_eq!(after[3]["enabled"], true);
    assert_eq!(without_enabled(&after), without_enabled(&original));
    assert!(marker_exists(&connection));
    assert!(!disable_legacy_group_keys_once(&connection).unwrap());
    assert_eq!(api_keys(&connection), after);
}

#[test]
fn missing_api_keys_writes_marker_without_creating_an_empty_array() {
    let db = TempDb::new("missing");
    let connection = db.connection();
    connection
        .execute(
            "INSERT INTO kv(key, value) VALUES ('apiKey', '\"legacy-single-key\"')",
            [],
        )
        .unwrap();

    assert!(!disable_legacy_group_keys_once(&connection).unwrap());
    assert!(marker_exists(&connection));
    let api_keys_row: i64 = connection
        .query_row("SELECT COUNT(*) FROM kv WHERE key = 'apiKeys'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(api_keys_row, 0);
    let legacy: String = connection
        .query_row("SELECT value FROM kv WHERE key = 'apiKey'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(legacy, "\"legacy-single-key\"");
}

#[test]
fn failure_in_marker_write_rolls_back_key_update() {
    let db = TempDb::new("marker-failure");
    let connection = db.connection();
    let original = json!([{
        "id": "group-active",
        "key": "enc1:opaque",
        "routeMode": "account_group"
    }]);
    set_api_keys(&connection, &original);
    connection
        .execute_batch(
            "CREATE TRIGGER block_marker BEFORE INSERT ON kv
             WHEN NEW.key = 'accountGroupsLegacyCleared'
             BEGIN SELECT RAISE(ABORT, 'marker blocked'); END;",
        )
        .unwrap();

    assert!(disable_legacy_group_keys_once(&connection).is_err());
    assert_eq!(api_keys(&connection), original);
    assert!(!marker_exists(&connection));
}

#[test]
fn failure_in_key_update_rolls_back_marker_and_key() {
    let db = TempDb::new("key-failure");
    let connection = db.connection();
    let original = json!([{
        "id": "group-active",
        "key": "enc1:opaque",
        "routeMode": "account_group"
    }]);
    set_api_keys(&connection, &original);
    connection
        .execute_batch(
            "CREATE TRIGGER block_api_keys BEFORE UPDATE ON kv
             WHEN OLD.key = 'apiKeys'
             BEGIN SELECT RAISE(ABORT, 'apiKeys blocked'); END;",
        )
        .unwrap();

    assert!(disable_legacy_group_keys_once(&connection).is_err());
    assert_eq!(api_keys(&connection), original);
    assert!(!marker_exists(&connection));
}

#[test]
fn only_enabled_legacy_key_is_disabled_without_special_case() {
    let db = TempDb::new("only-key");
    let connection = db.connection();
    let original = json!([{
        "id": "only-group-key",
        "key": "enc1:only-ciphertext",
        "enabled": true,
        "routeMode": "account_group",
        "accountGroupId": "dropped-group"
    }]);
    set_api_keys(&connection, &original);

    assert!(disable_legacy_group_keys_once(&connection).unwrap());
    let after = api_keys(&connection);
    assert_eq!(after[0]["enabled"], false);
    assert_eq!(without_enabled(&after), without_enabled(&original));
}

#[test]
fn marker_is_registered_as_reserved_configuration() {
    assert!(crate::server::db::schema::is_reserved(
        "accountGroupsLegacyCleared"
    ));
}
