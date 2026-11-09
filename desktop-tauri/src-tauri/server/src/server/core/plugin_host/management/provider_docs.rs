use std::collections::BTreeMap;

use agent2api_provider_protocol as proto;
use serde::{Deserialize, Serialize};

use crate::server::core::plugin_host::{self, install};

const MAX_DOCUMENT_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum LocalizedText {
    Text(String),
    Translations(BTreeMap<String, String>),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapabilityStatus {
    Supported,
    ImplementedUnverified,
    Unsupported,
    NotApplicable,
    #[default]
    Undeclared,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CapabilityCell {
    #[serde(default)]
    status: CapabilityStatus,
    #[serde(default)]
    note: Option<LocalizedText>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ProviderNotes {
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    notes: Vec<LocalizedText>,
    #[serde(default)]
    capabilities: BTreeMap<String, CapabilityCell>,
}

#[derive(Default, Deserialize)]
struct PackNotes {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    description: Option<LocalizedText>,
    #[serde(default)]
    providers: BTreeMap<String, ProviderNotes>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackDocumentation {
    id: String,
    name: String,
    version: String,
    description: Option<LocalizedText>,
    providers: Vec<ProviderDocumentation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDocumentation {
    id: String,
    label: String,
    enabled: bool,
    loaded: bool,
    capabilities: BTreeMap<String, CapabilityCell>,
    notes: Vec<LocalizedText>,
}

fn read_notes(pack: &plugin_host::PackSummary) -> PackNotes {
    // 放在 ui/ 下的说明数据与页面资源一样受现有插件签名覆盖。
    let root = install::plugins_dir()
        .join(&pack.id)
        .join(&pack.version)
        .join("ui");
    let result = (|| {
        let root = root.canonicalize().ok()?;
        let path = root.join("provider-docs.json").canonicalize().ok()?;
        if !path.starts_with(&root) {
            return None;
        }
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return None;
        }
        let notes: PackNotes = serde_json::from_slice(&bytes).ok()?;
        (notes.schema == 1).then_some(notes)
    })();
    result.unwrap_or_default()
}

fn runtime_capabilities(info: &proto::ProviderInfo) -> [(&'static str, bool); 6] {
    let daily = info
        .checkin
        .as_ref()
        .map(|checkin| checkin.daily)
        .unwrap_or(info.capabilities.checkin);
    let claims = info.capabilities.claims
        || info.checkin.as_ref().is_some_and(|checkin| {
            checkin.onboarding
                || checkin.plans
                || checkin.growth
                || checkin.modes.iter().any(|mode| {
                    matches!(mode, proto::CheckinMode::Claim | proto::CheckinMode::Full)
                })
        });
    [
        ("chat", info.capabilities.chat),
        ("refresh", info.capabilities.refresh),
        ("model_refresh", info.capabilities.model_refresh),
        ("usage", info.capabilities.usage),
        ("checkin", daily),
        ("claims", claims),
    ]
}

fn provider_documentation(
    provider: plugin_host::PackProviderSummary,
    info: Option<&proto::ProviderInfo>,
    notes: Option<&ProviderNotes>,
) -> ProviderDocumentation {
    let unverified = plugin_host::host_for_provider(&provider.id)
        .is_some_and(|host| host.options.wasm.as_ref().is_some_and(|options| !options.manifest.verified));
    let runtime: BTreeMap<&str, bool> = info
        .map(runtime_capabilities)
        .map(|values| values.into_iter().collect())
        .unwrap_or_default();
    let mut capabilities = BTreeMap::new();
    for key in [
        "chat",
        "refresh",
        "model_refresh",
        "usage",
        "checkin",
        "claims",
    ] {
        let declared = notes.and_then(|notes| notes.capabilities.get(key));
        let status = if declared
            .is_some_and(|cell| matches!(cell.status, CapabilityStatus::ImplementedUnverified))
        {
            CapabilityStatus::ImplementedUnverified
        } else if unverified && runtime.get(key) == Some(&true) {
            CapabilityStatus::ImplementedUnverified
        } else if runtime.get(key) == Some(&true) {
            CapabilityStatus::Supported
        } else {
            // false 只表明运行时能力位未开启，不能据此断言上游没有该能力。
            declared.map(|cell| cell.status).unwrap_or_default()
        };
        capabilities.insert(
            key.to_string(),
            CapabilityCell {
                status,
                note: declared.and_then(|cell| cell.note.clone()),
            },
        );
    }
    let mut descriptions = notes.map(|notes| notes.notes.clone()).unwrap_or_default();
    if let Some(description) = info
        .and_then(|info| info.checkin.as_ref())
        .and_then(|checkin| checkin.description.as_ref())
        .filter(|description| descriptions.is_empty() && !description.trim().is_empty())
    {
        descriptions.push(LocalizedText::Text(description.clone()));
    }
    ProviderDocumentation {
        id: provider.id,
        label: provider.label,
        enabled: provider.enabled,
        loaded: provider.loaded,
        capabilities,
        notes: descriptions,
    }
}

pub(super) fn snapshot(packs: &[plugin_host::PackSummary]) -> Vec<PackDocumentation> {
    packs
        .iter()
        .filter(|pack| pack.enabled)
        .filter_map(|pack| {
            let notes = read_notes(pack);
            let infos = plugin_host::host_for_pack(&pack.id)
                .and_then(|host| host.providers.lock().ok().map(|infos| infos.clone()))
                .unwrap_or_default();
            let providers: Vec<ProviderDocumentation> = pack
                .providers
                .iter()
                .filter(|provider| {
                    !crate::server::core::providers::availability::is_suspended(&provider.id)
                        && !notes.providers.get(&provider.id).is_some_and(|notes| notes.hidden)
                })
                .map(|provider| {
                    let info = infos.iter().find(|info| info.id.as_str() == provider.id);
                    provider_documentation(
                        provider.clone(),
                        info,
                        notes.providers.get(&provider.id),
                    )
                })
                .collect();
            if providers.is_empty() {
                return None;
            }
            Some(PackDocumentation {
                id: pack.id.clone(),
                name: pack.name.clone(),
                version: pack.version.clone(),
                description: notes.description,
                providers,
            })
        })
        .collect()
}
