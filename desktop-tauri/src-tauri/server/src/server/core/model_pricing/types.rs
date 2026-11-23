//! 模型计价的数据结构与配置校验（对照 `magpie` 计价口径）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const KEY_MODEL_PRICING: &str = "modelPricing";
pub const RATE_WINDOWS_MAX: usize = 8;

/// 阶梯计价档位（按单次请求总输入 token 数 `uncached + cacheRead + cacheWrite` 触发整次计价）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceTier {
    pub above: i64,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(rename = "cacheRead", alias = "cache_read", default)]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite", alias = "cache_write", default)]
    pub cache_write: f64,
    #[serde(rename = "cacheWrite1h", alias = "cache_write_1h", default, skip_serializing_if = "is_zero_f64")]
    pub cache_write_1h: f64,
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

fn is_empty_str(value: &str) -> bool {
    value.is_empty()
}

pub fn normalize_price_currency(value: &str) -> String {
    if value.trim().eq_ignore_ascii_case("cny") {
        "cny".to_string()
    } else {
        String::new()
    }
}

/// 模型单价（每百万 Token；默认美元，可为人民币 `"cny"`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(rename = "cacheRead", alias = "cache_read", default)]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite", alias = "cache_write", default)]
    pub cache_write: f64,
    #[serde(rename = "cacheWrite1h", alias = "cache_write_1h", default, skip_serializing_if = "is_zero_f64")]
    pub cache_write_1h: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<PriceTier>,
    /// 峰谷定价标记（`"deepseek"` / `"weekdays"` / `"ollama"` / `""`）。
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub peak: String,
    /// 单价币种（`"cny"` 表示人民币，空或 `"usd"` 表示美元）。
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub currency: String,
}

#[derive(Clone, Copy, Debug)]
pub struct FlatPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cache_write_1h: f64,
}

impl FlatPrice {
    pub fn one_hour(&self) -> f64 {
        if self.cache_write_1h > 0.0 {
            self.cache_write_1h
        } else {
            self.cache_write
        }
    }
}

impl ModelPrice {
    /// Claude 模型若声明了缓存写入单价但未显式给出 1 小时写入价，按 2 倍输入单价补全。
    pub fn apply_one_hour_for(&mut self, model_id: &str) {
        if self.cache_write <= 0.0
            || self.cache_write_1h > 0.0
            || !model_id.to_ascii_lowercase().contains("claude")
        {
            return;
        }
        self.cache_write_1h = 2.0 * self.input;
        for tier in &mut self.tiers {
            if tier.cache_write_1h <= 0.0 && tier.cache_write > 0.0 {
                tier.cache_write_1h = 2.0 * tier.input;
            }
        }
    }

    pub fn times(&self, factor: f64) -> Self {
        Self {
            input: self.input * factor,
            output: self.output * factor,
            cache_read: self.cache_read * factor,
            cache_write: self.cache_write * factor,
            cache_write_1h: self.cache_write_1h * factor,
            tiers: self
                .tiers
                .iter()
                .map(|tier| PriceTier {
                    above: tier.above,
                    input: tier.input * factor,
                    output: tier.output * factor,
                    cache_read: tier.cache_read * factor,
                    cache_write: tier.cache_write * factor,
                    cache_write_1h: tier.cache_write_1h * factor,
                })
                .collect(),
            peak: self.peak.clone(),
            currency: self.currency.clone(),
        }
    }

    pub fn currency_code(&self) -> &'static str {
        if self.currency.trim().eq_ignore_ascii_case("cny") {
            "cny"
        } else {
            "usd"
        }
    }

    pub fn at_prompt(&self, total_input_tokens: i64) -> FlatPrice {
        let mut flat = FlatPrice {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cache_write_1h: self.cache_write_1h,
        };
        for tier in &self.tiers {
            if total_input_tokens > tier.above {
                flat = FlatPrice {
                    input: tier.input,
                    output: tier.output,
                    cache_read: tier.cache_read,
                    cache_write: tier.cache_write,
                    cache_write_1h: tier.cache_write_1h,
                };
            }
        }
        flat
    }

    pub fn cost_split(
        &self,
        uncached_input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        cache_write_1h: i64,
    ) -> f64 {
        let uncached_input = uncached_input.max(0);
        let output = output.max(0);
        let cache_read = cache_read.max(0);
        let cache_write = cache_write.max(0);
        let c1h = cache_write_1h.clamp(0, cache_write);
        let c5m = cache_write - c1h;
        let flat = self.at_prompt(uncached_input.saturating_add(cache_read).saturating_add(cache_write));
        (uncached_input as f64 * flat.input
            + output as f64 * flat.output
            + cache_read as f64 * flat.cache_read
            + c5m as f64 * flat.cache_write
            + c1h as f64 * flat.one_hour())
            / 1_000_000.0
    }

    pub fn cost_usd(
        &self,
        uncached_input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        cache_write_1h: i64,
        fx_rate: f64,
    ) -> f64 {
        let raw = self.cost_split(uncached_input, output, cache_read, cache_write, cache_write_1h);
        if self.currency_code() == "cny" && fx_rate.is_finite() && fx_rate > 0.0 {
            raw / fx_rate
        } else {
            raw
        }
    }

    pub fn normalized(&self) -> Result<Self, String> {
        let check_num = |name: &str, value: f64| -> Result<f64, String> {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{name} 单价不能为负数"));
            }
            Ok(value)
        };
        let mut tiers = Vec::with_capacity(self.tiers.len());
        for tier in &self.tiers {
            if tier.above <= 0 {
                return Err("阶梯阈值 token 数必须大于 0".to_string());
            }
            tiers.push(PriceTier {
                above: tier.above,
                input: check_num("阶梯输入", tier.input)?,
                output: check_num("阶梯输出", tier.output)?,
                cache_read: check_num("阶梯缓存读取", tier.cache_read)?,
                cache_write: check_num("阶梯缓存写入", tier.cache_write)?,
                cache_write_1h: check_num("阶梯 1h 缓存写入", tier.cache_write_1h)?,
            });
        }
        tiers.sort_by_key(|tier| tier.above);
        Ok(Self {
            input: check_num("输入", self.input)?,
            output: check_num("输出", self.output)?,
            cache_read: check_num("缓存读取", self.cache_read)?,
            cache_write: check_num("缓存写入", self.cache_write)?,
            cache_write_1h: check_num("1h 缓存写入", self.cache_write_1h)?,
            tiers,
            peak: self.peak.trim().to_string(),
            currency: normalize_price_currency(&self.currency),
        })
    }

    pub fn parse_json(value: &Value, id_hint: Option<&str>) -> Option<Self> {
        let obj = value.as_object()?;
        let num = |keys: &[&str]| -> Option<f64> {
            for key in keys {
                if let Some(v) = obj.get(*key).and_then(Value::as_f64) {
                    if v.is_finite() && v >= 0.0 {
                        return Some(v);
                    }
                }
            }
            None
        };
        let input = num(&["input"]);
        let output = num(&["output"]);
        let cache_read = num(&["cacheRead", "cache_read"]);
        let cache_write = num(&["cacheWrite", "cache_write"]);
        let cache_write_1h = num(&["cacheWrite1h", "cache_write_1h"]);
        if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
            return None;
        }
        let base_input = input.unwrap_or(0.0);
        let base_output = output.unwrap_or(0.0);
        let base_read = cache_read.unwrap_or(0.0);
        let base_write = cache_write.unwrap_or(0.0);
        let mut tiers = Vec::new();
        if let Some(items) = obj.get("tiers").and_then(Value::as_array) {
            for item in items {
                let Some(tier_obj) = item.as_object() else { continue };
                if let Some(meta) = tier_obj.get("tier").and_then(Value::as_object) {
                    if let Some(kind) = meta.get("type").and_then(Value::as_str) {
                        if !kind.is_empty() && kind != "context" {
                            continue;
                        }
                    }
                }
                let above = tier_obj
                    .get("tier")
                    .and_then(|meta| meta.get("size"))
                    .and_then(Value::as_i64)
                    .or_else(|| tier_obj.get("above").and_then(Value::as_i64))
                    .unwrap_or(0);
                if above <= 0 {
                    continue;
                }
                tiers.push(parse_tier_object(
                    tier_obj,
                    above,
                    base_input,
                    base_output,
                    base_read,
                    base_write,
                ));
            }
        }
        if tiers.is_empty() {
            if let Some(over) = obj.get("context_over_200k").and_then(Value::as_object) {
                tiers.push(parse_tier_object(
                    over,
                    200_000,
                    base_input,
                    base_output,
                    base_read,
                    base_write,
                ));
            }
        }
        tiers.sort_by_key(|tier| tier.above);
        let mut out = Self {
            input: base_input,
            output: base_output,
            cache_read: base_read,
            cache_write: base_write,
            cache_write_1h: cache_write_1h.unwrap_or(0.0),
            tiers,
            peak: obj.get("peak").and_then(Value::as_str).unwrap_or("").trim().to_string(),
            currency: normalize_price_currency(obj.get("currency").and_then(Value::as_str).unwrap_or("")),
        };
        if let Some(id) = id_hint {
            out.apply_one_hour_for(id);
        }
        Some(out)
    }
}

fn parse_tier_object(
    obj: &Map<String, Value>,
    above: i64,
    base_input: f64,
    base_output: f64,
    base_read: f64,
    base_write: f64,
) -> PriceTier {
    let pick = |keys: &[&str], fallback: f64| -> f64 {
        for key in keys {
            if let Some(v) = obj.get(*key).and_then(Value::as_f64) {
                if v.is_finite() && v >= 0.0 {
                    return v;
                }
            }
        }
        fallback
    };
    PriceTier {
        above,
        input: pick(&["input"], base_input),
        output: pick(&["output"], base_output),
        cache_read: pick(&["cacheRead", "cache_read"], base_read),
        cache_write: pick(&["cacheWrite", "cache_write"], base_write),
        cache_write_1h: pick(&["cacheWrite1h", "cache_write_1h"], 0.0),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateWindow {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<u8>,
    pub start: String,
    pub end: String,
    pub rate: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderPricingRule {
    #[serde(rename = "priceRate", default, skip_serializing_if = "Option::is_none")]
    pub price_rate: Option<f64>,
    #[serde(rename = "rateWindows", default, skip_serializing_if = "Vec::is_empty")]
    pub rate_windows: Vec<RateWindow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPricingSettings {
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(rename = "modelPrices", default)]
    pub model_prices: BTreeMap<String, ModelPrice>,
    #[serde(rename = "modelSameAs", default)]
    pub model_same_as: BTreeMap<String, String>,
    #[serde(rename = "providerPricing", default)]
    pub provider_pricing: BTreeMap<String, ProviderPricingRule>,
}

fn default_currency() -> String {
    "usd".to_string()
}

impl Default for ModelPricingSettings {
    fn default() -> Self {
        Self {
            currency: default_currency(),
            model_prices: BTreeMap::new(),
            model_same_as: BTreeMap::new(),
            provider_pricing: BTreeMap::new(),
        }
    }
}

pub fn parse_clock_minutes(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    let (h, m) = trimmed.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    if !h.bytes().all(|b| b.is_ascii_digit()) || !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hh: i32 = h.parse().ok()?;
    let mm: i32 = m.parse().ok()?;
    if hh > 24 || mm > 59 {
        return None;
    }
    let total = hh * 60 + mm;
    if total > 1440 {
        return None;
    }
    Some(total)
}

pub fn validate_rate_value(rate: f64, allow_zero: bool) -> Result<f64, String> {
    if !rate.is_finite() || (!allow_zero && rate <= 0.0) || (allow_zero && rate < 0.0) || rate > 1000.0 {
        return Err("倍率必须在 0 到 1000 之间".to_string());
    }
    if (rate * 1000.0 - (rate * 1000.0).round()).abs() > 1e-6 {
        return Err("倍率最多支持三位小数".to_string());
    }
    Ok((rate * 1000.0).round() / 1000.0)
}

pub fn normalize_clock_str(raw: &str) -> Option<(String, i32)> {
    let minutes = parse_clock_minutes(raw)?;
    let hh = minutes / 60;
    let mm = minutes % 60;
    Some((format!("{hh:02}:{mm:02}"), minutes))
}

pub fn normalize_provider_rule(rule: &ProviderPricingRule) -> Result<ProviderPricingRule, String> {
    let price_rate = match rule.price_rate {
        Some(rate) => Some(validate_rate_value(rate, false)?),
        None => None,
    };
    if rule.rate_windows.len() > RATE_WINDOWS_MAX {
        return Err(format!("单个提供商最多配置 {RATE_WINDOWS_MAX} 个分时倍率时段"));
    }
    let mut windows = Vec::with_capacity(rule.rate_windows.len());
    for (idx, window) in rule.rate_windows.iter().enumerate() {
        let Some((start_norm, start_min)) = normalize_clock_str(&window.start) else {
            return Err(format!("第 {} 个时段的开始时间格式应为 HH:MM", idx + 1));
        };
        let Some((end_norm, end_min)) = normalize_clock_str(&window.end) else {
            return Err(format!("第 {} 个时段的结束时间格式应为 HH:MM（可填 24:00）", idx + 1));
        };
        if start_min >= 1440 {
            return Err(format!("第 {} 个时段的开始时间不能是 24:00", idx + 1));
        }
        if start_min == end_min {
            return Err(format!("第 {} 个时段的开始和结束时间不能相同", idx + 1));
        }
        let mut days = window.days.clone();
        days.sort_unstable();
        days.dedup();
        if days.iter().any(|day| *day > 6) {
            return Err(format!("第 {} 个时段的星期取值范围应为 0（周日）到 6（周六）", idx + 1));
        }
        let rate = validate_rate_value(window.rate, false)?;
        windows.push(RateWindow {
            days,
            start: start_norm,
            end: end_norm,
            rate,
        });
    }
    Ok(ProviderPricingRule {
        price_rate,
        rate_windows: windows,
    })
}

impl ModelPricingSettings {
    pub fn normalized(&self) -> Result<Self, String> {
        let currency = match self.currency.trim().to_ascii_lowercase().as_str() {
            "cny" => "cny".to_string(),
            _ => "usd".to_string(),
        };
        let mut model_prices = BTreeMap::new();
        for (key, price) in &self.model_prices {
            let trimmed = key.trim();
            if trimmed.is_empty() || !trimmed.contains('/') {
                return Err(format!("自定义单价键格式无效: {trimmed}"));
            }
            model_prices.insert(trimmed.to_string(), price.normalized()?);
        }
        let mut model_same_as = BTreeMap::new();
        for (key, target) in &self.model_same_as {
            let k = key.trim();
            let v = target.trim();
            if !k.is_empty() && !v.is_empty() {
                model_same_as.insert(k.to_string(), v.to_string());
            }
        }
        let mut provider_pricing = BTreeMap::new();
        for (provider, rule) in &self.provider_pricing {
            let pid = provider.trim();
            if pid.is_empty() {
                continue;
            }
            let normalized = normalize_provider_rule(rule)?;
            if normalized.price_rate.is_some() || !normalized.rate_windows.is_empty() {
                provider_pricing.insert(pid.to_string(), normalized);
            }
        }
        Ok(Self {
            currency,
            model_prices,
            model_same_as,
            provider_pricing,
        })
    }
}
