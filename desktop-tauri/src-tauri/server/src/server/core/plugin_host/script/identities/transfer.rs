//! 账号导出 / 导入时的脚本插件身份映射（同包自动重映射、未安装包无冲突预登记）。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    account_providers, build_synced_snapshot, is_builtin_or_suspended, load_from_db,
    native_reserved_ids, package_aliases, parse_synced_value, prune_unused, shared_db,
    valid_provider_id, version_one, IdentityRecord, SyncedIdentities, KV_KEY,
};

/// 账号导出时，抽取与本次导出提供商集合相关的脚本插件包身份映射。
pub(crate) fn export_for_providers(providers: &HashSet<String>) -> Option<Value> {
    if providers.is_empty() {
        return None;
    }
    let registry = super::super::storage::read().ok().unwrap_or_default();
    let existing = shared_db().and_then(|db| load_from_db(&db));
    let all = build_synced_snapshot(&registry, existing.as_ref(), None);
    let mut selected = BTreeMap::new();
    for (pkg_id, record) in all.packages {
        let matched: BTreeMap<String, String> = record
            .aliases
            .into_iter()
            .filter(|(_, allocated)| providers.contains(allocated))
            .collect();
        if !matched.is_empty() {
            selected.insert(
                pkg_id,
                IdentityRecord {
                    aliases: matched,
                    ..record
                },
            );
        }
    }
    if selected.is_empty() {
        return None;
    }
    serde_json::to_value(SyncedIdentities {
        version: version_one(),
        packages: selected,
    })
    .ok()
}

fn allocate_non_conflicting_alias(
    original: &str,
    package_id: &str,
    occupied: &HashSet<String>,
) -> Option<String> {
    if valid_provider_id(original)
        && !is_builtin_or_suspended(original)
        && !occupied.contains(original)
    {
        return Some(original.to_string());
    }
    let prefix = if original.starts_with("custom-") {
        "magpie-"
    } else {
        ""
    };
    let truncated = &original[..original.len().min(90)];
    let candidate = format!("{prefix}{truncated}-plugin");
    if valid_provider_id(&candidate)
        && !is_builtin_or_suspended(&candidate)
        && !occupied.contains(&candidate)
    {
        return Some(candidate);
    }
    let suffix = &format!("{:x}", Sha256::digest(package_id.as_bytes()))[..12];
    let fallback = format!("{candidate}-{suffix}");
    (valid_provider_id(&fallback)
        && !is_builtin_or_suspended(&fallback)
        && !occupied.contains(&fallback))
    .then_some(fallback)
}

/// 账号导入前预处理 `scriptPluginIdentities`：
/// - 若目标机器已安装同一脚本插件包，则将导入记录的 provider 映射到本机该包当前运行 ID；
/// - 若尚未安装，则在避免撞车的前提下预登记其身份，允许账号安全导入并可后续一键补全插件。
pub(crate) fn prepare_import(
    raw: &Value,
    imported_providers: &HashSet<String>,
) -> Result<HashMap<String, String>, String> {
    let incoming = parse_synced_value(raw)?;
    let mut registry = super::super::storage::read().unwrap_or_default();
    let db = shared_db();
    let active_accounts = db.as_ref().and_then(account_providers);
    if let Some(accounts) = active_accounts.as_ref() {
        prune_unused(&mut registry, accounts);
    }
    let mut occupied = native_reserved_ids();
    for package in &registry.packages {
        for id in package_aliases(package).values() {
            occupied.insert(id.clone());
        }
    }
    for aliases in registry.identities.values() {
        for id in aliases.values() {
            occupied.insert(id.clone());
        }
    }

    let mut remap = HashMap::new();
    let mut pending_meta: BTreeMap<String, IdentityRecord> = BTreeMap::new();
    let mut registry_changed = false;

    for (pkg_id, record) in incoming.packages {
        let used_aliases: BTreeMap<String, String> = record
            .aliases
            .iter()
            .filter(|(_, allocated)| imported_providers.contains(*allocated))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if used_aliases.is_empty() {
            continue;
        }
        if let Some(local_pkg) = registry.packages.iter().find(|pkg| pkg.id == pkg_id) {
            let local_aliases = package_aliases(local_pkg);
            for (original, remote_allocated) in used_aliases {
                if let Some(local_allocated) = local_aliases.get(&original) {
                    if local_allocated != &remote_allocated {
                        remap.insert(remote_allocated, local_allocated.clone());
                    }
                }
            }
            continue;
        }
        let existing_for_pkg = registry.identities.get(&pkg_id).cloned().unwrap_or_default();
        let mut next_for_pkg = existing_for_pkg.clone();
        for (original, remote_allocated) in used_aliases {
            if let Some(existing_alloc) = next_for_pkg.get(&original) {
                if existing_alloc != &remote_allocated {
                    remap.insert(remote_allocated, existing_alloc.clone());
                }
                continue;
            }
            let target_alloc = if !occupied.contains(&remote_allocated) {
                remote_allocated.clone()
            } else {
                let Some(allocated) =
                    allocate_non_conflicting_alias(&original, &pkg_id, &occupied)
                else {
                    continue;
                };
                allocated
            };
            if target_alloc != remote_allocated {
                remap.insert(remote_allocated, target_alloc.clone());
            }
            occupied.insert(target_alloc.clone());
            next_for_pkg.insert(original, target_alloc);
        }
        if next_for_pkg != existing_for_pkg {
            registry
                .identities
                .insert(pkg_id.clone(), next_for_pkg.clone());
            registry_changed = true;
        }
        if !next_for_pkg.is_empty() {
            pending_meta.insert(
                pkg_id,
                IdentityRecord {
                    aliases: next_for_pkg,
                    ..record
                },
            );
        }
    }

    if registry_changed {
        let _ = super::super::storage::save(&registry);
    }
    if let Some(db) = db {
        if !pending_meta.is_empty() {
            let mut merged = load_from_db(&db).unwrap_or_default();
            for (pkg_id, record) in pending_meta {
                merged.packages.entry(pkg_id).or_insert(record);
            }
            let next = build_synced_snapshot(&registry, Some(&merged), None);
            if let Ok(serialized) = serde_json::to_string(&next) {
                let _ = db.with(|conn| {
                    conn.execute(
                        "INSERT INTO kv (key, value) VALUES (?1, ?2)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        rusqlite::params![KV_KEY, serialized],
                    )
                });
            }
        }
    }
    Ok(remap)
}
