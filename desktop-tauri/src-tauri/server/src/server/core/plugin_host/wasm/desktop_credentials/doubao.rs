use super::{absolute_env, file, invalid, regular};
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

pub(super) fn read() -> Result<Value, proto::WireError> {
    #[cfg(windows)]
    {
        windows::read()
    }
    #[cfg(not(windows))]
    {
        Err(proto::WireError::with_code(
            "当前系统未接入豆包工作本机 Cookie 解密，请使用网页登录或粘贴凭证",
            "desktop_platform_unsupported",
        ))
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use ::windows::{
        core::Owned,
        Win32::{
            Foundation::HLOCAL,
            Security::Cryptography::{
                CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            },
        },
    };
    use aes_gcm::{aead::consts::U12, AeadInOut, Aes256Gcm, KeyInit, Nonce};
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use zeroize::{Zeroize, Zeroizing};

    pub(super) fn read() -> Result<Value, proto::WireError> {
        let root = absolute_env("LOCALAPPDATA")
            .ok_or_else(invalid)?
            .join("DoubaoWork")
            .join("User Data");
        let client_metadata = metadata(&root);
        let local = file(&root.join("Local State"), 8 * 1024 * 1024)?;
        let state: Value =
            super::super::super::managed_task::declaration::parse_object(&local, 8 * 1024 * 1024)
                .map_err(|_| invalid())?;
        let encoded = state["os_crypt"]["encrypted_key"]
            .as_str()
            .filter(|value| value.len() <= 16 * 1024)
            .ok_or_else(invalid)?;
        let blob = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| invalid())?,
        );
        let key = unprotect(blob.strip_prefix(b"DPAPI").ok_or_else(invalid)?)?;
        let mut profiles = Vec::new();
        let mut scanned = 0;
        for entry in std::fs::read_dir(&root).map_err(|_| invalid())? {
            scanned += 1;
            if scanned > 2048 {
                return Err(invalid());
            }
            let entry = entry.map_err(|_| invalid())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "Default" || name.starts_with("Profile ") {
                profiles.push(entry.path());
            }
            if profiles.len() > 32 {
                return Err(invalid());
            }
        }
        profiles.sort();
        let mut accounts = Vec::new();
        for profile in profiles {
            let source = profile.join("Network").join("Cookies");
            if !source.try_exists().map_err(|_| invalid())? {
                continue;
            }
            if regular(&source)?.len() > 32 * 1024 * 1024 {
                return Err(invalid());
            }
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar =
                    std::path::PathBuf::from(format!("{}{suffix}", source.to_string_lossy()));
                if sidecar.try_exists().map_err(|_| invalid())?
                    && regular(&sidecar)?.len() > 32 * 1024 * 1024
                {
                    return Err(invalid());
                }
            }
            let source = rusqlite::Connection::open_with_flags(
                &source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| invalid())?;
            source
                .busy_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| invalid())?;
            let temp = tempfile::tempdir().map_err(|_| invalid())?;
            let path = temp.path().join("Cookies");
            let mut copy = rusqlite::Connection::open(&path).map_err(|_| invalid())?;
            let backup =
                rusqlite::backup::Backup::new(&source, &mut copy).map_err(|_| invalid())?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if std::time::Instant::now() >= deadline {
                    return Err(invalid());
                }
                let step = backup.step(128).map_err(|_| invalid())?;
                if std::fs::metadata(&path).map_err(|_| invalid())?.len() > 32 * 1024 * 1024 {
                    return Err(invalid());
                }
                if matches!(step, rusqlite::backup::StepResult::Done) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            drop(backup);
            if std::fs::metadata(&path).map_err(|_| invalid())?.len() > 32 * 1024 * 1024 {
                return Err(invalid());
            }
            let version: i64 = copy
                .query_row(
                    "SELECT CAST(value AS INTEGER) FROM meta WHERE key='version'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| invalid())?;
            let mut stmt = copy.prepare("SELECT host_key,name,value,encrypted_value,path,expires_utc FROM cookies WHERE host_key IN ('.doubao.com','doubao.com','www.doubao.com','.www.doubao.com') AND path='/' LIMIT 257").map_err(|_| invalid())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|_| invalid())?;
            let mut cookies = Vec::new();
            let mut cookie_bytes = 0usize;
            let mut scanned_rows = 0usize;
            let now = crate::server::logging::now_ms() / 1000;
            for row in rows {
                let (host, name, plain, encrypted, path, expires) = row.map_err(|_| invalid())?;
                scanned_rows += 1;
                if scanned_rows > 256
                    || name.is_empty()
                    || name.len() > 256
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() && !b"=;,\"\\".contains(&byte))
                {
                    return Err(invalid());
                }
                let expires = if expires <= 0 {
                    None
                } else {
                    Some(expires / 1_000_000 - 11_644_473_600)
                };
                if expires.is_some_and(|expires| expires <= now) {
                    continue;
                }
                let value = if !plain.is_empty() {
                    plain
                } else {
                    decrypt(&encrypted, &key, &host, version)?
                };
                if value.is_empty() {
                    continue;
                }
                if value.len() > 16 * 1024 || value.chars().any(|ch| ch.is_control() || ch == ';') {
                    return Err(invalid());
                }
                cookie_bytes = cookie_bytes
                    .checked_add(name.len() + value.len() + 3)
                    .filter(|size| *size <= 64 * 1024)
                    .ok_or_else(invalid)?;
                cookies.push(
                    json!({"name":name,"value":value,"domain":host,"path":path,"expires":expires}),
                );
            }
            if cookies
                .iter()
                .any(|cookie| matches!(cookie["name"].as_str(), Some("sessionid" | "sessionid_ss")))
            {
                accounts.push(cookies);
            }
        }
        if accounts.len() != 1 {
            return Err(proto::WireError::with_code(
                "未找到唯一的豆包工作已登录账号；多个客户端账号请改用网页登录或手工凭证",
                "desktop_credentials_ambiguous",
            ));
        }
        let mut value = json!({"cookies":accounts.remove(0)});
        if let Some(fields) = client_metadata.as_object() {
            for (key, field) in fields {
                value[key] = field.clone();
            }
        }
        for (source_key, target_key) in [("device_id", "deviceId"), ("install_id", "iid")] {
            let entry = &state["aha"]["device"][source_key];
            if let Some(text) = entry.as_str().filter(|text| {
                !text.is_empty()
                    && text.len() <= 64
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
            }) {
                value[target_key] = json!(text);
            } else if let Some(number) = entry.as_u64() {
                value[target_key] = json!(number.to_string());
            }
        }
        if value.to_string().len() > 256 * 1024 {
            return Err(invalid());
        }
        Ok(value)
    }
    fn metadata(root: &std::path::Path) -> Value {
        let Some(application) = root.parent().map(|root| root.join("Application")) else {
            return Value::Null;
        };
        let mut paths = Vec::new();
        for installed in installed_roots() {
            paths.push(installed.join("manifest.json"));
            paths.push(installed.join("app").join("manifest.json"));
        }
        let plain = application.join("manifest.json");
        if plain.is_file() {
            paths.push(plain);
        }
        if let Ok(entries) = std::fs::read_dir(&application) {
            for entry in entries.take(32).flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(|ch: char| ch.is_ascii_digit())
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
                {
                    let path = entry.path().join("manifest.json");
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
        }
        let mut found = Vec::new();
        for path in paths {
            let Ok(bytes) = file(&path, 64 * 1024) else {
                continue;
            };
            let Ok(value) = super::super::super::managed_task::declaration::parse_object::<Value>(
                &bytes,
                64 * 1024,
            ) else {
                continue;
            };
            let Some(version) = value["version"].as_str().filter(|version| {
                !version.is_empty()
                    && version.len() <= 64
                    && version
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
            }) else {
                continue;
            };
            let mut metadata = json!({"clientVersion":version});
            if let Some(channel) = value["channel"].as_str().filter(|channel| {
                !channel.is_empty()
                    && channel.len() <= 64
                    && channel
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
            }) {
                metadata["channel"] = json!(channel);
            } else if let Some(channel) = value["channel"].as_u64() {
                metadata["channel"] = json!(channel.to_string());
            }
            if !found.contains(&metadata) {
                found.push(metadata);
            }
        }
        if found.len() == 1 {
            found.remove(0)
        } else {
            Value::Null
        }
    }
    fn installed_roots() -> Vec<std::path::PathBuf> {
        use ::windows::core::PCWSTR;
        use ::windows::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
        };
        let key = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\DoubaoWork"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut roots = Vec::new();
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for field in ["InstallLocation", "UninstallString"] {
                let name = field.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
                let mut data = [0u16; 2048];
                let mut size = (data.len() * 2) as u32;
                let result = unsafe {
                    RegGetValueW(
                        hive,
                        PCWSTR(key.as_ptr()),
                        PCWSTR(name.as_ptr()),
                        RRF_RT_REG_SZ,
                        None,
                        Some(data.as_mut_ptr().cast()),
                        Some(&mut size),
                    )
                };
                if result.0 != 0 || size < 2 || size as usize > data.len() * 2 {
                    continue;
                }
                let end = data
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(data.len());
                let Ok(raw) = String::from_utf16(&data[..end]) else {
                    continue;
                };
                let raw = raw.trim();
                let path = if field == "UninstallString" {
                    if let Some(quoted) = raw.strip_prefix('"') {
                        quoted.split_once('"').map(|(path, _)| path)
                    } else {
                        raw.strip_suffix("uninstall.exe").map(|_| raw)
                    }
                } else {
                    Some(raw.trim_matches('"'))
                };
                let Some(path) = path else {
                    continue;
                };
                let path = std::path::PathBuf::from(path);
                if !path.is_absolute() {
                    continue;
                }
                let root = if field == "UninstallString" {
                    path.parent().map(std::path::Path::to_path_buf)
                } else {
                    Some(path)
                };
                if let Some(root) = root.filter(|root| root.join("DoubaoWork.exe").is_file()) {
                    roots.push(root);
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }
    fn decrypt(
        blob: &[u8],
        key: &[u8],
        host: &str,
        version: i64,
    ) -> Result<String, proto::WireError> {
        if blob.len() > 20 * 1024 || blob.len() < 31 || !blob.starts_with(b"v10") {
            return Err(invalid());
        }
        let nonce = Nonce::<U12>::try_from(&blob[3..15]).map_err(|_| invalid())?;
        let mut plain = Zeroizing::new(blob[15..].to_vec());
        Aes256Gcm::new_from_slice(key)
            .map_err(|_| invalid())?
            .decrypt_in_place(&nonce, &[], &mut *plain)
            .map_err(|_| invalid())?;
        let data = if version >= 24 {
            let hash = Sha256::digest(host);
            if plain.len() <= 32 || plain[..32] != hash[..] {
                return Err(invalid());
            }
            &plain[32..]
        } else {
            plain.as_slice()
        };
        String::from_utf8(data.to_vec()).map_err(|_| invalid())
    }
    struct Output(CRYPT_INTEGER_BLOB);
    impl Drop for Output {
        fn drop(&mut self) {
            if !self.0.pbData.is_null() {
                unsafe {
                    std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                    drop(Owned::new(HLOCAL(self.0.pbData.cast())));
                }
            }
        }
    }
    fn unprotect(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, proto::WireError> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut output = Output(CRYPT_INTEGER_BLOB::default());
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
        .map_err(|_| invalid())?;
        if output.0.pbData.is_null() || output.0.cbData != 32 {
            return Err(invalid());
        }
        Ok(Zeroizing::new(
            unsafe { std::slice::from_raw_parts(output.0.pbData, 32) }.to_vec(),
        ))
    }
}
