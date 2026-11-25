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

    struct FoundAccount {
        session_key: (String, String),
        cookies: Vec<Value>,
        extra: Value,
    }

    pub(super) fn read() -> Result<Value, proto::WireError> {
        let roots = candidate_roots();
        if roots.is_empty() {
            return Err(invalid());
        }
        let mut accounts: Vec<FoundAccount> = Vec::new();
        let mut saw_v20 = false;
        for root in roots {
            let local_state_path = root.join("Local State");
            if !local_state_path.try_exists().unwrap_or(false) {
                continue;
            }
            let Ok(local) = file(&local_state_path, 8 * 1024 * 1024) else {
                continue;
            };
            let Ok(state) = super::super::super::managed_task::declaration::parse_object::<Value>(
                &local,
                8 * 1024 * 1024,
            ) else {
                continue;
            };
            let Some(encoded) = state["os_crypt"]["encrypted_key"]
                .as_str()
                .filter(|value| value.len() <= 16 * 1024)
            else {
                continue;
            };
            let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                continue;
            };
            let blob = Zeroizing::new(decoded);
            let Some(dpapi) = blob.strip_prefix(b"DPAPI") else {
                continue;
            };
            let Ok(key) = unprotect(dpapi) else {
                continue;
            };
            let client_metadata = metadata(&root);
            let chromium_version = read_chromium_version(&root);
            let profiles = collect_profiles(&root)?;
            for profile in profiles {
                let source = {
                    let net = profile.join("Network").join("Cookies");
                    if net.try_exists().unwrap_or(false) {
                        net
                    } else {
                        let legacy = profile.join("Cookies");
                        if legacy.try_exists().unwrap_or(false) {
                            legacy
                        } else {
                            continue;
                        }
                    }
                };
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
                let Ok(source) = rusqlite::Connection::open_with_flags(
                    &source,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                ) else {
                    continue;
                };
                let _ = source.busy_timeout(std::time::Duration::from_secs(2));
                let temp = tempfile::tempdir().map_err(|_| invalid())?;
                let path = temp.path().join("Cookies");
                let mut copy = rusqlite::Connection::open(&path).map_err(|_| invalid())?;
                let Ok(backup) = rusqlite::backup::Backup::new(&source, &mut copy) else {
                    continue;
                };
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut backed_up = true;
                loop {
                    if std::time::Instant::now() >= deadline {
                        backed_up = false;
                        break;
                    }
                    match backup.step(128) {
                        Ok(rusqlite::backup::StepResult::Done) => break,
                        Ok(_) => {
                            if std::fs::metadata(&path)
                                .map(|meta| meta.len() > 32 * 1024 * 1024)
                                .unwrap_or(true)
                            {
                                return Err(invalid());
                            }
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => {
                            backed_up = false;
                            break;
                        }
                    }
                }
                drop(backup);
                if !backed_up {
                    continue;
                }
                if std::fs::metadata(&path).map_err(|_| invalid())?.len() > 32 * 1024 * 1024 {
                    return Err(invalid());
                }
                let version: i64 = copy
                    .query_row(
                        "SELECT CAST(value AS INTEGER) FROM meta WHERE key='version'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(20);
                let Ok(mut stmt) = copy.prepare("SELECT host_key,name,value,encrypted_value,path,expires_utc FROM cookies WHERE host_key IN ('.doubao.com','doubao.com','www.doubao.com','.www.doubao.com') AND path='/' LIMIT 257") else {
                    continue;
                };
                let Ok(rows) = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                }) else {
                    continue;
                };
                let mut cookies = Vec::new();
                let mut cookie_bytes = 0usize;
                let mut scanned_rows = 0usize;
                let now = crate::server::logging::now_ms() / 1000;
                for row in rows {
                    let (host, name, plain, encrypted, path, expires) =
                        row.map_err(|_| invalid())?;
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
                    } else if encrypted.starts_with(b"v20") {
                        saw_v20 = true;
                        continue;
                    } else {
                        match decrypt(&encrypted, &key, &host, version) {
                            Ok(value) => value,
                            Err(_) => continue,
                        }
                    };
                    if value.is_empty() {
                        continue;
                    }
                    if value.len() > 16 * 1024
                        || value.chars().any(|ch| ch.is_control() || ch == ';')
                    {
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
                let sid = cookie_value(&cookies, "sessionid");
                let sid_ss = cookie_value(&cookies, "sessionid_ss");
                if sid.is_empty() && sid_ss.is_empty() {
                    continue;
                }
                let mut extra = json!({});
                if let Some(fields) = client_metadata.as_object() {
                    for (key, field) in fields {
                        extra[key] = field.clone();
                    }
                }
                if let Some(chromium) = &chromium_version {
                    extra["chromiumVersion"] = json!(chromium);
                }
                for (source_key, target_key) in [("device_id", "deviceId"), ("install_id", "iid")] {
                    let entry = &state["aha"]["device"][source_key];
                    let text = entry
                        .as_str()
                        .map(str::trim)
                        .map(str::to_string)
                        .or_else(|| entry.as_u64().map(|number| number.to_string()));
                    if let Some(text) = text.filter(|text| {
                        !text.is_empty()
                            && text.len() <= 64
                            && (target_key != "deviceId"
                                || crate::server::core::plugin_host::wasm::checkin_call::doubao_daily::credentials::is_numeric_id(text))
                            && text
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
                    }) {
                        extra[target_key] = json!(text);
                    }
                }
                if let Some((web_id, tea_uuid)) = read_leveldb_ids(&profile) {
                    if !web_id.is_empty() {
                        extra["webId"] = json!(web_id);
                    }
                    if !tea_uuid.is_empty() {
                        extra["teaUuid"] = json!(tea_uuid);
                    }
                }
                let session_key = (sid, sid_ss);
                if let Some(existing) = accounts
                    .iter_mut()
                    .find(|item| item.session_key == session_key)
                {
                    if cookies.len() > existing.cookies.len() {
                        existing.cookies = cookies;
                        existing.extra = extra;
                    }
                } else {
                    accounts.push(FoundAccount {
                        session_key,
                        cookies,
                        extra,
                    });
                }
            }
        }
        if accounts.is_empty() && saw_v20 {
            return Err(proto::WireError::with_code(
                "检测到本机豆包客户端启用了新版系统级 Cookie 加密（v20），无法直接解密，请改用「豆包工作网页登录」或粘贴 Cookie",
                "desktop_credentials_v20_unsupported",
            ));
        }
        if accounts.len() != 1 {
            return Err(proto::WireError::with_code(
                "未找到唯一的豆包工作已登录账号；多个客户端账号请改用网页登录或手工凭证",
                "desktop_credentials_ambiguous",
            ));
        }
        let found = accounts.remove(0);
        let mut value = json!({"cookies": found.cookies});
        if let Some(fields) = found.extra.as_object() {
            for (key, field) in fields {
                value[key] = field.clone();
            }
        }
        if value.to_string().len() > 256 * 1024 {
            return Err(invalid());
        }
        Ok(value)
    }

    fn cookie_value(cookies: &[Value], name: &str) -> String {
        cookies
            .iter()
            .find(|cookie| cookie["name"].as_str() == Some(name))
            .and_then(|cookie| cookie["value"].as_str())
            .unwrap_or("")
            .to_string()
    }

    fn candidate_roots() -> Vec<std::path::PathBuf> {
        let mut roots = Vec::new();
        for env_key in ["LOCALAPPDATA", "APPDATA"] {
            let Some(base) = absolute_env(env_key) else {
                continue;
            };
            for folder in ["DoubaoWork", "Doubao"] {
                let candidate = base.join(folder).join("User Data");
                if candidate.is_dir() && !roots.contains(&candidate) {
                    roots.push(candidate);
                }
            }
        }
        roots
    }

    fn collect_profiles(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, proto::WireError> {
        let mut profiles = Vec::new();
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(root).map_err(|_| invalid())? {
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
        let aha_users = root.join("aha").join("users");
        if aha_users.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&aha_users) {
                for entry in entries.take(32).flatten() {
                    let explorer = entry.path().join("profile_explorer");
                    if explorer.is_dir() {
                        profiles.push(explorer);
                    }
                }
            }
        }
        profiles.sort();
        profiles.dedup();
        Ok(profiles)
    }

    fn read_chromium_version(root: &std::path::Path) -> Option<String> {
        let bytes = file(&root.join("Last Version"), 128).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?.trim();
        if text.is_empty()
            || text.len() > 64
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            return None;
        }
        Some(text.to_string())
    }

    fn read_leveldb_ids(profile: &std::path::Path) -> Option<(String, String)> {
        let dir = profile.join("Local Storage").join("leveldb");
        if !dir.is_dir() {
            return None;
        }
        let mut entries = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "ldb" | "log"))
            })
            .take(32)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        let mut web_id = String::new();
        let mut tea_uuid = String::new();
        let needle = b"__tea_cache_tokens_1044603";
        for entry in entries {
            let Ok(bytes) = file(&entry.path(), 4 * 1024 * 1024) else {
                continue;
            };
            let mut offset = 0usize;
            while let Some(pos) = bytes[offset..]
                .windows(needle.len())
                .position(|window| window == needle)
            {
                let start = offset + pos + needle.len();
                let window = &bytes[start..bytes.len().min(start + 2048)];
                if let Some(json_start) = window.iter().position(|byte| *byte == b'{') {
                    if let Some(json_end) = window[json_start..]
                        .iter()
                        .position(|byte| *byte == b'}')
                    {
                        if let Ok(parsed) = serde_json::from_slice::<Value>(
                            &window[json_start..=json_start + json_end],
                        ) {
                            if let Some(id) = parsed["web_id"].as_str().filter(|id| {
                                crate::server::core::plugin_host::wasm::checkin_call::doubao_daily::credentials::is_numeric_id(id)
                            }) {
                                web_id = id.to_string();
                            }
                            if let Some(uuid) = parsed["user_unique_id"].as_str().filter(|id| {
                                !id.is_empty()
                                    && id.len() <= 128
                                    && id
                                        .bytes()
                                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
                            }) {
                                tea_uuid = uuid.to_string();
                            }
                        }
                    }
                }
                offset = start;
            }
        }
        if web_id.is_empty() && tea_uuid.is_empty() {
            None
        } else {
            Some((web_id, tea_uuid))
        }
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
        let mut roots = Vec::new();
        for reg_key in [
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\DoubaoWork",
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Doubao",
        ] {
            let key = reg_key.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
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
                    if let Some(root) = root.filter(|root| {
                        root.join("DoubaoWork.exe").is_file() || root.join("Doubao.exe").is_file()
                    }) {
                        roots.push(root);
                    }
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
            let alt = Sha256::digest(host.trim_start_matches('.'));
            if plain.len() <= 32 || (plain[..32] != hash[..] && plain[..32] != alt[..]) {
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
