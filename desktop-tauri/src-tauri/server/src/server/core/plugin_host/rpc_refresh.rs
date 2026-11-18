//! 只观察原账号的既有续期登记，不补建槽、恢复旧代次或开放普通插件换证。

use crate::server::core::account_store::{
    credential_version::{Conflict, Error, FreeEnrollmentError, SlotFailure, SlotOutcome},
    AccountStore,
};
use crate::server::core::providers::adapter::ProviderAdapter;
use crate::server::errors::GatewayError;

use super::rpc::RpcAdapter;

pub(super) async fn readiness(store: &AccountStore, provider: &str, account: &str) -> Result<(), GatewayError> {
    let store = store.clone();
    let account = account.to_string();
    let provider = provider.to_string();
    let failure_provider = provider.clone();
    crate::server::core::blocking::run(move || match provider.as_str() {
        "codearts" => match store.read_codearts_ready(&account) {
            Ok(SlotOutcome::Committed(_)) => Ok(()),
            Ok(SlotOutcome::Rejected { error, .. }) => Err(codearts(error)),
            Err(Error::Conflict(Conflict::UnestablishedVersion)) => Err(registration("codearts")),
            Err(Error::Conflict(Conflict::DatabaseEpochChanged | Conflict::HostSourceChanged))
            | Ok(SlotOutcome::CommitUnconfirmed { .. } | SlotOutcome::CommittedButUnusable) => Err(unconfirmed("codearts")),
            _ => Err(unavailable("codearts")),
        },
        "cline-free" => {
            if store.plugin_account(&account).is_some_and(|record| record["desktop"] == true) { return Ok(()); }
            match store.read_free_refresh_origin(&account) {
            Ok(_) => Ok(()),
            Err(FreeEnrollmentError::IdentityConflict | FreeEnrollmentError::RefreshRegistrationMissing) => Err(registration("cline-free")),
            Err(FreeEnrollmentError::Unavailable | FreeEnrollmentError::Storage | FreeEnrollmentError::KeyUnavailable
                | FreeEnrollmentError::Transaction(_)) => Err(unavailable("cline-free")),
            _ => Err(unconfirmed("cline-free")),
            }
        },
        _ => Ok(()),
    }).await.map_err(|_| unavailable(&failure_provider))?
}

fn codearts(error: SlotFailure) -> GatewayError {
    match error {
        SlotFailure::Missing | SlotFailure::Account(Error::Conflict(Conflict::UnestablishedVersion)) => registration("codearts"),
        SlotFailure::NotReady | SlotFailure::ReceiptChanged => unconfirmed("codearts"),
        SlotFailure::RequiresNewAuthorization => GatewayError::bad_request("CodeArts 原登录授权已失效，需要重新登录；不会用临时凭据续期延长旧授权")
            .with_code("refresh_token_expired"),
        _ => unavailable("codearts"),
    }
}

pub(super) fn registration(provider: &str) -> GatewayError {
    let (code, message) = if provider == "cline-free" {
        ("cline_free_refresh_registration_required", "Cline Free 当前旧账号没有可用的自动续期登记。请到「账号设置 → 凭证设置」粘贴官方重新登录后的完整凭证，并勾选自动续期授权；原账号会保留")
    } else {
        ("codearts_refresh_registration_required", "CodeArts 当前账号没有可用的受控续期登记。请在官方重新获取有效凭据并更新此账号；需要自动续期时，通过 CodeArts 显式登记入口确认完整原 OAuth 材料，余额查询不会自行补建登记")
    };
    GatewayError::with_status(409, message).with_code(code)
}

fn unconfirmed(provider: &str) -> GatewayError {
    let (code, message) = if provider == "cline-free" {
        ("cline_free_refresh_unconfirmed", "Cline Free 上次续期或登记来源未确认，请先查看原操作状态；不会再次换证、解封旧登记或读取其他额度池")
    } else {
        ("codearts_refresh_unconfirmed", "CodeArts 上次续期或登记来源未确认，请先查看原操作状态；不会再次换证、重新占槽或沿用旧值冒充成功")
    };
    GatewayError::with_status(409, message).with_code(code)
}

fn unavailable(provider: &str) -> GatewayError {
    GatewayError::with_status(503, format!("{} 原账号续期前提暂不可确认，未发送换证请求；请刷新账号状态", if provider == "cline-free" { "Cline Free" } else { "CodeArts" }))
        .with_code("credential_refresh_unavailable")
}

impl RpcAdapter {
    pub(super) async fn prepare_read_credentials(&self, store: &AccountStore, account: &str) -> Result<(), GatewayError> {
        if readiness(store, self.provider_id(), account).await.is_err() {
            // 未登记不阻断仍有效的只读访问；真正到期由原查询返回，不自动创建续期资格。
            return Ok(());
        }
        if self.credentials_expiring_async(store, account).await {
            self.refresh_access_token(store, account).await?;
        }
        Ok(())
    }
}
