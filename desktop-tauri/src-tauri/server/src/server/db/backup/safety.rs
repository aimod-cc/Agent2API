use std::path::{Path, PathBuf};

use chrono::Local;
use rusqlite::backup::Backup;
use rusqlite::Connection;

use super::complete_backup;
use super::retention::{prune_backups, BackupKind};

/// 保留最近几份「恢复前安全备份」。整库级留影每份都等于整个库，
/// 留 2 份足够覆盖「这次恢复错了 + 上一次也错了」的回退链，再多就是纯占盘。
const KEEP_SAFETY_BACKUPS: usize = 2;

pub(super) fn create(main: &mut Connection, config_dir: &Path) -> Result<PathBuf, String> {
    let backup_dir = config_dir.join("backups");
    std::fs::create_dir_all(&backup_dir).map_err(|error| format!("创建备份目录失败: {error}"))?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("pre-restore-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder
        .tempdir_in(&backup_dir)
        .map_err(|error| format!("创建安全备份目录失败: {error}"))?;
    let target = directory.path().join("database.db");
    let mut dest =
        Connection::open(&target).map_err(|error| format!("创建安全备份失败: {error}"))?;
    {
        let backup =
            Backup::new(main, &mut dest).map_err(|error| format!("初始化安全备份失败: {error}"))?;
        complete_backup(&backup, "创建安全备份")?;
    }
    dest.close()
        .map_err(|(_, error)| format!("关闭安全备份失败: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("设置安全备份权限失败: {error}"))?;
    }
    let random = directory
        .path()
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let name = format!(
        "pre-restore-{}-{}.db",
        Local::now().format("%Y%m%d_%H%M%S"),
        random.trim_start_matches("pre-restore-")
    );
    let saved = backup_dir.join(name);
    std::fs::rename(&target, &saved).map_err(|error| format!("保存安全备份失败: {error}"))?;
    // 写成功之后再清旧的：反过来的话，清理期间新备份还没落盘，
    // 一旦进程在这里退出就同时少了「旧的」和「新的」。
    // 清理失败不影响本次恢复（见 retention 模块头）。
    prune_backups(&backup_dir, BackupKind::PreRestore, KEEP_SAFETY_BACKUPS);
    Ok(saved)
}
