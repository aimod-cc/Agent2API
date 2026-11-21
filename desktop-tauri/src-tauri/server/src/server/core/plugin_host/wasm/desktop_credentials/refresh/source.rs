use std::io::Write;
use std::path::PathBuf;

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{changed, super::{home, app_data, regular, file, invalid, object}};

pub(super) struct Source {
    path: PathBuf,
    value: Value,
    bytes: Zeroizing<Vec<u8>>,
    database_key: Option<String>,
    group: &'static str,
    revision: Value,
    pub(super) credentials: Value,
    provider: String,
}

fn text(value: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|key| match &value[*key] {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().into()),
        Value::Number(value) => Some(value.to_string()), _ => None,
    }).unwrap_or_default()
}
fn claims(token: &str) -> Value {
    token.trim_start_matches("workos:").split('.').nth(1)
        .and_then(|value| URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null)
}
fn same_user(reference: &Value, current: &Value, keys: &[&str]) -> Result<(), proto::WireError> {
    let expected = text(&reference["secret"],keys);
    let actual = text(current,keys);
    if expected.is_empty() || actual != expected { return Err(changed()); }
    Ok(())
}
fn cli_path() -> Result<PathBuf, proto::WireError> {
    #[cfg(any(windows,target_os="macos"))]
    return Ok(app_data()?.join("kiro-cli/data.sqlite3"));
    #[cfg(not(any(windows,target_os="macos")))]
    Ok(super::super::absolute_env("XDG_DATA_HOME").unwrap_or(home()?.join(".local/share")).join("kiro-cli/data.sqlite3"))
}

impl Source {
    pub(super) fn read(provider: &str, reference: &Value) -> Result<Self, proto::WireError> {
        if reference["provider"] != provider || reference["desktop"] != true { return Err(changed()); }
        let (path, database_key, group, credentials) = match provider {
            "kiro" => {
                let source = text(&reference["secret"], &["source"]);
                let credentials = super::super::kiro::read_source(if source.is_empty(){"kiro"}else{&source})?;
                let cli = credentials["source"] == "kiro-cli";
                let path = if cli { cli_path()? } else { home()?.join(".aws/sso/cache/kiro-auth-token.json") };
                let method = text(&credentials,&["method"]);
                let expected_method = text(&reference["secret"], &["method", "authMethod"]);
                if !expected_method.is_empty() && method != expected_method { return Err(changed()); }
                let expected_fingerprint = text(&reference["secret"], &["desktopFingerprint"]);
                let fingerprint = format!("{:x}", Sha256::digest(text(&credentials, &["access", "accessToken"])));
                if !expected_fingerprint.is_empty() && expected_fingerprint != fingerprint {
                    let expected = text(&reference["secret"], &["accountId", "email"]);
                    let actual = text(&credentials, &["accountId", "email"]);
                    if expected.is_empty() || actual.is_empty() || !actual.eq_ignore_ascii_case(&expected) {
                        return Err(changed());
                    }
                }
                let key = cli.then(||format!("kirocli:{}:token",if method=="idc"{"odic"}else{&method}));
                (path,key,"",credentials)
            }
            "autoclaw" | "autoclaw-intl" => {
                let root = super::super::absolute_env("AUTOCLAW_USER_DATA_DIR").unwrap_or(app_data()?.join("AutoClaw"));
                let response = super::super::autoclaw::read(provider)?;
                if response["source"] != "auth-file" { return Err(invalid()); }
                let credentials: Value = serde_json::from_str(response["content"].as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
                same_user(reference,&credentials,&["userId"])?;
                (root.join("auth.json"),None,"",credentials)
            }
            "cline-free" => {
                let path=home()?.join(".cline/data/settings/providers.json");
                let value=object(&path,256*1024)?;
                let auth=value.pointer("/providers/cline/settings/auth").filter(|value|value.is_object()).ok_or_else(invalid)?;
                let access=text(auth,&["accessToken"]);
                let uid=text(&claims(&access),&["external_id"]);
                let expected=text(&reference["secret"],&["account","userId","accountId"]);
                let email=text(&claims(&access),&["email"]);
                let email=if email.is_empty(){text(&auth["metadata"]["userInfo"],&["email"])}else{email};
                let legacy=reference["secret"]["desktopReadOnly"]!=true && expected.contains('@') && expected==email;
                if uid.is_empty() || (!expected.is_empty() && expected!=uid && !legacy) { return Err(changed()); }
                let mut credentials=auth.clone();credentials["account"]=json!(uid);
                (path,None,"cline",credentials)
            }
            _=>return Err(invalid()),
        };
        let (value,bytes): (Value, Zeroizing<Vec<u8>>) = if let Some(key)=&database_key {
            let db=super::super::database(&path)?;
            let row:Option<String>=db.query_row("SELECT value FROM auth_kv WHERE key=?1 AND length(CAST(value AS BLOB))<=131072",[key],|row|row.get(0)).optional().map_err(|_|invalid())?;
            let row=row.ok_or_else(invalid)?;
            let value=serde_json::from_str(&row).map_err(|_|invalid())?;
            (value,Zeroizing::new(row.into_bytes()))
        }else{
            let bytes=Zeroizing::new(file(&path,256*1024)?);
            let value=serde_json::from_slice(&bytes).map_err(|_|invalid())?;
            (value,bytes)
        };
        let confirmed = match provider {
            "kiro" => super::super::kiro::read_source(credentials["source"].as_str().ok_or_else(invalid)?)?,
            "autoclaw" | "autoclaw-intl" => {
                let response = super::super::autoclaw::read(provider)?;
                serde_json::from_str(response["content"].as_str().ok_or_else(invalid)?).map_err(|_|invalid())?
            }
            _ => {
                let auth = value.pointer("/providers/cline/settings/auth").ok_or_else(invalid)?;
                let mut auth = auth.clone();
                auth["account"] = credentials["account"].clone();
                auth
            }
        };
        if confirmed != credentials { return Err(changed()); }
        if database_key.is_none() && file(&path,256*1024)?.as_slice() != bytes.as_slice() { return Err(changed()); }
        let revision=json!(format!("{:x}",Sha256::digest(serde_json::to_vec(&credentials).map_err(|_|invalid())?)));
        Ok(Self{path,value,bytes,database_key,group,revision,credentials,provider:provider.into()})
    }
    pub(super) fn revision(&self)->Value{self.revision.clone()}
    pub(super) fn lock_key(&self)->String{format!("{}:{}",self.path.to_string_lossy(),self.group)}
    pub(super) fn refreshable(&self)->bool{!text(&self.credentials,&["refresh","refreshToken","refresh_token"]).is_empty()}
    pub(super) fn refresh_stamp(&self)->String{format!("{:x}",Sha256::digest(format!("{}\0{}",self.path.to_string_lossy(),text(&self.credentials,&["refresh","refreshToken","refresh_token"]))))}

    pub(super) fn persist(self,next:&Value,reference:&Value)->Result<Value,proto::WireError>{
        let access=text(next,&["access","accessToken","token"]);
        let refresh=text(next,&["refresh","refreshToken"]);
        if access.is_empty()||refresh.is_empty(){return Err(invalid());}
        let expires=next["expiresAt"].as_i64().or_else(||next["expires"].as_i64());
        if let Some(key)=&self.database_key{
            regular(&self.path)?;
            let mut db=rusqlite::Connection::open_with_flags(&self.path,rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|_|invalid())?;
            db.busy_timeout(std::time::Duration::from_secs(2)).map_err(|_|invalid())?;
            let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_|invalid())?;
            let mut value=self.value;
            replace(&mut value, &["access_token", "accessToken"], "access_token", json!(access));
            replace(&mut value, &["refresh_token", "refreshToken"], "refresh_token", json!(refresh));
            replace(&mut value, &["expires_at", "expiresAt"], "expires_at",
                expires.map(iso).transpose()?.map(Value::String).unwrap_or(Value::Null));
            let encoded=serde_json::to_string(&value).map_err(|_|invalid())?;
            if tx.execute("UPDATE auth_kv SET value=?1 WHERE key=?2 AND value=?3",rusqlite::params![encoded,key,std::str::from_utf8(&self.bytes).map_err(|_|invalid())?]).map_err(|_|invalid())?!=1{return Err(changed());}
            tx.commit().map_err(|_|invalid())?;
        }else{
            if file(&self.path,256*1024)?.as_slice()!=self.bytes.as_slice(){return Err(changed());}
            let mut value=self.value;
            if self.group=="cline"{
                let auth=value.pointer_mut("/providers/cline/settings/auth").ok_or_else(invalid)?;
                auth["accessToken"]=json!(access);auth["refreshToken"]=json!(refresh);auth["expiresAt"]=json!(expires);
            }else if self.provider.starts_with("autoclaw"){
                let state=self.path.with_file_name("Local State");
                value["token"]=json!(super::super::autoclaw::seal(&access,&text(&value,&["token"]),&state)?);
                value["refreshToken"]=json!(super::super::autoclaw::seal(&refresh,&text(&value,&["refreshToken"]),&state)?);
                value["updatedAt"]=json!(crate::server::logging::now_ms());
            }else{
                replace(&mut value, &["accessToken", "access_token"], "accessToken", json!(access));
                replace(&mut value, &["refreshToken", "refresh_token"], "refreshToken", json!(refresh));
                replace(&mut value, &["expiresAt", "expires_at"], "expiresAt",
                    expires.map(iso).transpose()?.map(Value::String).unwrap_or(Value::Null));
            }
            let bytes=serde_json::to_vec_pretty(&value).map_err(|_|invalid())?;
            let mut tmp=tempfile::NamedTempFile::new_in(self.path.parent().ok_or_else(invalid)?).map_err(|_|invalid())?;
            tmp.as_file().set_permissions(regular(&self.path)?.permissions()).map_err(|_|invalid())?;
            tmp.write_all(&bytes).and_then(|_|tmp.as_file().sync_all()).map_err(|_|invalid())?;
            if file(&self.path,256*1024)?.as_slice()!=self.bytes.as_slice(){return Err(changed());}
            tmp.persist(&self.path).map_err(|_|invalid())?;
        }
        let mut reference = reference.clone();
        if self.provider == "kiro" {
            reference["secret"]["desktopFingerprint"] = json!(format!("{:x}", Sha256::digest(access.as_bytes())));
        }
        let check = Source::read(&self.provider, &reference)?;
        if text(&check.credentials, &["access", "accessToken", "token"]) != access
            || text(&check.credentials, &["refresh", "refreshToken", "refresh_token"]) != refresh {
            return Err(changed());
        }
        Ok(json!({"updated":true,"expiresAt":expires}))
    }
}
fn replace(value: &mut Value, aliases: &[&str], fallback: &str, next: Value) {
    if let Some(fields) = value.as_object_mut() {
        for alias in aliases {
            if fields.contains_key(*alias) { fields.insert((*alias).into(), next.clone()); }
        }
        fields.insert(fallback.into(), next);
    }
}
fn iso(at:i64)->Result<String,proto::WireError>{chrono::DateTime::from_timestamp_millis(at).map(|time|time.to_rfc3339_opts(chrono::SecondsFormat::Secs,true)).ok_or_else(invalid)}
