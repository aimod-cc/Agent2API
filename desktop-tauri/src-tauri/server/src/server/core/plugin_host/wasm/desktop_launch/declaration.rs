use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CAPABILITY: &str = "desktop.launch.v1";
pub const CHECK: &str = "desktop.launch.check";
pub const PREPARE: &str = "desktop.launch.prepare";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub version: u32,
    pub clients: Vec<Client>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub id: String,
    pub label: String,
    pub platform: String,
    pub executables: Vec<String>,
    pub locations: Vec<Location>,
    pub identity: Identity,
    pub resources: Vec<Resource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub root: Root,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Root {
    Home,
    AppData,
    LocalAppData,
    ProgramFiles,
    ProgramFilesX86,
    Installation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Identity {
    JsonProduct { path: String, field: String, values: Vec<String> },
    PeProduct { values: Vec<String>, minimum_version: [u16; 3], absent: Vec<String> },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub label: String,
    pub storage: Storage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Storage {
    JsonFile { location: Location, fields: Vec<String>, #[serde(default)] create: bool },
    Sqlite { location: Location, #[serde(default)] variants: Vec<Location>, table: String, key_column: String, value_column: String, keys: Vec<String> },
    Keyring { target: String, username: String },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialPlan {
    pub version: u32,
    pub client: String,
    pub expect: Value,
    pub changes: Vec<Change>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub resource: String,
    pub value: Value,
}

impl Drop for CredentialPlan {
    fn drop(&mut self) {
        super::wipe(&mut self.expect);
        for change in &mut self.changes { super::wipe(&mut change.value); }
    }
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !matches!(value, "__proto__" | "prototype" | "constructor")
}

pub fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub fn relative(value: &str) -> bool {
    !(value.is_empty() || value.len() > 1024 || value.contains(['\\', ':', '\0', '<', '>', '"', '|', '?', '*']))
        && !value.chars().any(char::is_control)
        && value.split('/').all(|part| {
            let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !part.is_empty() && part != "." && part != ".." && !part.ends_with(['.', ' '])
                && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !((base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4 && matches!(base.as_bytes()[3], b'1'..=b'9'))
        })
}

fn names(values: &[String], maximum: usize) -> bool {
    !values.is_empty() && values.len() <= maximum && values.iter().all(|value| text(value, 256))
        && values.iter().collect::<HashSet<_>>().len() == values.len()
}

fn resource_location(location: &Location) -> bool {
    if !relative(&location.path) || matches!(location.root, Root::ProgramFiles | Root::ProgramFilesX86) { return false; }
    let first = location.path.split('/').next().unwrap_or("").to_ascii_lowercase();
    !matches!(first.as_str(), "microsoft" | "windows" | "programs" | "temp" | ".ssh" | ".aws" | ".azure" | ".gnupg" | ".agent2api" | ".zcode" | "google" | "mozilla")
        && location.path.split('/').count() >= 2
}

impl Declaration {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.clients.is_empty() || self.clients.len() > 8 { return Err("本机客户端声明的版本或数量无效".into()); }
        let mut clients = HashSet::new();
        for client in &self.clients {
            if !identifier(&client.id) || !clients.insert(&client.id) || !text(&client.label, 128)
                || client.platform != "windows" || !names(&client.executables, 8)
                || client.executables.iter().any(|name| !relative(name) || name.contains('/') || !name.to_ascii_lowercase().ends_with(".exe"))
                || client.locations.is_empty() || client.locations.len() > 16
                || client.locations.iter().any(|location| !relative(&location.path) || matches!(location.root, Root::Installation))
            { return Err("本机客户端名称、平台或程序范围无效".into()); }
            match &client.identity {
                Identity::JsonProduct { path, field, values } if relative(path) && identifier(field) && names(values, 8) => {},
                Identity::PeProduct { values, absent, .. } if names(values, 8) && !absent.is_empty() && absent.len() <= 8 && absent.iter().all(|path| relative(path)) => {},
                _ => return Err("本机客户端身份校验声明无效".into()),
            }
            let mut resources = HashSet::new();
            if client.resources.is_empty() || client.resources.len() > 8 { return Err("本机凭证资源数量无效".into()); }
            for resource in &client.resources {
                if !identifier(&resource.id) || !resources.insert(&resource.id) || !text(&resource.label, 128) { return Err("本机凭证资源标识无效".into()); }
                match &resource.storage {
                    Storage::JsonFile { location, fields, .. } if resource_location(location) && names(fields, 32) && fields.iter().all(|field| identifier(field)) => {},
                    Storage::Sqlite { location, variants, table, key_column, value_column, keys } if resource_location(location)
                        && variants.len() <= 8 && variants.iter().all(resource_location)
                        && [table, key_column, value_column].iter().all(|value| identifier(value)) && names(keys, 32) => {},
                    Storage::Keyring { target, username } if text(target, 256) && target.contains(':') && text(username, 128) => {},
                    _ => return Err("本机凭证写入范围无效".into()),
                }
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> Value {
        json!({ "version": self.version, "clients": self.clients.iter().map(|client| json!({
            "id": client.id, "label": client.label, "platform": client.platform,
        })).collect::<Vec<_>>() })
    }
}
