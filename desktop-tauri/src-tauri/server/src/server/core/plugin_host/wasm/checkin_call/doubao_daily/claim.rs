//! 每日签到只在当前官方false且本次Pending确认落盘后发送；再次调用只查当日状态。

use std::sync::atomic::Ordering;

use super::{gate, network, parse, DailyBinding, ObservationOutcome, ObservedFalse, OfficialConfirmation};
use super::super::CheckinCallRef;
use crate::server::core::account_store::credential_version::doubao_state::SnapshotReceipt;

pub(in crate::server::core::plugin_host::wasm::checkin_call) struct DailyRun {
    pub(in crate::server::core::plugin_host::wasm::checkin_call) outcome: ObservationOutcome,
    pub(in crate::server::core::plugin_host::wasm::checkin_call) binding: DailyBinding,
    pub(in crate::server::core::plugin_host::wasm::checkin_call) submitted: bool,
    pub(in crate::server::core::plugin_host::wasm::checkin_call) claimed: bool,
}

impl ObservedFalse {
    pub(in crate::server::core::plugin_host::wasm::checkin_call) async fn claim_once(
        self, binding: DailyBinding, call: &CheckinCallRef, snapshot: &mut SnapshotReceipt,
    ) -> DailyRun {
        let mut output = DailyRun { outcome: ObservationOutcome::Unknown(super::DailyError::QueryUnknown), binding, submitted: false, claimed: false };
        let today = self.selection.selection.owner.day.clone();
        let state = snapshot.state();
        if state.unknown.as_ref().is_some_and(|pending| pending.day == today)
            || state.official.as_ref().is_some_and(|official| official.day == today) {
            output.outcome = ObservationOutcome::Unknown(super::DailyError::PendingConfirmation);
            return output;
        }
        if !self.selection.selection.owner.egress.matches(call)
            || !self.selection.selection.owner.egress.matches_snapshot(snapshot)
            || self.selection.selection.owner.phase.load(Ordering::Acquire) != 5 {
            return output;
        }
        let selection = self.selection;
        if snapshot.checkpoint_pending().is_err() { return output; }
        if gate::checkpoint(&selection.selection.owner, None).await.is_err() { return output; }
        let response = network::submit_checkin(&selection.selection.owner, &selection.selection.target.scene).await;
        output.submitted = selection.selection.owner.phase.load(Ordering::Acquire) >= 6;
        if let Ok(value) = &response {
            output.claimed = parse::successful(value, true, true).is_ok()
                && value["data"]["checked_in"] == true && value["data"].get("idempotent")
                    .is_none_or(|value| value.as_bool() == Some(false));
        }
        let owner = &selection.selection.owner;
        if owner.phase.load(Ordering::Acquire) == 6 {
            if gate::checkpoint(owner, Some((6, 7))).await.is_err() { return output; }
        }
        if owner.phase.load(Ordering::Acquire) != 7 { return output; }
        let target = &selection.selection.target;
        let queried = network::read(owner, target.endpoint, Some(&target.discovery_group), 7, 8).await;
        let checked = match queried {
            Ok(value) => {
                let daily = value.value(owner, target.endpoint, Some(&target.discovery_group)).and_then(|value| {
                    parse::unique_daily(value, &target.discovery_group, target.endpoint, &owner.credentials)
                });
                match daily {
                    Ok(daily) if daily.scene == target.scene && daily.item == target.item => {
                        match gate::checkpoint(owner, Some((8, 9))).await {
                            Ok(()) => owner.clock().map(|observed_at| (daily.checked, observed_at)),
                            Err(error) => Err(error),
                        }
                    }
                    Ok(_) => Err(super::DailyError::MissingOriginalTask),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        if let Ok((true, observed_at)) = checked {
            output.outcome = ObservationOutcome::OfficialConfirmation(OfficialConfirmation { selection, observed_at });
        }
        output
    }
}
