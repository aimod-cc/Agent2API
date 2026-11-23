//! 模型有效价格解析与单次请求费用估算（对齐 `magpie` 优先级、倍率与分时窗口）。

use serde_json::{json, Value};

use super::catalog::{bare_id, maker_list_price_for_provider, provider_list_price};
use super::peak::{is_peak_at, rate_window_at, PEAK_FACTOR};
use super::types::{ModelPrice, ModelPricingSettings, ProviderPricingRule};

#[derive(Clone, Debug)]
pub struct ResolvedTariff {
    pub price: ModelPrice,
    /// 来源标签：`"custom"` / `"provider_wildcard"` / `"model_wildcard"` / `"official"` / `"same_as"`
    pub source: &'static str,
    pub same_as: Option<String>,
    pub price_rate: Option<f64>,
    pub provider_rule: ProviderPricingRule,
}

impl ResolvedTariff {
    /// 计算在 `ts_ms` 时刻的实际生效单价（含峰谷档与提供商分时窗口）。
    pub fn effective_at(&self, ts_ms: i64) -> ModelPrice {
        let mut out = self.price.clone();
        if !out.peak.is_empty() && is_peak_at(&out.peak, ts_ms) {
            out = out.times(PEAK_FACTOR);
        }
        out.peak.clear();
        if let Some(rate) = rate_window_at(&self.provider_rule.rate_windows, ts_ms) {
            if rate > 0.0 && (rate - 1.0).abs() > f64::EPSILON {
                out = out.times(rate);
            }
        }
        out
    }

    pub fn to_view_json(&self, now_ms: i64) -> Value {
        let active_now = self.effective_at(now_ms);
        let active_window_rate = rate_window_at(&self.provider_rule.rate_windows, now_ms);
        let in_peak_now = !self.price.peak.is_empty() && is_peak_at(&self.price.peak, now_ms);
        json!({
            "priced": true,
            "source": self.source,
            "sameAs": self.same_as,
            "priceRate": self.price_rate,
            "activeWindowRate": active_window_rate,
            "inPeak": in_peak_now,
            "peak": (!self.price.peak.is_empty()).then_some(self.price.peak.clone()),
            "currency": self.price.currency_code(),
            "base": self.price,
            "effective": active_now,
        })
    }
}

fn find_map_entry<'a, T>(map: &'a std::collections::BTreeMap<String, T>, key: &str) -> Option<&'a T> {
    if let Some(found) = map.get(key) {
        return Some(found);
    }
    let lower = key.to_ascii_lowercase();
    map.iter()
        .find(|(k, _)| k.to_ascii_lowercase() == lower)
        .map(|(_, v)| v)
}

fn provider_rule_for(settings: &ModelPricingSettings, provider_id: &str) -> ProviderPricingRule {
    if provider_id.is_empty() {
        return ProviderPricingRule::default();
    }
    find_map_entry(&settings.provider_pricing, provider_id)
        .cloned()
        .unwrap_or_default()
}

/// 按 magpie 优先级解析某提供商下某模型的基础定价：
/// 1. `<provider>/<model>`（自定义精确覆盖，不乘 `priceRate`，受峰谷/分时窗口缩放）
/// 2. `<provider>/*`（提供商全模型覆盖，同上）
/// 3. `*/<model>`（跨提供商模型覆盖，同上）
/// 4. 提供商专属官方目录价 × `priceRate`
/// 5. 模型制造方官方基准价 × `priceRate`
/// 6. 用户配置的「同款模型计价参考 `sameAs`」对应官方基准价 × `priceRate`
pub fn resolve_tariff(
    settings: &ModelPricingSettings,
    provider_id: &str,
    model_id: &str,
) -> Option<ResolvedTariff> {
    let pid = provider_id.trim();
    let mid = model_id.trim();
    if mid.is_empty() {
        return None;
    }
    let rule = provider_rule_for(settings, pid);
    let peak_hint = (!pid.is_empty()).then_some(pid);

    let bare_mid = bare_id(mid);
    let mut keys_custom: Vec<(String, &'static str)> = Vec::new();
    if !pid.is_empty() {
        keys_custom.push((format!("{pid}/{mid}"), "custom"));
        if bare_mid != mid.to_ascii_lowercase() {
            keys_custom.push((format!("{pid}/{bare_mid}"), "custom"));
        }
        keys_custom.push((format!("{pid}/*"), "provider_wildcard"));
    }
    keys_custom.push((format!("*/{mid}"), "model_wildcard"));
    if bare_mid != mid.to_ascii_lowercase() {
        keys_custom.push((format!("*/{bare_mid}"), "model_wildcard"));
    }

    for (key, source) in keys_custom {
        if let Some(custom) = find_map_entry(&settings.model_prices, &key) {
            let mut price = custom.clone();
            price.apply_one_hour_for(mid);
            if let Some(hint) = peak_hint {
                let peak = super::peak::peak_kind_for(hint);
                if !peak.is_empty() {
                    price.peak = peak.to_string();
                }
            }
            return Some(ResolvedTariff {
                price,
                source,
                same_as: None,
                price_rate: None,
                provider_rule: rule,
            });
        }
    }

    let rate = rule.price_rate.filter(|r| *r > 0.0);
    if !pid.is_empty() {
        if let Some(mut price) = provider_list_price(pid, mid) {
            if let Some(r) = rate {
                price = price.times(r);
            }
            return Some(ResolvedTariff {
                price,
                source: "official",
                same_as: None,
                price_rate: rate,
                provider_rule: rule,
            });
        }
    }

    let pid_opt = (!pid.is_empty()).then_some(pid);
    if let Some(mut price) = maker_list_price_for_provider(pid_opt, mid, peak_hint) {
        if let Some(r) = rate {
            price = price.times(r);
        }
        return Some(ResolvedTariff {
            price,
            source: "official",
            same_as: None,
            price_rate: rate,
            provider_rule: rule,
        });
    }

    let mut same_keys = Vec::new();
    if !pid.is_empty() {
        same_keys.push(format!("{pid}/{mid}"));
        if bare_mid != mid.to_ascii_lowercase() {
            same_keys.push(format!("{pid}/{bare_mid}"));
        }
    }
    same_keys.push(format!("*/{mid}"));
    if bare_mid != mid.to_ascii_lowercase() {
        same_keys.push(format!("*/{bare_mid}"));
    }
    for key in same_keys {
        if let Some(target) = find_map_entry(&settings.model_same_as, &key) {
            if let Some(mut price) = maker_list_price_for_provider(pid_opt, target, None) {
                if let Some(r) = rate {
                    price = price.times(r);
                }
                return Some(ResolvedTariff {
                    price,
                    source: "same_as",
                    same_as: Some(target.clone()),
                    price_rate: rate,
                    provider_rule: rule,
                });
            }
        }
    }

    None
}

pub struct RequestCostDetail {
    pub cost_usd: f64,
    pub source: &'static str,
    pub same_as: Option<String>,
}

/// 拆分 `prompt_tokens` 为未命中缓存的输入部分，并按有效单价算出单条请求的 USD 费用与计价来源。
pub fn calc_request_cost_detail(
    settings: &ModelPricingSettings,
    provider: &str,
    model: &str,
    upstream_model: &str,
    ts_ms: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cache_creation_1h_tokens: i64,
) -> Option<Option<RequestCostDetail>> {
    let prompt = prompt_tokens.max(0);
    let completion = completion_tokens.max(0);
    let cache_read = cache_read_tokens.max(0);
    let cache_write = cache_creation_tokens.max(0);
    let cache_write_1h = cache_creation_1h_tokens.clamp(0, cache_write);
    if prompt == 0 && completion == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }

    let primary_model = upstream_model.trim();
    let fallback_model = model.trim();
    let effective_model = if !primary_model.is_empty() {
        primary_model
    } else {
        fallback_model
    };
    let tariff = resolve_tariff(settings, provider, effective_model).or_else(|| {
        if !primary_model.is_empty() && !fallback_model.is_empty() && primary_model != fallback_model {
            resolve_tariff(settings, provider, fallback_model)
        } else {
            None
        }
    });
    let Some(tariff) = tariff else {
        return Some(None);
    };

    let price = tariff.effective_at(ts_ms);
    let cached_sum = cache_read.saturating_add(cache_write);
    let uncached_input = if prompt >= cached_sum {
        prompt - cached_sum
    } else {
        prompt
    };
    let fx_rate = super::fx::current_rate_soon().rate;
    let usd = price.cost_usd(
        uncached_input,
        completion,
        cache_read,
        cache_write,
        cache_write_1h,
        fx_rate,
    );
    let cost_usd = if usd.is_finite() && usd >= 0.0 { usd } else { 0.0 };
    Some(Some(RequestCostDetail {
        cost_usd,
        source: tariff.source,
        same_as: tariff.same_as,
    }))
}

pub fn calc_request_cost_usd(
    settings: &ModelPricingSettings,
    provider: &str,
    model: &str,
    upstream_model: &str,
    ts_ms: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cache_creation_1h_tokens: i64,
) -> Option<Option<f64>> {
    calc_request_cost_detail(
        settings,
        provider,
        model,
        upstream_model,
        ts_ms,
        prompt_tokens,
        completion_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        cache_creation_1h_tokens,
    )
    .map(|inner| inner.map(|detail| detail.cost_usd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn domestic_provider_uses_native_cny_and_keeps_intl_models_usd() {
        let settings = ModelPricingSettings::default();
        let glm = resolve_tariff(&settings, "wps-lingxi", "GLM-5.3").expect("glm-5.3");
        assert_eq!(glm.price.currency_code(), "cny");
        assert!((glm.price.input - 8.0).abs() < 1e-6);
        assert!((glm.price.output - 28.0).abs() < 1e-6);

        let flash = resolve_tariff(&settings, "wps-lingxi", "GLM-5.3-Flash").expect("glm-5.3-flash");
        assert_eq!(flash.price.currency_code(), "cny");
        assert!((flash.price.input - 0.8).abs() < 1e-6);
        assert!((flash.price.output - 2.8).abs() < 1e-6);

        let qwen = resolve_tariff(&settings, "wps-lingxi", "qwen3.8_flash").expect("qwen3.8_flash");
        assert_eq!(qwen.price.currency_code(), "cny");
        assert!((qwen.price.input - 0.8).abs() < 1e-6);
        assert!((qwen.price.output - 2.7).abs() < 1e-6);

        let minimax = resolve_tariff(&settings, "wps-lingxi", "minimax_m3").expect("minimax_m3");
        assert_eq!(minimax.price.currency_code(), "cny");
        assert!((minimax.price.input - 2.1).abs() < 1e-6);
        assert!((minimax.price.output - 8.4).abs() < 1e-6);

        let doubao = resolve_tariff(&settings, "wps-lingxi", "doubao_seed_2_1_pro_260401").expect("doubao");
        assert_eq!(doubao.price.currency_code(), "cny");
        assert!((doubao.price.input - 6.0).abs() < 1e-6);
        assert!((doubao.price.output - 30.0).abs() < 1e-6);

        let deepseek = resolve_tariff(&settings, "wps-lingxi", "deepseek-v4.1-flash").expect("deepseek");
        assert_eq!(deepseek.price.currency_code(), "cny");
        assert!((deepseek.price.input - 1.0).abs() < 1e-6);
        assert!((deepseek.price.output - 4.0).abs() < 1e-6);

        let claude = resolve_tariff(&settings, "wps-lingxi", "claude-sonnet-4-5").expect("claude");
        assert_eq!(claude.price.currency_code(), "usd");

        let intl_glm = resolve_tariff(&settings, "workbuddy-intl", "glm-5.3").expect("intl glm");
        assert_eq!(intl_glm.price.currency_code(), "usd");
    }

    #[test]
    fn custom_cny_price_converts_cost_to_usd_and_preserves_same_as_region() {
        let mut model_prices = BTreeMap::new();
        model_prices.insert(
            "custom/local-model".to_string(),
            ModelPrice {
                input: 10.0,
                output: 20.0,
                currency: "cny".to_string(),
                ..Default::default()
            },
        );
        let mut model_same_as = BTreeMap::new();
        model_same_as.insert("*/alias-glm".to_string(), "glm-5.3".to_string());
        let settings = ModelPricingSettings {
            model_prices,
            model_same_as,
            ..Default::default()
        };

        let cny_price = settings.model_prices.get("custom/local-model").unwrap();
        let usd = cny_price.cost_usd(500_000, 250_000, 0, 0, 0, 7.0);
        // 0.5M * 10 + 0.25M * 20 = 10 CNY => 10 / 7 USD
        assert!((usd - 10.0 / 7.0).abs() < 1e-9);

        let detail = calc_request_cost_detail(
            &settings,
            "custom",
            "local-model",
            "",
            0,
            1_000_000,
            0,
            0,
            0,
            0,
        )
        .unwrap()
        .unwrap();
        assert!((detail.cost_usd - (10.0 / super::super::fx::FX_FALLBACK)).abs() < 1e-9);

        let same_cn = resolve_tariff(&settings, "wps-lingxi", "alias-glm").expect("same_as cn");
        assert_eq!(same_cn.price.currency_code(), "cny");
        assert!((same_cn.price.input - 8.0).abs() < 1e-6);

        let same_intl = resolve_tariff(&settings, "workbuddy-intl", "alias-glm").expect("same_as intl");
        assert_eq!(same_intl.price.currency_code(), "usd");
    }
}
