//! 国内人民币官方基准单价与国内渠道判定。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use super::catalog::candidate_keys;
use super::peak::peak_kind_for;
use super::types::ModelPrice;
use crate::server::core::custom_providers;
use crate::server::core::model_meta::snapshot;

const EMBEDDED_CN_PRICES: &str = include_str!("cn_official_prices.json");

static CN_PRICE_TABLE: OnceLock<HashMap<String, ModelPrice>> = OnceLock::new();

fn table() -> &'static HashMap<String, ModelPrice> {
    CN_PRICE_TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(EMBEDDED_CN_PRICES) else {
            return map;
        };
        for (raw_id, raw_val) in entries {
            let Some(mut price) = ModelPrice::parse_json(&raw_val, Some(&raw_id)) else {
                continue;
            };
            price.currency = "cny".to_string();
            for key in candidate_keys(&raw_id) {
                map.entry(key).or_insert_with(|| price.clone());
            }
            let norm = snapshot::key_of(&raw_id);
            if !norm.is_empty() {
                map.entry(norm).or_insert(price);
            }
        }
        map
    })
}

/// 判断给定提供商是否属于默认以人民币结算的国内渠道。
pub fn is_domestic_cn_provider(provider_id: &str) -> bool {
    let pid = provider_id.trim().to_ascii_lowercase();
    if pid.is_empty() {
        return false;
    }
    if matches!(
        pid.as_str(),
        "workbuddy"
            | "qoder"
            | "qwenwork"
            | "qwenwork-enterprise"
            | "autoclaw"
            | "zcode"
            | "accio-cn"
            | "monkeycode"
            | "modelscope"
            | "tabbit"
            | "minimax-code"
            | "codebuddy"
            | "raccoon"
            | "catpaw"
            | "codearts"
            | "trae"
            | "loomy"
            | "kuku"
            | "officeace"
            | "orcaterm"
            | "chatglm"
            | "atomcode"
            | "lobsterai"
            | "doubaowork"
            | "wps-lingxi"
            | "mimo-app"
            | "intern-inkstone"
            | "codar"
            | "joycode"
            | "kimi-work"
            | "iflow"
            | "iflow-cli"
            | "dumate"
            | "deepseek-web"
            | "zhipuai"
            | "moonshotai-cn"
            | "minimax-cn"
            | "alibaba-cn"
            | "volcengine"
            | "baidu"
            | "baidu-qianfan"
            | "stepfun-cn"
            | "tencent-tokenhub-cn"
            | "huawei-maas"
            | "iflytek"
            | "sensenova"
    ) {
        return true;
    }

    let Some(custom) = custom_providers::get(provider_id.trim()) else {
        return false;
    };
    let preset = custom
        .get("presetKey")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if matches!(
        preset.as_str(),
        "beichen"
            | "glm-cn"
            | "minimax-cn"
            | "volcengine-ark"
            | "alicode"
            | "xiaomi-tokenplan"
            | "moonshot-cn"
            | "stepfun-cn"
            | "tencent-tokenhub-cn"
            | "sensenova"
    ) {
        return true;
    }

    let base_url = custom
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if base_url.is_empty() {
        return false;
    }
    [
        ".bigmodel.cn",
        ".minimaxi.com",
        ".moonshot.cn",
        ".volces.com",
        "dashscope.aliyuncs.com",
        "token-plan-cn.xiaomimimo.com",
        ".lkeap.cloud.tencent.com",
        ".sensenova.cn",
        "qianfan.baidubce.com",
        "spark-api-open.xf-yun.com",
        "maas-api.cn-",
    ]
    .iter()
    .any(|domain| base_url.contains(domain))
}

/// 查找国内人民币官方基准单价。
pub fn lookup(model_id: &str, peak_provider_hint: Option<&str>) -> Option<ModelPrice> {
    let map = table();
    let candidates = candidate_keys(model_id);
    for cand in &candidates {
        if let Some(price) = map.get(cand) {
            let mut resolved = price.clone();
            resolved.currency = "cny".to_string();
            resolved.apply_one_hour_for(model_id);
            if let Some(pid) = peak_provider_hint {
                let peak = peak_kind_for(pid);
                if !peak.is_empty() {
                    resolved.peak = peak.to_string();
                }
            }
            return Some(resolved);
        }
    }
    None
}
