//! 包在停止旧宿主之前完成解包与验签；登记失败时恢复同版本重装的原目录。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agent2api_provider_protocol as proto;

use super::install::{self, InstalledPack, PackRegistry, PackSummary, SignatureState};

// 安装与下载共用有限的 ZIP 硬上限；解压总量与文件数仍由 install 独立限制。
pub(crate) const MAX_PACK_BYTES: usize = 24 * 1024 * 1024;

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct PreparedPack {
    staging: PathBuf,
    manifest: proto::PackManifest,
    entry: String,
    sha256: String,
    signed: bool,
}

impl Drop for PreparedPack {
    fn drop(&mut self) {
        if self.staging.exists() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
}

pub(super) fn ensure_entry_executable(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("读取插件入口失败（{}）：{error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("插件入口不是普通文件：{}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode();
        if mode & 0o100 == 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o100))
                .map_err(|error| format!("设置插件入口执行权限失败（{}）：{error}", path.display()))?;
            let updated = std::fs::metadata(path)
                .map_err(|error| format!("读取插件入口权限失败（{}）：{error}", path.display()))?;
            if updated.permissions().mode() & 0o100 == 0 {
                return Err(format!(
                    "插件入口仍不可执行，请检查数据目录是否支持 Unix 执行权限：{}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

pub fn prepare(
    bytes: &[u8],
    expected_id: Option<&str>,
    expected_version: Option<&str>,
    require_signed: bool,
) -> Result<PreparedPack, String> {
    if bytes.is_empty() || bytes.len() > MAX_PACK_BYTES {
        return Err(format!(
            "插件包必须为非空且不超过 {} MiB 的 zip",
            MAX_PACK_BYTES / (1024 * 1024)
        ));
    }
    let staging = install::plugins_dir().join(".staging").join(format!(
        "pack-{}-{}-{}",
        std::process::id(),
        crate::server::logging::now_ms(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建临时目录失败：{error}"))?;
    let prepared = (|| {
        install::extract_zip(bytes, &staging)?;
        let manifest = install::read_manifest(&staging)?;
        install::validate_manifest(&manifest, proto::PROTOCOL_VERSION)?;
        if let Some(reason) = super::compatibility::version_error(&manifest.id, &manifest.version) {
            return Err(reason);
        }
        if manifest.id.is_empty()
            || !manifest
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("插件包 id 只能包含小写字母、数字和连字符".to_string());
        }
        if manifest.version.is_empty()
            || !manifest
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
            || crate::server::core::update::compare_versions(&manifest.version, &manifest.version)
                .is_none()
        {
            return Err("插件包版本号不是合法的语义化版本".to_string());
        }
        if expected_id.is_some_and(|id| id != manifest.id)
            || expected_version.is_some_and(|version| version != manifest.version)
        {
            return Err("下载包的 id 或版本与所选发行版不一致，已拒绝安装".to_string());
        }
        let target = super::host_target();
        let entry = manifest
            .entry
            .get(&target)
            .cloned()
            .ok_or_else(|| format!("插件包没有为当前平台（{target}）提供入口"))?;
        if !staging.join(&entry).is_file() {
            return Err(format!("包里的入口二进制不存在：{entry}"));
        }
        let (signature, problem) = install::verify_signature(&staging, &manifest, &target);
        if signature == SignatureState::Invalid {
            return Err(format!("签名校验失败：{}", problem.unwrap_or_default()));
        }
        if require_signed && signature != SignatureState::Verified {
            return Err("自动安装只接受通过官方签名校验的插件包，请手动安装未签名包".to_string());
        }
        ensure_entry_executable(&staging.join(&entry))?;
        Ok(PreparedPack {
            staging: staging.clone(),
            manifest,
            entry,
            sha256: install::sha256_hex(bytes),
            signed: signature.is_signed(),
        })
    })();
    if prepared.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    prepared
}

impl PreparedPack {
    pub fn id(&self) -> &str { &self.manifest.id }

    pub(super) fn record(&self, registry: &PackRegistry) -> InstalledPack {
        let previous = registry.packs.iter().find(|pack| pack.id == self.manifest.id);
        let providers = self.manifest.providers.iter().map(|provider| {
            let id = provider.id.0.clone();
            let enabled = previous.and_then(|pack| pack.providers.get(&id)).copied()
                .or_else(|| super::ownership::migrated_enabled(registry, &self.manifest.id, &id))
                .unwrap_or(provider.default_enabled);
            (id, enabled)
        }).collect();
        InstalledPack {
            id: self.manifest.id.clone(), version: self.manifest.version.clone(),
            enabled: previous.map(|pack| pack.enabled).unwrap_or(true), providers,
            entry: self.entry.clone(), sha256: self.sha256.clone(), signed: self.signed,
            installed_at: crate::server::logging::now_ms(),
            manifest_providers: self.manifest.providers.clone(),
        }
    }

    pub fn install(self) -> Result<PackSummary, String> {
        let transaction = self.install_pending()?;
        let id = transaction.id.clone();
        transaction.commit();
        install::list_packs().into_iter().find(|pack| pack.id == id)
            .ok_or_else(|| "安装后读不到注册表条目".to_string())
    }

    pub(super) fn install_pending(self) -> Result<InstalledTransaction, String> {
        let mut registry = PackRegistry::load();
        let previous_registry = registry.clone();
        let record = self.record(&registry);
        let version_dir = install::plugins_dir()
            .join(&self.manifest.id)
            .join(&self.manifest.version);
        let parent = version_dir
            .parent()
            .ok_or_else(|| "插件目录无效".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| format!("创建包目录失败：{error}"))?;
        let backup = parent.join(format!(
            ".previous-{}-{}",
            crate::server::logging::now_ms(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let replaced = version_dir.exists();
        if replaced {
            install::move_dir_retrying(&version_dir, &backup)
                .map_err(|error| format!("备份旧版本失败：{error}"))?;
        }
        if let Err(error) = install::move_dir_retrying(&self.staging, &version_dir) {
            if replaced {
                install::move_dir_retrying(&backup, &version_dir).map_err(|restore| {
                    format!("落盘失败：{error}；恢复旧文件失败：{restore}；备份保留在 {}", backup.display())
                })?;
            }
            return Err(format!("落盘失败：{error}"));
        }
        match registry.packs.iter_mut().find(|pack| pack.id == record.id) {
            Some(slot) => *slot = record,
            None => registry.packs.push(record),
        }
        if let Err(error) = registry.save() {
            install::remove_dir_retrying(&version_dir)
                .map_err(|restore| format!("保存插件登记失败：{error}；移除新文件失败：{restore}；旧备份已保留"))?;
            if replaced {
                install::move_dir_retrying(&backup, &version_dir).map_err(|restore| {
                    format!("保存插件登记失败：{error}；恢复旧文件失败：{restore}；备份保留在 {}", backup.display())
                })?;
            }
            return Err(error);
        }
        Ok(InstalledTransaction {
            id: self.manifest.id.clone(), previous_registry, version_dir,
            backup: replaced.then_some(backup),
        })
    }
}

pub(super) struct InstalledTransaction {
    id: String,
    previous_registry: PackRegistry,
    version_dir: PathBuf,
    backup: Option<PathBuf>,
}

impl InstalledTransaction {
    pub fn commit(self) {
        if let Some(backup) = self.backup {
            if let Err(error) = install::remove_dir_retrying(&backup) {
                crate::server::logging::log("[Plugin]", &format!("插件更新成功，但临时备份清理失败：{error}"));
            }
        }
    }

    pub fn rollback(self) -> Result<(), String> {
        install::remove_dir_retrying(&self.version_dir)
            .map_err(|error| format!("移除未能启动的插件文件失败，旧版本备份已保留：{error}"))?;
        if let Some(backup) = self.backup {
            install::move_dir_retrying(&backup, &self.version_dir)
                .map_err(|error| format!("恢复插件文件失败，旧版本备份已保留：{error}"))?;
        }
        self.previous_registry.save().map_err(|error| format!("恢复插件登记失败：{error}"))
    }
}
