use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{config, CodingError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryInput {
    #[serde(default)]
    pub path: String,
}

pub fn browse(input: DirectoryInput) -> Result<Value, CodingError> {
    let path = if input.path.trim().is_empty() { config::canonical_directory(&config::display_path(&config::user_home()))? }
        else { config::canonical_directory(&input.path)? };
    let mut entries = Vec::new();
    let mut limited = false;
    for entry in std::fs::read_dir(&path).map_err(|_| CodingError::new(403, "无法读取这个文件夹，请检查访问权限"))? {
        let Ok(entry) = entry else { continue; };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        let Ok(directory) = config::canonical_directory(&config::display_path(&entry.path())) else { continue; };
        if entries.len() >= 500 { limited = true; break; }
        entries.push(json!({ "name": name, "path": config::display_path(&directory) }));
    }
    entries.sort_by(|a, b| super::string(a, "name").to_lowercase().cmp(&super::string(b, "name").to_lowercase()));
    let mut locations = vec![json!({ "name": "home", "path": config::display_path(&config::user_home()) })];
    if cfg!(windows) {
        for drive in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", drive as char));
            if root.is_dir() { locations.push(json!({ "name": format!("{}:", drive as char), "path": config::display_path(&root) })); }
        }
    } else { locations.push(json!({ "name": "/", "path": "/" })); }
    Ok(json!({ "path": config::display_path(&path), "parent": path.parent().map(config::display_path),
        "entries": entries, "locations": locations, "limited": limited }))
}
