//! 隐私脱敏模块：在请求离开本机前识别并遮罩密钥、个人隐私信息与自定义保密词，
//! 并在响应流 / JSON 中透明还原（原值仅驻留内存，绝不落盘或写入日志）。

mod detectors;
mod rules;
mod stream;
mod vault;
mod walker;

#[cfg(test)]
mod tests;

use serde_json::{json, Value};

pub use detectors::{mask_text, MaskTextResult, PrivacyHitRecord};
pub use rules::{
    category_catalog, default_categories, kind_label_zh, normalize_kind, CompiledPrivacySettings,
    CustomRuleMatchType, PrivacyAction, PrivacyCustomRule, PrivacyProviderMode, PrivacySettings,
    MAX_ALLOWLIST, MAX_CUSTOM_RULES, MAX_KIND_CHARS, MAX_NAME_CHARS, MAX_PREFIX_CHARS,
    MAX_PROVIDERS, MAX_REGEX_CHARS, MAX_TEST_TEXT_BYTES, MAX_WORDS, MAX_WORD_CHARS,
    MIN_PREFIX_CHARS, PERSONAL_CATEGORIES,
};
pub use stream::{unmask_outcome, StreamChannelRestorer};
pub use vault::PrivacyVault;
pub use walker::{mask_chat_body, mask_native_body, unmask_json_response, PrivacyBodyOutcome};

/// 试一试实时预览：使用独立临时保险库，不污染全局脱敏映射。
pub fn preview_text(text: &str, compiled: &CompiledPrivacySettings) -> Value {
    let vault = PrivacyVault::ephemeral();
    match mask_text(text, compiled, &vault) {
        MaskTextResult::Unchanged => json!({
            "maskedText": text,
            "restoredText": text,
            "blocked": false,
            "blockMessage": Value::Null,
            "matches": [],
        }),
        MaskTextResult::Handled {
            text: replacement,
            hits,
            blocked_message,
        } => {
            let masked_text = replacement.unwrap_or_else(|| text.to_string());
            let restored_text = if blocked_message.is_none() && compiled.settings.reversible {
                vault.restore_text(&masked_text, false).0
            } else {
                masked_text.clone()
            };
            let matches: Vec<Value> = hits
                .iter()
                .map(|hit| {
                    json!({
                        "kind": hit.kind,
                        "label": kind_label_zh(&hit.kind),
                        "count": hit.count,
                        "action": hit.action.as_str(),
                    })
                })
                .collect();
            json!({
                "maskedText": masked_text,
                "restoredText": restored_text,
                "blocked": blocked_message.is_some(),
                "blockMessage": blocked_message,
                "matches": matches,
            })
        }
    }
}
