use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::core::{account_store::AccountStore, key_scope::KeyScope, providers::{catalog, images::ImageOperation}, routing};
use crate::server::errors::GatewayError;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageTarget {
    pub provider: String,
    pub model: String,
    pub account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageConfig {
    pub generation: ImageTarget,
    pub editing: Option<ImageTarget>,
    pub max_images: u32,
    pub max_concurrency: u32,
    pub retention_hours: u32,
    pub storage_mb: u32,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self { generation: ImageTarget::default(), editing: None, max_images: 1,
            max_concurrency: 2, retention_hours: 24, storage_mb: 512 }
    }
}

pub(super) fn deserialize_stored<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<ImageConfig>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

pub fn load() -> Option<ImageConfig> {
    let config = super::load().image_generation?;
    if !(1..=10).contains(&config.max_images) || !(1..=8).contains(&config.max_concurrency)
        || !(1..=168).contains(&config.retention_hours) || !(64..=4096).contains(&config.storage_mb) {
        return None;
    }
    Some(config)
}

fn unavailable(message: impl Into<String>) -> GatewayError {
    GatewayError::with_status(400, message).with_code("image_configuration")
}

pub fn target(config: &ImageConfig, operation: ImageOperation) -> Result<&ImageTarget, GatewayError> {
    match operation {
        ImageOperation::Generate => Ok(&config.generation),
        ImageOperation::Edit => config.editing.as_ref().ok_or_else(|| unavailable("尚未启用参考图编辑")),
    }
}

pub fn validate_target(store: &AccountStore, target: &ImageTarget, operation: ImageOperation) -> Result<(), GatewayError> {
    if target.provider.is_empty() || target.model.is_empty() {
        return Err(unavailable("请选择生图提供商与模型"));
    }
    let view = catalog::manage_view(store);
    let capability = match operation {
        ImageOperation::Generate => "supports_image_generation",
        ImageOperation::Edit => "supports_image_editing",
    };
    let available = view.get("models").and_then(Value::as_array).is_some_and(|models| models.iter().any(|model| {
        model["provider"].as_str() == Some(&target.provider)
            && model["id"].as_str() == Some(&target.model)
            && model["enabled"].as_bool() == Some(true)
            && model[capability].as_bool() == Some(true)
    }));
    if !available {
        return Err(unavailable("所选图片模型已停用或不支持该能力，请检查账号、插件与模型目录"));
    }
    let snapshot = store.routing_snapshot();
    let accounts: Vec<Value> = snapshot.accounts.iter().map(|account| account.as_ref().clone()).collect();
    let candidates = crate::server::core::routing_scope::apply_accounts(&accounts, &target.model);
    if !candidates.iter().any(|account| {
        routing::provider_of(account) == target.provider && account["enabled"].as_bool() != Some(false)
            && routing::account_id(account).is_some_and(|id| {
                (target.account_id.is_empty() || id == target.account_id) && store.get_session_by_id(id).is_some()
            })
    }) {
        return Err(unavailable("没有符合当前路由的已启用生图账号，固定账号失效时不会自动换号"));
    }
    Ok(())
}

pub fn restrict_scope(scope: Option<&KeyScope>, target: &ImageTarget) -> Result<KeyScope, GatewayError> {
    let scope = scope.cloned().unwrap_or_default();
    if !scope.allows_model(&target.model) {
        return Err(GatewayError::with_status(403, "当前网关 Key 无权使用所选图片模型").with_code("permission_denied"));
    }
    scope.restrict_image_target(&target.provider, &target.account_id).ok_or_else(|| {
        GatewayError::with_status(403, "当前网关 Key 无权使用所选生图提供商").with_code("permission_denied")
    })
}

pub fn save(store: &AccountStore, mut config: ImageConfig) -> Result<Value, GatewayError> {
    for item in std::iter::once(&mut config.generation).chain(config.editing.iter_mut()) {
        item.provider = item.provider.trim().to_string();
        item.model = item.model.trim().to_string();
        item.account_id = item.account_id.trim().to_string();
    }
    if !(1..=10).contains(&config.max_images) || !(1..=8).contains(&config.max_concurrency)
        || !(1..=168).contains(&config.retention_hours) || !(64..=4096).contains(&config.storage_mb) {
        return Err(unavailable("数量须为 1–10，并发须为 1–8，保留时间须为 1–168 小时，存储须为 64–4096 MiB"));
    }
    validate_target(store, &config.generation, ImageOperation::Generate)?;
    if let Some(editing) = &config.editing {
        validate_target(store, editing, ImageOperation::Edit)?;
    }
    super::update(|value| value.image_generation = Some(config)).map_err(unavailable)?;
    Ok(management_view(store))
}

pub fn clear() -> Result<(), GatewayError> {
    super::update(|value| value.image_generation = None).map(|_| ()).map_err(unavailable)
}

pub fn management_view(store: &AccountStore) -> Value {
    let config = load();
    let view = catalog::manage_view(store);
    let models: Vec<Value> = view.get("models").and_then(Value::as_array).into_iter().flatten()
        .filter(|model| model["enabled"].as_bool() == Some(true)
            && (model["supports_image_generation"].as_bool() == Some(true) || model["supports_image_editing"].as_bool() == Some(true)))
        .map(|model| json!({ "id": model["id"], "provider": model["provider"], "provider_label": model["providerLabel"],
            "generation": model["supports_image_generation"], "editing": model["supports_image_editing"] })).collect();
    let accounts: Vec<Value> = store.routing_snapshot().accounts.iter()
        .filter(|account| models.iter().any(|model| model["provider"].as_str() == Some(routing::provider_of(account))))
        .filter_map(|account| {
            let id = routing::account_id(account)?;
            Some(json!({ "id": id, "name": routing::account_name(account, id), "provider": routing::provider_of(account),
                "enabled": account["enabled"].as_bool() != Some(false) }))
        }).collect();
    let generation_error = config.as_ref().and_then(|value| validate_target(store, &value.generation, ImageOperation::Generate).err()).map(|error| error.message);
    let editing_error = config.as_ref().and_then(|value| value.editing.as_ref())
        .and_then(|value| validate_target(store, value, ImageOperation::Edit).err()).map(|error| error.message);
    json!({ "config": config, "defaults": ImageConfig::default(), "models": models, "accounts": accounts,
        "generation_error": generation_error, "editing_error": editing_error,
        "timeout_seconds": super::timeout_seconds(super::IMAGE_GENERATION_SERVICE_ID) })
}

pub fn editing_available(store: &AccountStore, scope: Option<&KeyScope>) -> bool {
    load().and_then(|config| config.editing).is_some_and(|target| {
        restrict_scope(scope, &target).is_ok() && validate_target(store, &target, ImageOperation::Edit).is_ok()
    })
}
