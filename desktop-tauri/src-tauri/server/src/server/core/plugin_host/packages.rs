//! 包在停止旧宿主之前完成解包与验签；登记失败时恢复同版本重装的原目录。

pub(super) mod provenance;

use std::path::PathBuf;
use std::sync::Arc;
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
    artifact: Option<Arc<provenance::VerifiedArtifact>>,
}

impl Drop for PreparedPack {
    fn drop(&mut self) {
        if self.staging.exists() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
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
        let extracted = install::extract_zip(bytes, &staging)?;
        let manifest = extracted.manifest()?;
        install::validate_manifest(&manifest, proto::PROTOCOL_VERSION)?;
        if manifest.id == super::script::HOST_ID || super::script::is_package(&manifest.id) {
            return Err("该插件标识保留给脚本运行环境，不能安装为原生插件".to_string());
        }
        let is_wasm = extracted.validate(&staging, &manifest)?;
        if manifest.id.is_empty() || manifest.id.len() > 128
            || !manifest
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("插件包 id 只能包含小写字母、数字和连字符".to_string());
        }
        if manifest.version.is_empty() || manifest.version.len() > 128
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
        let entry = install::entry_for_target(&manifest, &target)
            .ok_or_else(|| format!("插件包没有为当前平台（{target}）提供入口"))?;
        if !staging.join(&entry).is_file() {
            return Err(format!("包里的入口二进制不存在：{entry}"));
        }
        let signature = extracted.signature(&manifest, &target)?;
        if require_signed && signature != SignatureState::Verified {
            return Err("此下载方式只接受通过可信签名校验的插件包；未签名包请从原来源下载后手动安装".to_string());
        }
        if !is_wasm { return Err(super::support::LEGACY_REASON.to_string()); }
        let artifact = provenance::VerifiedArtifact::from_prepared(extracted, &manifest, &entry, &target, signature, &staging)?;
        Ok(PreparedPack {
            staging: staging.clone(),
            manifest,
            entry,
            sha256: install::sha256_hex(bytes),
            signed: signature.is_signed(),
            artifact,
        })
    })();
    if prepared.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    prepared
}

impl PreparedPack {
    pub fn id(&self) -> &str { &self.manifest.id }

    pub(crate) fn preview(&self) -> serde_json::Value {
        let registry = PackRegistry::load();
        let previous = registry.packs.iter().find(|pack| pack.id == self.manifest.id);
        let record = self.record(&registry);
        serde_json::json!({
            "id": self.manifest.id,
            "name": super::pack_view::display_name(&self.manifest.name),
            "version": self.manifest.version,
            "signed": self.signed,
            "sha256": self.sha256,
            "previousVersion": previous.map(|pack| &pack.version),
            "enabled": record.enabled,
            "providers": self.manifest.providers,
        })
    }

    pub(super) fn record(&self, registry: &PackRegistry) -> InstalledPack {
        let previous = registry.packs.iter().find(|pack| pack.id == self.manifest.id);
        let providers = self.manifest.providers.iter().map(|provider| {
            let id = provider.id.0.clone();
            let enabled = previous.and_then(|pack| pack.providers.get(&id)).copied()
                .or_else(|| registry.packs.iter().find_map(|pack| {
                    pack.manifest_providers.iter().any(|provider| provider.id.as_str() == id)
                        .then(|| pack.enabled && pack.providers.get(&id).copied().unwrap_or(true))
                }))
                .unwrap_or(provider.default_enabled);
            (id, enabled)
        }).collect();
        InstalledPack {
            id: self.manifest.id.clone(), version: self.manifest.version.clone(),
            enabled: previous.map(|pack| pack.enabled).unwrap_or(true)
                && self.manifest.providers.iter().any(|provider|
                    !crate::server::core::providers::availability::is_suspended(provider.id.as_str())),
            providers,
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
        self.install_pending_inner(None)
    }

    pub(super) fn install_pending_changed(self, change: provenance::FileChange) -> Result<InstalledTransaction, String> {
        self.install_pending_inner(Some(change))
    }

    fn install_pending_inner(self, change: Option<provenance::FileChange>) -> Result<InstalledTransaction, String> {
        let mut registry = PackRegistry::load();
        if super::wasm::manifest::is_wasm(&self.manifest) {
            if let Some(other) = registry.packs.iter().find(|pack| pack.id != self.manifest.id
                && super::support::installed_state(pack) == super::PackSupportState::Supported && pack.manifest_providers.iter()
                    .any(|provider| self.manifest.providers.iter().any(|item| item.id == provider.id))) {
                return Err(format!("该提供商已由独立插件「{}」接管，请先卸载对应插件", other.id));
            }
        }
        let previous_registry = registry.clone();
        if let Some(provider) = self.manifest.providers.iter()
            .find(|provider| super::script::package_for_provider(provider.id.as_str()).is_some()) {
            return Err(format!("提供商 {} 已由脚本插件接管，请先停用对应脚本插件", provider.id));
        }
        let record = self.record(&registry);
        // 准备失败不撤销；开始目录/登记变更前再次核对原ZIP事实，并永久关闭旧代次。
        if let Some(artifact) = &self.artifact { artifact.recheck(&self.staging)?; }
        let change = change.unwrap_or_else(|| provenance::begin_file_change(&self.manifest.id));
        let ordinal = registry.packs.iter().position(|pack| pack.id == record.id).unwrap_or(registry.packs.len());
        let mut witness = self.artifact.as_ref().and_then(|artifact| provenance::begin_install(artifact.clone(), &record, ordinal, &change));
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
        let save = provenance::begin_registry_save(&registry, Some(&self.manifest.id));
        let save = match registry.save_with_witness(save) {
            Ok(save) => save,
            Err(error) => {
                install::remove_dir_retrying(&version_dir)
                    .map_err(|restore| format!("保存插件登记失败：{error}；移除新文件失败：{restore}；旧备份已保留"))?;
                if replaced {
                    install::move_dir_retrying(&backup, &version_dir).map_err(|restore| {
                        format!("保存插件登记失败：{error}；恢复旧文件失败：{restore}；备份保留在 {}", backup.display())
                    })?;
                }
                return Err(error);
            }
        };
        if let Some(witness) = &mut witness { witness.saved_by(&save); }
        Ok(InstalledTransaction {
            id: self.manifest.id.clone(), previous_registry, version_dir,
            backup: replaced.then_some(backup), witness, change,
        })
    }
}

pub(super) struct InstalledTransaction {
    id: String,
    previous_registry: PackRegistry,
    version_dir: PathBuf,
    backup: Option<PathBuf>,
    witness: Option<provenance::PendingInstall>,
    change: provenance::FileChange,
}

impl InstalledTransaction {
    pub(super) fn take_start_source(&mut self) -> Option<provenance::StartSource> {
        self.witness.as_mut()?.take_start_source(&self.version_dir)
    }

    pub(super) fn accept_start(&mut self, receipt: provenance::StartReceipt) -> Result<(), &'static str> {
        self.witness.as_mut().ok_or("本次安装没有可信启动来源")?.accept_start(receipt)
    }

    pub fn commit(mut self) {
        if let Some(witness) = self.witness { witness.commit(&self.version_dir, &mut self.change); }
        if let Some(backup) = self.backup {
            if let Err(error) = install::remove_dir_retrying(&backup) {
                crate::server::logging::log("[Plugin]", &format!("插件更新成功，但临时备份清理失败：{error}"));
            }
        }
    }

    pub fn rollback(self) -> Result<(), String> {
        provenance::begin_change(&self.id);
        // 只恢复普通安装；旧JSON记录没有重新签发见证的资格。
        install::remove_dir_retrying(&self.version_dir)
            .map_err(|error| format!("移除未能启动的插件文件失败，旧版本备份已保留：{error}"))?;
        if let Some(backup) = self.backup {
            install::move_dir_retrying(&backup, &self.version_dir)
                .map_err(|error| format!("恢复插件文件失败，旧版本备份已保留：{error}"))?;
        }
        self.previous_registry.save().map_err(|error| format!("恢复插件登记失败：{error}"))
    }
}
