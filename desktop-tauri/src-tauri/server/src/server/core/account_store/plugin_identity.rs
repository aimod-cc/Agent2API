//! 插件账号落库前的身份解析与主键分配。旧账号保留原 ID，新账号按 provider 隔离。

use std::sync::MutexGuard;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::{kind_from_id, ProviderKind};

use super::account_identity;
use super::sql;
use super::state::StoredAccount;
use super::store::AccountStore;

pub(super) fn incoming(provider: &str, identity: &str, secret: &Value, public: &Value, desktop: bool) -> StoredAccount {
    let mut fields = Map::new();
    fields.insert("provider".to_string(), Value::String(provider.to_string()));
    fields.insert("pluginSecret".to_string(), secret.clone());
    fields.insert("pluginPublic".to_string(), public.clone());
    fields.insert("desktop".to_string(), Value::Bool(desktop));
    let mut record = StoredAccount::from_map(fields);
    if (kind_from_id(provider).is_none() || super::is_cline_family(provider) || native_identity(provider)) && !identity.trim().is_empty() {
        record.set(IDENTITY_KEY, Value::String(identity.to_string()));
    }
    if native_identity(provider) { record.set("pluginIdentityContract", Value::from(1)); }
    mirror_identity(&mut record, provider);
    record
}

/// 身份 / 端点字段从 `pluginSecret` / `pluginPublic` 镜像到**记录顶层**。
///
/// ── 为什么必须镜像（issue #380）──────────────────────────────
/// 网关有一批读路径只认顶层字段，最典型的是计费侧
/// （`billing::account_of` → `session_from_record` → `record.uid()`）。
/// 插件落库时把凭证与展示字段都塞进 `pluginSecret` / `pluginPublic`，
/// 顶层什么都没有 —— 于是「网页登录新建的账号」对话正常（插件自己从
/// `pluginSecret` 取凭证）但签到 / 余额全报「找不到该会话对应的账号记录」。
///
/// 内置实现的落库路径显式写顶层 `uid`（`store_crud`），插件路径必须遵守
/// 同一契约。`edition` 也要一起写：缺了它 `resolve_edition(None)` 会回落
/// `cn`，国际版账号被当成国内版反查，即使补了 uid 也匹配不上。
///
/// ── 键集合怎么定 ────────────────────────────────────────────
/// 取「网关会读、插件会写」的交集：身份（uid / nickname / type / 企业两段）
/// 与端点四件套（edition / endpoint / prefixPath / platform）。这些都是
/// **非敏感**字段（`pluginPublic` 里本来就原样下发），镜像不扩大暴露面。
/// 凭证类（accessToken / refreshToken）不在这里 —— 它们有独立的
/// `MIRROR_KEYS` 契约（只在记录本来就有该键时写）。
const MIRRORED_IDENTITY_KEYS: [&str; 9] = [
    "uid",
    "nickname",
    "type",
    "enterpriseId",
    "enterpriseName",
    "edition",
    "endpoint",
    "prefixPath",
    "platform",
];

/// 从 `pluginPublic`（优先）与 `pluginSecret` 取身份字段写到顶层。
///
/// `pluginPublic` 优先：那是插件的 `account_view` 产出，已经做过归一
/// （例如 `edition` 被翻成 `cn` / `intl` 的稳定 id，而 secret 里可能是
/// 用户随手填的 `international`）。
///
/// ── 只在顶层缺失时才写 ──────────────────────────────────────
/// 顶层是**网关的权威字段**：用户可以在账号设置里改 `endpoint`（指向
/// staging / 自建网关），也可以手工修 `uid`。无条件用插件快照覆盖会让
/// 「重新登录一次」把用户的手工配置洗掉，而症状（打回默认站点）与
/// 「登录态过期」极像，很难查。
///
/// 代价是「插件侧改了值、顶层不跟着变」—— 那种情况由用户显式改账号
/// 设置来表达，不是本函数该猜的。
pub(super) fn mirror_identity(record: &mut StoredAccount, provider: &str) {
    let public = record.get("pluginPublic").cloned().unwrap_or(Value::Null);
    let secret = record.get("pluginSecret").cloned().unwrap_or(Value::Null);
    for key in MIRRORED_IDENTITY_KEYS {
        if has_text_value(record, key) {
            continue;
        }
        let value = public
            .get(key)
            .filter(|value| !value.is_null())
            .or_else(|| secret.get(key).filter(|value| !value.is_null()));
        let Some(value) = value else { continue };
        // 空串是「插件没给这个字段」，不是有效值：写进顶层会让 `uid()` 之类的
        // 读取拿到 `Some("")`，反查照样失败却看不出是缺失。
        let empty = value.as_str().is_some_and(|text| text.trim().is_empty());
        if empty {
            continue;
        }
        record.set(key, value.clone());
    }
    // WorkBuddy 系：插件没给 edition 时按 provider 补一次。判据是「这家有没有
    // 地区概念」，不看插件是否实现——缺 edition 会让国际版账号被反查成国内版。
    if !has_text_value(record, "edition") && record.get("pluginIdentityContract").and_then(Value::as_u64) != Some(1) {
        if let Some(region) = crate::server::core::providers::workbuddy::Region::from_provider_id(provider) {
            record.set("edition", Value::String(region.edition().id.to_string()));
        }
    }
}

/// 顶层是否已有一个**非空**的同名字段。
fn has_text_value(record: &StoredAccount, key: &str) -> bool {
    record
        .get(key)
        .is_some_and(|value| match value.as_str() {
            Some(text) => !text.trim().is_empty(),
            None => !value.is_null(),
        })
}

const IDENTITY_KEY: &str = "pluginIdentity";

fn native_identity(provider: &str) -> bool {
    crate::server::core::plugin_host::wasm::has_capability(provider, "accounts.identity.v1")
}

fn trusted_identity(provider: &str, identity: &str, fields: &Map<String, Value>) -> bool {
    if native_identity(provider) { return !identity.trim().is_empty() && identity.len() <= 512; }
    !identity.trim().is_empty() && !account_identity::guest(provider, fields)
        && (kind_from_id(provider).is_none() || account_identity::stable(provider, fields).is_some())
}

pub(super) fn remember_identity(record: &mut StoredAccount, provider: &str, identity: &str) {
    if trusted_identity(provider, identity, record.fields()) {
        if native_identity(provider) { record.set("pluginIdentityContract", Value::from(1)); }
        record.set(IDENTITY_KEY, Value::String(identity.to_string()));
    } else {
        record.remove(IDENTITY_KEY);
    }
}

fn same_plugin_identity(record: &StoredAccount, provider: &str, identity: &str, fields: &Map<String, Value>) -> bool {
    trusted_identity(provider, identity, fields)
        && (record.get(IDENTITY_KEY).and_then(Value::as_str) == Some(identity) || record.id() == identity)
        && account_identity::reference(provider, record.fields()) == account_identity::reference(provider, fields)
        && !account_identity::conflicts(provider, record.fields(), fields)
}

fn new_id(provider: &str, identity: &str, fields: &Map<String, Value>) -> Result<String, String> {
    let seed = if account_identity::guest(provider, fields) {
        None
    } else {
        account_identity::stable(provider, fields)
            .or_else(|| trusted_identity(provider, identity, fields).then(|| identity.to_string()))
            .or_else(|| account_identity::credential(provider, fields))
    };
    let suffix = match seed {
        Some(seed) => {
            let mut hash = Sha256::new();
            hash.update(provider.as_bytes());
            hash.update([0]);
            hash.update([u8::from(account_identity::reference(provider, fields))]);
            hash.update([0]);
            hash.update(seed.as_bytes());
            format!("{:x}", hash.finalize())
        }
        None => {
            let mut random = [0u8; 16];
            getrandom::getrandom(&mut random)
                .map_err(|_| "无法生成安全的随机账号 id，请重试".to_string())?;
            random.iter().map(|byte| format!("{byte:02x}")).collect()
        }
    };
    Ok(format!("{provider}-account-{suffix}"))
}

fn known_desktop_id(provider: &str, identity: &str, fields: &Map<String, Value>) -> Option<String> {
    if !account_identity::desktop(fields) { return None; }
    let expected = match kind_from_id(provider) {
        Some(ProviderKind::CatPaw) => "desktop-auth".to_string(),
        Some(ProviderKind::Raccoon) => "raccoon-desktop".to_string(),
        Some(ProviderKind::AutoClaw | ProviderKind::AutoClawIntl | ProviderKind::ClineFree | ProviderKind::ClinePass) => {
            format!("{provider}-desktop")
        }
        _ => return None,
    };
    (identity == expected).then_some(expected)
}

impl AccountStore {
    pub(super) fn plugin_account_for_upsert(
        &self,
        guard: &MutexGuard<'_, ()>,
        provider: &str,
        identity: &str,
        account: &StoredAccount,
    ) -> Result<(String, Option<StoredAccount>), String> {
        if native_identity(provider) { return self.plugin_contract_upsert(guard, provider, identity, account); }
        if let Some(region) = crate::server::core::providers::qoder::endpoints::Region::from_provider_id(provider) {
            let secret = account.get("pluginSecret").unwrap_or(&Value::Null);
            return self.qoder_account_for_upsert(guard, region, identity, secret);
        }
        let records = self.with_conn(guard, |conn| {
            let ids: Vec<String> = sql::queue_rows(conn)?.into_iter()
                .filter(|row| row.provider == provider).map(|row| row.id).collect();
            ids.into_iter().map(|id| sql::load_by_id(conn, &id))
                .collect::<rusqlite::Result<Vec<_>>>()
        }).map_err(|error| error.message)?;
        let mut records: Vec<StoredAccount> = records.into_iter().flatten().collect();
        records.sort_by_key(|record| (record.added_at(), record.order_key(), record.id().to_string()));

        let desktop_id = known_desktop_id(provider, identity, account.fields());
        if let Some(id) = desktop_id.as_deref() {
            if let Some(record) = records.iter().find(|record| record.id() == id) {
                if !account_identity::desktop(record.fields()) {
                    return Err("桌面端账号 ID 已被普通账号占用，未覆盖原账号".to_string());
                }
                return Ok((id.to_string(), Some(record.clone())));
            }
        }
        let occupied = if identity.trim().is_empty() {
            None
        } else {
            self.with_conn(guard, |conn| sql::load_by_id(conn, identity)).map_err(|error| error.message)?
        };
        // 同 ID 仍要核对身份；插件 ID 不带域或地区时不能覆盖已存在的另一条账号。
        if let Some(record) = occupied.as_ref().filter(|record| record.provider() == provider) {
            if account_identity::conflicts(provider, record.fields(), account.fields()) {
                return Err("插件账号 ID 与已有账号的业务身份冲突，未覆盖原账号".to_string());
            }
        }
        let existing = records.into_iter().find(|record| {
            account_identity::same(provider, record.fields(), account.fields())
                || same_plugin_identity(record, provider, identity, account.fields())
        });
        if let Some(record) = existing {
            return Ok((record.id().to_string(), Some(record)));
        }
        let id = match desktop_id {
            Some(id) => id,
            None => new_id(provider, identity, account.fields())?,
        };
        if self.with_conn(guard, |conn| sql::load_by_id(conn, &id)).map_err(|error| error.message)?.is_some() {
            return Err("账号 ID 已被其它账号占用，未覆盖原账号".to_string());
        }
        Ok((id, None))
    }
}
