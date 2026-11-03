//! `{config_dir}/backups/` 的保留上限：同类安全备份只留最近 N 份。
//!
//! ── 为什么必须有上限 ────────────────────────────────────────
//! 这个目录里放的是**整库级**的留影：
//!   - 整库恢复前的 `pre-restore-*.db`（SQLite Backup API 写出的完整库）；
//!   - 云同步下载前的 `cloud-sync-*.json`（业务快照）。
//! 两者都是「用户后悔时的最后一根稻草」，所以不能不留；但**都没有清理**时，
//! 每做一次恢复 / 每次自动同步下载都会再落一份，磁盘占用随操作次数线性增长 ——
//! 实测里这被误认为「启动时复制整库把 C 盘吃满」。
//!
//! ── 为什么按名字而不是按体积 ────────────────────────────────
//! 体积阈值会随库增长失效（今天 50MB 算大，明天 500MB 算正常）。文件名里的
//! 时间戳是结构化事实，排序稳定；同秒内的多份再用文件 mtime 与名字兜底，
//! 保证「删的一定是最旧的」。
//!
//! ── 为什么必须校验**完整**文件名格式 ────────────────────────
//! 这个函数会**删文件**，而 `backups/` 是用户也能打开的普通目录：只按
//! 「前缀 + 后缀」匹配的话，用户自己放的 `cloud-sync-笔记.json`、
//! `pre-restore-手动备份.db` 都会被当成程序产物计数，超上限时被删掉 ——
//! 那是不可逆的数据丢失。因此只认本程序**确实会生成**的两种名字：
//!   - `pre-restore-{YYYYmmdd}_{HHMMSS}-{随机串}.db`（`safety.rs`）
//!   - `cloud-sync-{YYYYmmdd}-{HHMMSS}.json`（`cloud_sync::snapshot`）
//! 任何形状不符的文件一律跳过，不计数、不删除。
//!
//! ── 失败语义 ────────────────────────────────────────────────
//! 尽力而为：清理失败只打一行警告，绝不让「备份已经写成功了，但删旧的失败」
//! 反过来把恢复 / 下载判成失败。删不掉的旧备份最坏只是继续占盘。

use std::path::Path;

use crate::server::logging;

/// 备份文件的种类：决定「什么样的文件名才算本程序生成的备份」。
#[derive(Clone, Copy)]
pub(crate) enum BackupKind {
    /// 整库恢复前的安全备份：`pre-restore-20261103_163000-a1b2c3.db`
    PreRestore,
    /// 云同步下载前的业务快照：`cloud-sync-20261103-163000.json`
    CloudSync,
}

impl BackupKind {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::PreRestore => {
                // 前缀 + `YYYYmmdd_HHMMSS` + `-` + 随机串 + `.db`
                let Some(rest) = name.strip_prefix("pre-restore-") else {
                    return false;
                };
                let Some(rest) = rest.strip_suffix(".db") else {
                    return false;
                };
                let Some((stamp, random)) = rest.split_once('-') else {
                    return false;
                };
                is_stamp(stamp, '_') && !random.is_empty() && random.chars().all(|c| c.is_ascii_alphanumeric())
            }
            Self::CloudSync => {
                // 前缀 + `YYYYmmdd-HHMMSS` + `.json`
                let Some(rest) = name.strip_prefix("cloud-sync-") else {
                    return false;
                };
                let Some(stamp) = rest.strip_suffix(".json") else {
                    return false;
                };
                is_stamp(stamp, '-')
            }
        }
    }
}

/// `YYYYmmdd` + 分隔符 + `HHMMSS`（都是 ASCII 数字）。
fn is_stamp(stamp: &str, separator: char) -> bool {
    let Some((date, time)) = stamp.split_once(separator) else {
        return false;
    };
    date.len() == 8
        && time.len() == 6
        && date.chars().all(|c| c.is_ascii_digit())
        && time.chars().all(|c| c.is_ascii_digit())
}

/// 只保留最近 `keep` 份**本程序生成**的该种备份，返回删掉的数量。
///
/// `keep == 0` 视为「不清理」而不是「全删」：调用方误传 0 时把用户的留影
/// 全删掉是不可接受的，而「保留 0 份」这个需求本身也不存在。
pub(crate) fn prune_backups(dir: &Path, kind: BackupKind, keep: usize) -> usize {
    if keep == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        // 目录不存在 = 还没做过任何备份，正常路径
        return 0;
    };
    let mut found: Vec<(std::time::SystemTime, String, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !kind.matches(name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        // mtime 读不到就退化成「最旧」（UNIX_EPOCH）：宁可先删它，也不因为它
        // 元数据异常而让整个清理停摆。
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        found.push((modified, name.to_string(), entry.path()));
    }
    if found.len() <= keep {
        return 0;
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let remove_count = found.len() - keep;
    let mut removed = 0usize;
    for (_, _, path) in found.into_iter().take(remove_count) {
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) => logging::console_line(
                "[Backup]",
                &format!("⚠️  清理旧备份失败（{}）：{error}", path.display()),
            ),
        }
    }
    if removed > 0 {
        logging::console_line(
            "[Backup]",
            &format!("已清理 {removed} 份旧备份（保留最近 {keep} 份）"),
        );
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "backup-retention-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 超过上限时删最旧的，且只动匹配前缀 + 后缀的文件。
    #[test]
    fn keeps_newest_and_leaves_other_files_alone() {
        let dir = temp_dir("keep-newest");
        for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::write(dir.join(format!("cloud-sync-{stamp}.json")), b"{}").expect("写备份");
        }
        std::fs::write(dir.join("cloud-sync-20260101-000000.json.tmp"), b"{}").expect("写半成品");
        std::fs::write(dir.join("pre-restore-20260101_000000-abc.db"), b"x").expect("写整库备份");
        std::fs::write(dir.join("notes.txt"), b"x").expect("写无关文件");

        let removed = prune_backups(&dir, BackupKind::CloudSync, 2);

        assert_eq!(removed, 1, "三份只该删最旧的一份");
        assert!(!dir.join("cloud-sync-20260101-000000.json").exists());
        assert!(dir.join("cloud-sync-20260102-000000.json").exists());
        assert!(dir.join("cloud-sync-20260103-000000.json").exists());
        assert!(
            dir.join("cloud-sync-20260101-000000.json.tmp").exists(),
            "写入中的临时文件不是备份，不能删"
        );
        assert!(dir.join("pre-restore-20260101_000000-abc.db").exists());
        assert!(dir.join("notes.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没超上限时一个都不删；`keep = 0` 不当成「全删」。
    #[test]
    fn does_nothing_below_limit_or_with_zero_keep() {
        let dir = temp_dir("below-limit");
        std::fs::write(dir.join("pre-restore-20260101_000000-abc.db"), b"x").expect("写备份");

        assert_eq!(prune_backups(&dir, BackupKind::PreRestore, 2), 0);
        assert_eq!(prune_backups(&dir, BackupKind::PreRestore, 0), 0);
        assert!(dir.join("pre-restore-20260101_000000-abc.db").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 用户放在 backups/ 里、只是「长得像」的文件绝不能被删。
    ///
    /// 这条是审查的阻塞项：只按前后缀匹配时，下面每一个都会被算成备份，
    /// 超过上限就被删掉 —— 而这是不可逆的。
    #[test]
    fn lookalike_files_are_never_touched() {
        let dir = temp_dir("lookalike");
        // 程序真正会生成的三份（超过 keep = 2）
        for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::write(dir.join(format!("cloud-sync-{stamp}.json")), b"{}").expect("写备份");
        }
        let lookalikes = [
            "cloud-sync-notes.json",
            "cloud-sync-20260101-000000.backup.json",
            "cloud-sync-2026010-000000.json",
            "cloud-sync-20260101-00000.json",
            "cloud-sync-20260101_000000.json",
            "pre-restore-manual.db",
            "pre-restore-20260101-000000-abc.db",
            "pre-restore-20260101_000000-.db",
            "pre-restore-20260101_000000-abc.sqlite",
        ];
        for name in lookalikes {
            std::fs::write(dir.join(name), b"user data").expect("写用户文件");
        }

        let removed = prune_backups(&dir, BackupKind::CloudSync, 2);

        assert_eq!(removed, 1, "只删真正超出的那一份程序备份");
        for name in lookalikes {
            assert!(
                dir.join(name).exists(),
                "{name} 不是程序生成的备份，绝不能删"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
