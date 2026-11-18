use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::MutexGuard;

use super::{account_identity, sql, state::StoredAccount, AccountStore};

/// 原合同声明的固定匹配；CodeArts事务先核对真实域/user，再借此保留旧ID。
pub(super) fn contract_declared_match(
    record: &StoredAccount,
    identity: &str,
    desktop: bool,
) -> bool {
    record.get("pluginIdentity").and_then(Value::as_str) == Some(identity)
        && account_identity::desktop(record.fields()) == desktop
}

pub(super) fn contract_matches(
    record: &StoredAccount,
    provider: &str,
    identity: &str,
    account: &StoredAccount,
) -> bool {
    // DeepSeek 旧合同只存了令牌摘要。仅本次在线校验后仍完全相同的令牌
    // 可补入服务端账号 ID 并保留旧主键，不接受插件自报的旧 ID 或身份别名。
    if provider == "deepseek-web" && !account_identity::desktop(account.fields())
        && !account_identity::desktop(record.fields()) {
        let old_secret = record.get("pluginSecret").unwrap_or(&Value::Null);
        let old_public = record.get("pluginPublic").unwrap_or(&Value::Null);
        let new_secret = account.get("pluginSecret").unwrap_or(&Value::Null);
        let old_uid = old_public["uid"].as_str().filter(|uid| uid.len() == 32
            && uid.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let user_id = new_secret["userId"].as_str().filter(|id| !id.is_empty()
            && id.len() <= 256 && !id.chars().any(char::is_control));
        if let (Some(old_uid), Some(user_id), Some(old_token), Some(new_token)) = (
            old_uid, user_id, old_secret["accessToken"].as_str(), new_secret["accessToken"].as_str(),
        ) {
            let legacy_identity = format!("deepseek-web-{old_uid}");
            if old_public["credentialIdentity"] == true && old_secret["userId"].is_null()
                && !old_token.is_empty() && old_token == new_token
                && identity == format!("deepseek-web-user-{user_id}")
                && (record.get("pluginIdentity").and_then(Value::as_str) == Some(legacy_identity.as_str())
                    || record.id() == legacy_identity) {
                return true;
            }
        }
    }
    // 千问旧记录没有额度池身份。只沿用本轮线上核对资料仍持有的同一令牌，
    // 不把同用户另一团队或更换后的令牌合并到旧主键，也不接受调用方自报别名。
    if provider == "qwenwork" && !account_identity::desktop(account.fields())
        && !account_identity::desktop(record.fields()) {
        let old_secret = record.get("pluginSecret").unwrap_or(&Value::Null);
        let new_secret = account.get("pluginSecret").unwrap_or(&Value::Null);
        let old_uid = old_secret["userId"].as_str().filter(|id| !id.is_empty()
            && id.len() <= 256 && !id.chars().any(char::is_control));
        if let (Some(uid), Some(old_token), Some(new_token), Some(team), Some(org)) = (
            old_uid, old_secret["accessToken"].as_str(), new_secret["accessToken"].as_str(),
            new_secret["teamId"].as_str(), new_secret["orgId"].as_str(),
        ) {
            let old_identity = format!("qwenwork-{uid}");
            let pool = format!("{:x}", Sha256::digest(serde_json::json!([org, team]).to_string().as_bytes()));
            let current_identity = format!("qwenwork-{uid}-biz-{}", &pool[..24]);
            if old_secret["isBiz"].is_null() && old_secret["is_biz"].is_null()
                && old_secret["orgId"].is_null() && old_secret["organizationId"].is_null()
                && old_secret["teamId"].is_null() && old_secret["team_id"].is_null()
                && new_secret["isBiz"] == true && new_secret["userId"].as_str() == Some(uid)
                && !old_token.is_empty() && old_token == new_token && !team.is_empty()
                && team.len() <= 256 && org.len() <= 256
                && !team.chars().any(char::is_control) && !org.chars().any(char::is_control)
                && identity == current_identity
                && (record.get("pluginIdentity").and_then(Value::as_str) == Some(old_identity.as_str())
                    || record.id() == old_identity) {
                return true;
            }
        }
    }
    if record.get("pluginIdentityContract").and_then(Value::as_u64) == Some(1) {
        contract_declared_match(
            record,
            identity,
            account_identity::desktop(account.fields()),
        )
    } else {
        let mut legacy = account.fields().clone();
        legacy.remove("pluginIdentityContract");
        legacy.remove("pluginIdentity");
        account_identity::same(provider, record.fields(), &legacy)
            || record.get("pluginIdentity").and_then(Value::as_str) == Some(identity)
                && account_identity::reference(provider, record.fields())
                    == account_identity::reference(provider, &legacy)
                && !account_identity::conflicts(provider, record.fields(), &legacy)
    }
}

/// 同一纯ID规则供受锁CodeArts事务复用；没有数据库、注册表或任意来源参数。
pub(super) fn contract_id(provider: &str, identity: &str, desktop: bool) -> String {
    let mut hash = Sha256::new();
    hash.update(provider.as_bytes());
    hash.update([0, u8::from(desktop), 0]);
    hash.update(identity.as_bytes());
    format!("{provider}-account-{:x}", hash.finalize())
}

impl AccountStore {
    pub(super) fn plugin_contract_upsert(
        &self,
        guard: &MutexGuard<'_, ()>,
        provider: &str,
        identity: &str,
        account: &StoredAccount,
    ) -> Result<(String, Option<StoredAccount>), String> {
        if identity.trim().is_empty() || identity.len() > 512 || identity.contains('\0') {
            return Err("插件必须提供非空且稳定的账号身份".into());
        }
        let records = self
            .with_conn(guard, |conn| {
                sql::queue_rows(conn)?
                    .into_iter()
                    .filter(|row| row.provider == provider)
                    .map(|row| sql::load_by_id(conn, &row.id))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.message)?;
        let mut matches = Vec::new();
        for record in records.into_iter().flatten() {
            if contract_matches(&record, provider, identity, account) {
                matches.push(record);
            }
        }
        if matches.len() > 1 {
            return Err("多条旧账号匹配同一插件身份，未自动合并或覆盖".into());
        }
        if let Some(record) = matches.pop() {
            return Ok((record.id().to_string(), Some(record)));
        }
        let id = contract_id(
            provider,
            identity,
            account_identity::desktop(account.fields()),
        );
        if self
            .with_conn(guard, |conn| sql::load_by_id(conn, &id))
            .map_err(|error| error.message)?
            .is_some()
        {
            return Err("账号身份与已有主键冲突，未覆盖原账号".into());
        }
        Ok((id, None))
    }
}
