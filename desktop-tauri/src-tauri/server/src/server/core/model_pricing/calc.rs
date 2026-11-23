//! 模型有效价格解析与单次请求费用估算（对齐 `magpie` 优先级、倍率与分时窗口）。

use serde_json::{json, Value};

use super::catalog::{bare_id, maker_list_price, provider_list_price};
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

    if let Some(mut price) = maker_list_price(mid, peak_hint) {
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
            if let Some(mut price) = maker_list_price(target, None) {
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
    let usd = price.cost_split(
        uncached_input,
        completion,
        cache_read,
        cache_write,
        cache_write_1h,
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
