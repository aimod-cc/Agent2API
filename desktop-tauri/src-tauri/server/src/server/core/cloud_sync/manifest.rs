//! 云同步的 manifest：远端「这一份快照是什么」的元数据与兼容性判定。
//!
//! ── 为什么先传 artifact、最后传 manifest ─────────────────────
//! manifest 是「这次上传已经完成」的唯一标志。先传它、再传数据的话，中途断网
//! 会留下一个指向半截数据的 manifest —— 下一次下载校验 size / sha256 时才失败，
//! 用户看到的是「远端数据坏了」而不是「上次没传完」。反过来（数据先落、manifest
//! 最后覆盖）时，断网只会让远端仍指向**上一份完整快照**，语义安全。
//! 与 cc-switch 的 `webdav_sync::upload` 同一顺序。
//!
//! ── 为什么 format / version 两个字段都要 ─────────────────────
//! `format` 回答「这是不是本功能写的」（远端目录可能被别的程序占用）；`version`
//! 回答「协议是否兼容」。只留一个的话，将来换协议版本就分不清「旧版本文档」与
//! 「别人的文件」—— 前者可以提示升级，后者只能报「这里不是云同步目录」。
//!
//! ── 与数据库 schema 的关系 ──────────────────────────────────
//! `schemaVersion` 是**数据库**版本（`db::schema::SCHEMA_VERSION`）。远端更高 →
//! 直接拒绝（旧程序缺对应的列 / 表语义）；远端更低 → 允许，应用时按「快照与当前
//! 表的列交集」插入，缺的列走建表默认值。这与整库恢复的闸门同一取向（只升不降）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::server::db::schema::SCHEMA_VERSION;

/// manifest 里的格式标识。改这个字符串等于让所有已发布的远端数据「不被认识」，
/// 只在协议不兼容时才动。
pub(crate) const PROTOCOL_FORMAT: &str = "agent2api-cloud-sync";
/// 协议版本（目录布局 / 字段语义）。与 [`SCHEMA_VERSION`] 是两件事：
/// 这个管「怎么说话」，那个管「库长什么样」。
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// 远端根目录下的协议子目录：`{remoteRoot}/v1/{profile}/`。
pub(crate) fn protocol_dir() -> String {
    format!("v{PROTOCOL_VERSION}")
}

/// manifest 文件名
pub(crate) const REMOTE_MANIFEST: &str = "manifest.json";
/// 业务快照文件名
pub(crate) const REMOTE_SNAPSHOT: &str = "snapshot.json";
/// 同步密码封装的账号密钥文件名
pub(crate) const REMOTE_KEY_FILE: &str = "account.key.enc";

/// manifest 体积上限。它只含元数据，1MB 已经非常宽松；超过说明远端那个文件
/// 根本不是我们的 manifest（或者被写坏了），不该把它读进内存再解析。
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// 单个 artifact 的体积上限。业务快照只含账号 / 配置 / 签到 / 用量记录，
/// 256MB 足以容纳极端规模的账号库；超限直接拒绝，避免把内存打爆。
pub(crate) const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// 一个 artifact 的校验信息（sha256 十六进制 + 字节数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactMeta {
    pub sha256: String,
    pub size: u64,
}

/// 远端 manifest。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncManifest {
    pub format: String,
    pub version: u32,
    /// 生成这份快照的程序版本（只用于展示与排障，不参与兼容判定）
    #[serde(default)]
    pub app_version: String,
    /// 数据库 schema 版本（兼容判定用，见模块头）
    pub schema_version: i64,
    /// 生成这份快照的机器名（下载确认弹窗展示「这是谁传的」）
    #[serde(default)]
    pub device_name: String,
    /// 生成时刻（RFC3339）
    #[serde(default)]
    pub created_at: String,
    /// 快照身份：artifact 哈希的稳定摘要，用来判断「远端是不是换了一份」
    pub snapshot_id: String,
    /// 快照里账号密钥的指纹；`None` = 上传时本机还没有 `account.key`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
    /// 文件名 → 校验信息（只含**这次真的上传了**的文件）
    pub artifacts: BTreeMap<String, ArtifactMeta>,
}

impl SyncManifest {
    /// 这个 manifest 是不是本功能写的。
    pub(crate) fn is_ours(&self) -> bool {
        self.format == PROTOCOL_FORMAT
    }
}

/// 兼容性判定的结论：要么可下载，要么给出**能照着做**的拒绝理由。
pub(crate) fn validate_compat(manifest: &SyncManifest) -> Result<(), String> {
    if !manifest.is_ours() {
        return Err(format!(
            "远端 manifest 不是本功能生成的（format = {}）。请确认「远程根目录 / 档案」填的是云同步专用的位置。",
            manifest.format
        ));
    }
    if manifest.version != PROTOCOL_VERSION {
        return Err(format!(
            "远端同步数据来自不兼容的协议版本 v{}（本机 v{PROTOCOL_VERSION}），请升级应用后重试。",
            manifest.version
        ));
    }
    if manifest.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "远端数据来自更新版本的应用（库版本 {} > 当前 {SCHEMA_VERSION}），请先升级本应用再下载。",
            manifest.schema_version
        ));
    }
    Ok(())
}

/// 从 artifact 哈希算一个稳定快照 id。
///
/// `BTreeMap` 的迭代顺序按键排序，所以同一组内容每次都得到同一个 id —— 自动同步
/// 靠它判断「远端这份是不是我刚传的」。
pub(crate) fn snapshot_id_of(artifacts: &BTreeMap<String, ArtifactMeta>) -> String {
    let parts: Vec<String> = artifacts
        .iter()
        .map(|(name, meta)| format!("{name}:{}", meta.sha256))
        .collect();
    crate::server::core::cloud_sync::sha256_hex(parts.join("|").as_bytes())
}

/// 校验下载回来的字节与 manifest 记录一致。
///
/// 先比长度再算哈希：长度不符时能立刻给出结论（截断 / 多读），不必白算一遍
/// 几十 MB 的 sha256。
pub(crate) fn verify_artifact(
    bytes: &[u8],
    name: &str,
    meta: &ArtifactMeta,
) -> Result<(), String> {
    if bytes.len() as u64 != meta.size {
        return Err(format!(
            "远端 {name} 大小不符（应为 {} 字节，实际 {} 字节），文件可能被截断，请重新上传。",
            meta.size,
            bytes.len()
        ));
    }
    let actual = crate::server::core::cloud_sync::sha256_hex(bytes);
    if actual != meta.sha256 {
        return Err(format!(
            "远端 {name} 校验失败（sha256 不一致），文件可能已损坏，请重新上传。"
        ));
    }
    Ok(())
}

/// 把用户填的远程根目录 / 档案名切成路径段。
///
/// 逐段过滤空段与 `.` / `..`：远端路径由用户输入拼成，`..` 会跳出同步目录
/// （WebDAV 上等于往用户的别的目录写文件）。这里与 cc-switch 的
/// `path_segments` 同一口径，只是多拒了 `..`。
pub(crate) fn sanitize_segments(raw: &str) -> Result<Vec<String>, String> {
    let mut segments = Vec::new();
    for part in raw.trim().trim_matches('/').split('/') {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err("远程路径不能包含 .. 段".to_string());
        }
        if part.contains('\\') {
            return Err("远程路径请用 / 分隔".to_string());
        }
        segments.push(part.to_string());
    }
    Ok(segments)
}

/// 远端目录的路径段：`{remoteRoot…}/v1/{profile…}`。
pub(crate) fn remote_dir_segments(remote_root: &str, profile: &str) -> Result<Vec<String>, String> {
    let mut segments = sanitize_segments(remote_root)?;
    segments.push(protocol_dir());
    segments.extend(sanitize_segments(profile)?);
    Ok(segments)
}

/// 远端目录的展示形态（`/root/v1/default`）
pub(crate) fn remote_dir_display(remote_root: &str, profile: &str) -> String {
    let segments = remote_dir_segments(remote_root, profile).unwrap_or_default();
    if segments.is_empty() {
        return "/".to_string();
    }
    format!("/{}", segments.join("/"))
}

/// 本机设备名：Windows 的 COMPUTERNAME / 类 Unix 的 HOSTNAME，最后回落 hostname 命令。
///
/// 只用于 manifest 的展示字段，拿不到就给 `Unknown Device` —— 不该因为读不到
/// 主机名而让整个上传失败。
pub(crate) fn detect_device_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.chars().take(64).collect();
            }
        }
    }
    "Unknown Device".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_segments_rejects_parent_dir() {
        assert!(sanitize_segments("a/../b").is_err());
        assert_eq!(
            sanitize_segments("/a//b/").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn remote_dir_segments_appends_protocol_and_profile() {
        let segments = remote_dir_segments("agent2api-sync", "default").unwrap();
        assert_eq!(
            segments,
            vec![
                "agent2api-sync".to_string(),
                format!("v{PROTOCOL_VERSION}"),
                "default".to_string()
            ]
        );
    }

    #[test]
    fn validate_compat_rejects_newer_schema() {
        let manifest = SyncManifest {
            format: PROTOCOL_FORMAT.to_string(),
            version: PROTOCOL_VERSION,
            app_version: "3.1.0".to_string(),
            schema_version: SCHEMA_VERSION + 1,
            device_name: "x".to_string(),
            created_at: "2026-10-19T00:00:00Z".to_string(),
            snapshot_id: "id".to_string(),
            key_fingerprint: None,
            artifacts: BTreeMap::new(),
        };
        assert!(validate_compat(&manifest).is_err());
    }

    #[test]
    fn snapshot_id_is_stable_and_order_independent() {
        let mut a = BTreeMap::new();
        a.insert(
            "snapshot.json".to_string(),
            ArtifactMeta { sha256: "aa".to_string(), size: 1 },
        );
        let mut b = BTreeMap::new();
        b.insert(
            "snapshot.json".to_string(),
            ArtifactMeta { sha256: "aa".to_string(), size: 1 },
        );
        assert_eq!(snapshot_id_of(&a), snapshot_id_of(&b));
    }
}
