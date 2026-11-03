use std::path::Path;

use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

use crate::server::core::{cloud_sync, limiter, usage_records};
use crate::server::db::schema;
use crate::server::{access, config, logging};

use super::{
    account_key_warning, archive, complete_backup, keys, parse_account_key_fingerprint, safety,
    set_phase, sql_import, table_row_counts, validate_required_tables, Db, RestorePhase,
    TempRestoreDb, BACKUP_SQL_HEADER,
};

pub(super) fn import(
    db: &Db,
    source: &Path,
    config_dir: &Path,
    password: &str,
) -> Result<Value, String> {
    set_phase(RestorePhase::Decrypting);
    let mut backup = archive::read(source, password, config_dir)?;
    let header = sql_import::header(&mut backup.sql)?;
    let header = header.trim_start_matches('\u{feff}');
    if !header.trim_start().starts_with(BACKUP_SQL_HEADER) {
        return Err("仅支持由 Agent2API 导出的整库备份（缺少文件头标记）".into());
    }
    let warning = if backup.keys.is_none() {
        account_key_warning(parse_account_key_fingerprint(header))
    } else {
        None
    };
    let mut temp = TempRestoreDb::new(config_dir)?;
    set_phase(RestorePhase::Applying);
    sql_import::apply(&temp.conn, &mut backup.sql)?;
    if !temp.conn.is_autocommit() {
        let _ = temp.conn.execute_batch("ROLLBACK;");
        return Err("备份文件的事务未完成，文件可能被截断".into());
    }
    set_phase(RestorePhase::Validating);
    let version: i64 = temp
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("读取备份库版本失败: {error}"))?;
    if backup
        .schema_version
        .is_some_and(|expected| expected != version)
    {
        return Err("备份清单与数据库版本不一致".into());
    }
    if version > schema::SCHEMA_VERSION {
        return Err(format!(
            "备份来自更新版本的应用（库版本 {version} > 当前 {}），请先升级本应用",
            schema::SCHEMA_VERSION
        ));
    }
    validate_required_tables(&temp.conn)?;
    schema::migrate(&temp.conn).map_err(|error| format!("对齐备份库 schema 失败: {error}"))?;
    let integrity: String = temp
        .conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("检查备份完整性失败: {error}"))?;
    if integrity != "ok" {
        return Err("备份数据库完整性检查未通过".into());
    }
    let restored_keys = backup
        .keys
        .as_ref()
        .map(|keys| keys.names())
        .unwrap_or_default();
    let reencrypted_secrets = if let Some(keys) = backup.keys.as_ref() {
        keys::validate(&temp.conn, keys)?;
        set_phase(RestorePhase::Reencrypting);
        keys::reencrypt(&mut temp.conn, keys)?
    } else {
        0
    };
    let tables = table_row_counts(&temp.conn, &[]);
    set_phase(RestorePhase::SafetyBackup);
    let suppression = cloud_sync::auto_sync::SuppressionGuard::new();
    let safety = db.with_mut(|main| {
        db.invalidate_accounts();
        let safety = safety::create(main, config_dir)?;
        set_phase(RestorePhase::Replacing);
        if let Err(error) = replace(&temp.conn, main) {
            set_phase(RestorePhase::RollingBack);
            let rollback = Connection::open_with_flags(&safety, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| format!("打开安全备份失败: {error}"))
                .and_then(|old| replace(&old, main));
            db.invalidate_accounts();
            return match rollback {
                Ok(()) => Err(format!("{error}；已自动还原替换前的数据，本机密钥未更换")),
                Err(rollback_error) => Err(format!("{error}；自动还原失败: {rollback_error}。安全备份保存在 {}，本机密钥未更换", safety.display())),
            };
        }
        db.invalidate_accounts();
        Ok(safety)
    }).ok_or("数据库连接不可用，恢复未执行")??;
    db.invalidate_accounts();
    set_phase(RestorePhase::Reloading);
    config::reload();
    usage_records::reload_facts();
    limiter::sources::reload();
    limiter::counter::invalidate();
    access::load_refresh_tokens();
    access::clear_access_tokens();
    drop(suppression);
    let safety_name = safety
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    logging::log(
        "[Storage]",
        &format!(
            "📥 整库恢复完成: {} 张表，已迁移 {reencrypted_secrets} 项加密凭证；安全备份: {}",
            tables.len(),
            safety.display()
        ),
    );
    let mut result = json!({
        "tables": tables,
        "safetyBackup": safety_name,
        "restoredKeys": restored_keys,
        "reencryptedSecrets": reencrypted_secrets,
        "localKeysPreserved": true,
        "encrypted": backup.keys.is_some(),
    });
    if let Some(warning) = warning {
        logging::log("[Storage]", &format!("⚠️ 整库恢复密钥警告: {warning}"));
        result["warning"] = json!(warning);
    }
    Ok(result)
}

fn replace(source: &Connection, target: &mut Connection) -> Result<(), String> {
    let backup =
        Backup::new(source, target).map_err(|error| format!("初始化数据库替换失败: {error}"))?;
    complete_backup(&backup, "恢复主数据库")
}
