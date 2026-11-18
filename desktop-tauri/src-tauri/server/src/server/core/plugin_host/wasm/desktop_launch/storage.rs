mod target;
pub(super) mod journal;
pub use target::Target;
pub use journal::Journal;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::declaration::{Client, CredentialPlan};

#[derive(Serialize, Deserialize)]
pub struct Edit {
    pub resource: String,
    pub target: Target,
    pub before: Value,
    pub after: Value,
}

impl Drop for Edit {
    fn drop(&mut self) { super::wipe(&mut self.before); super::wipe(&mut self.after); }
}

pub fn targets(client: &Client, executable: &Path) -> Result<BTreeMap<String, Target>, String> {
    let mut targets = BTreeMap::new();
    for resource in &client.resources {
        let target = Target::from_resource(resource, executable)?;
        if targets.values().any(|previous| target.same_resource(previous)) { return Err("客户端声明了重复凭证位置，未写入".into()); }
        targets.insert(resource.id.clone(), target);
    }
    Ok(targets)
}

pub fn edits(plan: &CredentialPlan, targets: &BTreeMap<String, Target>) -> Result<Vec<Edit>, String> {
    if plan.version != 1 || plan.changes.len() != targets.len() { return Err("插件没有准备完整的本机凭证修改方案".into()); }
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for change in &plan.changes {
        if !seen.insert(&change.resource) { return Err("插件重复修改同一凭证资源".into()); }
        let target = targets.get(&change.resource).ok_or("插件试图修改未授权的本机资源")?.clone();
        let before = target.snapshot()?;
        let after = target.desired(&change.value)?;
        result.push(Edit { resource: change.resource.clone(), target, before, after });
    }
    Ok(result)
}

pub fn rollback(edits: &[Edit]) -> Result<(), String> {
    let mut failed = Vec::new();
    for edit in edits.iter().rev() {
        match edit.target.snapshot().map(super::Secret) {
            Ok(current) if current.0 == edit.before => {},
            Ok(current) if current.0 == edit.after => {
                if let Err(error) = edit.target.write(&edit.after, &edit.before) { failed.push(error); }
            }
            Ok(_) => failed.push("客户端凭证已被其他程序改变，未用备份覆盖".into()),
            Err(error) => failed.push(error),
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("；")) }
}
