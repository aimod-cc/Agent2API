//! 客户端用户级提示词、Skill 与 MCP：路径在服务端确定，写入必须匹配读到的文件版本。

mod mcp;
mod paths;
mod skills;

use std::path::Path;
use serde_json::{json, Value};
use super::{backups, clients::Plan, files, preview, ClientDef, Profile};

pub fn revision(bytes: Option<&[u8]>) -> String { backups::hash(bytes) }

pub fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!("{} 是链接，拒绝修改客户端资源", ancestor.display()));
        }
    }
    Ok(())
}

pub fn capabilities(def: &ClientDef) -> Value {
    let paths = paths::for_client(def);
    json!({ "prompt": paths.prompt.is_some(), "skills": !paths.skills.is_empty(), "mcp": paths.mcp.is_some(), "note": paths.note })
}

/// 该客户端可安装 Skill 的根目录（按配置文件里的优先级排列）。
///
/// 工具服务的「本机接入」要用它把 Skill 包写进客户端；与「Agent 配置」页的
/// Skill 资源面板同一份事实（都来自 `paths::for_client`），不另抄一份。
pub fn skill_roots(def: &ClientDef) -> Vec<std::path::PathBuf> {
    paths::for_client(def).skills
}

/// 该客户端 MCP 配置文件的路径（不支持 MCP 的客户端返回 None）。
pub fn mcp_config_path(def: &ClientDef) -> Option<std::path::PathBuf> {
    paths::for_client(def).mcp.as_ref().map(|(path, _, _)| path.clone())
}

pub fn snapshot(def: &ClientDef, kind: &str) -> Result<Value, String> {
    let paths = paths::for_client(def);
    match kind {
        "prompt" => {
            let path = paths.prompt.ok_or(paths.note)?;
            let bytes = files::read_limited(&path, 1024 * 1024)?;
            Ok(json!({ "path": path, "exists": bytes.is_some(), "content": files::text(bytes.as_deref())?, "revision": revision(bytes.as_deref()) }))
        }
        "skills" => skills::list(&paths),
        "mcp" => mcp::read(&paths),
        _ => Err("客户端资源类型无效".into()),
    }
}

pub fn skill(def: &ClientDef, id: &str) -> Result<Value, String> {
    let paths = paths::for_client(def);
    let path = skills::doc(&paths, id)?;
    let bytes = files::read_limited(&path, 1024 * 1024)?;
    Ok(json!({ "id": id, "path": path, "content": files::text(bytes.as_deref())?, "revision": revision(bytes.as_deref()) }))
}

fn plan(def: &ClientDef, payload: &Value) -> Result<Plan, String> {
    let paths = paths::for_client(def);
    let field = |key| payload.get(key).and_then(Value::as_str).unwrap_or("");
    let kind = field("kind");
    let delete = payload.get("delete").and_then(Value::as_bool) == Some(true);
    let change = match kind {
        "prompt" => {
            let path = paths.prompt.ok_or(paths.note)?;
            reject_links(&path)?;
            let mut change = files::Change::load(path)?;
            let content = field("content");
            if content.len() > 1024 * 1024 { return Err("提示词超过 1 MB".into()); }
            change.after = Some(content.as_bytes().to_vec());
            change
        }
        "skills" => skills::change(&paths, field("id"), field("content"), delete)?,
        "mcp" => mcp::change(def, &paths, field("name"), if delete { None } else { Some(payload.get("config").ok_or("缺少 MCP 配置")?) })?,
        _ => return Err("客户端资源类型无效".into()),
    };
    if field("revision") != revision(change.before.as_deref()) { return Err("客户端资源已被其他程序修改，请重新读取后再编辑".into()); }
    Ok(Plan { changes: vec![change], warnings: vec![paths.note.into(), "此操作作用在网关所在机器，不会执行 Skill 脚本或启动 MCP 服务。".into()],
        profile: Profile { id: format!("{kind}:{}{}", field("id"), field("name")), kind: "resource".into(), ..Default::default() }, operation: "resource".into() })
}

pub fn create_preview(def: &ClientDef, payload: &Value) -> Result<Value, String> {
    let plan = plan(def, payload)?;
    preview::create_restore(def.id, &plan)
}

pub fn apply(def: &ClientDef, payload: &Value) -> Result<Value, String> {
    let plan = plan(def, payload)?;
    let token = payload.get("token").and_then(Value::as_str).unwrap_or("");
    preview::verify(&plan, def.id, token)?;
    let path = plan.changes[0].path.clone();
    super::apply::apply_resource(def, &path, &plan)?;
    snapshot(def, payload.get("kind").and_then(Value::as_str).unwrap_or(""))
}
