use std::path::Path;

use serde_json::Value;

use super::super::{
    dsh_patch::{self, Composition, DEFAULT_MODEL, PROVIDERS},
    files::{self, Change},
    Profile,
};
use super::{dsh, Plan};

pub fn read(path: &Path) -> Result<Vec<Profile>, String> {
    let composition = Composition::load(path)?;
    dsh::read(path, &composition.settings()?)
}

pub fn plan(path: &Path, profile: &Profile, operation: &str) -> Result<Plan, String> {
    let composition = Composition::load(path)?;
    let before = composition.settings()?;
    let mut after = before.clone();
    let mut warnings = vec![
        "只修改所选 DSH 配置，不会同步覆盖其他运行方式。".into(),
        "写入完成仅表示文件已校验，尚未验证 DSH 是否加载；必要时重启所选运行方式并新建会话。"
            .into(),
    ];
    let extra = if operation == "native-delete" {
        warnings.push("DSH 密钥可能由其他运行方式共用，删除连接时保留独立密钥文件。".into());
        let extra = dsh::delete(path, &mut after, profile)?;
        if after.get(DEFAULT_MODEL).is_none() {
            let fallback = composition
                .inherited
                .get(DEFAULT_MODEL)
                .and_then(|entry| entry.value.get("config"))
                .cloned()
                .ok_or("无法确认 DSH 内置默认模型，请先在客户端切换默认模型再删除")?;
            if !fallback.is_object()
                || super::super::dsh_document::dynamic(&fallback)
                || fallback.get("provider").and_then(Value::as_str)
                    == Some(profile.native.source_id.as_str())
            {
                return Err("所选提供商仍被 DSH 默认配置引用，请先在客户端切换默认模型".into());
            }
            after[DEFAULT_MODEL] = fallback;
            warnings.push(
                "删除当前默认提供商后恢复 DSH 内置模型；共享密钥保留，避免影响其他配置。".into(),
            );
        }
        extra
    } else {
        dsh::write(path, &mut after, profile, operation, &mut warnings)?
    };
    let mut change = Change::load(path.into())?;
    let original = files::text(change.before.as_deref())?;
    let mut text = original.clone();
    for id in [PROVIDERS, DEFAULT_MODEL] {
        if before.get(id) == after.get(id) {
            continue;
        }
        composition.editable(id, path)?;
        text = dsh_patch::update_config(&text, id, after.get(id).unwrap_or(&Value::Null))?;
    }
    if text == original {
        change.after = change.before.clone();
    } else {
        if Composition::preview(path, &text)?.settings()? != after {
            return Err("DSH 叠加后的模型配置与预期不一致，已停止写入".into());
        }
        change.after = Some(text.into_bytes());
    }
    let mut changes = vec![change];
    changes.extend(extra);
    composition.guards(&mut changes);
    for change in &changes {
        change.check()?;
    }
    let mut planned_profile = profile.clone();
    if operation == "native-delete" {
        planned_profile.kind = "native-delete".into();
    }
    Ok(Plan {
        changes,
        warnings,
        profile: planned_profile,
        operation: operation.into(),
    })
}
