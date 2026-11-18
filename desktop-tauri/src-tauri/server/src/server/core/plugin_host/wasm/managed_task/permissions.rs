//! 管理端以明确确认和真实安装来源形成批准；执行门关闭时仅展示与撤销旧记录。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    declaration::{self, Rules},
    permission_install,
    permission_store::Grants,
    policy,
};
use crate::server::core::plugin_host::{
    self as host,
    install::{InstalledPack, PackRegistry},
};

const EXECUTION_READY: bool = true;
const NOT_READY: &str = "当前主程序未开放受控任务执行权限";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    pack: String,
    #[serde(flatten)]
    rules: Rules,
    caller_digest: String,
    module_digest: String,
    declaration_digest: String,
    available: bool,
    authorized: bool,
    grant_present: bool,
    execution_ready: bool,
}

pub(crate) async fn snapshot(pack_id: &str) -> Result<Value, String> {
    let _mutation = host::operations::mutation_guard().await;
    snapshot_current(pack_id)
}

fn snapshot_current(pack_id: &str) -> Result<Value, String> {
    if !declaration::pack_id(pack_id) {
        return Err("插件标识无效".into());
    }
    let registry = PackRegistry::load();
    let pack = permission_install::find_pack(&registry, pack_id)?;
    let grants = Grants::load()?;
    snapshot_with(pack, &registry, &grants)
}

fn snapshot_with(
    pack: &InstalledPack,
    registry: &PackRegistry,
    grants: &Grants,
) -> Result<Value, String> {
    let pack_id = pack.id.as_str();
    let rules = declaration::rules_for(pack.manifest_providers.first().map(|provider| provider.id.as_str()).unwrap_or(""));
    let grant_present = grants.present(pack_id, rules.provider, rules.profile);
    let installed = permission_install::inspect(pack).ok();
    if installed.is_none() && !grant_present {
        return Ok(json!({ "managedTasks": [] }));
    }
    let mut row = Row {
        pack: pack_id.to_string(),
        rules,
        caller_digest: if declaration::digest_text(&pack.sha256) {
            pack.sha256.clone()
        } else {
            String::new()
        },
        module_digest: String::new(),
        declaration_digest: String::new(),
        available: false,
        authorized: false,
        grant_present,
        execution_ready: EXECUTION_READY,
    };
    if let Some(installed) = installed {
        row.declaration_digest = installed.declaration_digest.clone();
        let owners = host::ownership::owners(registry);
        if let Some(loaded) = permission_install::loaded(pack, &installed, &owners) {
            // 只公开实际加载字节的摘要；文件变化或代次不可确认时留空，而非回退磁盘摘要。
            row.module_digest = loaded.module_digest().to_string();
            if EXECUTION_READY {
                if let Some(scope) = loaded.installation_scope() {
                    row.available = true;
                    row.authorized = super::approval_fence::installation_confirmed(&scope);
                }
            }
        }
    }
    // 有声明或有旧记录都不能把尚未接通的执行能力显示为可用/已授权。
    Ok(json!({ "managedTasks": [row] }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawChange {
    kind: String,
    pack: String,
    provider: String,
    profile: String,
    caller_digest: String,
    module_digest: String,
    declaration_digest: String,
    authorized: bool,
    confirmed: bool,
}

pub(crate) struct Change(RawChange);

impl Change {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let change: RawChange = declaration::parse_object(bytes, 16 * 1024)
            .map_err(|_| "受控任务授权请求字段无效或存在重复字段")?;
        if change.kind != "managedTask"
            || !declaration::pack_id(&change.pack)
            || policy::profile(&change.provider) != Some(change.profile.as_str())
            || [
                &change.caller_digest,
                &change.module_digest,
                &change.declaration_digest,
            ]
            .iter()
            .any(|text| !declaration::digest_text(text) && (change.authorized || !text.is_empty()))
        {
            return Err("受控任务授权参数无效".into());
        }
        Ok(Self(change))
    }

    pub(crate) fn pack(&self) -> &str {
        &self.0.pack
    }
}

/// 只在已认证管理configure的ready分支核对后构造；不提供JSON/摘要构造器。
pub(in crate::server) struct Confirmation {
    operation: Arc<ConfirmationIdentity>,
    loaded: permission_install::Loaded,
    installed: permission_install::Installed,
    pack: InstalledPack,
    scope: Arc<permission_install::InstallationScope>,
}

pub(super) struct ConfirmationIdentity {
    _private: (),
}

impl Confirmation {
    pub(super) fn pack(&self) -> &str {
        &self.pack.id
    }

    pub(super) fn operation(&self) -> &Arc<ConfirmationIdentity> {
        &self.operation
    }

    pub(super) fn scope(&self) -> &Arc<permission_install::InstallationScope> {
        &self.scope
    }

    pub(in crate::server) fn current_for_save(&self) -> bool {
        // 配置更新锁内仅try宿主门，避免与持Host的普通配置写者互相等待。
        self.loaded
            .with_current_memory(|| self.scope.try_hold().is_ok())
            .unwrap_or(false)
    }

    pub(super) fn with_current_memory<R>(&self, inspect: impl FnOnce() -> R) -> Option<R> {
        self.loaded.with_current_memory(inspect)
    }

    pub(in crate::server) fn updated_grants(
        &self,
        current: Option<&Value>,
    ) -> Result<Value, String> {
        Grants::with_confirmation(current, self)
    }

    fn response_row(&self, grants: &Grants) -> Result<Value, String> {
        let rules = declaration::rules_for(&self.installed.options.provider);
        let grant_present = grants.present(&self.pack.id, rules.provider, rules.profile);
        // 与本次待保存材料同源；编码先于保存，不再对Loaded/Lease做第二次展示读取。
        serde_json::to_value(Row {
            pack: self.pack.id.clone(),
            rules,
            caller_digest: self.pack.sha256.clone(),
            module_digest: self.loaded.module_digest().to_string(),
            declaration_digest: self.installed.declaration_digest.clone(),
            available: false,
            authorized: false,
            grant_present,
            execution_ready: EXECUTION_READY,
        })
        .map_err(|_| "受控任务授权状态无法编码".to_string())
    }

    pub(super) fn grant_parts(
        &self,
    ) -> (
        &InstalledPack,
        &permission_install::Installed,
        &permission_install::Loaded,
    ) {
        (&self.pack, &self.installed, &self.loaded)
    }
}

fn validate_request(change: &RawChange, registry: &PackRegistry) -> Result<Confirmation, String> {
    if !change.authorized || !change.confirmed {
        return Err("请先明确确认受控任务权限".into());
    }
    let pack = permission_install::find_pack(registry, &change.pack)?;
    let installed = permission_install::inspect(pack)?;
    let owners = host::ownership::owners(registry);
    let loaded = permission_install::loaded(pack, &installed, &owners)
        .ok_or("插件未启用、实际运行模块不可确认或不再拥有该提供商")?;
    if installed.options.provider != change.provider
        || change.caller_digest != pack.sha256
        || change.module_digest != loaded.module_digest()
        || change.declaration_digest != installed.declaration_digest
    {
        return Err("插件包、实际运行模块或任务声明已变化，请重新查看后确认".into());
    }
    if !loaded.still_current(pack, &installed, &owners) {
        return Err("插件运行代次或提供商归属已变化，请重新查看".into());
    }
    let scope = Arc::new(
        loaded
            .installation_scope()
            .ok_or("当前安装没有有效的受控任务声明来源")?,
    );
    Ok(Confirmation {
        operation: Arc::new(ConfirmationIdentity { _private: () }),
        loaded,
        installed,
        pack: pack.clone(),
        scope,
    })
}

pub(crate) async fn configure(change: Change) -> Result<Value, String> {
    let change = change.0;
    let _mutation = host::operations::mutation_guard().await;
    let registry = PackRegistry::load();
    let pack = permission_install::find_pack(&registry, &change.pack)?;
    if change.authorized {
        // 在任何确认材料/写入之前关闸；解析Change或装载Grants不会产生内存批准。
        if !EXECUTION_READY {
            return Err(NOT_READY.to_string());
        }
        let confirmation = validate_request(&change, &registry)?;
        let mut row = confirmation.response_row(&Grants::load()?)?;
        let result = crate::server::config::plugin_task_grants::confirm(confirmation)?;
        let authorized = EXECUTION_READY && result.saved == Some(true) && result.authorized;
        row["authorized"] = json!(authorized);
        // 失败结果不能证明当前来源仍可用，已核对摘要仅用于关联本次确认。
        row["available"] = json!(authorized);
        if result.saved == Some(true) {
            row["grantPresent"] = json!(true);
        }
        return Ok(json!({
            "managedTasks": [row],
            "saved": result.saved,
            "authorized": authorized,
            "confirmationStatus": result.message,
        }));
    }
    // 锁内读当前授权键再撤销；配置写入失败不恢复旧审批代次。
    let revoked = Grants::revoke_current(&change.pack, &change.provider, &change.profile)?;
    let mut result = snapshot_with(pack, &registry, &revoked.value)?;
    result["saved"] = json!(revoked.saved);
    Ok(result)
}
