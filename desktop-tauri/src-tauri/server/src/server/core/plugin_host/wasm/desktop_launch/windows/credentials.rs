use serde::{Deserialize, Serialize};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST, CRED_TYPE_GENERIC};

#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub username: String,
    pub bytes: Vec<u8>,
    pub persist: u32,
}

impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.bytes.zeroize();
    }
}

fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

struct Owned(*mut CREDENTIALW);
impl Drop for Owned {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        unsafe {
            if let Some(record) = self.0.as_mut() {
                if record.CredentialBlobSize <= 2560 && !record.CredentialBlob.is_null() {
                    std::slice::from_raw_parts_mut(record.CredentialBlob, record.CredentialBlobSize as usize).zeroize();
                }
            }
            CredFree(self.0.cast());
        }
    }
}

pub fn read_credential(target: &str) -> Result<Option<Credential>, String> {
    let name = wide(target);
    let mut value = std::ptr::null_mut();
    if let Err(error) = unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut value) } {
        if error.code() == ERROR_NOT_FOUND.to_hresult() { return Ok(None); }
        return Err("系统凭证不可读，未覆盖现有登录态".into());
    }
    let owned = Owned(value);
    let record = unsafe { owned.0.as_ref() }.ok_or("系统凭证格式无效")?;
    if record.CredentialBlobSize > 2560 || record.CredentialBlobSize > 0 && record.CredentialBlob.is_null() {
        return Err("系统凭证内容超出支持范围".into());
    }
    let username = if record.UserName.is_null() { String::new() } else {
        let mut units = Vec::new();
        for index in 0..=512 {
            let unit = unsafe { *record.UserName.0.add(index) };
            if unit == 0 { break; }
            if index == 512 { return Err("系统凭证用户名超出支持范围".into()); }
            units.push(unit);
        }
        String::from_utf16(&units).map_err(|_| "系统凭证用户名格式无效")?
    };
    let bytes = if record.CredentialBlobSize == 0 { Vec::new() } else {
        unsafe { std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize) }.to_vec()
    };
    Ok(Some(Credential { username, bytes, persist: record.Persist.0 }))
}

pub fn write_credential(target: &str, credential: Option<&Credential>) -> Result<(), String> {
    let target = wide(target);
    let Some(credential) = credential else {
        return match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()), Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(_) => Err("系统凭证恢复失败".into()),
        };
    };
    if credential.bytes.len() > 2560 || !(1..=3).contains(&credential.persist) {
        return Err("客户端凭证超出系统凭据库支持范围，未写入".into());
    }
    let username = wide(&credential.username);
    let record = CREDENTIALW {
        Flags: CRED_FLAGS(0), Type: CRED_TYPE_GENERIC, TargetName: PWSTR(target.as_ptr().cast_mut()),
        CredentialBlobSize: credential.bytes.len() as u32, CredentialBlob: credential.bytes.as_ptr().cast_mut(),
        Persist: CRED_PERSIST(credential.persist), UserName: PWSTR(username.as_ptr().cast_mut()), ..Default::default()
    };
    unsafe { CredWriteW(&record, 0) }.map_err(|_| "系统凭证写入失败，原登录态将保留或恢复".into())
}
