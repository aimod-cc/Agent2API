use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

/// 统计主 `.db` 及其同目录 `-wal` 的复合修改时间戳与总大小，确保 WAL 未合并前也能感知增量写入。
pub fn file_fingerprint(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = |m: &std::fs::Metadata| -> i64 {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    };
    let mut max_mtime = mtime(&meta);
    let mut total_size = meta.len() as i64;

    let wal_path_str = format!("{}-wal", path.to_string_lossy());
    let wal_path = Path::new(&wal_path_str);
    if let Ok(wal_meta) = std::fs::metadata(wal_path) {
        max_mtime = max_mtime.max(mtime(&wal_meta));
        total_size = total_size.saturating_add(wal_meta.len() as i64);
    }
    Some((max_mtime, total_size))
}

fn configure_readonly(conn: &Connection) {
    let _ = conn.busy_timeout(Duration::from_millis(800));
    let _ = conn.execute_batch("PRAGMA query_only = ON;");
}

/// 严格只读打开第三方 SQLite 文件；若因锁冲突或 WAL 权限受阻，复制到临时目录后只读读取。
pub fn open_readonly_with_snapshot<F, T>(path: &Path, mut work: F) -> Result<T, String>
where
    F: FnMut(&Connection) -> rusqlite::Result<T>,
{
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(conn) = Connection::open_with_flags(path, flags) {
        configure_readonly(&conn);
        if let Ok(result) = work(&conn) {
            return Ok(result);
        }
    }

    let dir = tempfile::tempdir().map_err(|err| format!("创建临时快照目录失败: {err}"))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("snapshot.db");
    let snap_db = dir.path().join(file_name);
    std::fs::copy(path, &snap_db).map_err(|err| format!("复制只读快照失败: {err}"))?;

    for suffix in ["-wal", "-shm"] {
        let sibling_str = format!("{}{}", path.to_string_lossy(), suffix);
        let sibling = Path::new(&sibling_str);
        if sibling.is_file() {
            let dest = dir.path().join(format!("{file_name}{suffix}"));
            let _ = std::fs::copy(sibling, dest);
        }
    }

    let snap_conn =
        Connection::open_with_flags(&snap_db, flags).map_err(|err| format!("打开只读快照失败: {err}"))?;
    configure_readonly(&snap_conn);
    work(&snap_conn).map_err(|err| format!("读取本地数据库失败: {err}"))
}
