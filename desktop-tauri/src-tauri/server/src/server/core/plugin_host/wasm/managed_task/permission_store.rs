//! 授权记录与账号来源权限独立；旧记录不证明当前模块或执行能力已获准。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    declaration::{self, Declaration},
    policy,
};
use crate::server::config;

pub(super) const KEY: &str = config::plugin_task_grants::KEY;
pub(super) const MAX_GRANTS: usize = 256;
const INVALID: &str = "受控任务授权记录格式无效";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Grant {
    pack: String,
    provider: String,
    profile: String,
    caller_digest: String,
    module_digest: String,
    declaration_digest: String,
    declaration: Declaration,
    // 旧记录可缺省，只保留可见/可撤销；新确认保存完整wasm来源，不能据此恢复批准。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_digest: Option<String>,
}

impl Grant {
    pub(super) fn from_confirmation(confirmation: &super::permissions::Confirmation) -> Self {
        let (pack, installed, loaded) = confirmation.grant_parts();
        let source_digest = loaded
            .source_digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            pack: pack.id.clone(),
            provider: installed.options.provider.clone(),
            profile: policy::profile(&installed.options.provider).unwrap_or("").to_string(),
            caller_digest: pack.sha256.clone(),
            module_digest: loaded.module_digest().to_string(),
            declaration_digest: installed.declaration_digest.clone(),
            declaration: installed.declaration.clone(),
            source_digest: Some(source_digest),
        }
    }
}

type Profiles = BTreeMap<String, Grant>;
type Providers = BTreeMap<String, Profiles>;
type Records = BTreeMap<String, Providers>;

pub(super) struct Grants(Records);

impl Grants {
    pub(super) fn load() -> Result<Self, String> {
        let value = config::with_raw(|raw| raw.get(KEY).cloned());
        let loaded = value.map(|value| Self::from_value(value.as_ref()));
        match loaded {
            Some(Ok(grants)) => Ok(grants),
            Some(Err(error)) => {
                super::approval_fence::fail_closed();
                Err(error)
            }
            None => {
                super::approval_fence::fail_closed();
                Err("受控任务授权存储尚未就绪".to_string())
            }
        }
    }

    pub(super) fn from_value(value: Option<&Value>) -> Result<Self, String> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(Self(BTreeMap::new()));
        };
        let packs = value.as_object().ok_or(INVALID)?;
        if packs.len() > MAX_GRANTS {
            return Err(INVALID.into());
        }
        let mut records = BTreeMap::new();
        let mut count = 0usize;
        for (pack, providers) in packs {
            if !declaration::pack_id(pack) {
                return Err(INVALID.into());
            }
            let providers = providers
                .as_object()
                .filter(|items| !items.is_empty() && items.len() <= 1)
                .ok_or(INVALID)?;
            let mut parsed_providers = BTreeMap::new();
            for (provider, profiles) in providers {
                if policy::profile(provider).is_none() {
                    return Err(INVALID.into());
                }
                let profiles = profiles
                    .as_object()
                    .filter(|items| !items.is_empty() && items.len() <= 1)
                    .ok_or(INVALID)?;
                let mut parsed_profiles = BTreeMap::new();
                for (profile, value) in profiles {
                    count = count.checked_add(1).ok_or(INVALID)?;
                    if count > MAX_GRANTS || policy::profile(provider) != Some(profile.as_str()) {
                        return Err(INVALID.into());
                    }
                    let fields = value.as_object().ok_or(INVALID)?;
                    if !matches!(fields.len(), 7 | 8)
                        || fields.iter().any(|(key, value)| match key.as_str() {
                            "pack" | "provider" | "profile" | "callerDigest" | "moduleDigest"
                            | "declarationDigest" | "sourceDigest" => {
                                value.as_str().is_none_or(|text| text.len() > 128)
                            }
                            "declaration" => !Declaration::value_shape(value),
                            _ => true,
                        })
                    {
                        return Err(INVALID.into());
                    }
                    let grant: Grant =
                        serde_json::from_value(value.clone()).map_err(|_| INVALID)?;
                    if grant.pack != *pack
                        || grant.provider != *provider
                        || grant.profile != *profile
                        || !declaration::digest_text(&grant.caller_digest)
                        || !declaration::digest_text(&grant.module_digest)
                        || !declaration::digest_text(&grant.declaration_digest)
                        || grant
                            .source_digest
                            .as_ref()
                            .is_some_and(|digest| !declaration::digest_text(digest))
                        || !grant.declaration.requested()
                        || grant.declaration.validate_provider(provider).is_err()
                    {
                        return Err(INVALID.into());
                    }
                    // 声明摘要与当前规则不匹配只是旧授权失效，仍需保留可见、可撤销的记录。
                    parsed_profiles.insert(profile.clone(), grant);
                }
                parsed_providers.insert(provider.clone(), parsed_profiles);
            }
            records.insert(pack.clone(), parsed_providers);
        }
        Ok(Self(records))
    }

    /// 只供已验证管理确认的固定写入；反序列化得到的记录不能生成确认材料。
    pub(super) fn with_confirmation(
        value: Option<&Value>,
        confirmation: &super::permissions::Confirmation,
    ) -> Result<Value, String> {
        let mut grants = Self::from_value(value)?;
        let grant = Grant::from_confirmation(confirmation);
        if !grants.present(&grant.pack, &grant.provider, &grant.profile)
            && grants
                .0
                .values()
                .flat_map(|providers| providers.values())
                .map(BTreeMap::len)
                .sum::<usize>()
                >= MAX_GRANTS
        {
            return Err("受控任务授权记录已达上限".into());
        }
        grants
            .0
            .entry(grant.pack.clone())
            .or_default()
            .entry(grant.provider.clone())
            .or_default()
            .insert(grant.profile.clone(), grant);
        grants.to_value()
    }

    pub(super) fn present(&self, pack: &str, provider: &str, profile: &str) -> bool {
        self.0
            .get(pack)
            .and_then(|items| items.get(provider))
            .is_some_and(|items| items.contains_key(profile))
    }

    pub(super) fn revoke_current(
        pack: &str,
        provider: &str,
        profile: &str,
    ) -> Result<config::plugin_task_grants::ChangeResult<Self>, String> {
        if !declaration::pack_id(pack) || policy::profile(provider) != Some(profile)
        {
            return Err(INVALID.into());
        }
        config::plugin_task_grants::revoke(|current| {
            let mut grants = Self::from_value(Some(current))?;
            grants.revoke(pack, provider, profile);
            let value = grants.to_value()?;
            Ok((value, grants))
        })
    }

    fn revoke(&mut self, pack: &str, provider: &str, profile: &str) {
        if let Some(providers) = self.0.get_mut(pack) {
            if let Some(profiles) = providers.get_mut(provider) {
                profiles.remove(profile);
                if profiles.is_empty() {
                    providers.remove(provider);
                }
            }
            if providers.is_empty() {
                self.0.remove(pack);
            }
        }
    }

    fn to_value(&self) -> Result<Value, String> {
        serde_json::to_value(&self.0).map_err(|_| "受控任务授权记录无法编码".into())
    }
}
