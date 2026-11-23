//! 已安装脚本插件的「有没有新版本」。
//!
//! 三种来源的「最新」不是一回事，这里按来源分别问上游：
//!   * **Magpie 分发源**（`magpie:<包名>` 装的）→ 官方分发索引里该包的 `latest`；
//!   * **npm 源**（`source` 缺省的那些）→ npm 的 `dist-tags.latest`；
//!   * **GitHub 源** → 那个分支/标签当前指向的提交，与装的那次记下的 `commit` 比
//!     （版本号在这里没有可比性，所以「最新」显示的是提交短号）。
//!
//! 结果落一份小缓存（`website-updates.json`），定时检查统一由「自动维护 / 定时任务」调度；
//! 插件页的「检查更新」可以强制立刻刷一次（[`check`]）。页面轮询只读缓存，不碰网络。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use super::storage::{Package, PackageSource};

/// 后台自动检查的最小间隔（仅非强制直接刷新时参考）。
const REFRESH_MS: i64 = 6 * 3_600_000;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Entry {
    /// 上游的最新版本；GitHub 源是提交短号
    latest: String,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Cache {
    checked_at: i64,
    packages: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckOutcome {
    pub id: String,
    pub name: String,
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub error: Option<String>,
    pub source_label: String,
}

fn update_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn path() -> std::path::PathBuf {
    super::root().join("website-updates.json")
}

fn cache() -> Cache {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .unwrap_or_default()
}

/// 已安装插件的可更新状态（同步读缓存；界面每次轮询都读它）。
pub(crate) fn snapshot() -> Value {
    let cached = cache();
    let items: Vec<Value> = cached
        .packages
        .iter()
        .map(|(id, entry)| json!({
            "id": id, "latest": entry.latest, "available": entry.available, "error": entry.error,
        }))
        .collect();
    json!({ "checkedAt": cached.checked_at, "updates": items })
}

fn save_cache(cached: &Cache) {
    if let Ok(bytes) = serde_json::to_vec(cached) {
        if let Err(error) = super::storage::atomic_write(&path(), &bytes) {
            crate::server::logging::verbose("[PluginUpdates]", &format!("保存更新检查结果失败：{error}"));
        }
    }
}

fn installed_latest_label(package: &Package) -> String {
    match &package.source {
        Some(PackageSource::Github { commit, .. }) if commit.len() >= 12 => commit[..12].to_string(),
        _ => package.version.clone(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PackageFingerprint {
    Local(String),
    Magpie(String),
    Npm(String),
    Github { repository: String, reference: String, commit: String },
}

fn package_fingerprint(package: &Package) -> PackageFingerprint {
    match &package.source {
        Some(PackageSource::Local { .. }) => PackageFingerprint::Local(package.version.clone()),
        Some(PackageSource::Magpie { .. }) => PackageFingerprint::Magpie(package.version.clone()),
        Some(PackageSource::Github { repository, reference, commit, .. }) => PackageFingerprint::Github {
            repository: repository.clone(),
            reference: reference.clone(),
            commit: commit.clone(),
        },
        None => PackageFingerprint::Npm(package.version.clone()),
    }
}

/// 界面上的「检查更新」：立刻刷一次，返回刷新后的快照。
pub async fn check() -> Value {
    refresh(true).await
}

/// 刷一次。返回刷新后的快照。
pub(super) async fn refresh(force: bool) -> Value {
    let _ = refresh_outcomes(force).await;
    snapshot()
}

/// 安装或更新成功后，立即把该插件的本地更新状态写为最新，避免完成后界面仍残留旧提示。
pub(super) async fn record_installed(package: &Package, installed_latest: bool) {
    let entry = if installed_latest {
        Entry { latest: installed_latest_label(package), available: false, error: None }
    } else {
        let mut distribution = None;
        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            check_entry(package, true, &mut distribution),
        ).await {
            Ok(entry) => entry,
            Err(_) => Entry { error: Some("检查插件最新版本超时".into()), ..Entry::default() },
        }
    };
    let _guard = update_lock().lock().await;
    let mut cached = cache();
    cached.packages.insert(package.id.clone(), entry);
    save_cache(&cached);
}

/// 卸载成功后同步移除对应的缓存条目。
pub(super) async fn remove_entry(id: &str) {
    let _guard = update_lock().lock().await;
    let mut cached = cache();
    if cached.packages.remove(id).is_some() {
        save_cache(&cached);
    }
}

/// 执行一轮更新检查并返回每个已安装脚本插件的检查明细（供自动维护记录使用）。
pub(crate) async fn refresh_outcomes(force: bool) -> Result<Vec<CheckOutcome>, String> {
    let _guard = update_lock().lock().await;
    refresh_inner(force).await
}

fn source_label(package: &Package) -> String {
    match &package.source {
        Some(PackageSource::Local { .. }) => "本地安装".to_string(),
        Some(PackageSource::Magpie { .. }) => "Magpie 分发源".to_string(),
        Some(PackageSource::Github { repository, .. }) => format!("GitHub ({repository})"),
        None => "npm".to_string(),
    }
}

async fn check_entry(
    package: &Package,
    force: bool,
    distribution: &mut Option<Result<Value, String>>,
) -> Entry {
    match &package.source {
        Some(PackageSource::Local { .. }) => Entry { latest: package.version.clone(), ..Entry::default() },
        Some(PackageSource::Magpie { .. }) => {
            if distribution.is_none() {
                *distribution = Some(super::distribution::index().await);
            }
            match distribution.as_ref().expect("上一行刚填过") {
                Ok(index) => magpie_entry(index, package),
                Err(error) => Entry { error: Some(error.clone()), ..Entry::default() },
            }
        }
        Some(PackageSource::Github { repository, reference, commit, .. }) => {
            if !force {
                if let Some(sha) = super::catalog::github_commit_for(repository, reference) {
                    Entry { available: !sha.eq_ignore_ascii_case(commit), latest: sha[..12].to_string(), error: None }
                } else {
                    github_entry(repository, reference, commit).await
                }
            } else {
                github_entry(repository, reference, commit).await
            }
        }
        // 没有 source = npm 装的
        None => npm_entry(&package.name, &package.version).await,
    }
}

async fn refresh_inner(force: bool) -> Result<Vec<CheckOutcome>, String> {
    let registry = super::storage::read()?;
    let now = crate::server::logging::now_ms();
    let existing = cache();
    let names = super::runtime::package_names(&registry);
    if !force && now.saturating_sub(existing.checked_at) < REFRESH_MS {
        return Ok(registry.packages.iter().map(|package| {
            let entry = existing.packages.get(&package.id).cloned().unwrap_or_default();
            CheckOutcome {
                id: package.id.clone(),
                name: names.get(&package.id).cloned().unwrap_or_else(|| package.name.clone()),
                current: package.version.clone(),
                latest: entry.latest,
                available: entry.available,
                error: entry.error,
                source_label: source_label(package),
            }
        }).collect());
    }
    if registry.packages.is_empty() {
        if !existing.packages.is_empty() || force {
            let cached = Cache { checked_at: now, packages: BTreeMap::new() };
            save_cache(&cached);
        }
        return Ok(Vec::new());
    }
    let initial_fingerprints: BTreeMap<String, PackageFingerprint> = registry
        .packages
        .iter()
        .map(|package| (package.id.clone(), package_fingerprint(package)))
        .collect();
    // 懒加载：只有真的装了 magpie 分发源的包，才去拉那份索引（几 MB）。
    let mut distribution: Option<Result<Value, String>> = None;
    let mut packages = BTreeMap::new();
    let mut outcomes = Vec::with_capacity(registry.packages.len());
    for package in &registry.packages {
        let entry = check_entry(package, force, &mut distribution).await;
        outcomes.push(CheckOutcome {
            id: package.id.clone(),
            name: names.get(&package.id).cloned().unwrap_or_else(|| package.name.clone()),
            current: package.version.clone(),
            latest: entry.latest.clone(),
            available: entry.available,
            error: entry.error.clone(),
            source_label: source_label(package),
        });
        packages.insert(package.id.clone(), entry);
    }
    // 防止在网络查询期间发生安装/更新/卸载后，被本轮旧快照覆盖回去。
    if let Ok(latest_registry) = super::storage::read() {
        let current_by_id: BTreeMap<&str, &Package> = latest_registry
            .packages
            .iter()
            .map(|package| (package.id.as_str(), package))
            .collect();
        let disk_cache = cache();
        packages.retain(|id, entry| {
            let Some(current) = current_by_id.get(id.as_str()) else {
                return false;
            };
            if initial_fingerprints.get(id).is_some_and(|before| *before != package_fingerprint(current)) {
                if let Some(newer) = disk_cache.packages.get(id) {
                    *entry = newer.clone();
                } else {
                    return false;
                }
            }
            true
        });
    }
    // 只保留还在装的插件：卸载 / 换来源后旧条目不再有意义。
    let cached = Cache { checked_at: now, packages };
    save_cache(&cached);
    Ok(outcomes)
}

fn magpie_entry(index: &Value, package: &Package) -> Entry {
    let latest = index["packages"]
        .get(&package.name)
        .and_then(|item| item["latest"].as_str())
        .unwrap_or("")
        .to_string();
    if latest.is_empty() {
        return Entry { error: Some("官方分发索引里没有这个包".into()), ..Entry::default() };
    }
    Entry { available: latest != package.version, latest, error: None }
}

async fn npm_entry(name: &str, version: &str) -> Entry {
    let url = format!("https://registry.npmjs.org/{}/latest", name.replace('/', "%2f"));
    let latest = match super::download::npm_metadata(&url).await {
        Ok(Some(bytes)) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["version"].as_str().map(str::to_string))
            .unwrap_or_default(),
        Ok(None) => String::new(),
        Err(error) => return Entry { error: Some(error), ..Entry::default() },
    };
    if latest.is_empty() {
        return Entry { error: Some("npm 上没有这个包的版本信息".into()), ..Entry::default() };
    }
    Entry { available: latest != version, latest, error: None }
}

async fn github_entry(repository: &str, reference: &str, commit: &str) -> Entry {
    match super::source::fetch_github_commit(repository, reference).await {
        Ok(sha) => Entry { available: !sha.eq_ignore_ascii_case(commit), latest: sha[..12].to_string(), error: None },
        Err(error) => Entry { error: Some(error), ..Entry::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn github_package(id: &str, commit: &str) -> Package {
        Package {
            id: id.into(),
            name: "magpie-amazon-quick".into(),
            version: "0.2.20".into(),
            icon: None,
            provider_icons: BTreeMap::new(),
            source: Some(PackageSource::Github {
                repository: "3056810551/magpie-amazon-quick".into(),
                reference: "main".into(),
                commit: commit.into(),
                sha256: "0".repeat(64),
            }),
            directory: format!("{id}/dir"),
            enabled: true,
            installed_at: 0,
            providers: Vec::new(),
            disabled_providers: Vec::new(),
            options: Value::Null,
            provider_options: Value::Null,
            settings_schema: Value::Null,
            provider_aliases: BTreeMap::new(),
            error: None,
        }
    }

    #[tokio::test]
    async fn record_installed_and_remove_entry_sync_cache_immediately() {
        let old_pkg = github_package("script-test-sync", "8572d4704447d307ecd4709ab31029ae9ada61d8");
        let new_pkg = github_package("script-test-sync", "4d9d2f32d7c2e872a84c9eff01c0b70b602cd7e5");
        assert_ne!(package_fingerprint(&old_pkg), package_fingerprint(&new_pkg));

        {
            let _guard = update_lock().lock().await;
            let mut cached = cache();
            cached.packages.insert(
                old_pkg.id.clone(),
                Entry { latest: "4d9d2f32d7c2".into(), available: true, error: None },
            );
            save_cache(&cached);
        }

        record_installed(&new_pkg, true).await;
        let after_update = cache();
        let entry = after_update.packages.get(&new_pkg.id).expect("应保留新插件更新条目");
        assert!(!entry.available);
        assert_eq!(entry.latest, "4d9d2f32d7c2");
        assert!(entry.error.is_none());

        remove_entry(&new_pkg.id).await;
        assert!(!cache().packages.contains_key(&new_pkg.id));
    }
}
