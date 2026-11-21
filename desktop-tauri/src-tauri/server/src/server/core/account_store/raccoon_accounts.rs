//! 小浣熊账号：**存量记录的公开形态投影**（网关侧只剩这一件事）。
//!
//! ── 为什么这里没有「添加 / 导入 / 刷新回写」了 ─────────────────
//! 小浣熊的实现已整体搬到提供商插件：
//!
//! ```text
//!   凭证解析 / JWT / 桌面端 auth.json   plugins/raccoon/credentials.rs + jwt.rs
//!   添加账号（手动 / 文件 / 桌面端导入）  plugins/raccoon 的 account.normalize
//!   token 续期 + 比较-再写               plugins/raccoon 的 refresh + store.save
//!   网页登录 / 回调换码                  plugins/raccoon/oauth.rs + login.callback
//!   目录 / 余额 / 签到 / 新手任务        plugins/raccoon/{models,balance,onboarding}.rs
//! ```
//!
//! 网关这边**只保留公开形态**：存量内置记录（没有 `pluginSecret`）在列表里
//! 仍要能正常显示 —— 否则用户升级后会看到一排「userId 空、tokenTail 空」的
//! 空行，而账号其实是能用的（装上插件后由插件接管转发）。
//!
//! 新账号由插件写、带 `pluginSecret`，`public_account` 的第一条分支就把它们
//! 交给插件的 `account_view`，**不会走到这里**。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! 本文件只读已有记录、**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::store::live_desktop_credentials;
use crate::server::core::account_store::store::AccountStore;
use crate::server::core::account_store::store_util::token_tail_of;

const ACCESS_KEYS: &[&str] = &["accessToken", "access_token", "token", "auth_token"];
const REFRESH_KEYS: &[&str] = &["refreshToken", "refresh_token", "refresh"];

pub(crate) fn strip_bearer(raw: &str) -> String {
    let mut value = raw.trim();
    loop {
        let Some(prefix) = value.get(..6) else {
            return value.to_string();
        };
        if !prefix.eq_ignore_ascii_case("bearer") {
            return value.to_string();
        }
        let rest = &value[6..];
        if rest.is_empty() {
            return String::new();
        }
        if rest.trim_start().len() == rest.len() {
            return value.to_string();
        }
        value = rest.trim_start();
    }
}

pub(crate) fn is_iam_token(token: &str) -> bool {
    let token = strip_bearer(token);
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')) else {
        return false;
    };
    let Ok(Value::Object(claims)) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    claims
        .get("iss")
        .and_then(Value::as_str)
        .is_some_and(|issuer| issuer.ends_with("/iam"))
}

pub(crate) fn has_valid_refresh_token(access: &str, refresh: &str) -> bool {
    let refresh = strip_bearer(refresh);
    if refresh.is_empty() || is_iam_token(access) {
        return false;
    }
    let access = strip_bearer(access);
    access.is_empty() || refresh != access
}

pub(crate) fn first_non_empty(map: &Map<String, Value>, keys: &[&str]) -> String {
    fn text<'a>(source: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
        source?
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
    let auth = map.get("auth").and_then(Value::as_object);
    keys.iter()
        .find_map(|key| text(Some(map), key).or_else(|| text(auth, key)))
        .unwrap_or("")
        .to_string()
}

fn remove_keys(map: &mut Map<String, Value>, keys: &[&str]) -> bool {
    let mut changed = false;
    for key in keys {
        changed |= map.remove(*key).is_some();
    }
    changed
}

fn clear_refresh_flags(record: &mut StoredAccount) -> bool {
    let Some(mut public) = record
        .get("pluginPublic")
        .and_then(Value::as_object)
        .cloned()
    else {
        return false;
    };
    let mut changed = false;
    for flag in ["hasRefreshToken", "canRefresh"] {
        if public.get(flag).and_then(Value::as_bool) == Some(true) {
            public.insert(flag.to_string(), Value::Bool(false));
            changed = true;
        }
    }
    if changed {
        record.set("pluginPublic", Value::Object(public));
    }
    changed
}

/// 清理小浣熊账号中不可用的 refreshToken（IAM 凭据、RT==AT 脏值或插件迁移后的顶层残留）。
pub(crate) fn sanitize_record(record: &mut StoredAccount) -> bool {
    if record.provider() != super::RACCOON_PROVIDER_ID {
        return false;
    }
    let is_desktop = record.is_desktop();
    let mut changed = false;
    if let Some(mut secret) = record
        .get("pluginSecret")
        .and_then(Value::as_object)
        .cloned()
    {
        let access = first_non_empty(&secret, ACCESS_KEYS);
        let refresh = first_non_empty(&secret, REFRESH_KEYS);
        let valid = !refresh.is_empty() && has_valid_refresh_token(&access, &refresh);
        if !valid {
            let mut secret_changed = remove_keys(&mut secret, REFRESH_KEYS);
            if let Some(Value::Object(auth)) = secret.get_mut("auth") {
                secret_changed |= remove_keys(auth, REFRESH_KEYS);
            }
            if secret_changed {
                record.set("pluginSecret", Value::Object(secret));
                changed = true;
            }
            changed |= remove_keys(record.fields_mut(), REFRESH_KEYS);
            if !is_desktop || !access.is_empty() {
                changed |= clear_refresh_flags(record);
            }
        }
    } else {
        let access = first_non_empty(record.fields(), ACCESS_KEYS);
        let refresh = first_non_empty(record.fields(), REFRESH_KEYS);
        if !refresh.is_empty() && !has_valid_refresh_token(&access, &refresh) {
            changed |= remove_keys(record.fields_mut(), REFRESH_KEYS);
            if !is_desktop || !access.is_empty() {
                changed |= clear_refresh_flags(record);
            }
        }
    }
    changed
}

pub(crate) fn clean_dirty_refresh_tokens(accounts: &mut [StoredAccount]) -> usize {
    let mut cleaned = 0;
    for record in accounts {
        if sanitize_record(record) {
            cleaned += 1;
        }
    }
    cleaned
}

impl AccountStore {
    /// 存量小浣熊记录的公开形态（`AccountStore::public_account` 的一支）。
    ///
    /// ── 桌面端账号按**实时值**展示 ─────────────────────────────
    /// 这类账号记录里本来就（按设计）没有 token，`tokenTail` / 过期时间 /
    /// `hasRefreshToken` 若只读记录会永远是空的，界面看起来就是「没有凭证」。
    /// `live_desktop_credentials` 在有桌面端登录态时给出实时值；读不到（客户端
    /// 退出登录、文件被删、或实现已搬到插件）时回落记录里的存量值 —— 于是
    /// 「登录态消失」表现为字段变空，而不是整条记录消失。
    ///
    /// `tokenExpiresAt` 兼容旧数据直接搬过来的 `tokenExpiresAt` 键名。
    pub fn to_raccoon_public_account(&self, record: &StoredAccount) -> Value {
        let proxy = crate::server::core::proxies::describe_account_proxy(Some(
            &record.proxy(),
        ));
        let stored_tail = record
            .get("tokenTail")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let stored_expires = record.expires_at().or_else(|| record.token_expires_at());
        let (token_tail, expires_at, has_refresh) = match live_desktop_credentials(record) {
            Some((token, refresh_token, expires_at)) => (
                if token.is_empty() {
                    stored_tail
                } else {
                    token_tail_of(&token)
                },
                if expires_at > 0.0 { Some(expires_at) } else { stored_expires },
                has_valid_refresh_token(&token, &refresh_token),
            ),
            None => (
                stored_tail,
                stored_expires,
                has_valid_refresh_token(&record.access_token(), &record.refresh_token()),
            ),
        };
        let mut public = Map::new();
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        public.insert("provider".to_string(), Value::String(record.provider()));
        public.insert("name".to_string(), Value::String(record.name()));
        public.insert("userId".to_string(), Value::String(record.user_id()));
        public.insert("tokenTail".to_string(), Value::String(token_tail));
        public.insert(
            "tokenExpiresAt".to_string(),
            expires_at
                .map(crate::server::core::account_store::state::json_number)
                .unwrap_or(Value::Null),
        );
        public.insert("hasRefreshToken".to_string(), Value::Bool(has_refresh));
        public.insert("desktop".to_string(), Value::Bool(record.is_desktop()));
        public.insert("source".to_string(), Value::String(record.source()));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert("proxy".to_string(), proxy);
        // `rateLimits` 是**限额冷却标记**（账号×模型），不是积分字段：
        // 选路层（`routing::pick_account_by_priority`）正是从公开形态里读它来决定
        // 「这个账号对该模型是否还在冷却期内」。少了它，小浣熊账号在 429 之后的
        // **下一次请求**仍会被选中（同一次请求内的降级靠 tried_ids 兜住了），
        // 于是客户端重试会一直撞在同一个限额账号上，而不是降级到下一个。
        // 默认给 `{}`（与 workbuddy 的公开形态同一兜底口径）。
        public.insert(
            "rateLimits".to_string(),
            record
                .get("rateLimits")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
        );
        // 与 workbuddy 的公开形态同口径：本切片所有账号都视为可用
        public.insert("available".to_string(), Value::Bool(true));
        Value::Object(public)
    }
}
