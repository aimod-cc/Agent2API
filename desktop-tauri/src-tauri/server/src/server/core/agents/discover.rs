use super::{clients, files, ClientDef, ConfigFormat, Profile};
use std::path::Path;

pub fn read_live(def: &ClientDef, path: &Path) -> (Vec<Profile>, Option<String>) {
    match read(def, path) {
        Ok(profiles) => (profiles, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

pub fn read(def: &ClientDef, path: &Path) -> Result<Vec<Profile>, String> {
    if def.id == "dsh" && super::dsh_paths::is_patch(path) { return clients::read_dsh(path); }
    let bytes = files::read(path)?;
    let has_env = def.format == ConfigFormat::JsonEnv && path.with_file_name(".env").is_file();
    if bytes.is_none() && !has_env {
        return Ok(Vec::new());
    }
    let text = files::text(bytes.as_deref())?;
    if text.trim().is_empty() && !has_env {
        return Ok(Vec::new());
    }
    let value = files::parse(def.format, &text)?;
    clients::read(def, path, &value)
}
