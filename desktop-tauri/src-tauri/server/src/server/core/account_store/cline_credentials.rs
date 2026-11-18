use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{account_identity, credential_version::{FreeEnrollmentInput, FreeEnrollmentError}, plugin_accounts, sql,
    AccountStore, AccountStoreError};
use crate::server::{db::FreeEnrollmentTransaction, logging};

impl AccountStore {
    pub(crate) fn replace_cline_free_credentials(&self, id: &str, expected_updated_at: i64,
        input: FreeEnrollmentInput, authorize_refresh: bool,
    ) -> Result<Value, AccountStoreError> {
        valid_material(&input)?;
        let db = self.database().ok_or_else(save_unconfirmed)?;
        let source = db.capture_cline_free_enrollment_source().map_err(|_| save_unconfirmed())?;
        let key = Zeroizing::new(crate::server::core::crypto::try_cached_key(
            crate::server::core::crypto::SecretScope::Accounts).ok_or_else(save_unconfirmed)?);
        let mut retained = sql::credential_version::cline_free_enrollment::RetainedRows::new();
        let guard = self.guard();
        let saved = db.cline_free_enrollment_transaction(&source, |conn| {
            let Some(mut record) = sql::load_by_id(conn, id).map_err(|_| save_unconfirmed())? else {
                return Err(AccountStoreError::new("账号不存在", 404));
            };
            if record.provider() != "cline-free" || account_identity::desktop(record.fields()) {
                return Err(AccountStoreError::new("此入口只更新当前 Cline Free 普通账号，不处理 Pass 或桌面实时凭证", 400));
            }
            if record.updated_at() != expected_updated_at {
                return Err(AccountStoreError::new("账号已经变化，请重新打开账号设置后再更新凭证", 409));
            }
            let original_uid = account_identity::stable("cline-free", record.fields());
            if original_uid.as_deref() != Some(input.uid()) {
                return Err(AccountStoreError::new("这份凭证不属于当前账号，未覆盖原账号；其他账号请通过添加入口导入", 409));
            }
            let incoming = super::plugin_identity::incoming("cline-free", input.identity(), input.secret(), input.public(), false);
            if account_identity::conflicts("cline-free", record.fields(), incoming.fields()) {
                return Err(AccountStoreError::new("凭证身份与原账号不一致，未保存", 409));
            }
            let original = record.clone();
            valid_material(&input)?;
            record.set("pluginSecret", input.secret().clone());
            let mut public = input.public().clone();
            public["canRefresh"] = Value::Bool(authorize_refresh);
            record.set("pluginPublic", public);
            record.set("pluginIdentity", json!(input.identity()));
            record.set("pluginIdentityContract", json!(1));
            record.set("source", json!("plugin"));
            record.set("account", json!(input.uid()));
            record.set("uid", json!(input.uid()));
            record.remove("needsRelogin");
            record.remove("needsReloginReason");
            plugin_accounts::credential_version_mirror(&mut record, input.secret());
            for (aliases, canonical) in [(&["access_token", "token"][..], "accessToken"),
                (&["refresh_token"][..], "refreshToken")] {
                for alias in aliases {
                    if record.get(alias).is_some() { record.set(alias, input.secret()[canonical].clone()); }
                }
            }
            for container in ["auth", "metadata"] {
                if record.get(container).is_some() { record.remove(container); }
            }
            record.set("usageCredentialRevision", json!(record.get("usageCredentialRevision")
                .and_then(Value::as_u64).unwrap_or(0).saturating_add(1)));
            record.set_updated_at(logging::now_ms());
            sql::update_in_place(conn, &record).map_err(|_| save_unconfirmed())?;
            if authorize_refresh {
                sql::credential_version::cline_free_enrollment::reauthorize::authorize(
                    conn, &key, &original, &input, &mut retained).map_err(|error| match error {
                        FreeEnrollmentError::ExistingSlot => AccountStoreError::new(
                            "这份刷新凭据已用于原登记，请使用官方重新登录后取得的新凭据；未再次授权或发送换证请求", 409),
                        FreeEnrollmentError::IdentityConflict | FreeEnrollmentError::ExistingAccount => AccountStoreError::new(
                            "账号或续期登记身份冲突，未覆盖原登记", 409),
                        _ => save_unconfirmed(),
                    })?;
            }
            let saved = sql::load_by_id(conn, id).map_err(|_| save_unconfirmed())?.ok_or_else(save_unconfirmed)?;
            if saved.get("pluginSecret") != Some(input.secret()) || saved.provider() != "cline-free" {
                return Err(save_unconfirmed());
            }
            valid_material(&input)?;
            Ok(saved)
        });
        drop(guard);
        drop(retained);
        let saved = match saved {
            FreeEnrollmentTransaction::Committed { value, storage_closed: true } => value,
            FreeEnrollmentTransaction::Rejected { error, storage_closed: true } => return Err(error),
            _ => return Err(save_unconfirmed()),
        };
        Ok(self.public_account(&saved))
    }
}

fn valid_material(input: &FreeEnrollmentInput) -> Result<(), AccountStoreError> {
    let now = logging::now_ms();
    if now <= 0 || input.expires_at_ms().is_some_and(|expiry| expiry <= now) {
        return Err(AccountStoreError::new("请粘贴官方重新登录后取得的有效凭证，当前这份已经到期", 400));
    }
    Ok(())
}

fn save_unconfirmed() -> AccountStoreError {
    AccountStoreError::new("凭证保存结果未确认，请刷新后核对；不会自动重试", 503)
}
