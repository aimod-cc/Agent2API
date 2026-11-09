use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::db::Db;
use super::{config::{self, CodingConfig}, now, CodingError};

const MAX_PROJECTS: usize = 500;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingProject {
    pub id: String,
    pub directory: String,
    pub created_at: i64,
    #[serde(default)]
    pub legacy: bool,
}

impl CodingProject {
    pub fn public(&self) -> Value {
        json!({ "id": self.id, "name": name(&self.directory), "directory": self.directory,
            "createdAt": self.created_at, "accessible": config::canonical_directory(&self.directory).is_ok(), "legacy": self.legacy })
    }
}

pub fn name(directory: &str) -> String {
    Path::new(directory).file_name().map(|value| value.to_string_lossy().into_owned()).filter(|value| !value.is_empty())
        .unwrap_or_else(|| directory.to_string())
}

fn key(scope: &str) -> String { format!("assistantCodexProjects:{scope}") }

fn identity(path: &Path) -> String {
    let text = config::display_path(path);
    let normalized = if cfg!(windows) { text.to_lowercase() } else { text };
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

fn project(path: &Path, legacy: bool) -> CodingProject {
    CodingProject { id: identity(path), directory: config::display_path(path), created_at: now(), legacy }
}

fn read(conn: &Connection, scope: &str) -> Result<Vec<CodingProject>, CodingError> {
    let text: Option<String> = conn.query_row("SELECT value FROM kv WHERE key=?1", [key(scope)], |row| row.get(0)).optional()
        .map_err(|_| CodingError::new(500, "读取 Codex 项目失败"))?;
    text.map(|text| serde_json::from_str(&text).map_err(|_| CodingError::new(500, "Codex 项目记录已损坏，未覆盖原数据")))
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn write(conn: &Connection, scope: &str, projects: &[CodingProject]) -> Result<(), CodingError> {
    let text = serde_json::to_string(projects).map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
    conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key(scope), text])
        .map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
    Ok(())
}

pub fn migrate_legacy(conn: &Connection, config: &CodingConfig) -> Result<(), CodingError> {
    let scope = config.scope();
    let mut projects = read(conn, &scope)?;
    for raw in &config.workspace_roots {
        let path = config::canonical_directory(raw).unwrap_or_else(|_| PathBuf::from(raw));
        let item = project(&path, true);
        if !projects.iter().any(|existing| existing.id == item.id) { projects.push(item); }
    }
    write(conn, &scope, &projects)
}

pub fn load(db: &Db, scope: &str) -> Result<Vec<CodingProject>, CodingError> {
    db.with(|conn| read(conn, scope)).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
}

pub fn public(projects: &[CodingProject]) -> Vec<Value> { projects.iter().map(CodingProject::public).collect() }

pub fn find<'a>(projects: &'a [CodingProject], path: &Path) -> Option<&'a CodingProject> {
    projects.iter().filter_map(|project| config::canonical_directory(&project.directory).ok().map(|root| (project, root)))
        .filter(|(_, root)| path.starts_with(root)).max_by_key(|(_, root)| root.components().count()).map(|(project, _)| project)
}

pub fn directory(projects: &[CodingProject], raw: &str) -> Result<PathBuf, CodingError> {
    let path = config::canonical_directory(raw)?;
    if find(projects, &path).is_none() { return Err(CodingError::new(403, "请先通过“新建项目”添加这个文件夹，再打开其中的会话")); }
    Ok(path)
}

pub fn include_history(db: &Db, scope: &str, directories: &[String]) -> Result<Vec<CodingProject>, CodingError> {
    db.with_mut(|conn| {
        let tx = conn.transaction().map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
        let mut projects = read(&tx, scope)?;
        let mut changed = false;
        for raw in directories {
            if projects.len() >= MAX_PROJECTS { break; }
            let Ok(path) = directory(&projects, raw) else { continue; };
            let item = project(&path, false);
            if !projects.iter().any(|existing| existing.id == item.id) { projects.push(item); changed = true; }
        }
        if changed { write(&tx, scope, &projects)?; }
        tx.commit().map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
        Ok(projects)
    }).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInput {
    pub directory: String,
    pub folder_name: Option<String>,
}

fn validate_name(name: &str) -> Result<&str, CodingError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 255 || matches!(name, "." | "..") || name.chars().any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        || name.ends_with('.') || name.ends_with(' ') {
        return Err(CodingError::new(400, "文件夹名称无效，请只填写名称，不要填写路径"));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if cfg!(windows) && (matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| stem.strip_prefix(prefix).is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')))) {
        return Err(CodingError::new(400, "这个文件夹名称被系统保留，请换一个名称"));
    }
    Ok(name)
}

pub fn create(db: &Db, config: &CodingConfig, input: ProjectInput) -> Result<Value, CodingError> {
    let parent = config::canonical_directory(&input.directory)?;
    let folder_name = input.folder_name.as_deref().map(validate_name).transpose()?;
    let path = folder_name.map(|name| parent.join(name)).unwrap_or_else(|| parent.clone());
    if path.parent().is_none() { return Err(CodingError::new(400, "请选择具体的项目文件夹，不要使用整个磁盘或文件系统根目录")); }
    let scope = config.scope();
    let mut created = false;
    let saved = db.with_mut(|conn| {
        let tx = conn.transaction().map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
        let mut projects = read(&tx, &scope)?;
        if projects.len() >= MAX_PROJECTS { return Err(CodingError::new(400, "项目数量已达到上限")); }
        if folder_name.is_some() {
            std::fs::create_dir(&path).map_err(|error| if error.kind() == std::io::ErrorKind::AlreadyExists {
                CodingError::new(409, "这个文件夹已经存在，请选择已有文件夹，或换一个名称")
            } else { CodingError::new(403, "无法创建文件夹，请检查位置和写入权限") })?;
            created = true;
        }
        let canonical = config::canonical_directory(&config::display_path(&path))?;
        let item = project(&canonical, false);
        let item = if let Some(existing) = projects.iter_mut().find(|existing| existing.id == item.id) {
            existing.legacy = false;
            existing.clone()
        } else { projects.push(item.clone()); item };
        write(&tx, &scope, &projects)?;
        tx.commit().map_err(|_| CodingError::new(500, "保存 Codex 项目失败"))?;
        Ok(item)
    }).ok_or_else(|| CodingError::new(500, "数据库不可用"))?;
    match saved {
        Ok(item) => Ok(json!({ "project": item.public(), "createdDirectory": created })),
        Err(_) if created => Err(CodingError::new(500, "文件夹已创建，但项目保存失败；可选择这个已有文件夹重试，文件夹没有被删除")),
        Err(error) => Err(error),
    }
}
