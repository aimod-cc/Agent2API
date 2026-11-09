use super::*;
use crate::server::db::schema;
use crate::test_support;

const PASSWORD: &str = "backup-test-password";

fn database(path: &Path, name: &str) -> Db {
    let db = Db::open(&path.join(name)).unwrap();
    db.with(|conn| {
        conn.execute(
            "INSERT INTO kv (key,value) VALUES ('test-marker','\"original\"')",
            [],
        )
        .unwrap();
    })
    .unwrap();
    db
}

fn marker(db: &Db) -> String {
    db.with(|conn| {
        conn.query_row("SELECT value FROM kv WHERE key='test-marker'", [], |row| {
            row.get(0)
        })
        .unwrap()
    })
    .unwrap()
}

fn put_account(db: &Db, id: &str, data: &str) {
    db.with(|conn| {
        conn.execute(
            "INSERT INTO accounts (id,provider,priority,enabled,added_at,data) VALUES (?1,'workbuddy',0,1,0,?2)",
            rusqlite::params![id, data],
        ).unwrap();
    }).unwrap();
}

fn sql_file(path: &Path, name: &str, sql: &str) -> std::path::PathBuf {
    let file = path.join(name);
    std::fs::write(&file, sql).unwrap();
    file
}

#[test]
fn encrypted_backup_restores_accounts_without_replacing_local_keys() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::encrypted_backup_restores_accounts_without_replacing_local_keys"
        ),
        || {
            let home = crate::paths::config_dir();
            let source = database(&home, "source.db");
            let account = json!({"id":"saved-account","provider":"workbuddy","accessToken":"private-token","unknown":{"keep":true}});
            let encrypted = crypto::seal(
                account.to_string().as_bytes(),
                crypto::SecretScope::Accounts,
            )
            .unwrap();
            put_account(&source, "saved-account", &encrypted);
            let exported = home.join("backup.a2ab");
            let result = export_to_file(&source, &exported, false, PASSWORD).unwrap();
            assert_eq!(result["encrypted"], true);
            assert!(!String::from_utf8_lossy(&std::fs::read(&exported).unwrap())
                .contains("private-token"));

            let local_key = [91u8; 32];
            let key_path = crypto::key_file_path(crypto::SecretScope::Accounts);
            std::fs::write(&key_path, local_key).unwrap();
            crypto::reset_cache(crypto::SecretScope::Accounts);
            let target = database(&home, "target.db");
            put_account(&target, "old-account", "{\"id\":\"old-account\"}");
            let result = import_from_file(&target, &exported, &home, PASSWORD).unwrap();
            assert_eq!(result["localKeysPreserved"], true);
            assert_eq!(result["reencryptedSecrets"], 1);
            assert_eq!(std::fs::read(&key_path).unwrap(), local_key);
            target
                .with(|conn| {
                    let (id, data): (String, String) = conn
                        .query_row("SELECT id,data FROM accounts", [], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })
                        .unwrap();
                    assert_eq!(id, "saved-account");
                    assert_ne!(data, encrypted);
                    let plain = crypto::open(&data, crypto::SecretScope::Accounts).unwrap();
                    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), account);
                    assert_eq!(
                        conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row
                            .get::<_, i64>(0))
                            .unwrap(),
                        1
                    );
                })
                .unwrap();
            assert!(home
                .join("backups")
                .join(result["safetyBackup"].as_str().unwrap())
                .is_file());
        },
    );
}

#[test]
fn wrong_password_corruption_and_truncation_leave_target_unchanged() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::wrong_password_corruption_and_truncation_leave_target_unchanged"
        ),
        || {
            let home = crate::paths::config_dir();
            let source = database(&home, "source.db");
            source
                .with(|conn| {
                    conn.execute(
                        "UPDATE kv SET value='\"replacement\"' WHERE key='test-marker'",
                        [],
                    )
                    .unwrap();
                })
                .unwrap();
            let target = database(&home, "target.db");
            let exported = home.join("backup.a2ab");
            export_to_file(&source, &exported, true, PASSWORD).unwrap();
            assert!(import_from_file(&target, &exported, &home, "incorrect-password").is_err());
            assert_eq!(marker(&target), "\"original\"");
            let bytes = std::fs::read(&exported).unwrap();
            let mut corrupt = bytes.clone();
            *corrupt.last_mut().unwrap() ^= 1;
            for (name, bytes) in [
                ("corrupt.a2ab", corrupt),
                ("short.a2ab", bytes[..bytes.len() - 1].to_vec()),
            ] {
                let path = home.join(name);
                std::fs::write(&path, bytes).unwrap();
                assert!(import_from_file(&target, &path, &home, PASSWORD).is_err());
                assert_eq!(marker(&target), "\"original\"");
            }
        },
    );
}

#[test]
fn invalid_sql_and_newer_backups_cannot_replace_the_database() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::invalid_sql_and_newer_backups_cannot_replace_the_database"
        ),
        || {
            let home = crate::paths::config_dir();
            let db = database(&home, "target.db");
            let escape = home.join("outside.db");
            let escaped = escape.to_string_lossy().replace('\'', "''");
            let cases = [
                "not an Agent2API backup".to_string(),
                format!("{BACKUP_SQL_HEADER}\nCREATE TABLE kv (key TEXT, value TEXT);"),
                format!("{BACKUP_SQL_HEADER}\nBEGIN; CREATE TABLE kv (key TEXT, value TEXT);"),
                format!(
                    "{BACKUP_SQL_HEADER}\nPRAGMA user_version={};",
                    schema::SCHEMA_VERSION + 1
                ),
                format!("{BACKUP_SQL_HEADER}\nATTACH DATABASE '{escaped}' AS outside;"),
                format!("{BACKUP_SQL_HEADER}\nPRAGMA writable_schema=ON;"),
                format!("{BACKUP_SQL_HEADER}\nCREATE TABLE broken(\0);"),
            ];
            for (index, sql) in cases.iter().enumerate() {
                let source = sql_file(&home, &format!("invalid-{index}.sql"), sql);
                assert!(
                    import_from_file(&db, &source, &home, "").is_err(),
                    "case={index}"
                );
                assert_eq!(marker(&db), "\"original\"", "case={index}");
                assert!(!escape.exists(), "越界 SQL 不能创建外部数据库");
            }
        },
    );
}

#[test]
fn interrupted_database_replacement_rolls_back_to_the_safety_backup() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::interrupted_database_replacement_rolls_back_to_the_safety_backup"
        ),
        || {
            let home = crate::paths::config_dir();
            let target = database(&home, "target.db");
            let source = database(&home, "source.db");
            source
                .with(|conn| {
                    conn.execute(
                        "UPDATE kv SET value='\"replacement\"' WHERE key='test-marker'",
                        [],
                    )
                    .unwrap()
                })
                .unwrap();
            let original = snapshot_to_memory(&target).unwrap();
            let mut sql = Vec::new();
            source
                .with(|conn| dump_sql_into(conn, &[], &mut sql).unwrap())
                .unwrap();
            let path = home.join("source.sql");
            std::fs::write(&path, sql).unwrap();
            restore::fail_next_replace();
            let error = import_from_file(&target, &path, &home, "").unwrap_err();
            assert!(error.contains("已自动还原"), "{error}");
            assert_eq!(marker(&target), "\"original\"");
            let expected: i64 = original
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            let actual: i64 = target
                .with(|conn| {
                    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                        .unwrap()
                })
                .unwrap();
            assert_eq!(actual, expected);
        },
    );
}

#[test]
fn sql_export_preserves_blob_null_quotes_and_trigger_semantics() {
    let source = Connection::open_in_memory().unwrap();
    source.execute_batch("CREATE TABLE values_test(id INTEGER PRIMARY KEY AUTOINCREMENT, text_value TEXT, bytes BLOB, missing TEXT);\nCREATE TABLE audit(value TEXT);\nCREATE TRIGGER track_insert AFTER INSERT ON values_test BEGIN INSERT INTO audit VALUES ('new'); END;").unwrap();
    source
        .execute(
            "INSERT INTO values_test(text_value,bytes,missing) VALUES (?1,?2,NULL)",
            rusqlite::params!["引号';\0换行\n", vec![0u8, 255, 1]],
        )
        .unwrap();
    source
        .execute("INSERT INTO values_test(text_value) VALUES ('deleted')", [])
        .unwrap();
    source
        .execute("DELETE FROM values_test WHERE id=2", [])
        .unwrap();
    let mut sql = Vec::new();
    dump_sql_into(&source, &[], &mut sql).unwrap();
    let target = Connection::open_in_memory().unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&sql).unwrap();
    sql_import::apply(&target, &mut file).unwrap();
    let row: (String, Vec<u8>, Option<String>) = target
        .query_row(
            "SELECT text_value,bytes,missing FROM values_test",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("引号';\0换行\n".into(), vec![0, 255, 1], None));
    assert_eq!(
        target
            .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    target
        .execute(
            "INSERT INTO values_test(text_value) VALUES ('after restore')",
            [],
        )
        .unwrap();
    assert_eq!(target.last_insert_rowid(), 3);
    assert_eq!(
        target
            .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn invalid_export_password_does_not_overwrite_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path(), "db.sqlite");
    let target = dir.path().join("existing.a2ab");
    std::fs::write(&target, b"keep this backup").unwrap();
    assert!(export_to_file(&db, &target, false, "short").is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep this backup");
}
