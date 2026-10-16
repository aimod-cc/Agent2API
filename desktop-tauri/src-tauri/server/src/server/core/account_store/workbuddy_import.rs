//! WorkBuddy 官方桌面客户端登录态导入。
//!
//! 官方客户端（Tencent WorkBuddy / WorkBuddy AI）登录后把会话凭证**明文**落在
//! 本机（官方 FAQ 的排障步骤里就让你列这个目录）：
//!   Windows：`%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\`
//!   macOS：  `~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/`
//! 文件是嵌套形 JSON `{auth:{accessToken, refreshToken, expiresAt(毫秒), …},
//! account:{uid, nickname, …}}`，与 `store_crud::add_account` 的嵌套形同构 ——
//! 因此本文件只做「定位文件 + 读出来」，落库全部复用 `add_account`
//! （撞 id 保护、edition 解析、token 长度校验都在那边）。
//!
//! ── 两家两个文件前缀（按 provider 参数化，与 AutoClaw 的地区同款）──
//!   `workbuddy`（国内版）      → `workbuddy-desktop.info`（实测 domain
//!                                `www.workbuddy.cn`）
//!   `workbuddy-intl`（国际版） → `workbuddy-desktop-ai.info`（实测 domain
//!                                `www.workbuddy.ai`，产品名 WorkBuddy AI）
//!
//! ── 历次登录的残留副本 ────────────────────────────────────
//! 每次重新登录官方客户端会把上一份改名成
//! `workbuddy-desktop[-ai].<UTC时间戳>.<pid>.<uuid>.info` 留在原目录**不删**。
//! 选择顺序：不带时间戳的当前文件优先（它就是最新登录态）；不存在或解析失败
//! 时按文件名取最新的带时间戳备份（ISO 形态的时间戳字典序即时间序）——备份里
//! 的 accessToken 可能已过期，refreshToken 一般仍在有效期内，落库后由刷新链路续期。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! 只读文件 + 落库，没有网络请求；错误一律 `AccountStoreError`（400 带可操作
//! 文案），不 panic（release 是 panic=abort）。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::providers::{kind_from_id, kind_id, ProviderKind};
use crate::server::logging;

/// 官方客户端凭证目录（Windows）：`%LOCALAPPDATA%\CodeBuddyExtension\…`
fn auth_directory_windows() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|base| {
            PathBuf::from(base)
                .join("CodeBuddyExtension")
                .join("Data")
                .join("Public")
                .join("auth")
        })
}

/// 官方客户端凭证目录（macOS）：`~/Library/Application Support/CodeBuddyExtension/…`
/// （路径取自官方 FAQ 排障步骤的 `ls` 命令，逐字一致）
fn auth_directory_macos() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("CodeBuddyExtension")
            .join("Data")
            .join("Public")
            .join("auth"),
    )
}

/// 当前（不带时间戳）登录态文件的文件名。
fn current_file_name(intl: bool) -> &'static str {
    if intl {
        "workbuddy-desktop-ai.info"
    } else {
        "workbuddy-desktop.info"
    }
}

/// 该目录下本家候选文件的前缀（含结尾的点，天然把另一家排除在外：
/// `workbuddy-desktop.` 不匹配 `workbuddy-desktop-ai.2026-…`，反之亦然）。
fn backup_prefix(intl: bool) -> &'static str {
    if intl {
        "workbuddy-desktop-ai."
    } else {
        "workbuddy-desktop."
    }
}

/// 挑出本家最新的登录态文件：当前文件优先，缺失时取文件名时间戳最新的备份。
/// 返回 None = 目录里没有任何本家文件。
fn select_auth_file(dir: &Path, intl: bool) -> Option<PathBuf> {
    let current = dir.join(current_file_name(intl));
    if current.is_file() {
        return Some(current);
    }
    let prefix = backup_prefix(intl);
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".info"))
        })
        .collect();
    // 时间戳在文件名里（ISO 形态、零填充），字典序就是时间序
    backups.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    backups.into_iter().next()
}

impl AccountStore {
    /// 从本机官方客户端的登录态文件导入一个 WorkBuddy 账号。
    ///
    /// `provider` 必须是 `workbuddy` / `workbuddy-intl`（决定读哪一组文件、账号
    /// 落进哪一家）；`name` 是用户填的备注名，缺省用文件里的昵称。落库复用
    /// [`AccountStore::add_account`]：嵌套形原样传入，缺 token / uid 时由它报 400。
    pub fn import_workbuddy_desktop_account(
        &self,
        provider: &str,
        name: Option<&str>,
    ) -> Result<Value, AccountStoreError> {
        let intl = match kind_from_id(provider) {
            Some(ProviderKind::WorkBuddy) => false,
            Some(ProviderKind::WorkBuddyIntl) => true,
            _ => {
                return Err(AccountStoreError::bad_request(format!(
                    "导入桌面端登录态只支持 WorkBuddy 国内版 / 国际版，不支持：{provider}"
                )))
            }
        };
        let dir = auth_directory_windows()
            .or_else(auth_directory_macos)
            .ok_or_else(|| {
                AccountStoreError::bad_request(
                    "无法定位用户目录，找不到官方客户端的登录态文件",
                )
            })?;
        let Some(path) = select_auth_file(&dir, intl) else {
            return Err(AccountStoreError::bad_request(format!(
                "未在官方客户端目录找到登录态文件（{}）：请先在官方 Tencent WorkBuddy 客户端里登录一次",
                dir.display()
            )));
        };
        let text = std::fs::read_to_string(&path).map_err(|error| {
            AccountStoreError::bad_request(format!(
                "读取登录态文件失败（{}）：{error}",
                path.display()
            ))
        })?;
        let payload: Value = serde_json::from_str(&text).map_err(|error| {
            AccountStoreError::bad_request(format!(
                "登录态文件不是合法 JSON（{}）：{error}",
                path.display()
            ))
        })?;
        let account = self.add_account(&payload, name, Some(kind_id(if intl {
            ProviderKind::WorkBuddyIntl
        } else {
            ProviderKind::WorkBuddy
        })));
        if account.is_ok() {
            logging::log(
                "[Accounts]",
                &format!(
                    "📥 已从官方客户端导入 {}账号（{}）",
                    if intl { "国际版" } else { "国内版" },
                    path.display()
                ),
            );
        }
        account
    }
}
