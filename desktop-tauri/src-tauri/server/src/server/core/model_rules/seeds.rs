use super::{set_membership, Mapping, ModelRules};

const EXTRA_ALIASES: &[(&str, &str, &str)] = &[
    ("raccoon", "sn-deepseek-v4-1-flash", "deepseek-v4.1-flash"),
    (
        "cline-free",
        "cline-free/deepseek-v4.1-flash",
        "deepseek-v4.1-flash",
    ),
    (
        "cline-pass",
        "cline-pass/deepseek-v4.1-flash",
        "deepseek-v4.1-flash",
    ),
    ("cline-free", "z-ai/glm-5.3-flash", "glm-5.3-flash"),
    ("cline-pass", "cline-pass/glm-5.3-flash", "glm-5.3-flash"),
];

pub(super) fn seed_extra_aliases(
    rules: &mut ModelRules,
    provider: &str,
    id: &str,
    ids: &[String],
    added: &mut Vec<String>,
) -> bool {
    let mut touched = false;
    for (owner, target, alias) in EXTRA_ALIASES {
        if !owner.eq_ignore_ascii_case(provider) || !target.eq_ignore_ascii_case(id) {
            continue;
        }
        let key = format!("{provider}:{id}#alias:{alias}");
        if rules
            .seeded
            .iter()
            .any(|item| item.eq_ignore_ascii_case(&key))
        {
            continue;
        }
        rules.seeded.push(key);
        touched = true;
        if ids.iter().any(|other| other.eq_ignore_ascii_case(alias)) {
            continue;
        }
        let exists = rules.mappings.iter().any(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && mapping.target.eq_ignore_ascii_case(target)
                && mapping
                    .provider
                    .as_deref()
                    .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        });
        if exists {
            continue;
        }
        let occupied = rules.mappings.iter().any(|mapping| {
            mapping.alias.eq_ignore_ascii_case(alias)
                && mapping
                    .provider
                    .as_deref()
                    .map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        });
        rules.mappings.push(Mapping {
            alias: alias.to_string(),
            target: id.to_string(),
            provider: Some(provider.to_string()),
            reasoning: None,
            enabled: !occupied,
        });
        if !occupied {
            added.push(format!("{alias} → {id}"));
        }
    }
    touched
}

pub const QODER_DEFAULT_ENABLED: &[&str] = &[];

pub fn seed_qoder_defaults(_provider: &str, _label: &str, _ids: &[String]) -> Option<String> {
    None
}

pub fn migrate_qoder_default_enabled() -> Option<String> {
    migrate_whitelist_disabled(
        "migration:qoder-visibility-v1",
        &["qoder", "qoder-intl"],
        "Qoder",
    )
}

pub fn migrate_workbuddy_default_enabled() -> Option<String> {
    migrate_whitelist_disabled(
        "migration:workbuddy-visibility-v1",
        &["workbuddy", "workbuddy-intl"],
        "WorkBuddy",
    )
}

fn migrate_whitelist_disabled(mark: &str, providers: &[&str], label: &str) -> Option<String> {
    let reopened = super::mutations::mutate(|rules| {
        if rules
            .seeded
            .iter()
            .any(|item| item.eq_ignore_ascii_case(mark))
        {
            return Ok(Vec::new());
        }
        let mut reopened = Vec::new();
        let targets = rules
            .disabled
            .iter()
            .filter_map(|entry| {
                entry
                    .provider
                    .as_ref()
                    .filter(|owner| {
                        providers
                            .iter()
                            .any(|provider| owner.eq_ignore_ascii_case(provider))
                    })
                    .map(|provider| (provider.clone(), entry.id.clone()))
            })
            .collect::<Vec<_>>();
        for (provider, id) in targets {
            let legacy_key = format!("{provider}:{id}");
            if !rules
                .seeded
                .iter()
                .any(|item| item.eq_ignore_ascii_case(&legacy_key))
            {
                continue;
            }
            set_membership(&mut rules.disabled, Some(&provider), &id, false);
            reopened.push(format!("{provider}/{id}"));
        }
        rules.seeded.push(mark.to_string());
        Ok(reopened)
    })
    .ok()?;
    if reopened.is_empty() {
        return None;
    }
    Some(format!(
        "🧩 {label} 迁移: 重新打开 {} 个被旧白名单误关的模型 [{}]",
        reopened.len(),
        reopened.join(", ")
    ))
}

pub fn seed_hidden_disabled(
    provider: &str,
    label: &str,
    entries: &[(String, &str)],
) -> Option<String> {
    let disabled = super::mutations::mutate(|rules| {
        let mut disabled = Vec::new();
        for (id, visibility) in entries {
            let id = id.trim();
            if id.is_empty() || *visibility != "hidden" {
                continue;
            }
            let key = format!("visibility:{provider}:{id}");
            if rules
                .seeded
                .iter()
                .any(|item| item.eq_ignore_ascii_case(&key))
            {
                continue;
            }
            rules.seeded.push(key);
            if rules.default_enabled(provider, id) {
                set_membership(&mut rules.disabled, Some(provider), id, true);
                disabled.push(id.to_string());
            }
        }
        Ok(disabled)
    })
    .ok()?;
    if disabled.is_empty() {
        return None;
    }
    Some(format!(
        "🧩 {label}可见性规则: 默认关闭 {} 个不可见模型 [{}]",
        disabled.len(),
        disabled.join(", ")
    ))
}
