use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;

use crate::server::db::schema;

static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

struct TempPath(PathBuf);

impl TempPath {
    fn new(label: &str) -> Self {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent2api-account-groups-{label}-{}-{sequence}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn open(&self) -> Connection {
        let connection = Connection::open(&self.0).expect("临时迁移库应当能打开");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("应当能启用外键");
        connection
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", self.0.display(), suffix));
        }
    }
}

fn create_base(connection: &Connection, with_assistant_sessions: bool) {
    let assistant = if with_assistant_sessions {
        "CREATE TABLE assistant_sessions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            messages TEXT NOT NULL DEFAULT '[]',
            model TEXT,
            reasoning TEXT,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO assistant_sessions(id, title, messages, updated_at)
        VALUES ('keep-session', '保留', '[{\"role\":\"user\",\"content\":\"hi\"}]', 9);"
    } else {
        ""
    };
    connection
        .execute_batch(&format!(
            "CREATE TABLE accounts (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                priority INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                added_at INTEGER NOT NULL DEFAULT 0,
                data TEXT NOT NULL
            );
            CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO accounts(id, provider, priority, enabled, added_at, data)
            VALUES ('legacy-account', 'workbuddy', 1, 1, 7, 'enc1:opaque-account');
            INSERT INTO kv(key, value) VALUES ('locale', '\"zh-CN\"');
            {assistant}
            PRAGMA user_version = 16;"
        ))
        .expect("v16 夹具应当能创建");
}

fn create_old_group_tables(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE account_groups (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE account_group_members (
                group_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                member_type TEXT NOT NULL,
                member_id TEXT NOT NULL,
                PRIMARY KEY (group_id, position),
                UNIQUE (group_id, member_type, member_id),
                FOREIGN KEY (group_id) REFERENCES account_groups(id) ON DELETE CASCADE
            );
            CREATE INDEX idx_account_group_members_lookup
              ON account_group_members(group_id, position);
            INSERT INTO account_groups(id, name, name_key, created_at, updated_at)
            VALUES ('old-group', '旧组', '旧组', 1, 1);
            INSERT INTO account_group_members(group_id, position, member_type, member_id)
            VALUES ('old-group', 0, 'provider', 'workbuddy');",
        )
        .expect("旧组夹具应当能创建");
}

fn user_version(connection: &Connection) -> i64 {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("应当能读取 user_version")
}

fn object_exists(connection: &Connection, kind: &str, name: &str) -> bool {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            rusqlite::params![kind, name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

fn table_count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn assert_preserved_business_data(connection: &Connection) {
    assert_eq!(
        connection
            .query_row(
                "SELECT data FROM accounts WHERE id = 'legacy-account'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "enc1:opaque-account"
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM kv WHERE key = 'locale'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "\"zh-CN\""
    );
}

#[test]
fn beta_v16_adds_empty_groups_and_keeps_assistant_sessions() {
    let path = TempPath::new("beta-v16");
    let connection = path.open();
    create_base(&connection, true);

    schema::migrate(&connection).expect("beta v16 应能升级到 v17");
    assert_eq!(user_version(&connection), schema::SCHEMA_VERSION);
    assert!(object_exists(&connection, "table", "assistant_sessions"));
    assert_eq!(table_count(&connection, "assistant_sessions"), 1);
    assert!(object_exists(&connection, "table", "account_groups"));
    assert!(object_exists(&connection, "table", "account_group_members"));
    assert!(object_exists(
        &connection,
        "index",
        "idx_account_group_members_lookup"
    ));
    assert_eq!(table_count(&connection, "account_groups"), 0);
    assert_eq!(table_count(&connection, "account_group_members"), 0);
    assert_preserved_business_data(&connection);

    schema::migrate(&connection).expect("重复迁移必须幂等");
    assert_eq!(user_version(&connection), schema::SCHEMA_VERSION);
    assert_eq!(table_count(&connection, "assistant_sessions"), 1);
}

#[test]
fn old_v16_drops_old_group_rows_and_creates_beta_assistant_sessions() {
    let path = TempPath::new("old-v16");
    let connection = path.open();
    create_base(&connection, false);
    create_old_group_tables(&connection);

    schema::migrate(&connection).expect("旧账号组 v16 应能升级到 v17");
    assert_eq!(user_version(&connection), schema::SCHEMA_VERSION);
    assert!(object_exists(&connection, "table", "assistant_sessions"));
    assert_eq!(table_count(&connection, "account_groups"), 0);
    assert_eq!(table_count(&connection, "account_group_members"), 0);
    assert_preserved_business_data(&connection);
}

#[test]
fn v17_failure_rolls_back_groups_sessions_and_version() {
    let beta = TempPath::new("rollback-beta");
    let beta_connection = beta.open();
    create_base(&beta_connection, true);
    beta_connection
        .execute(
            "CREATE VIEW idx_account_group_members_lookup AS SELECT 1 AS value",
            [],
        )
        .unwrap();
    assert!(schema::migrate(&beta_connection).is_err());
    assert_eq!(user_version(&beta_connection), 16);
    assert_eq!(table_count(&beta_connection, "assistant_sessions"), 1);
    assert!(!object_exists(&beta_connection, "table", "account_groups"));
    beta_connection
        .execute("DROP VIEW idx_account_group_members_lookup", [])
        .unwrap();
    schema::migrate(&beta_connection).expect("移除冲突后重试应当成功");
    assert_eq!(user_version(&beta_connection), schema::SCHEMA_VERSION);

    let old = TempPath::new("rollback-old");
    let old_connection = old.open();
    create_base(&old_connection, false);
    create_old_group_tables(&old_connection);
    old_connection
        .execute("DROP INDEX idx_account_group_members_lookup", [])
        .unwrap();
    old_connection
        .execute(
            "CREATE VIEW idx_account_group_members_lookup AS SELECT 1 AS value",
            [],
        )
        .unwrap();
    assert!(schema::migrate(&old_connection).is_err());
    assert_eq!(user_version(&old_connection), 16);
    assert_eq!(table_count(&old_connection, "account_groups"), 1);
    assert_eq!(table_count(&old_connection, "account_group_members"), 1);
    assert!(!object_exists(
        &old_connection,
        "table",
        "assistant_sessions"
    ));
    old_connection
        .execute("DROP VIEW idx_account_group_members_lookup", [])
        .unwrap();
    schema::migrate(&old_connection).expect("移除冲突后重试应当成功");
    assert_eq!(user_version(&old_connection), schema::SCHEMA_VERSION);
    assert_eq!(table_count(&old_connection, "account_groups"), 0);
}

#[test]
fn fresh_database_and_newer_schema_are_handled_atomically() {
    let fresh = TempPath::new("fresh");
    let fresh_connection = fresh.open();
    schema::migrate(&fresh_connection).expect("全新库应直接迁到当前版本");
    assert_eq!(user_version(&fresh_connection), schema::SCHEMA_VERSION);
    assert!(object_exists(
        &fresh_connection,
        "table",
        "assistant_sessions"
    ));
    assert!(object_exists(&fresh_connection, "table", "account_groups"));
    assert_eq!(table_count(&fresh_connection, "account_groups"), 0);

    let newer = TempPath::new("newer");
    {
        let connection = newer.open();
        connection
            .execute_batch(
                "CREATE TABLE sentinel(value TEXT NOT NULL);
                 INSERT INTO sentinel(value) VALUES ('untouched');
                 PRAGMA user_version = 18;",
            )
            .unwrap();
    }
    let before = std::fs::read(&newer.0).expect("应能读取迁移前数据库");
    let connection = newer.open();
    let error = schema::migrate(&connection).expect_err("高版本库必须拒绝");
    assert!(error.to_string().contains("高于当前程序"));
    assert_eq!(user_version(&connection), 18);
    drop(connection);
    assert_eq!(
        std::fs::read(&newer.0).expect("应能读取迁移后数据库"),
        before,
        "拒绝打开不能修改数据库文件"
    );
}
