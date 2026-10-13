//! Qoder 凭证续期：同一凭证只刷新一次，保存时确认账号未被重新导入或删除。
//!
//! 两条续期路径，都挂在 **openapi 主机**上、都不需要 COSY 签名：
//!   - 设备流凭据（`dt-` / `drt-`）→ `POST /api/v1/deviceToken/refresh`；
//!   - PAT 换来的作业令牌（`jt-` / `jrt-`）→ `POST /api/v1/jobToken/refresh`，
//!     或直接用 PAT 重新兑换（记录里带 PAT 时优先走这条）。
//!
//! **不要**改回 `{center}/algo/api/v3/user/refresh_token`：那条要求 appcode 签名，
//! 设备流凭据打过去只会被 WAF 以 `403 Request discarded` 丢掉（详见
//! `endpoints::DEVICE_REFRESH_PATH` 的说明）。

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::server::core::account_store::{AccountStore, CredentialWrite};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::refresh_flight::{self, Join, Table};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::auth;
use super::credentials::{self, Credentials};
use super::endpoints;
use super::endpoints::Region;

static FLIGHTS: OnceLock<Table<Credentials>> = OnceLock::new();

pub fn snapshot(
    store: &AccountStore,
    region: Region,
    account_id: &str,
) -> Result<(Value, Credentials), GatewayError> {
    let record = store.qoder_account_record(region, account_id)
        .ok_or_else(|| GatewayError::with_status(404, "Qoder 账号不存在，请先添加账号"))?;
    let mut credentials = Credentials::from_payload(&record)?;
    credentials.complete_identity()?;
    Ok((record, credentials))
}

pub async fn ensure_fresh(
    store: &AccountStore,
    region: Region,
    account_id: &str,
    force: bool,
) -> Result<Credentials, GatewayError> {
    let (record, credentials) = snapshot(store, region, account_id)?;
    if !force && !credentials.expiring() {
        return Ok(credentials);
    }
    if !credentials.can_refresh() {
        return Err(GatewayError::with_status(400, "Qoder 账号没有刷新凭证，请重新登录或添加 PAT"));
    }
    let key = format!("{}:{}:{}:{}:{}", store.file_string(), account_id, credentials.region.id(),
        refresh_flight::fingerprint(&credentials.access_token), refresh_flight::fingerprint(&credentials.refresh_token));
    match FLIGHTS.get_or_init(Table::new).join(&key) {
        Join::Waiter(waiter) => waiter.wait().await,
        Join::Leader(leader) => {
            let result = refresh_and_save(store, &record, &credentials).await;
            leader.finish(result.clone());
            result
        }
    }
}

async fn refresh_and_save(
    store: &AccountStore,
    record: &Value,
    credentials: &Credentials,
) -> Result<Credentials, GatewayError> {
    let proxy = auth::account_proxy(record)?;
    let mut fresh = if let Some(pat) = credentials.pat() {
        let mut fresh = auth::exchange_pat(pat, credentials.region, proxy.as_ref()).await?;
        fresh.machine_id = credentials.machine_id.clone();
        fresh.complete_identity()?;
        fresh
    } else {
        refresh_via_open_api(credentials, proxy.as_ref()).await?
    };
    fresh.complete_identity()?;
    if fresh.user_id != credentials.user_id || fresh.region != credentials.region {
        return Err(GatewayError::with_status(400, "Qoder 续期返回了不同账号，旧凭证未被覆盖"));
    }
    match store.update_qoder_credentials_if_current(record, &fresh)
        .map_err(|error| GatewayError::with_status(error.status_code, error.message))?
    {
        CredentialWrite::Written => Ok(fresh),
        CredentialWrite::Stale => {
            let id = record.get("id").and_then(Value::as_str).unwrap_or("");
            snapshot(store, credentials.region, id).map(|(_, credentials)| credentials)
        }
    }
}

/// 用刷新令牌换一组新凭据（**openapi 主机，无需签名**）。
///
/// 路径按刷新令牌前缀分派：`jrt-`（PAT 换来的作业刷新令牌）走
/// `/api/v1/jobToken/refresh`，**其余一律走设备端点** `/api/v1/deviceToken/refresh`。
/// 默认落在设备端点是刻意的 —— 我们自己的账号全部来自网页设备授权（`drt-`），
/// 未知格式大概率同族；而作业端点对 `drt-` 是硬拒（实测 400 `Bad request`）。
/// 两个端点不可互换，所以判据是**令牌前缀**而不是「记录里有没有 PAT」：
/// PAT 与设备刷新令牌可能同时存在于一条记录里（PAT 是兜底），按存在性分派会让
/// PAT 劫持设备刷新，把设备令牌换成作业令牌，那之后 COSY 身份就与记录里的
/// `machineId` 对不上了。
///
/// 上游**每次刷新都轮换**：返回新的访问令牌 + 新的刷新令牌，旧的立刻失效。
/// 因此新令牌必须整体回写（`refresh_token` 缺失时才退回旧值，见下）。
async fn refresh_via_open_api(
    credentials: &Credentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<Credentials, GatewayError> {
    let refresh = credentials.oauth_refresh();
    if refresh.is_empty() {
        return Err(GatewayError::with_status(400, "Qoder 账号没有刷新凭证，请重新登录或添加 PAT"));
    }
    let path = if refresh.starts_with("jrt-") {
        endpoints::JOB_REFRESH_PATH
    } else {
        endpoints::DEVICE_REFRESH_PATH
    };
    let response = auth::request(
        "POST",
        &format!("{}{}", credentials.region.open_api(), path),
        // 字段名是 snake_case 的 `refresh_token`，与取令牌时的 camelCase
        // （`refreshToken`）不是同一套 —— 发错名字上游按缺参处理。
        Some(&json!({ "refresh_token": refresh })),
        &endpoints::refresh_headers(),
        proxy,
    ).await?;
    // 401/403 在续期语义下只有一个含义：这把刷新令牌已经不能用了（过期、
    // 被轮换掉，或与所选地区不符 —— 两站令牌不通用）。给一句能照做的提示，
    // 而不是把上游的 `Request discarded` 原样透出去。
    if matches!(response.status, 401 | 403) {
        return Err(GatewayError::with_status(
            401,
            "Qoder 刷新令牌已失效或与所选地区不符，请重新登录该账号",
        ));
    }
    let data = auth::payload(response, "凭证续期")?;
    // 响应带用户标识时先核对身份：把 A 账号的令牌写进 B 账号的记录，
    // 会让两个账号同时不可用，且从余额/模型列表上都看不出来。
    // **只认 `user_id` 这几个键**：设备响应的 `id` 是设备会话号而非账号
    // （对照实现专门标注过这个坑）。
    let returned_user = credentials::text(&data, &["user_id", "uid", "userId"]);
    if !returned_user.is_empty() && !credentials.user_id.is_empty() && returned_user != credentials.user_id {
        return Err(GatewayError::with_status(502, "Qoder 续期返回了不同账号，旧凭证未被覆盖"));
    }
    // 访问令牌的键名不固定：设备端点回 `device_token`，作业端点回 `token`。
    // `secret` 按给定顺序取第一个非空值，命中即用。
    let token = credentials::secret(&data, &["device_token", "deviceToken", "token", "access_token"])?;
    if token.is_empty() {
        return Err(GatewayError::with_status(502, "Qoder 续期响应缺少令牌，旧凭证未被覆盖"));
    }
    let refresh_token = credentials::secret(&data, &["refresh_token", "refreshToken"])?;
    if refresh_token.contains('|') {
        return Err(GatewayError::with_status(502, "Qoder 续期响应的 refresh_token 格式无效"));
    }
    let mut fresh = credentials.clone();
    fresh.access_token = token;
    // 续期响应没带新刷新令牌时保留旧值：上游按「轮换」语义工作，此处只做兜底，
    // 免得把仍然有效的那一半洗成空（空刷新令牌会让账号再也刷不动）。
    fresh.refresh_token = format!("{}|{}|{}",
        if refresh_token.is_empty() { refresh } else { &refresh_token },
        credentials.user_id, credentials.machine_id);
    // 只认绝对时间字段。**不要**回落到 `expires_in`：同一份协议里它的单位
    // 在两家参考实现中被分别当成秒和毫秒（实测设备端点回 2591999994，
    // 按毫秒算正好 30 天），猜错会写出一个几十年后的过期时间，让自动续期
    // 再也不触发。缺字段时按设备令牌的 30 天寿命兜底，宁可早刷一次。
    fresh.expires_at = Some(
        credentials::timestamp(data.get("expires_at"))
            .or_else(|| credentials::timestamp(data.get("expiresAt")))
            .or_else(|| credentials::timestamp(data.get("expire_time")))
            .or_else(|| credentials::timestamp(data.get("expireTime")))
            .unwrap_or_else(|| logging::now_ms() + 30 * 24 * 60 * 60 * 1000),
    );
    Ok(fresh)
}
