use agent2api_provider_protocol as proto;
use tokio::time::Instant;

use crate::server::core::account_store::credential_version::RowSource;

pub(super) async fn check(row: &RowSource, deadline: Instant) -> Result<(), proto::WireError> {
    super::super::doubao_local_gate::wait(deadline, super::invalid, || {
        Ok(row.try_final_check(|| Ok(())))
    }).await
}

