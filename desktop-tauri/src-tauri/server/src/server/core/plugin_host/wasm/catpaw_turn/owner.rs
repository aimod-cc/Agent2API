//! 原worker独占此Owner：创建、唯一Response读者和独立清理责任不拆给插件。
//! 完整EOF暂留原证据待结算；清理未知保留原Owner，不以本地退场冒充远端销毁。
#[path = "handoff.rs"]
mod handoff;

use std::sync::Arc;

use agent2api_provider_protocol as proto;
use serde_json::json;

use super::{cleanup::{CleanupOwner, Outcome, Reason}, error, input::StartInput, network, reader::{Chunk, Reader}, registry::{nonce, Record}, source};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;
use crate::server::core::egress::managed::FrozenHttpClient;
use super::super::catpaw_account::Account as RowSource;

pub(super) struct Owner {
    source: SourceRef,
    input: StartInput,
    cleanup: CleanupOwner,
    reader: Option<Reader>,
    receipt: String,
    catalog: Option<super::catalog::Witness>,
    catalog_issued: bool,
    tool_reservation: Option<handoff::ToolReservation>,
}

impl Owner {
    pub(super) fn new(source: SourceRef, account: RowSource, input: StartInput) -> Result<Self, proto::WireError> {
        let receipt = format!("catpaw_receipt_{}", nonce()?);
        Ok(Self { source, input, cleanup: CleanupOwner::new(account), reader: None, receipt, catalog: None, catalog_issued: false, tool_reservation: None })
    }
    pub(super) fn account(&self) -> &RowSource { self.cleanup.account() }
    pub(super) fn turn_opened(&self) -> bool { self.reader.is_some() }
    pub(super) fn has_round(&self, conversation: &str) -> bool { self.cleanup.has_round(conversation) }
    pub(super) fn check_read_source(&self) -> Result<(), proto::WireError> { source::check(&self.source, self.cleanup.account()) }
    pub(super) fn transport_eof(&self) -> bool { self.reader.as_ref().is_some_and(Reader::transport_eof) }
    pub(super) async fn read(&mut self) -> Result<Chunk, proto::WireError> {
        self.reader.as_mut().ok_or_else(error::protocol)?.next().await
    }
    pub(super) fn protocol(&self) -> Option<&super::protocol::Protocol> { self.reader.as_ref().map(Reader::protocol) }
    /// 只预约已核原工具证据；true也不表示HTTP交付、交接完成或可以续接。
    pub(super) fn reserve_tool_evidence(&mut self, record: &Arc<Record>, sequence: u64, claims: &serde_json::Value) -> Result<bool, proto::WireError> {
        handoff::reserve(self, record, sequence, claims)
    }
    pub(super) fn continuation_matches(&self, next: &SourceRef) -> bool {
        self.tool_reservation.as_ref().is_some_and(|reservation| reservation.validate_next(next).is_ok())
    }

    pub(super) fn validate_continuation(&self, next: &SourceRef, input: &StartInput) -> Result<(), proto::WireError> {
        let reservation = self.tool_reservation.as_ref().ok_or_else(super::settlement::unconfirmed)?;
        reservation.validate_next(next)?;
        let turn = super::json_text::parse(&input.turn, super::json_text::MAX_BYTES)?;
        let message = super::json_text::encode(&turn["message"], 8 * 1024 * 1024)?;
        reservation.validate_message(&message)
    }

    pub(super) fn rebind_continuation(&mut self, next: SourceRef, input: StartInput) -> Result<(), proto::WireError> {
        self.cleanup.begin_next_turn()?;
        self.source = next;
        self.input = input;
        self.reader = None;
        self.tool_reservation = None;
        self.receipt = format!("catpaw_receipt_{}", nonce()?);
        self.catalog = None;
        self.catalog_issued = false;
        Ok(())
    }

    pub(super) async fn commit_handoff(&self, record: &Record, sequence: u64) -> Result<serde_json::Value, proto::WireError> {
        source::final_value(&self.source, self.cleanup.account(), || record.handoff_value(sequence)).await
    }

    pub(super) fn bind_output(&self, record: &Arc<Record>, sequence: u64) -> Result<(), proto::WireError> {
        let message = self.protocol().and_then(super::protocol::Protocol::message)
            .ok_or_else(super::settlement::invalid_evidence)?;
        self.source.output_slot()?.bind_message(&self.source, record, sequence, message)
    }

    pub(super) fn check_settlement(&self, constraints: &serde_json::Value, claims: &serde_json::Value) -> Result<(), proto::WireError> {
        let reader = self.reader.as_ref().filter(|reader| reader.settlement_ready()).ok_or_else(super::settlement::invalid_evidence)?;
        super::settlement::compare(reader.protocol(), constraints, claims)
    }
    pub(super) async fn complete(&mut self, record: &Record, sequence: u64) -> Result<serde_json::Value, proto::WireError> {
        source::check(&self.source, self.cleanup.account())?;
        self.cleanup.complete(&self.source, record, sequence).await
    }
    pub(super) async fn close(&mut self, record: &Record, reason: Reason) -> Outcome {
        if let Some(reader) = &mut self.reader { reader.discard_transport(); }
        self.cleanup.close(record, reason).await
    }

    pub(super) async fn execute(&mut self, record: &Record) -> Result<(), proto::WireError> {
        source::check(&self.source, self.cleanup.account())?;
        if record.abandoned() { return Err(error::ended()); }
        if record.prepared.continuation {
            if !self.has_round(&record.ids.conversation) || self.cleanup.client().is_none() {
                return Err(error::unavailable());
            }
            self.catalog_issued = true;
            let witness = super::catalog::verify(&self.source, self.cleanup.account(),
                self.cleanup.client().ok_or_else(error::unavailable)?, &record.prepared).await?;
            self.catalog = Some(witness);
            let request = network::prepare(self.cleanup.client().ok_or_else(error::unknown)?,
                self.cleanup.account(), network::Route::Turn, &self.input.turn)?;
            source::submit(&self.source, self.cleanup.account(), record).await?;
            record.accept_inherited_round(&self.receipt)?;
            source::subsequent(&self.source, self.cleanup.account(), || record.mark_turn_submission()).await?;
            self.cleanup.mark_turn();
            let capture = self.source.capture();
            let response = network::open_turn(self.cleanup.client().ok_or_else(error::unknown)?, request, capture.as_ref()).await?;
            self.reader = Some(Reader::new(response, &record.ids.conversation, capture));
            return Ok(());
        }
        let path = self.source.take_egress().map_err(|_| error::unavailable())?;
        let record_end = record.ended_token();
        let call_end = self.source.ended_token();
        let frozen = tokio::select! {
            biased;
            _ = record_end.cancelled() => return Err(error::ended()),
            _ = call_end.cancelled() => return Err(error::ended()),
            result = tokio::time::timeout_at(self.source.deadline(), FrozenHttpClient::from_path(path)) => {
                result.map_err(|_| error::unavailable())?.map_err(|_| error::unavailable())?
            }
        };
        self.cleanup.set_client(frozen);
        source::check(&self.source, self.cleanup.account())?;
        if record.abandoned() { return Err(error::ended()); }
        if self.catalog_issued || self.catalog.is_some() { return Err(error::conflict()); }
        self.catalog_issued = true;
        let witness = super::catalog::verify(
            &self.source,
            self.cleanup.account(),
            self.cleanup.client().ok_or_else(error::unavailable)?,
            &record.prepared,
        ).await?;
        self.catalog = Some(witness);
        self.catalog.as_ref().ok_or_else(error::unavailable)?.check(&self.source, &record.prepared)?;
        if record.abandoned() { return Err(error::ended()); }
        source::check(&self.source, self.cleanup.account())?;
        self.catalog.as_ref().ok_or_else(error::unavailable)?.check(&self.source, &record.prepared)?;
        let round_request = network::prepare(self.cleanup.client().ok_or_else(error::unavailable)?, self.cleanup.account(), network::Route::Round, &self.input.round)?;
        // 构造先于原Call/原行最终门，门内仅freeze与CAS，无锁内网络。
        source::submit(&self.source, self.cleanup.account(), record).await?;
        let round = network::short(self.cleanup.client().ok_or_else(error::unknown)?, round_request, network::Route::Round, &record.ids.conversation).await?;
        // 原HTTP明确受理产生回执；保存到封闭清理Owner先于任何后续可失败记录。
        self.cleanup.accept_round(round);
        record.round_accepted(&self.receipt)?;
        if record.abandoned() { return Err(error::ended()); }
        source::check(&self.source, self.cleanup.account())?;
        let running = json!({"conversationId": record.ids.conversation, "eventType": "conversation", "data": {"status": "running"}});
        let request = network::prepare(self.cleanup.client().ok_or_else(error::unknown)?, self.cleanup.account(), network::Route::Running, &running.to_string())?;
        source::subsequent(&self.source, self.cleanup.account(), || record.mark_running_submission()).await?;
        network::short(self.cleanup.client().ok_or_else(error::unknown)?, request, network::Route::Running, &record.ids.conversation).await?;
        record.running_accepted()?;
        if record.abandoned() { return Err(error::ended()); }
        source::check(&self.source, self.cleanup.account())?;
        let request = network::prepare(self.cleanup.client().ok_or_else(error::unknown)?, self.cleanup.account(), network::Route::Turn, &self.input.turn)?;
        source::subsequent(&self.source, self.cleanup.account(), || record.mark_turn_submission()).await?;
        // 最终门确实成功且即将poll原请求才登记实际turn投递；门内CAS不能冒充投递。
        self.cleanup.mark_turn();
        // 已投递的future不受等待端取消影响；成功后同一worker持有唯一Response。
        let capture = self.source.capture();
        let response = network::open_turn(self.cleanup.client().ok_or_else(error::unknown)?, request, capture.as_ref()).await?;
        self.reader = Some(Reader::new(response, &record.ids.conversation, capture));
        Ok(())
    }
}
