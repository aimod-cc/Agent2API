//! 脚本插件提供商身份映射的清理、持久化与跨机对齐。
//!
//! 同一脚本插件包在不同机器上可能因历史安装顺序不同而拿到不同的提供商 ID
//! （例如原名 `teleagent` 与冲突避让名 `teleagent-plugin`）。本模块负责：
//!   1. 卸载或启动时清理「已卸载且账号库中已无账号」的占位，避免永久霸占原名；
//!   2. 将活跃与仍留有账号的包身份映射（不含本机路径或凭据）写入数据库保留键
//!      `scriptPluginIdentities`，随云同步与整库备份一起迁移；
//!   3. 云同步下载、整库恢复或启动时将本机 `registry.json` 与库中记录对齐；
//!   4. 账号导出 / 导入时按同一包 ID 做安全重映射，并支持旧快照手动关联冲突别名。

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::source::InstallSource;
use super::storage::{Package, PackageSource, Registry};
use crate::server::core::account_store::plugin_accounts;
use crate::server::core::cloud_sync;
use crate::server::core::plugin_host;
use crate::server::core::providers;
use crate::server::db::Db;

mod transfer;
pub(crate) use transfer::{export_for_providers, prepare_import};

/// `kv` 表中的保留键名（已登记到 `db::schema::RESERVED_KV_KEYS` 与云同步白名单）。
pub(crate) const KV_KEY: &str = "scriptPluginIdentities";

const MAX_SYNCED_PACKAGES: usize = 256;
const MAX_ALIASES_PER_PACKAGE: usize = 64;

fn version_one() -> u32 {
    1
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct IdentityRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SyncedIdentities {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, IdentityRecord>,
}

impl Default for SyncedIdentities {
    fn default() -> Self {
        Self {
            version: version_one(),
            packages: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReconcileOutcome {
    pub changed: bool,
    pub needs_reload: bool,
}

pub(crate) fn shared_db() -> Option<Db> {
    plugin_accounts::shared().and_then(|store| store.database().cloned())
}

/// 查询账号表中实际出现过的全部提供商 ID（含当前隐藏的账号）。
///
/// 数据库不可用或查询失败时返回 `None`，调用方必须保守处理、不做清理。
pub(crate) fn account_providers(db: &Db) -> Option<HashSet<String>> {
    db.with(|conn| {
        let mut stmt = conn.prepare("SELECT DISTINCT provider FROM accounts")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut providers = HashSet::new();
        for row in rows {
            let provider = row?;
            let trimmed = provider.trim();
            if !trimmed.is_empty() {
                providers.insert(trimmed.to_string());
            }
        }
        Ok::<_, rusqlite::Error>(providers)
    })
    .and_then(Result::ok)
}

pub(crate) fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().next().is_some_and(|ch| ch.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
        && !id.starts_with("custom-")
}

fn valid_original_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().next().is_some_and(|ch| ch.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
}

fn valid_package_id(id: &str) -> bool {
    id.strip_prefix("script-").is_some_and(|hex| {
        !hex.is_empty()
            && hex.len() <= 64
            && hex.bytes().all(|ch| ch.is_ascii_hexdigit())
    })
}

fn is_builtin_or_suspended(id: &str) -> bool {
    providers::availability::BUILTIN_SUBSCRIPTION_IDS.contains(&id)
        || providers::availability::is_suspended(id)
        || providers::PROVIDERS.iter().any(|meta| meta.id == id)
}

pub(crate) fn package_aliases(package: &Package) -> BTreeMap<String, String> {
    if !package.provider_aliases.is_empty() {
        return package.provider_aliases.clone();
    }
    package
        .providers
        .iter()
        .map(|id| (id.clone(), id.clone()))
        .collect()
}

pub(crate) fn record_from_package(package: &Package) -> Option<IdentityRecord> {
    let aliases = package_aliases(package);
    if aliases.is_empty() {
        return None;
    }
    let (source, selector, repository) = match &package.source {
        Some(PackageSource::Github {
            repository,
            reference,
            ..
        }) => (
            Some(format!("github:{repository}")),
            (!reference.trim().is_empty()).then(|| reference.trim().to_string()),
            Some(repository.clone()),
        ),
        Some(PackageSource::Magpie { .. }) => (Some(format!("magpie:{}", package.name)), None, None),
        // 本地绝对路径仅在原机有效且可能含本机目录信息，不参与跨机同步。
        Some(PackageSource::Local { .. }) => (None, None, None),
        None => (Some(package.name.clone()), None, None),
    };
    Some(IdentityRecord {
        name: Some(package.name.clone()),
        source,
        selector,
        repository,
        aliases,
    })
}

fn clean_text(value: Option<&str>, limit: usize) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty()
        || trimmed.len() > limit
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// 校验并归一化外部或库内读取的身份映射，剔除非法包、本机路径与跨包冲突分配。
pub(crate) fn sanitize_synced(raw: SyncedIdentities) -> SyncedIdentities {
    let entries: Vec<_> = raw
        .packages
        .into_iter()
        .take(MAX_SYNCED_PACKAGES)
        .filter(|(pkg_id, _)| valid_package_id(pkg_id))
        .collect();
    // 统计跨包重复声明的 allocated ID：出现冲突时对冲突项全部丢弃，避免受哈希排序影响。
    let mut alloc_counts: HashMap<String, usize> = HashMap::new();
    for (_, record) in &entries {
        let mut per_pkg = HashSet::new();
        for (original, allocated) in record.aliases.iter().take(MAX_ALIASES_PER_PACKAGE) {
            if valid_original_id(original)
                && valid_provider_id(allocated)
                && !is_builtin_or_suspended(allocated)
                && per_pkg.insert(allocated.clone())
            {
                *alloc_counts.entry(allocated.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut packages = BTreeMap::new();
    for (pkg_id, record) in entries {
        let name = clean_text(record.name.as_deref(), 180)
            .filter(|name| super::packages::package_name(name) && super::packages::package_id(name) == pkg_id);
        let raw_source = clean_text(record.source.as_deref(), 260);
        let raw_selector = clean_text(record.selector.as_deref(), 200);
        let (source, selector, repository) = match raw_source.as_deref() {
            Some(source) => match InstallSource::parse(source, raw_selector.as_deref().unwrap_or("")) {
                Ok(InstallSource::Local { .. }) | Err(_) => (None, None, None),
                Ok(InstallSource::Github {
                    repository,
                    reference,
                }) => (
                    Some(format!("github:{repository}")),
                    (!reference.is_empty()).then_some(reference),
                    Some(repository),
                ),
                Ok(InstallSource::Magpie { name, version }) => (
                    Some(format!("magpie:{name}")),
                    (!version.is_empty()).then_some(version),
                    None,
                ),
                Ok(InstallSource::Npm { name, version }) => (
                    Some(name),
                    (!version.is_empty()).then_some(version),
                    None,
                ),
            },
            None => (None, None, None),
        };
        let mut aliases = BTreeMap::new();
        let mut seen_in_pkg = HashSet::new();
        for (original, allocated) in record.aliases.into_iter().take(MAX_ALIASES_PER_PACKAGE) {
            if !valid_original_id(&original)
                || !valid_provider_id(&allocated)
                || is_builtin_or_suspended(&allocated)
                || alloc_counts.get(&allocated).copied().unwrap_or(0) > 1
            {
                continue;
            }
            if seen_in_pkg.insert(allocated.clone()) {
                aliases.insert(original, allocated);
            }
        }
        if aliases.is_empty() {
            continue;
        }
        packages.insert(
            pkg_id,
            IdentityRecord {
                name,
                source,
                selector,
                repository,
                aliases,
            },
        );
    }
    SyncedIdentities {
        version: version_one(),
        packages,
    }
}

pub(crate) fn parse_synced_value(value: &Value) -> Result<SyncedIdentities, String> {
    if !value.is_object() {
        return Err("scriptPluginIdentities 必须是 JSON 对象".into());
    }
    let parsed: SyncedIdentities = serde_json::from_value(value.clone())
        .map_err(|_| "scriptPluginIdentities 格式无效".to_string())?;
    Ok(sanitize_synced(parsed))
}

pub(crate) fn load_from_db(db: &Db) -> Option<SyncedIdentities> {
    let raw: String = db
        .with(|conn| {
            conn.query_row("SELECT value FROM kv WHERE key = ?1", [KV_KEY], |row| {
                row.get::<_, String>(0)
            })
            .optional()
        })
        .and_then(Result::ok)??;
    serde_json::from_str::<SyncedIdentities>(&raw)
        .ok()
        .map(sanitize_synced)
}

/// 清理「未安装且账号库中已无任何残留账号」的历史占位，避免释放后仍永久霸占原名。
pub(crate) fn prune_unused(registry: &mut Registry, active_accounts: &HashSet<String>) -> bool {
    let installed: HashSet<String> = registry
        .packages
        .iter()
        .map(|package| package.id.clone())
        .collect();
    let before = registry.identities.len();
    registry.identities.retain(|pkg_id, aliases| {
        installed.contains(pkg_id)
            || aliases
                .values()
                .any(|provider| active_accounts.contains(provider))
    });
    registry.identities.len() != before
}

/// 构建准备写入 `kv` 的身份映射快照：合并当前已安装包与库中仍有账号的未安装包元数据。
pub(crate) fn build_synced_snapshot(
    registry: &Registry,
    existing: Option<&SyncedIdentities>,
    active_accounts: Option<&HashSet<String>>,
) -> SyncedIdentities {
    let mut packages = BTreeMap::new();
    let mut used_allocated = HashSet::new();

    for package in &registry.packages {
        if let Some(record) = record_from_package(package) {
            for allocated in record.aliases.values() {
                used_allocated.insert(allocated.clone());
            }
            packages.insert(package.id.clone(), record);
        }
    }

    let retain_uninstalled = |aliases: &BTreeMap<String, String>, used: &HashSet<String>| {
        !aliases.is_empty()
            && aliases.values().all(|id| !used.contains(id))
            && active_accounts.is_none_or(|accounts| aliases.values().any(|id| accounts.contains(id)))
    };

    if let Some(existing) = existing {
        for (pkg_id, record) in &existing.packages {
            if packages.contains_key(pkg_id) {
                continue;
            }
            if retain_uninstalled(&record.aliases, &used_allocated) {
                for allocated in record.aliases.values() {
                    used_allocated.insert(allocated.clone());
                }
                packages.insert(pkg_id.clone(), record.clone());
            }
        }
    }

    for (pkg_id, aliases) in &registry.identities {
        if packages.contains_key(pkg_id) {
            continue;
        }
        if retain_uninstalled(aliases, &used_allocated) {
            for allocated in aliases.values() {
                used_allocated.insert(allocated.clone());
            }
            packages.insert(
                pkg_id.clone(),
                IdentityRecord {
                    aliases: aliases.clone(),
                    ..IdentityRecord::default()
                },
            );
        }
    }

    sanitize_synced(SyncedIdentities {
        version: version_one(),
        packages,
    })
}

/// 将本机脚本插件身份映射同步落库；内容确实变更时通知云同步防抖队列。
pub(crate) fn sync_to_db(registry: &Registry, db: &Db) {
    let existing = load_from_db(db);
    let active_accounts = account_providers(db);
    let next = build_synced_snapshot(registry, existing.as_ref(), active_accounts.as_ref());
    if existing.is_none() && next.packages.is_empty() {
        return;
    }
    if existing.as_ref() == Some(&next) {
        return;
    }
    let Ok(serialized) = serde_json::to_string(&next) else {
        return;
    };
    let written = db
        .with(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![KV_KEY, serialized],
            )
        })
        .is_some_and(|result| result.is_ok());
    if written {
        cloud_sync::auto_sync::notify_change();
    }
}

fn remap_package_providers(package: &mut Package, new_aliases: &BTreeMap<String, String>) {
    let old_aliases = package_aliases(package);
    let mut old_to_new = HashMap::new();
    for (original, new_id) in new_aliases {
        let old_id = old_aliases
            .get(original)
            .map(String::as_str)
            .unwrap_or(original.as_str());
        if old_id != new_id {
            old_to_new.insert(old_id.to_string(), new_id.clone());
        }
    }
    if !old_to_new.is_empty() {
        let mut next_disabled = Vec::with_capacity(package.disabled_providers.len());
        for id in &package.disabled_providers {
            let mapped = old_to_new.get(id).cloned().unwrap_or_else(|| id.clone());
            if !next_disabled.contains(&mapped) {
                next_disabled.push(mapped);
            }
        }
        package.disabled_providers = next_disabled;
        if let Some(options) = package.provider_options.as_object_mut() {
            for (old_id, new_id) in &old_to_new {
                if !options.contains_key(new_id) {
                    if let Some(value) = options.remove(old_id) {
                        options.insert(new_id.clone(), value);
                    }
                }
            }
        }
    }
    let mut next_providers = Vec::new();
    for old_id in &package.providers {
        let mapped = old_to_new.get(old_id).cloned().unwrap_or_else(|| old_id.clone());
        if new_aliases.values().any(|id| id == &mapped) && !next_providers.contains(&mapped) {
            next_providers.push(mapped);
        }
    }
    for new_id in new_aliases.values() {
        if !next_providers.contains(new_id) {
            next_providers.push(new_id.clone());
        }
    }
    package.provider_aliases = new_aliases.clone();
    package.providers = next_providers;
}

/// 将数据库中的同步身份映射应用到本机 `registry`（用于云同步下载、整库恢复与启动对齐）。
pub(crate) fn apply_synced(
    registry: &mut Registry,
    synced: &SyncedIdentities,
    active_accounts: &HashSet<String>,
) -> ReconcileOutcome {
    let installed: HashSet<String> = registry
        .packages
        .iter()
        .map(|package| package.id.clone())
        .collect();
    let mut effective_remote: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut remote_owners: HashMap<String, String> = HashMap::new();

    for (pkg_id, record) in &synced.packages {
        if record.aliases.is_empty() {
            continue;
        }
        let relevant = installed.contains(pkg_id)
            || record
                .aliases
                .values()
                .any(|provider| active_accounts.contains(provider));
        if !relevant {
            continue;
        }
        for allocated in record.aliases.values() {
            remote_owners.insert(allocated.clone(), pkg_id.clone());
        }
        effective_remote.insert(pkg_id.clone(), record.aliases.clone());
    }

    let mut outcome = ReconcileOutcome::default();

    // 1. 将远端记录的包映射写入本机，并同步更新已安装同包的 provider 列表。
    for (pkg_id, remote_aliases) in &effective_remote {
        if registry.identities.get(pkg_id) != Some(remote_aliases) {
            registry
                .identities
                .insert(pkg_id.clone(), remote_aliases.clone());
            outcome.changed = true;
        }
        if let Some(package) = registry.packages.iter_mut().find(|item| &item.id == pkg_id) {
            let current_aliases = package_aliases(package);
            let providers_match = remote_aliases
                .values()
                .all(|id| package.providers.contains(id))
                && package.providers.len() == remote_aliases.len();
            if current_aliases != *remote_aliases || !providers_match {
                remap_package_providers(package, remote_aliases);
                outcome.changed = true;
                if package.enabled {
                    outcome.needs_reload = true;
                }
            }
        }
    }

    // 2. 本机装有远端未记录的其他包时，若其现用 ID 与远端有账号的包撞车，清空其冲突别名以便重载重分配。
    for package in &mut registry.packages {
        if effective_remote.contains_key(&package.id) {
            continue;
        }
        let collides = package_aliases(package)
            .values()
            .any(|id| remote_owners.get(id).is_some_and(|owner| owner != &package.id));
        if collides {
            package.provider_aliases.retain(|_, allocated| {
                remote_owners
                    .get(allocated)
                    .is_none_or(|owner| owner == &package.id)
            });
            package.providers.retain(|allocated| {
                remote_owners
                    .get(allocated)
                    .is_none_or(|owner| owner == &package.id)
            });
            if let Some(saved) = registry.identities.get_mut(&package.id) {
                saved.retain(|_, allocated| {
                    remote_owners
                        .get(allocated)
                        .is_none_or(|owner| owner == &package.id)
                });
                if saved.is_empty() {
                    registry.identities.remove(&package.id);
                }
            }
            outcome.changed = true;
            if package.enabled {
                outcome.needs_reload = true;
            }
        }
    }

    // 3. 移除与远端归属冲突的本机未安装旧占位。
    let before_ids = registry.identities.len();
    registry.identities.retain(|pkg_id, aliases| {
        effective_remote.contains_key(pkg_id)
            || installed.contains(pkg_id)
            || !aliases
                .values()
                .any(|id| remote_owners.get(id).is_some_and(|owner| owner != pkg_id))
    });
    if registry.identities.len() != before_ids {
        outcome.changed = true;
    }

    if prune_unused(registry, active_accounts) {
        outcome.changed = true;
    }

    outcome
}

/// 启动阶段对齐：合并数据库里的新身份映射（如整库拷贝/云同步后重启）并清理无账号僵尸占位。
pub(crate) fn reconcile_on_startup(registry: &mut Registry, db: &Db) -> bool {
    let Some(active_accounts) = account_providers(db) else {
        return false;
    };
    if let Some(synced) = load_from_db(db) {
        apply_synced(registry, &synced, &active_accounts).changed
    } else {
        prune_unused(registry, &active_accounts)
    }
}

/// 检查 `alias` 是否是 `original` 在 `identity.mjs` 规则下的合法身份或冲突派生名。
pub(crate) fn is_compatible_alias(original: &str, alias: &str, package_id: &str) -> bool {
    if !valid_original_id(original) || !valid_provider_id(alias) {
        return false;
    }
    if alias == original {
        return true;
    }
    let prefix = if original.starts_with("custom-") {
        "magpie-"
    } else {
        ""
    };
    let truncated = &original[..original.len().min(90)];
    let base = format!("{prefix}{truncated}-plugin");
    if alias == base {
        return true;
    }
    let suffix = &format!("{:x}", Sha256::digest(package_id.as_bytes()))[..12];
    alias == format!("{base}-{suffix}")
}

/// 手动将某个冲突别名（如旧版云同步带来的 `teleagent-plugin`）关联到本机已安装的同源脚本插件。
pub(crate) fn bind_alias_in_registry(
    registry: &mut Registry,
    package_id: &str,
    original: &str,
    alias: &str,
    active_accounts: &HashSet<String>,
    reserved_native: &HashSet<String>,
) -> Result<(), String> {
    let original = original.trim();
    let alias = alias.trim();
    if !is_compatible_alias(original, alias, package_id) {
        return Err("目标提供商标识与该脚本插件不匹配，未执行关联".into());
    }
    if is_builtin_or_suspended(alias) || reserved_native.contains(alias) {
        return Err("目标提供商标识已被内置或原生插件占用".into());
    }
    if !active_accounts.contains(alias) {
        return Err("账号库中没有等待恢复的该提供商账号".into());
    }

    // 清理无账号的已卸载占位，避免已卸旧包挡住关联。
    prune_unused(registry, active_accounts);

    if registry
        .packages
        .iter()
        .any(|pkg| pkg.id != package_id && pkg.providers.iter().any(|id| id == alias))
    {
        return Err("目标提供商标识已被另一个已安装插件占用".into());
    }
    if registry.identities.iter().any(|(owner, aliases)| {
        owner != package_id && aliases.values().any(|id| id == alias)
    }) {
        return Err("目标提供商标识已归属另一插件的历史账号，未覆盖".into());
    }

    let Some(package) = registry.packages.iter_mut().find(|pkg| pkg.id == package_id) else {
        return Err("脚本插件不存在".into());
    };
    let mut aliases = package_aliases(package);
    let Some(current) = aliases.get(original).cloned() else {
        return Err("该脚本插件不提供对应的原始提供商".into());
    };
    if current == alias {
        return Ok(());
    }
    if active_accounts.contains(&current) {
        return Err(format!(
            "当前插件在「{current}」下已有账号，无法直接切换为「{alias}」"
        ));
    }
    aliases.insert(original.to_string(), alias.to_string());
    remap_package_providers(package, &aliases);
    registry.identities.insert(package_id.to_string(), aliases);
    Ok(())
}

pub(crate) fn native_reserved_ids() -> HashSet<String> {
    let native_registry = plugin_host::PackRegistry::load();
    let mut reserved: HashSet<String> = plugin_host::ownership::owners(&native_registry)
        .keys()
        .cloned()
        .collect();
    for pack in native_registry.packs.iter().filter(|pack| {
        plugin_host::support::installed_state(pack) == plugin_host::PackSupportState::Supported
    }) {
        reserved.extend(plugin_host::ownership::declared_ids(pack));
    }
    for info in plugin_host::runtime::provider_infos() {
        if plugin_host::runtime::host_for_provider(info.id.as_str())
            .is_some_and(|owner| !owner.options.script)
        {
            reserved.insert(info.id.0);
        }
    }
    reserved
}

/// `/api/plugins` 返回给前端的脚本身份映射（用于缺失插件精准匹配与冲突别名恢复）。
pub(crate) fn snapshot_json() -> Value {
    let Some(db) = shared_db() else {
        return json!({});
    };
    let synced = load_from_db(&db).unwrap_or_default();
    serde_json::to_value(synced.packages).unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_package(id: &str, name: &str, aliases: &[(&str, &str)], enabled: bool) -> Package {
        let provider_aliases: BTreeMap<String, String> = aliases
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let providers: Vec<String> = provider_aliases.values().cloned().collect();
        Package {
            id: id.into(),
            name: name.into(),
            version: "0.2.1".into(),
            icon: None,
            provider_icons: BTreeMap::new(),
            source: Some(PackageSource::Github {
                repository: format!("goodtome/{name}"),
                reference: "main".into(),
                commit: "0".repeat(40),
                sha256: "0".repeat(64),
            }),
            directory: format!("{id}/dir"),
            enabled,
            installed_at: 0,
            providers,
            disabled_providers: Vec::new(),
            options: json!({}),
            provider_options: json!({}),
            settings_schema: json!({}),
            provider_aliases,
            error: None,
        }
    }

    #[test]
    fn prune_unused_releases_uninstalled_without_accounts_and_keeps_with_accounts() {
        let pkg_a = super::super::packages::package_id("opencode-teleagent-auth");
        let pkg_b = super::super::packages::package_id("magpie-teleagent-login");
        let mut registry = Registry {
            packages: vec![],
            identities: BTreeMap::from([
                (pkg_a.clone(), BTreeMap::from([("teleagent".into(), "teleagent".into())])),
                (pkg_b.clone(), BTreeMap::from([("teleagent".into(), "teleagent-plugin".into())])),
            ]),
        };
        let accounts = HashSet::from(["teleagent-plugin".to_string()]);
        assert!(prune_unused(&mut registry, &accounts));
        assert!(!registry.identities.contains_key(&pkg_a), "无账号的已卸载占位应释放");
        assert!(registry.identities.contains_key(&pkg_b), "仍有账号的已卸载占位必须保留");
    }

    #[test]
    fn apply_synced_rebinds_installed_package_to_remote_alias() {
        let pkg_b = super::super::packages::package_id("magpie-teleagent-login");
        let mut docker_registry = Registry {
            packages: vec![sample_package(
                &pkg_b,
                "magpie-teleagent-login",
                &[("teleagent", "teleagent")],
                true,
            )],
            identities: BTreeMap::from([(
                pkg_b.clone(),
                BTreeMap::from([("teleagent".into(), "teleagent".into())]),
            )]),
        };
        let synced = SyncedIdentities {
            version: 1,
            packages: BTreeMap::from([(
                pkg_b.clone(),
                IdentityRecord {
                    name: Some("magpie-teleagent-login".into()),
                    source: Some("github:goodtome/magpie-teleagent-login".into()),
                    selector: Some("main".into()),
                    repository: Some("goodtome/magpie-teleagent-login".into()),
                    aliases: BTreeMap::from([("teleagent".into(), "teleagent-plugin".into())]),
                },
            )]),
        };
        let accounts = HashSet::from(["teleagent-plugin".to_string()]);
        let outcome = apply_synced(&mut docker_registry, &synced, &accounts);
        assert!(outcome.changed);
        assert!(outcome.needs_reload);
        assert_eq!(docker_registry.packages[0].providers, vec!["teleagent-plugin"]);
        assert_eq!(
            docker_registry.packages[0].provider_aliases.get("teleagent").map(String::as_str),
            Some("teleagent-plugin")
        );
    }

    #[test]
    fn bind_alias_accepts_conflict_suffix_and_blocks_overwrite_when_accounts_exist() {
        let pkg_b = super::super::packages::package_id("magpie-teleagent-login");
        let mut registry = Registry {
            packages: vec![sample_package(
                &pkg_b,
                "magpie-teleagent-login",
                &[("teleagent", "teleagent")],
                true,
            )],
            identities: BTreeMap::new(),
        };
        let both = HashSet::from(["teleagent".to_string(), "teleagent-plugin".to_string()]);
        assert!(
            bind_alias_in_registry(&mut registry, &pkg_b, "teleagent", "teleagent-plugin", &both, &HashSet::new()).is_err(),
            "原标识下已有账号时禁止直接切换"
        );
        let only_alias = HashSet::from(["teleagent-plugin".to_string()]);
        assert!(
            bind_alias_in_registry(&mut registry, &pkg_b, "teleagent", "unrelated-plugin", &only_alias, &HashSet::new()).is_err(),
            "不兼容的无关标识必须拒绝"
        );
        bind_alias_in_registry(&mut registry, &pkg_b, "teleagent", "teleagent-plugin", &only_alias, &HashSet::new())
            .expect("同源冲突别名应允许关联");
        assert_eq!(registry.packages[0].providers, vec!["teleagent-plugin"]);
    }

    #[test]
    fn sanitize_synced_strips_local_sources_and_conflicts() {
        let pkg_a = super::super::packages::package_id("pkg-alpha");
        let pkg_b = super::super::packages::package_id("pkg-beta");
        let cleaned = sanitize_synced(SyncedIdentities {
            version: 1,
            packages: BTreeMap::from([
                (
                    pkg_a.clone(),
                    IdentityRecord {
                        name: Some("pkg-alpha".into()),
                        source: Some("file:/etc/passwd".into()),
                        aliases: BTreeMap::from([
                            ("safe".into(), "safe-provider".into()),
                            ("teleagent".into(), "teleagent".into()),
                        ]),
                        ..IdentityRecord::default()
                    },
                ),
                (
                    pkg_b.clone(),
                    IdentityRecord {
                        name: Some("pkg-beta".into()),
                        source: Some("github:goodtome/magpie-teleagent-login".into()),
                        aliases: BTreeMap::from([
                            ("teleagent".into(), "teleagent".into()),
                            ("other".into(), "workbuddy".into()),
                        ]),
                        ..IdentityRecord::default()
                    },
                ),
            ]),
        });
        let alpha = cleaned.packages.get(&pkg_a).expect("合法别名应保留");
        assert_eq!(alpha.source.as_deref(), None, "本地 file: 路径不得参与同步");
        assert_eq!(
            alpha.aliases,
            BTreeMap::from([("safe".into(), "safe-provider".into())]),
            "跨包撞车的 teleagent 应被剔除"
        );
        assert!(!cleaned.packages.contains_key(&pkg_b), "仅剩冲突与内置 ID 的条目应被整条剔除");
    }

    #[test]
    fn cloud_sync_roundtrip_carries_script_identities_and_reconciles_docker() {
        let pkg_b = super::super::packages::package_id("magpie-teleagent-login");
        let (desktop_db, _desktop_guard) =
            crate::server::db::test_temp::TempDb::open("script-id-desktop");
        let (docker_db, _docker_guard) =
            crate::server::db::test_temp::TempDb::open("script-id-docker");

        desktop_db
            .with(|conn| {
                conn.execute(
                    "INSERT INTO accounts (id, provider, priority, enabled, added_at, data)
                     VALUES ('acc-1', 'teleagent-plugin', 1, 1, 1000, '{\"id\":\"acc-1\",\"provider\":\"teleagent-plugin\"}')",
                    [],
                )
            })
            .unwrap()
            .unwrap();

        let desktop_registry = Registry {
            packages: vec![sample_package(
                &pkg_b,
                "magpie-teleagent-login",
                &[("teleagent", "teleagent-plugin")],
                true,
            )],
            identities: BTreeMap::from([(
                pkg_b.clone(),
                BTreeMap::from([("teleagent".into(), "teleagent-plugin".into())]),
            )]),
        };
        sync_to_db(&desktop_registry, &desktop_db);

        let snap = cloud_sync::snapshot::build(&desktop_db).expect("桌面端应生成云同步快照");
        assert!(snap.kv.contains_key(KV_KEY), "云同步快照必须携带 scriptPluginIdentities");
        cloud_sync::snapshot::apply(&docker_db, &snap, None).expect("Docker 端应成功应用快照");

        let mut docker_registry = Registry {
            packages: vec![sample_package(
                &pkg_b,
                "magpie-teleagent-login",
                &[("teleagent", "teleagent")],
                true,
            )],
            identities: BTreeMap::from([(
                pkg_b.clone(),
                BTreeMap::from([("teleagent".into(), "teleagent".into())]),
            )]),
        };
        assert!(reconcile_on_startup(&mut docker_registry, &docker_db));
        assert_eq!(docker_registry.packages[0].providers, vec!["teleagent-plugin"]);
        assert_eq!(
            docker_registry.packages[0].provider_aliases.get("teleagent").map(String::as_str),
            Some("teleagent-plugin")
        );
    }
}
