use std::sync::Arc;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::client::{catalog, ClientSettings};
use crate::server::errors::GatewayError;

pub const KEY_CLIENT_IDENTIFICATION: &str = "clientIdentification";

#[derive(Clone, Debug)]
pub struct ClientIdentificationSnapshot {
    pub settings: Result<ClientSettings, String>,
    pub revision: String,
}

impl Default for ClientIdentificationSnapshot {
    fn default() -> Self { from_value(None) }
}

fn revision(value: Option<&Value>) -> String {
    format!("{:x}", Sha256::digest(value.cloned().unwrap_or(Value::Null).to_string().as_bytes()))
}

fn from_value(value: Option<&Value>) -> ClientIdentificationSnapshot {
    let settings = match value {
        None => Ok(ClientSettings::default()),
        Some(value) => serde_json::from_value::<ClientSettings>(value.clone())
            .map_err(|error| format!("客户端识别配置格式不正确：{error}"))
            .and_then(|settings| { catalog::validate(&settings)?; Ok(settings) }),
    };
    ClientIdentificationSnapshot { settings, revision: revision(value) }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<ClientIdentificationSnapshot> {
    Arc::new(from_value(raw.get(KEY_CLIENT_IDENTIFICATION)))
}

pub fn current() -> Arc<ClientIdentificationSnapshot> {
    super::CONFIG.read().unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref().map(|config| config.client_identification.clone()).unwrap_or_default()
}

pub fn save(settings: ClientSettings, expected: &str) -> Result<(), GatewayError> {
    catalog::validate(&settings).map_err(GatewayError::bad_request)?;
    let value = serde_json::to_value(settings).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_field(KEY_CLIENT_IDENTIFICATION, |raw| {
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "客户端识别设置已在其他位置修改，请重新读取后再保存；当前草稿未丢失"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("客户端识别设置保存失败，未应用本次修改"))
}
