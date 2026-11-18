//! 只预约原Owner已完整读取并核对的工具证据；不是HTTP交付、交接或续接许可。
//! 原行、冻结出口和round收据仍由原CleanupOwner持有，本模块不取得或转移它们。
use std::sync::{Arc, Weak};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::super::{registry::Record, settlement, tool_evidence::ToolEvidence, output_view::ExpectedOutput};
use super::Owner;
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

/// 无Clone/serde/公开构造器；只保留原记录、原来源、真实EOF序号和原全批工具证据。
/// 字段暂不向外开放，最终HTTP交付未接通前不能提取此预约作为发送材料。
pub(super) struct ToolReservation {
    record: Weak<Record>,
    source: SourceRef,
    sequence: u64,
    evidence: ToolEvidence,
    output: Arc<super::super::output::OutputSlot>,
}

impl ToolReservation {
    pub(super) fn output_ready(&self) -> bool {
        self.record.upgrade().is_some_and(|record|
            self.output.observed(&self.source, &record, self.sequence))
    }

    pub(super) fn validate_next(&self, next: &SourceRef) -> Result<(), proto::WireError> {
        let original = self.record.upgrade().ok_or_else(settlement::unconfirmed)?;
        if !self.source.same_session(next) || !next.has_client_identity() || !self.output_ready()
            || !original.tool_ready() { return Err(settlement::unconfirmed()); }
        let previous = self.source.material().messages();
        let current = next.material().messages();
        if current.len() <= previous.len() + 1 || !current.starts_with(previous) {
            return Err(settlement::invalid_evidence());
        }
        let expected = ExpectedOutput::from_evidence(&self.evidence)?;
        if !expected.matches_assistant(&current[previous.len()])? { return Err(settlement::invalid_evidence()); }
        let results = &current[previous.len() + 1..];
        if results.len() != self.evidence.calls().len() { return Err(settlement::invalid_evidence()); }
        let mut seen = std::collections::BTreeSet::new();
        for result in results {
            let id = result["tool_call_id"].as_str().ok_or_else(settlement::invalid_evidence)?;
            let call = self.evidence.calls().iter().find(|call| call.id() == id)
                .ok_or_else(settlement::invalid_evidence)?;
            if !seen.insert(id) || result["role"] != "tool"
                || result.get("name").filter(|name| !name.is_null()).is_some_and(|name| name.as_str() != Some(call.name()))
                || result.get("finished").is_some_and(|value| value.as_bool() != Some(true)) {
                return Err(settlement::invalid_evidence());
            }
        }
        Ok(())
    }

    pub(super) fn validate_message(&self, message: &str) -> Result<(), proto::WireError> {
        self.evidence.candidate_results(message).map(|_| ())
    }
}

/// 仅原Owner内部调用。false只表示该完整原响应没有工具，仍须走原无工具结算校验。
pub(super) fn reserve(
    owner: &mut Owner,
    record: &Arc<Record>,
    sequence: u64,
    claims: &Value,
) -> Result<bool, proto::WireError> {
    if owner.tool_reservation.is_some()
        || !owner.source.same_source(&record.source)
        || !owner.has_round(&record.ids.conversation)
    {
        return Err(settlement::invalid_evidence());
    }
    // 原Reader独占原Response/Protocol，先锁外核真实EOF、全部声明和逐值原回包。
    let has_tools = owner
        .reader
        .as_ref()
        .ok_or_else(settlement::invalid_evidence)?
        .checked_tool_evidence(owner.source.material(), claims)?
        .is_some();
    if !has_tools {
        return Ok(false);
    }
    // 原调用门内只终检已有read交付事实并永久消费Record预约，不取行/出口或联网。
    owner.source.with_submission_gate(|_| {
        record.reserve_tool_evidence(&owner.source, &owner.receipt, sequence)
    })?;
    // Record消费后任何失败都不归还机会；唯一Owner移动原证据，不Clone/重造调用。
    let evidence = owner
        .reader
        .as_mut()
        .ok_or_else(settlement::invalid_evidence)?
        .take_tool_evidence()?;
    let output = owner.source.output_slot()?;
    output.bind(&owner.source, record, sequence, &evidence)?;
    owner.tool_reservation = Some(ToolReservation {
        record: Arc::downgrade(record),
        source: owner.source.clone(),
        sequence,
        evidence,
        output,
    });
    Ok(true)
}
