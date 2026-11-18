use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use super::super::declaration::{Resource, Storage};
use super::super::{paths, windows};
use crate::server::core::agents::files;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    JsonFile { path: PathBuf, fields: Vec<String>, create: bool },
    Sqlite { path: PathBuf, table: String, key_column: String, value_column: String, keys: Vec<String> },
    Keyring { target: String, username: String },
}

impl Target {
    pub fn from_resource(resource: &Resource, executable: &Path) -> Result<Self, String> {
        Ok(match &resource.storage {
            Storage::JsonFile { location, fields, create } => Self::JsonFile { path: paths::location(location, Some(executable))?, fields: fields.clone(), create: *create },
            Storage::Sqlite { location, variants, table, key_column, value_column, keys } => {
                let existing = std::iter::once(location).chain(variants).map(|location| paths::location(location, Some(executable)))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut selected: Vec<PathBuf> = Vec::new();
                for path in existing.into_iter().filter(|path| path.exists()) {
                    if !selected.iter().any(|previous| paths::same(previous, &path)) { selected.push(path); }
                }
                if selected.len() != 1 { return Err("客户端数据库位置缺失或存在多份，请先确认正在使用的客户端配置".into()); }
                Self::Sqlite { path: selected.remove(0), table: table.clone(), key_column: key_column.clone(), value_column: value_column.clone(), keys: keys.clone() }
            },
            Storage::Keyring { target, username } => Self::Keyring { target: target.clone(), username: username.clone() },
        })
    }

    pub fn same_resource(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Keyring { target: a, .. }, Self::Keyring { target: b, .. }) => a.eq_ignore_ascii_case(b),
            (Self::JsonFile { path: a, .. } | Self::Sqlite { path: a, .. },
             Self::JsonFile { path: b, .. } | Self::Sqlite { path: b, .. }) => paths::same(a, b),
            _ => false,
        }
    }

    pub fn display(&self) -> String {
        match self { Self::JsonFile { path, .. } | Self::Sqlite { path, .. } => path.display().to_string(), Self::Keyring { target, .. } => format!("Windows 凭据库 · {target}") }
    }

    pub fn probe(&self) -> Result<(), String> {
        match self {
            Self::JsonFile { path, create, .. } => {
                paths::checked(path, *create)?;
                if path.exists() && !path.is_file() { return Err("客户端凭证不是普通文件".into()); }
                if !path.exists() && !create { return Err("客户端凭证文件尚未初始化，请先在官方客户端登录".into()); }
                Ok(())
            }
            Self::Sqlite { .. } => self.connection(false).map(|_| ()),
            Self::Keyring { .. } => Ok(()),
        }
    }

    fn connection(&self, writable: bool) -> Result<Connection, String> {
        let Self::Sqlite { path, table, key_column, value_column, .. } = self else { return Err("不是客户端数据库资源".into()); };
        paths::checked(path, false)?;
        if !path.is_file() || std::fs::metadata(path).map_err(|_| "客户端数据库不可读")?.len() > 128 * 1024 * 1024 {
            return Err("客户端数据库不在支持范围内".into());
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
            if sidecar.exists() { paths::checked(&sidecar, false)?; }
        }
        let flags = if writable { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
        let db = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_| "客户端数据库不可读或正在使用")?;
        db.busy_timeout(std::time::Duration::from_secs(2)).map_err(|_| "客户端数据库等待配置失败")?;
        let mut statement = db.prepare(&format!("PRAGMA table_info(\"{table}\")")).map_err(|_| "客户端数据库布局不支持")?;
        let fields = statement.query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(5)?)))
            .map_err(|_| "客户端数据库布局不支持")?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| "客户端数据库布局不支持")?;
        let key = fields.iter().find(|(name, _, _)| name == key_column);
        let value = fields.iter().find(|(name, _, _)| name == value_column);
        if fields.len() != 2 || key.is_none_or(|(_, kind, _)| !kind.eq_ignore_ascii_case("TEXT"))
            || value.is_none_or(|(_, kind, _)| !["TEXT", "BLOB"].iter().any(|allowed| kind.eq_ignore_ascii_case(allowed))) {
            return Err("客户端数据库布局不支持，未尝试其他写入方式".into());
        }
        let mut unique = key.is_some_and(|(_, _, primary)| *primary > 0 && fields.iter().filter(|(_, _, primary)| *primary > 0).count() == 1);
        let indexes = db.prepare("SELECT name FROM pragma_index_list(?1) WHERE \"unique\"=1 AND partial=0")
            .map_err(|_| "客户端数据库索引不可确认")?.query_map([table], |row| row.get::<_, String>(0))
            .map_err(|_| "客户端数据库索引不可确认")?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| "客户端数据库索引不可确认")?;
        for index in indexes {
            let columns = db.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                .map_err(|_| "客户端数据库索引不可确认")?.query_map([index], |row| row.get::<_, Option<String>>(0))
                .map_err(|_| "客户端数据库索引不可确认")?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| "客户端数据库索引不可确认")?;
            unique |= columns.len() == 1 && columns[0].as_deref() == Some(key_column.as_str());
        }
        if !unique { return Err("客户端账号键不是唯一索引，未写入凭证".into()); }
        drop(statement);
        Ok(db)
    }

    pub fn read(&self) -> Result<Value, String> {
        match self {
            Self::JsonFile { path, fields, create } => {
                paths::checked(path, *create)?;
                let before = files::read_limited(path, 256 * 1024)?;
                let mut result = Map::new();
                if let Some(bytes) = before {
                    let value = super::super::Secret(serde_json::from_slice(&Zeroizing::new(bytes)).map_err(|_| "客户端凭证文件格式不支持")?);
                    let source = value.as_object().ok_or("客户端凭证文件格式不支持")?;
                    for field in fields { result.insert(field.clone(), source.get(field).cloned().unwrap_or(Value::Null)); }
                } else if !create { return Err("客户端凭证文件不存在".into()); }
                else { for field in fields { result.insert(field.clone(), Value::Null); } }
                Ok(Value::Object(result))
            }
            Self::Sqlite { table, key_column, value_column, keys, .. } => {
                let db = self.connection(false)?;
                let mut result = Map::new();
                for key in keys {
                    let value: Option<String> = db.query_row(&format!("SELECT \"{value_column}\" FROM \"{table}\" WHERE \"{key_column}\"=?1"), [key], |row| row.get(0))
                        .optional().map_err(|_| "客户端账号记录不可读")?;
                    if value.as_ref().is_some_and(|value| value.len() > 256 * 1024) { return Err("客户端账号记录过大".into()); }
                    result.insert(key.clone(), value.map(Value::String).unwrap_or(Value::Null));
                }
                Ok(Value::Object(result))
            }
            Self::Keyring { target, .. } => Ok(windows::read_credential(target)?.map(|credential|
                serde_json::to_value(&credential).map_err(|_| "系统凭证备份格式无效".to_string())).transpose()?.unwrap_or(Value::Null)),
        }
    }

    pub fn snapshot(&self) -> Result<Value, String> {
        let Self::JsonFile { path, fields, create } = self else { return self.read(); };
        paths::checked(path, *create)?;
        let bytes = files::read_limited(path, 256 * 1024)?.map(Zeroizing::new);
        let exists = bytes.is_some();
        if !exists && !create { return Err("客户端凭证文件不存在".into()); }
        let value = super::super::Secret(bytes.as_ref().map(|bytes| serde_json::from_slice(bytes)).transpose()
            .map_err(|_| "客户端凭证文件格式不支持")?.unwrap_or_else(|| Value::Object(Map::new())));
        let source = value.as_object().ok_or("客户端凭证文件格式不支持")?;
        Ok(serde_json::json!({ "exists": exists, "fields": fields.iter().map(|field| (field.clone(), serde_json::json!({
            "present": source.contains_key(field), "value": source.get(field).cloned().unwrap_or(Value::Null),
        }))).collect::<Map<_, _>>() }))
    }

    pub fn exposed(&self, snapshot: &Value) -> Value {
        match self {
            Self::JsonFile { fields, .. } => Value::Object(fields.iter().map(|field|
                (field.clone(), snapshot["fields"][field]["value"].clone())).collect()),
            _ => snapshot.clone(),
        }
    }

    pub fn desired(&self, value: &Value) -> Result<Value, String> {
        if value.to_string().len() > 256 * 1024 { return Err("客户端凭证修改方案过大".into()); }
        match self {
            Self::JsonFile { fields, .. } => {
                let values = value.as_object().ok_or("凭证修改必须是对象")?;
                if values.len() != fields.len() || fields.iter().any(|field| !values.contains_key(field)) { return Err("插件试图修改未授权的凭证字段".into()); }
                Ok(serde_json::json!({ "exists": true, "fields": fields.iter().map(|field| (field.clone(), serde_json::json!({
                    "present": !value[field].is_null(), "value": value[field],
                }))).collect::<Map<_, _>>() }))
            }
            Self::Sqlite { keys, .. } => {
                let values = value.as_object().ok_or("数据库凭证修改必须是对象")?;
                if values.len() != keys.len() || keys.iter().any(|key| !values.contains_key(key))
                    || values.values().any(|value| !value.is_null() && !value.is_string()) { return Err("插件试图修改未授权的数据库记录".into()); }
                Ok(value.clone())
            }
            Self::Keyring { username, .. } => {
                if !value.is_object() { return Err("系统凭证必须是对象".into()); }
                let bytes = serde_json::to_vec(value).map_err(|_| "系统凭证无法编码")?;
                if bytes.is_empty() || bytes.len() > 2560 { return Err("客户端凭证超出 Windows 凭据库大小限制，未写入".into()); }
                serde_json::to_value(&windows::Credential { username: username.clone(), bytes, persist: 2 }).map_err(|_| "系统凭证无法编码".into())
            }
        }
    }

    pub fn write(&self, expect: &Value, desired: &Value) -> Result<(), String> {
        let current = super::super::Secret(self.snapshot()?);
        if &current.0 != expect { return Err("客户端凭证已被其他程序修改，未覆盖".into()); }
        match self {
            Self::JsonFile { path, fields, create } => {
                let before = files::read_limited(path, 256 * 1024)?.map(Zeroizing::new);
                let value: Value = before.as_ref().map(|bytes| serde_json::from_slice(bytes)).transpose().map_err(|_| "客户端凭证文件格式不支持")?.unwrap_or_else(|| Value::Object(Map::new()));
                let mut value = super::super::Secret(value);
                let source = value.as_object_mut().ok_or("客户端凭证文件格式不支持")?;
                if expect["exists"] != Value::Bool(before.is_some()) || fields.iter().any(|field|
                    expect["fields"][field]["present"] != Value::Bool(source.contains_key(field))
                        || expect["fields"][field]["value"] != source.get(field).cloned().unwrap_or(Value::Null)) {
                    return Err("客户端凭证文件已变化，未覆盖".into());
                }
                for field in fields {
                    let item = &desired["fields"][field];
                    if item["present"] == true { source.insert(field.clone(), item["value"].clone()); } else { source.remove(field); }
                }
                let latest = files::read_limited(path, 256 * 1024)?.map(Zeroizing::new);
                if latest != before { return Err("客户端凭证文件已变化，未覆盖".into()); }
                if before.is_none() && !create { return Err("客户端凭证文件不存在".into()); }
                paths::checked(path, *create)?;
                if desired["exists"] == false {
                    if !source.is_empty() { return Err("新建凭证文件已被其他程序添加内容，未删除".into()); }
                    if before.is_some() { std::fs::remove_file(path).map_err(|_| "新建凭证文件无法恢复为不存在")?; }
                } else {
                    let encoded = Zeroizing::new(serde_json::to_vec(&value.0).map_err(|_| "客户端凭证无法编码")?);
                    files::write_atomic(path, &encoded)?;
                }
            }
            Self::Sqlite { table, key_column, value_column, keys, .. } => {
                let mut db = self.connection(true)?;
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_| "客户端数据库仍在使用，未写入")?;
                for key in keys {
                    let current: Option<String> = tx.query_row(&format!("SELECT \"{value_column}\" FROM \"{table}\" WHERE \"{key_column}\"=?1"), [key], |row| row.get(0)).optional().map_err(|_| "客户端账号记录不可读")?;
                    if current.map(Value::String).unwrap_or(Value::Null) != expect[key] { return Err("客户端账号记录已变化，未覆盖".into()); }
                }
                for key in keys {
                    match desired[key].as_str() {
                        Some(value) => { tx.execute(&format!("INSERT INTO \"{table}\" (\"{key_column}\",\"{value_column}\") VALUES (?1,?2) ON CONFLICT(\"{key_column}\") DO UPDATE SET \"{value_column}\"=excluded.\"{value_column}\""), [key.as_str(), value]).map_err(|_| "客户端账号记录写入失败")?; }
                        None => { tx.execute(&format!("DELETE FROM \"{table}\" WHERE \"{key_column}\"=?1"), [key]).map_err(|_| "客户端旧账号记录清理失败")?; }
                    }
                }
                tx.commit().map_err(|_| "客户端数据库提交结果未确认")?;
            }
            Self::Keyring { target, .. } => {
                let value = if desired.is_null() { None } else { Some(serde_json::from_value::<windows::Credential>(desired.clone()).map_err(|_| "系统凭证恢复数据无效")?) };
                windows::write_credential(target, value.as_ref())?;
            }
        }
        let verified = super::super::Secret(self.snapshot()?);
        if &verified.0 != desired { return Err("客户端凭证写入后的校验未通过".into()); }
        Ok(())
    }
}
