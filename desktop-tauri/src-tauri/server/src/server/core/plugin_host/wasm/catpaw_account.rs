//! CatPaw固定账号的真实RowSource读取与最小借用投影；不授予发送或轮次所有权。
//! 凭据依据本插件credentials.rs与原生catpaw/credentials.rs：Passport token + uid/loginName。
//! 只读原调用选定账号，不回落账号ID、桌面文件、环境变量或另一份来源。

use std::sync::{Arc, OnceLock};
use std::ops::Deref;
use sha2::{Digest, Sha256};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{Map, Value};
use tokio::{sync::Semaphore, time::Instant};

use super::conversation::SourceRef;
use crate::server::core::{
    account_store::{
        credential_version::{Error, RowProvider, RowSource, StorageFailure},
        plugin_accounts,
    },
    blocking,
};

const MAX_READS: usize = 4;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_TOKEN_CHARS: usize = 8192;
const MAX_USER_CHARS: usize = 256;
const MAX_UA_BYTES: usize = 1024;
const DESKTOP_ID: &str = "desktop-auth";

/// 字段只借用原来源，不实现Clone/Debug/serde；私有字段禁止调用方自由构造。
pub(in crate::server::core::plugin_host::wasm) struct Credentials<'a> {
    pub(in crate::server::core::plugin_host::wasm) token: &'a str,
    pub(in crate::server::core::plugin_host::wasm) user_uid: &'a str,
    pub(in crate::server::core::plugin_host::wasm) user_agent: &'a str,
    _sealed: (),
}

/// 本机引用只在当前调用读取，临时材料不写回账号库，后续提交仍核对原账号行。
pub(in crate::server::core::plugin_host::wasm) struct Account {
    row: RowSource,
    desktop: Option<serde_json::Value>,
    digest: Option<[u8; 32]>,
}
impl Deref for Account {
    type Target = RowSource;
    fn deref(&self) -> &RowSource { &self.row }
}
impl Account {
    pub(super) fn check_desktop(&self) -> Result<(), proto::WireError> {
        let Some(digest) = self.digest else { return Ok(()); };
        authorize_desktop(&self.row)?;
        let current = super::desktop_credentials::catpaw()?;
        let current: [u8;32] = Sha256::digest(current.to_string()).into();
        if current != digest { return Err(invalid_credentials()); }
        Ok(())
    }
}

// 只核原 RowSource 所指的真实保存账号，不以旧固定主键、额外 helper grant 或插件 JSON 补权。
fn authorize_desktop(account: &RowSource) -> Result<(), proto::WireError> {
    if account.provider() != RowProvider::CatPaw || account.identity() != DESKTOP_ID {
        return Err(invalid_binding());
    }
    let host = super::super::host_for_provider("catpaw").filter(|host| host.ready())
        .ok_or_else(unavailable)?;
    if host.options.wasm.as_ref().is_none_or(|options| options.provider != "catpaw"
        || !options.allows("desktop.credentials.v1") || !options.allows("native.helpers.v1")
        || !options.manifest.helpers.iter().any(|helper| helper == super::desktop_credentials::HELPER_CATPAW)) {
        return Err(proto::WireError::with_code("当前 CatPaw 插件未声明本家固定本机读取能力", "forbidden"));
    }
    let store = plugin_accounts::shared().ok_or_else(unavailable)?;
    let record = store.plugin_account(account.account()).ok_or_else(invalid_binding)?;
    if record["provider"] != "catpaw" || record["id"].as_str() != Some(account.account())
        || record["public"]["enabled"] != true || record["desktop"] != true
        || record["identity"].as_str() != Some(DESKTOP_ID) {
        return Err(invalid_binding());
    }
    Ok(())
}

fn read_slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_READS)))
        .clone()
}

/// 原Db事务提交后才取得来源；没有重试、重新选号或失效后重签的分支。
/// 等待取消/超时只丢弃迟到材料，原阻塞闭包及其名额一直保留到实际结束。
pub(in crate::server::core::plugin_host::wasm) async fn prepare(
    call: &SourceRef,
) -> Result<Account, proto::WireError> {
    check_call(call)?;
    let store = plugin_accounts::shared().ok_or_else(unavailable)?;
    let permit = read_slots().try_acquire_owned().map_err(|_| busy())?;
    let deadline = call.deadline().min(Instant::now() + READ_TIMEOUT);
    let ended = call.ended_token();
    let worker_call = call.clone();
    let work = blocking::run(move || -> Result<Account, proto::WireError> {
        let _permit = permit;
        check_call(&worker_call)?;
        worker_call.check_current()?;
        check_deadline(deadline)?;
        let account = store
            .prepare_row_source(RowProvider::CatPaw, worker_call.account())
            .map_err(storage_error)?;
        // 存储锁已经释放后才再进入原调用门；不反向持账号/Db锁检查SourceRef。
        check_call(&worker_call)?;
        worker_call.check_current()?;
        check_deadline(deadline)?;
        if account.provider() != RowProvider::CatPaw || account.account() != worker_call.account() {
            return Err(invalid_binding());
        }
        let is_desktop = account.account() == DESKTOP_ID || account.secret().as_object().is_some_and(desktop);
        let desktop = if is_desktop {
            authorize_desktop(&account)?;
            Some(super::desktop_credentials::catpaw()?)
        } else { None };
        let digest = desktop.as_ref().map(|value| Sha256::digest(value.to_string()).into());
        let account = Account { row: account, desktop, digest };
        credentials(&account)?;
        check_call(&worker_call)?;
        check_deadline(deadline)?;
        Ok(account)
    });
    let account = tokio::select! {
        biased;
        _ = ended.cancelled() => return Err(ended_error()),
        _ = tokio::time::sleep_until(deadline) => return Err(timed_out()),
        result = work => result.map_err(|_| unavailable())?,
    }?;
    check_call(call)?;
    check_deadline(deadline)?;
    Ok(account)
}

/// 只投影已存Passport材料；完整未知secret仍由RowSource保留并参与最终事务核对。
pub(in crate::server::core::plugin_host::wasm) fn credentials(
    account: &Account,
) -> Result<Credentials<'_>, proto::WireError> {
    if account.provider() != RowProvider::CatPaw { return Err(invalid_binding()); }
    let secret = account.desktop.as_ref().unwrap_or_else(|| account.row.secret())
        .as_object().ok_or_else(invalid_credentials)?;
    if secret
        .get("provider")
        .is_some_and(|value| !value.is_null() && value.as_str() != Some("catpaw"))
        || desktop(secret)
    {
        return Err(invalid_credentials());
    }
    let auth = optional_object(secret, "auth")?;
    let user = optional_object(secret, "account")?;
    for value in [
        secret.get("loginType"),
        auth.and_then(|auth| auth.get("loginType")),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_null())
    {
        let value = checked_text(value, 64)?;
        if !value.is_empty() && !value.eq_ignore_ascii_case("passport") {
            return Err(invalid_credentials());
        }
    }
    let token = token_alias([
        secret.get("token"),
        secret.get("accessToken"),
        secret.get("access_token"),
        secret.get("auth_token"),
        auth.and_then(|auth| auth.get("accessToken")),
        auth.and_then(|auth| auth.get("token")),
        auth.and_then(|auth| auth.get("access_token")),
    ])?
    .ok_or_else(invalid_credentials)?;
    let uid = user_alias([
        secret.get("uid"),
        secret.get("userId"),
        user.and_then(|user| user.get("uid")),
        user.and_then(|user| user.get("userId")),
    ])?;
    let login = user_alias([
        secret.get("loginName"),
        user.and_then(|user| user.get("loginName")),
    ])?;
    let user_uid = uid.or(login).ok_or_else(invalid_credentials)?;
    if user_uid == DESKTOP_ID || account.desktop.is_none() && account.identity() != user_uid || user_uid.contains(token) {
        return Err(invalid_credentials());
    }
    let raw_ua = crate::server::core::user_agent::override_value(
        "catpaw", account.user_agent().unwrap_or(""),
    );
    if raw_ua.len() > MAX_UA_BYTES || !raw_ua.bytes().all(|byte| matches!(byte, 0x20..=0x7e)) {
        return Err(missing_user_agent());
    }
    let user_agent = match raw_ua.trim() {
        "" => crate::server::core::egress::DEFAULT_USER_AGENT,
        value => value,
    };
    Ok(Credentials {
        token,
        user_uid,
        user_agent,
        _sealed: (),
    })
}

fn check_call(call: &SourceRef) -> Result<(), proto::WireError> {
    let account = call.account();
    if call.provider() != "catpaw"
        || account.is_empty()
        || account.len() > 512
        || account.trim() != account
        || account.chars().any(char::is_control)
    {
        return Err(invalid_binding());
    }
    if call.deadline() <= Instant::now() || call.ended_token().is_cancelled() {
        return Err(ended_error());
    }
    Ok(())
}

fn check_deadline(deadline: Instant) -> Result<(), proto::WireError> {
    if Instant::now() >= deadline {
        Err(timed_out())
    } else {
        Ok(())
    }
}

fn optional_object<'a>(
    fields: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, proto::WireError> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(fields)) => Ok(Some(fields)),
        Some(_) => Err(invalid_credentials()),
    }
}

fn desktop(fields: &Map<String, Value>) -> bool {
    fields
        .get("desktop")
        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
        || ["id", "identity", "pluginIdentity"]
            .iter()
            .any(|key| fields.get(*key).and_then(Value::as_str) == Some(DESKTOP_ID))
        || fields.get("source").is_some_and(|source| {
            matches!(
                source.as_str(),
                Some("desktop" | "desktop-auth" | "desktop-live")
            ) || source.get("kind").and_then(Value::as_str) == Some("desktop")
        })
}

fn checked_text(value: &Value, max_chars: usize) -> Result<&str, proto::WireError> {
    let raw = value.as_str().ok_or_else(invalid_credentials)?;
    if raw.len() > max_chars.saturating_mul(4) || raw.chars().any(char::is_control) {
        return Err(invalid_credentials());
    }
    let text = raw.trim();
    if text.chars().count() > max_chars || text.contains(';') {
        return Err(invalid_credentials());
    }
    Ok(text)
}

fn user_alias<'a, const N: usize>(
    values: [Option<&'a Value>; N],
) -> Result<Option<&'a str>, proto::WireError> {
    let mut found = None;
    for value in values
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
    {
        let text = checked_text(value, MAX_USER_CHARS)?;
        if text.is_empty() {
            continue;
        }
        if found.is_some_and(|previous| previous != text) {
            return Err(invalid_credentials());
        }
        found = Some(text);
    }
    Ok(found)
}

fn token_alias<'a, const N: usize>(
    values: [Option<&'a Value>; N],
) -> Result<Option<&'a str>, proto::WireError> {
    let mut found = None;
    for value in values
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
    {
        let raw = checked_text(value, MAX_TOKEN_CHARS)?;
        if raw.is_empty() {
            continue;
        }
        let token = if raw
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        {
            raw[7..].trim()
        } else {
            raw
        };
        if token.is_empty()
            || reqwest::header::HeaderValue::from_str(token).is_err()
            || found.is_some_and(|previous| previous != token)
        {
            return Err(invalid_credentials());
        }
        found = Some(token);
    }
    Ok(found)
}

fn storage_error(error: Error) -> proto::WireError {
    match error {
        Error::InvalidInput | Error::UnsupportedProvider => invalid_binding(),
        Error::InvalidRecord => invalid_credentials(),
        Error::LimitExceeded => proto::WireError::with_code(
            "CatPaw账号来源超过大小或结构上限，未继续读取",
            "catpaw_account_limit",
        ),
        Error::VersionExhausted | Error::Conflict(_) => proto::WireError::with_code(
            "CatPaw原账号或授权来源已变化，未重新取得来源",
            "catpaw_account_changed",
        ),
        Error::Storage(StorageFailure::Busy) => busy(),
        Error::Storage(_) => unavailable(),
    }
}

fn invalid_binding() -> proto::WireError {
    proto::WireError::with_code(
        "账号不属于本次CatPaw调用，未读取替代账号",
        "catpaw_account_binding",
    )
}
fn invalid_credentials() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw账号凭据或真实身份缺失、冲突或格式无效",
        "catpaw_account_credentials",
    )
}
fn missing_user_agent() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw账号缺少明确有效的User-Agent，未猜测客户端标识",
        "catpaw_account_user_agent",
    )
}
fn unavailable() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw原账号来源读取未确认，未继续使用替代来源",
        "catpaw_account_unavailable",
    )
}
fn busy() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw账号来源读取通道忙，未排队或重试",
        "catpaw_account_busy",
    )
}
fn timed_out() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw账号来源读取超时，未使用迟到材料",
        "catpaw_account_timeout",
    )
}
fn ended_error() -> proto::WireError {
    proto::WireError::with_code(
        "本次CatPaw调用已结束，未重新读取账号来源",
        "catpaw_account_call_ended",
    )
}
