//! 模型计价核心模块：官网基准价匹配、自定义覆盖、同款模型参考、倍率/分时窗、汇率转换与请求费用计算。

pub mod calc;
pub mod catalog;
pub mod cn_prices;
pub mod fx;
pub mod peak;
pub mod types;

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use crate::server::config;
use crate::server::db::Db;
use crate::server::errors::GatewayError;
use crate::server::logging;

pub use calc::{calc_request_cost_usd, resolve_tariff, ResolvedTariff};
pub use types::{
    ModelPrice, ModelPricingSettings, PriceTier, ProviderPricingRule, RateWindow, KEY_MODEL_PRICING,
};

static GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn bump_generation() -> u64 {
    let next = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    crate::server::core::providers::catalog::invalidate();
    next
}

pub fn install(db: Option<Db>) {
    fx::install(db.clone());
    catalog::install(db);
}

pub fn settings() -> ModelPricingSettings {
    config::with_raw(|raw| {
        raw.get(KEY_MODEL_PRICING)
            .and_then(|value| serde_json::from_value::<ModelPricingSettings>(value.clone()).ok())
            .and_then(|parsed| parsed.normalized().ok())
    })
    .flatten()
    .unwrap_or_default()
}

pub fn settings_view() -> Value {
    let current = settings();
    let now = logging::now_ms();
    let fx_snap = fx::current_rate_soon();
    json!({
        "currency": current.currency,
        "modelPrices": current.model_prices,
        "modelSameAs": current.model_same_as,
        "providerPricing": current.provider_pricing,
        "fx": fx_snap.to_json(now),
        "referenceModels": catalog::reference_model_suggestions(),
    })
}

pub fn save_settings(next: ModelPricingSettings) -> Result<Value, GatewayError> {
    let normalized = next
        .normalized()
        .map_err(GatewayError::bad_request)?;
    let serialized = serde_json::to_value(&normalized)
        .map_err(|err| GatewayError::new(format!("序列化计价配置失败: {err}")))?;
    config::try_update_raw_field(
        KEY_MODEL_PRICING,
        |slot| {
            let is_default = normalized.currency == "usd"
                && normalized.model_prices.is_empty()
                && normalized.model_same_as.is_empty()
                && normalized.provider_pricing.is_empty();
            *slot = if is_default { Value::Null } else { serialized };
            Ok(())
        },
        || GatewayError::new("保存模型计价设置失败"),
    )?;
    bump_generation();
    logging::log("[Pricing]", "已更新模型计价设置");
    Ok(settings_view())
}

/// 供模型管理列表行拼装当前有效单价信息（不触网络，O(1) 缓存检索）。
pub fn model_manage_pricing_json(
    settings: &ModelPricingSettings,
    provider_id: &str,
    model_id: &str,
    now_ms: i64,
) -> Value {
    match resolve_tariff(settings, provider_id, model_id) {
        Some(resolved) => resolved.to_view_json(now_ms),
        None => {
            let rule = settings
                .provider_pricing
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(provider_id))
                .map(|(_, v)| v);
            let same_key = format!("{provider_id}/{model_id}");
            let same_as = settings
                .model_same_as
                .get(&same_key)
                .or_else(|| settings.model_same_as.get(&format!("*/{model_id}")))
                .cloned();
            json!({
                "priced": false,
                "source": null,
                "sameAs": same_as,
                "priceRate": rule.and_then(|r| r.price_rate),
                "activeWindowRate": rule.and_then(|r| peak::rate_window_at(&r.rate_windows, now_ms)),
                "inPeak": false,
                "peak": null,
                "base": null,
                "effective": null,
            })
        }
    }
}
