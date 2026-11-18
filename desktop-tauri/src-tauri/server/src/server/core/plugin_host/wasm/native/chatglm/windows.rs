//! 只有父私有模块可调用；当前没有上层调用者，也不持有或伪造任何授权票据。

use super::{crypto, database, files, CredentialContent, ReadError, COOKIE_ACCESS, COOKIE_REFRESH};

pub(super) fn read() -> Result<CredentialContent, ReadError> {
    let mut input = files::read()?;
    // 先验证数据库状态/结构并完整检查候选行，坏行和重复项必须在任何 DPAPI 解密前拒绝。
    let cookies = database::read(&mut input.cookies)?;
    input.check()?;
    let key = crypto::key(&input.local_state)?;
    let (mut access, mut refresh) = (None, None);
    for cookie in cookies.rows {
        let token = crypto::token(&cookie.value, &cookie.encrypted, &key, cookies.version)?;
        match cookie.name {
            COOKIE_ACCESS => access = Some(token),
            COOKIE_REFRESH => refresh = Some(token),
            _ => return Err(ReadError::Malformed),
        }
    }
    let content = CredentialContent::new(access, refresh)?;
    input.check()?;
    Ok(content)
}
