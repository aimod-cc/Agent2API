use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Security::Cryptography::{
    CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW,
    CryptMsgClose, CryptMsgGetParam, CryptQueryObject, CERT_CONTEXT, CERT_FIND_SUBJECT_CERT,
    CERT_INFO, CERT_NAME_ATTR_TYPE, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_ENCODING_TYPE,
    CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE, CMSG_SIGNER_INFO,
    CMSG_SIGNER_INFO_PARAM, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct WintrustFileInfo {
    cb_struct: u32,
    file_path: *const u16,
    file: *mut c_void,
    known_subject: *mut Guid,
}

#[repr(C)]
struct WintrustData {
    cb_struct: u32,
    policy_callback_data: *mut c_void,
    sip_client_data: *mut c_void,
    ui_choice: u32,
    revocation_checks: u32,
    union_choice: u32,
    file_info: *mut WintrustFileInfo,
    state_action: u32,
    state_data: *mut c_void,
    url_reference: *mut u16,
    prov_flags: u32,
    ui_context: u32,
    signature_settings: *mut c_void,
}

#[link(name = "wintrust")]
unsafe extern "system" {
    fn WinVerifyTrust(hwnd: *mut c_void, action_id: *mut Guid, data: *mut c_void) -> i32;
}

const WTD_UI_NONE: u32 = 2;
const WTD_REVOKE_NONE: u32 = 0;
const WTD_CHOICE_FILE: u32 = 1;
const WTD_STATEACTION_VERIFY: u32 = 1;
const WTD_STATEACTION_CLOSE: u32 = 2;
const WTD_REVOCATION_CHECK_NONE: u32 = 0x0000_0010;
const WTD_CACHE_ONLY_URL_RETRIEVAL: u32 = 0x0000_1000;

struct StoreGuard(HCERTSTORE);
impl Drop for StoreGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CertCloseStore(Some(self.0), 0);
            }
        }
    }
}

struct MsgGuard(*mut c_void);
impl Drop for MsgGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CryptMsgClose(Some(self.0));
            }
        }
    }
}

struct CertGuard(*mut CERT_CONTEXT);
impl Drop for CertGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CertFreeCertificateContext(Some(self.0));
            }
        }
    }
}

fn verify_trust_offline(wide_path: &[u16]) -> Result<(), String> {
    let mut action = Guid {
        data1: 0x00aa_c56b,
        data2: 0xcd44,
        data3: 0x11d0,
        data4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee],
    };
    let mut file_info = WintrustFileInfo {
        cb_struct: std::mem::size_of::<WintrustFileInfo>() as u32,
        file_path: wide_path.as_ptr(),
        file: std::ptr::null_mut(),
        known_subject: std::ptr::null_mut(),
    };
    let mut data = WintrustData {
        cb_struct: std::mem::size_of::<WintrustData>() as u32,
        policy_callback_data: std::ptr::null_mut(),
        sip_client_data: std::ptr::null_mut(),
        ui_choice: WTD_UI_NONE,
        revocation_checks: WTD_REVOKE_NONE,
        union_choice: WTD_CHOICE_FILE,
        file_info: &mut file_info,
        state_action: WTD_STATEACTION_VERIFY,
        state_data: std::ptr::null_mut(),
        url_reference: std::ptr::null_mut(),
        prov_flags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ui_context: 0,
        signature_settings: std::ptr::null_mut(),
    };
    let invalid_hwnd = (-1isize) as *mut c_void;
    let status = unsafe { WinVerifyTrust(invalid_hwnd, &mut action, (&mut data as *mut WintrustData).cast()) };
    data.state_action = WTD_STATEACTION_CLOSE;
    unsafe {
        let _ = WinVerifyTrust(invalid_hwnd, &mut action, (&mut data as *mut WintrustData).cast());
    }
    if status != 0 {
        return Err("客户端程序数字签名无效或不受信任".into());
    }
    Ok(())
}

fn cert_name(cert: *const CERT_CONTEXT, name_type: u32, param: Option<*const c_void>) -> Option<String> {
    let len = unsafe { CertGetNameStringW(cert, name_type, 0, param, None) };
    if len <= 1 || len > 512 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let written = unsafe { CertGetNameStringW(cert, name_type, 0, param, Some(&mut buf)) };
    if written <= 1 {
        return None;
    }
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    let text = String::from_utf16(&buf[..end]).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn signer_names(wide_path: &[u16]) -> Result<Vec<String>, String> {
    let mut encoding = CERT_QUERY_ENCODING_TYPE::default();
    let mut store = HCERTSTORE::default();
    let mut msg: *mut c_void = std::ptr::null_mut();
    unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            wide_path.as_ptr().cast(),
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            Some(&mut encoding),
            None,
            None,
            Some(&mut store),
            Some(&mut msg),
            None,
        )
    }
    .map_err(|_| "客户端程序数字签名信息不可读")?;
    let _store = StoreGuard(store);
    let _msg = MsgGuard(msg);

    let mut signer_size = 0u32;
    unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, None, &mut signer_size) }
        .map_err(|_| "客户端程序数字签名信息不可读")?;
    if signer_size < std::mem::size_of::<CMSG_SIGNER_INFO>() as u32 || signer_size > 256 * 1024 {
        return Err("客户端程序数字签名格式无效".into());
    }
    let mut words = vec![0u64; (signer_size as usize).div_ceil(8)];
    unsafe {
        CryptMsgGetParam(
            msg,
            CMSG_SIGNER_INFO_PARAM,
            0,
            Some(words.as_mut_ptr().cast()),
            &mut signer_size,
        )
    }
    .map_err(|_| "客户端程序数字签名信息不可读")?;

    let signer_info = unsafe { &*(words.as_ptr().cast::<CMSG_SIGNER_INFO>()) };
    let mut cert_info = CERT_INFO::default();
    cert_info.Issuer = signer_info.Issuer;
    cert_info.SerialNumber = signer_info.SerialNumber;

    let cert = unsafe {
        CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_CERT,
            Some((&cert_info as *const CERT_INFO).cast()),
            None,
        )
    };
    if cert.is_null() {
        return Err("客户端程序签名证书缺失".into());
    }
    let _cert = CertGuard(cert);

    let mut names = Vec::new();
    const OID_CN: &[u8] = b"2.5.4.3\0";
    const OID_O: &[u8] = b"2.5.4.10\0";
    if let Some(cn) = cert_name(cert, CERT_NAME_ATTR_TYPE, Some(OID_CN.as_ptr().cast())) {
        names.push(cn);
    }
    if let Some(org) = cert_name(cert, CERT_NAME_ATTR_TYPE, Some(OID_O.as_ptr().cast())) {
        names.push(org);
    }
    if let Some(simple) = cert_name(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, None) {
        names.push(simple);
    }
    if names.is_empty() {
        return Err("客户端程序签名身份缺失".into());
    }
    Ok(names)
}

pub fn verify_authenticode(path: &Path, expected_signers: &[String]) -> Result<(), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    verify_trust_offline(&wide)?;
    let actual = signer_names(&wide)?;
    if !actual
        .iter()
        .any(|name| expected_signers.iter().any(|allowed| name.eq_ignore_ascii_case(allowed)))
    {
        return Err("客户端程序签名身份不符，未切换凭证".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsigned_file() {
        let dir = std::env::temp_dir().join(format!("a2a-sig-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("unsigned.exe");
        std::fs::write(&path, b"not a signed pe").expect("write temp file");
        let err = verify_authenticode(&path, &["Google LLC".into()]).unwrap_err();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        assert!(err.contains("数字签名"));
    }

    #[test]
    fn verifies_local_agy_signature_when_installed() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else { return; };
        let agy = std::path::PathBuf::from(local).join("agy/bin/agy.exe");
        if !agy.is_file() {
            return;
        }
        verify_authenticode(&agy, &["Google LLC".into()]).expect("official agy.exe should verify as Google LLC");
        let err = verify_authenticode(&agy, &["Wrong Signer Inc".into()]).unwrap_err();
        assert!(err.contains("签名身份不符"));
    }
}
