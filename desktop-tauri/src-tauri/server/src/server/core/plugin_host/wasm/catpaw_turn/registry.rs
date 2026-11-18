//! 表只持本次原记录和有限命令；handle/receipt不是来源或停止许可。
//! 本地worker退场与远端回收分开，不淘汰未知责任，不替换已有会话。
use std::collections::BTreeMap;
use std::sync::{atomic::{AtomicBool, AtomicU8, Ordering}, Arc, Mutex};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::{sync::{mpsc, oneshot}, task::AbortHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use super::{cleanup::Outcome, error, input::{Correlation, PreparedInput, StartInput}, network::Route, owner::Owner, read::{Command as ReadCommand, MAX_READS}, settle, settlement, source, worker::Command, PROFILE};
use super::super::catpaw_account::Account as RowSource;
use crate::server::core::plugin_host::{wasm::conversation::SourceRef, Host};

const MAX_RECORDS: usize = crate::server::config::runtime_capacity::POOL_EXECUTIONS;
const PREPARED: u8 = 0;
const RESERVED: u8 = 1;
const SUBMITTED: u8 = 2;
const OPENED: u8 = 3;
const BLOCKED: u8 = 4;
const WORKER_LOST: u8 = 5;
const EOF_AWAITING_SETTLEMENT: u8 = 6;
const CLOSING: u8 = 7;
const LOCAL_FINISHED: u8 = 8;
const SETTLEMENT_RESERVED: u8 = 9;
const COMPLETING: u8 = 10;
const COMPLETED: u8 = 11;
const TOOL_RESERVED: u8 = 12;
const TOOL_HANDED_OFF: u8 = 13;
const TOOL_TAKEN: u8 = 14;

#[derive(Default)]
pub(in crate::server::core::plugin_host::wasm) struct Registry {
    records: Mutex<BTreeMap<String, Arc<Record>>>,
    closed: AtomicBool,
}

pub(super) struct Record {
    pub(super) source: SourceRef,
    pub(super) prepared: PreparedInput,
    pub(super) ids: Correlation,
    pub(super) handle: String,
    stage: AtomicU8,
    submitted: AtomicBool,
    abandoned: AtomicBool,
    ended: CancellationToken,
    settled: CancellationToken,
    state: Mutex<State>,
    predecessor: Option<Arc<Record>>,
    handoff_owner: Mutex<Option<Owner>>,
    ownership_transferred: AtomicBool,
    watching: AtomicBool,
    watch_done: CancellationToken,
    _memory: crate::server::core::runtime_capacity::Allocation,
}

struct State {
    account: Option<RowSource>,
    retained_owner: Option<Owner>,
    failure: Option<proto::WireError>,
    abort: Option<AbortHandle>,
    receipt: Option<String>,
    round_accepted: bool,
    running_accepted: bool,
    running_submitted: bool,
    turn_submitted: bool,
    turn_opened: bool,
    creation_delivered: bool,
    commands: Option<mpsc::Sender<Command>>,
    next_sequence: u64,
    read: Option<ReadState>,
    read_lost: bool,
    last_delivered: Option<u64>,
    transport_eof: Option<u64>,
    eof_delivered: Option<u64>,
    cleanup: Option<Outcome>,
    stop_attempted: bool,
    event_attempted: bool,
    settlement: Option<SettlementState>,
    session_revision: u64,
    // Some永久保留工具预约消费事实；清理/失败/取消不能把它恢复为另一轮资格。
    tool_reserved: Option<u64>,
}
struct ReadState { sequence: u64, offered_eof: Option<bool> }
struct SettlementState {
    sequence: u64,
    lost: bool,
    completion_reserved: bool,
    completion_accepted: bool,
    commit_entered: bool,
    committed: bool,
    delivered: bool,
}

pub(super) fn prepare(registry: &Arc<Registry>, source: SourceRef, account: RowSource, params: Value) -> Result<Value, proto::WireError> {
    source::check(&source, &account)?;
    let mut prepared = PreparedInput::parse(&params, source.client_stream())?;
    let wants_tools = source.material().messages().iter().rev()
        .find(|message| !matches!(message["role"].as_str(), Some("system" | "developer")))
        .is_some_and(|message| message["role"] == "tool");
    let predecessor = if wants_tools {
        if !source.has_client_identity() { return Err(error::unavailable()); }
        let candidates = registry.records.lock().map_err(|_| error::unavailable())?
            .values().filter(|record| record.source.same_session(&source) && record.tool_ready())
            .cloned().collect::<Vec<_>>();
        let mut matched = candidates.into_iter().filter(|record| record.handoff_matches(&source));
        let previous = matched.next().ok_or_else(settlement::unconfirmed)?;
        if matched.next().is_some() { return Err(error::busy()); }
        prepared.continuation = true;
        prepared.revision = previous.prepared.revision.checked_add(1).ok_or_else(error::budget)?;
        Some(previous)
    } else { None };
    let mut issued = std::collections::BTreeSet::new();
    let mut unique = || { let value = nonce()?; if !issued.insert(value.clone()) { return Err(error::unavailable()); } Ok(value) };
    let handle = format!("catpaw_turn_{}", unique()?);
    let conversation = match &predecessor { Some(previous) => previous.ids.conversation.clone(), None => unique()? };
    let turn = unique()?;
    let response = format!("chatcmpl-{}", unique()?);
    let messages = (0..prepared.message_count).map(|_| unique()).collect::<Result<Vec<_>, _>>()?;
    let created = crate::server::logging::now_ms() / 1000;
    if created <= 0 { return Err(error::unavailable()); }
    let memory = crate::server::core::runtime_capacity::Allocation::new(
        crate::server::core::runtime_capacity::value_bytes(&params).saturating_add(64 * 1024))?;
    let record = Arc::new(Record {
        source, prepared, ids: Correlation { conversation, turn, response, messages, created }, handle: handle.clone(),
        stage: AtomicU8::new(PREPARED), submitted: AtomicBool::new(false), abandoned: AtomicBool::new(false),
        ended: CancellationToken::new(), settled: CancellationToken::new(),
        predecessor, handoff_owner: Mutex::new(None), ownership_transferred: AtomicBool::new(false),
        watching: AtomicBool::new(false), watch_done: CancellationToken::new(), _memory: memory,
        state: Mutex::new(State {
            account: Some(account), retained_owner: None, failure: None, abort: None, receipt: None,
            round_accepted: false, running_accepted: false, running_submitted: false, turn_submitted: false,
            turn_opened: false, creation_delivered: false, commands: None, next_sequence: 0, read: None, read_lost: false,
            last_delivered: None, transport_eof: None, eof_delivered: None, cleanup: None, stop_attempted: false, event_attempted: false,
            settlement: None, session_revision: 0, tool_reserved: None,
        }),
    });
    let response = record.prepared_view();
    let mut table = registry.records.lock().map_err(|_| error::unavailable())?;
    if registry.closed.load(Ordering::Acquire) { return Err(error::unavailable()); }
    table.retain(|_, old| !old.forgettable()
        && (old.submitted() || old.stage.load(Ordering::Acquire) != PREPARED || old.source.deadline() > Instant::now()));
    if table.len() >= MAX_RECORDS { return Err(error::budget()); }
    if table.values().any(|old| old.source.same_source(&record.source)
        || !record.source.client_session().is_empty() && old.source.same_session(&record.source)
            && old.blocks_session() && !record.predecessor.as_ref().is_some_and(|previous| Arc::ptr_eq(previous, old))) {
        return Err(error::busy());
    }
    if table.contains_key(&handle) { return Err(error::unavailable()); }
    table.insert(handle, record);
    Ok(response)
}

impl Registry {
    pub(super) fn get(&self, source: &SourceRef, handle: &str) -> Result<Arc<Record>, proto::WireError> {
        source.check_current().map_err(|_| error::unavailable())?;
        if self.closed.load(Ordering::Acquire) { return Err(error::unavailable()); }
        self.get_for_close(source, handle)
    }
    pub(super) fn get_for_close(&self, source: &SourceRef, handle: &str) -> Result<Arc<Record>, proto::WireError> {
        let table = self.records.lock().map_err(|_| error::unavailable())?;
        let record = table.get(handle).ok_or_else(error::conflict)?;
        if !record.source.same_source(source) { return Err(error::unavailable()); }
        Ok(record.clone())
    }
    pub(in crate::server::core::plugin_host::wasm) fn close_for_source(&self, source: &SourceRef) {
        if let Ok(table) = self.records.lock() {
            for record in table.values().filter(|record| record.source.same_source(source)) { record.release_call(); }
        }
    }
    pub(in crate::server::core::plugin_host::wasm) fn close_for_host(&self, host: &Host) {
        self.closed.store(true, Ordering::Release);
        if let Ok(table) = self.records.lock() {
            for record in table.values().filter(|record| record.source.belongs_to_host(host)) { record.force_close(); }
        }
    }
    pub(in crate::server::core::plugin_host::wasm) async fn wait_workers(&self) -> Result<(), proto::WireError> {
        let records = self.records.lock().map_err(|_| error::unavailable())?.values().cloned().collect::<Vec<_>>();
        for record in records {
            if record.stage.load(Ordering::Acquire) != PREPARED { record.settled.cancelled().await; }
            if record.watching.load(Ordering::Acquire) { record.watch_done.cancelled().await; }
        }
        Ok(())
    }
    pub(super) fn local_workers_finished(&self) -> bool {
        self.records.lock().is_ok_and(|table| table.values().all(|record| record.stage.load(Ordering::Acquire) == PREPARED || record.settled.is_cancelled()))
    }
    pub(in crate::server::core::plugin_host::wasm) fn runtime_release_ready(&self) -> bool {
        // 仅释放本地租约：唯一worker真实退场，必要stop/event明确受理，非远端销毁证明。
        self.records.lock().is_ok_and(|table| table.values().all(|record| {
            let local = record.stage.load(Ordering::Acquire) == PREPARED || record.settled.is_cancelled();
            local && (!record.watching.load(Ordering::Acquire) || record.watch_done.is_cancelled())
                && record.handoff_owner.try_lock().is_ok_and(|owner| owner.is_none())
                && record.state.lock().is_ok_and(|state| {
                state.retained_owner.is_none() && (!record.submitted() || state.cleanup == Some(Outcome::Confirmed)
                    || record.ownership_transferred.load(Ordering::Acquire)
                    || state.cleanup == Some(Outcome::NotNeeded) && state.session_revision == 1
                        && state.settlement.as_ref().is_some_and(|settlement| settlement.committed && settlement.completion_accepted))
            })
        }))
    }
    pub(super) fn owns(&self, record: &Arc<Record>) -> bool {
        self.records.lock().is_ok_and(|table| table.get(&record.handle).is_some_and(|current| Arc::ptr_eq(current, record)))
    }
}

impl Record {
    fn prepared_view(&self) -> Value {
        json!({"profile": PROFILE, "handle": self.handle,
            "mode": if self.prepared.continuation { "tool_continuation" } else { "new_round" },
            "conversation_id": self.ids.conversation, "turn_request_id": self.ids.turn,
            "message_ids": self.ids.messages, "response_id": self.ids.response, "created": self.ids.created,
            "session_revision": self.prepared.revision,
            "previous": self.predecessor.as_ref().map(|previous| json!({"conversation_id": previous.ids.conversation}))})
    }
    pub(super) fn input(&self, params: &Value) -> Result<StartInput, proto::WireError> {
        let input = StartInput::parse(params, &self.prepared, &self.ids, &self.handle)?;
        super::input::material::verify_projection(self.source.material(), &self.prepared, &self.ids, &input)?;
        Ok(input)
    }
    pub(super) fn reserve(&self) -> Result<RowSource, proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unavailable())?;
        if self.ended.is_cancelled() || self.source.ended_token().is_cancelled() { return Err(error::ended()); }
        self.stage.compare_exchange(PREPARED, RESERVED, Ordering::AcqRel, Ordering::Acquire).map_err(|_| error::conflict())?;
        state.account.take().ok_or_else(error::conflict)
    }
    pub(super) fn track(&self, abort: AbortHandle, commands: mpsc::Sender<Command>) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unavailable())?;
        if state.abort.is_some() || self.stage.load(Ordering::Acquire) != RESERVED { return Err(error::conflict()); }
        state.abort = Some(abort);
        state.commands = Some(commands);
        Ok(())
    }
    pub(super) fn mark_submission(&self) -> Result<(), proto::WireError> {
        let _state = self.state.lock().map_err(|_| error::unavailable())?;
        if self.ended.is_cancelled() { return Err(error::ended()); }
        self.stage.compare_exchange(RESERVED, SUBMITTED, Ordering::AcqRel, Ordering::Acquire).map_err(|_| error::conflict())?;
        self.submitted.store(true, Ordering::Release);
        Ok(())
    }
    pub(super) fn submitted(&self) -> bool { self.submitted.load(Ordering::Acquire) }
    pub(super) fn ended_token(&self) -> CancellationToken { self.ended.clone() }
    pub(super) fn abandon(&self) {
        let mut release = None;
        let mut local_release = false;
        if let Ok(mut state) = self.state.lock() {
            self.abandoned.store(true, Ordering::Release);
            self.ended.cancel();
            if self.stage.compare_exchange(PREPARED, LOCAL_FINISHED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                state.cleanup = Some(Outcome::NotNeeded);
                release = state.account.take();
                local_release = true;
            }
        } else {
            self.abandoned.store(true, Ordering::Release);
            self.ended.cancel();
            local_release = self.stage.compare_exchange(PREPARED, WORKER_LOST, Ordering::AcqRel, Ordering::Acquire).is_ok();
        }
        drop(release);
        if local_release { self.settled.cancel(); }
    }
    pub(super) fn force_close(&self) {
        self.ended.cancel();
        self.abandoned.store(true, Ordering::Release);
        self.abandon();
    }
    pub(super) fn abandoned(&self) -> bool { self.abandoned.load(Ordering::Acquire) || self.source.ended_token().is_cancelled() }
    pub(super) fn round_accepted(&self, receipt: &str) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        if !self.submitted() || state.round_accepted || state.receipt.is_some() { return Err(error::conflict()); }
        state.receipt = Some(receipt.to_string()); state.round_accepted = true; Ok(())
    }
    pub(super) fn mark_running_submission(&self) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        if self.ended.is_cancelled() { return Err(error::ended()); }
        if !state.round_accepted || state.running_submitted { return Err(error::conflict()); }
        state.running_submitted = true; Ok(())
    }
    pub(super) fn running_accepted(&self) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        if !state.round_accepted || !state.running_submitted || state.running_accepted { return Err(error::conflict()); }
        state.running_accepted = true; Ok(())
    }
    pub(super) fn mark_turn_submission(&self) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        if self.ended.is_cancelled() { return Err(error::ended()); }
        if !state.running_accepted || state.turn_submitted { return Err(error::conflict()); }
        state.turn_submitted = true; Ok(())
    }
    pub(super) fn creation_ready(&self, owner: &Owner, failure: Option<proto::WireError>) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        state.failure = failure;
        state.turn_opened = owner.turn_opened() && owner.has_round(&self.ids.conversation);
        self.stage.store(if state.turn_opened && state.failure.is_none() { OPENED } else { BLOCKED }, Ordering::Release);
        Ok(())
    }
    pub(super) fn response(&self) -> Result<Value, proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        if self.abandoned() { return Err(error::ended()); }
        if let Some(error) = &state.failure { return Err(error.clone()); }
        if self.stage.load(Ordering::Acquire) != OPENED || !state.round_accepted || !state.running_accepted || !state.turn_submitted || !state.turn_opened || state.creation_delivered { return Err(error::unknown()); }
        let receipt = state.receipt.as_ref().ok_or_else(error::unknown)?;
        let result = json!({"profile": PROFILE, "handle": self.handle, "receipt": receipt, "conversation_id": self.ids.conversation,
            "turn_request_id": self.ids.turn, "round_accepted": !self.prepared.continuation, "turn_opened": true, "status": 200, "content_type": "text/event-stream"});
        state.creation_delivered = true;
        Ok(result)
    }
    pub(super) fn reserve_read(&self, receipt: &str, sequence: u64, delivery: oneshot::Sender<Result<Value, proto::WireError>>) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::delivery())?;
        if self.abandoned() { return Err(error::ended()); }
        if let Some(error) = &state.failure { return Err(error.clone()); }
        if self.stage.load(Ordering::Acquire) != OPENED || !state.creation_delivered || state.receipt.as_deref() != Some(receipt) || state.read_lost
            || state.read.is_some() || state.next_sequence != sequence || sequence >= MAX_READS || state.transport_eof.is_some() { return Err(error::read_conflict()); }
        let commands = state.commands.as_ref().ok_or_else(error::delivery)?.clone();
        state.next_sequence += 1;
        state.read = Some(ReadState { sequence, offered_eof: None });
        if commands.try_send(Command::Read(ReadCommand { sequence, delivery })).is_err() {
            state.read_lost = true; self.abandoned.store(true, Ordering::Release); self.ended.cancel(); return Err(error::delivery());
        }
        Ok(())
    }
    pub(super) fn read_reserved(&self, sequence: u64) -> bool {
        self.state.lock().is_ok_and(|state| !state.read_lost && state.read.as_ref().is_some_and(|read| read.sequence == sequence && read.offered_eof.is_none()))
    }
    pub(super) fn note_transport_eof(&self, sequence: u64) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::protocol())?;
        if state.transport_eof.is_some() || state.read.as_ref().is_none_or(|read| read.sequence != sequence) { return Err(error::protocol()); }
        state.transport_eof = Some(sequence); Ok(())
    }
    pub(super) fn offer_read(&self, sequence: u64, eof: bool) -> Result<Value, proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::delivery())?;
        if self.abandoned() || state.read_lost { return Err(error::ended()); }
        if eof && state.transport_eof != Some(sequence) { return Err(error::protocol()); }
        let read = state.read.as_mut().filter(|read| read.sequence == sequence && read.offered_eof.is_none()).ok_or_else(error::read_conflict)?;
        read.offered_eof = Some(eof);
        if eof { self.stage.store(EOF_AWAITING_SETTLEMENT, Ordering::Release); }
        let receipt = state.receipt.as_ref().ok_or_else(error::delivery)?;
        Ok(json!({"profile": PROFILE, "handle": self.handle, "receipt": receipt, "sequence": sequence, "data": "", "eof": eof}))
    }
    pub(super) fn accept_read_delivery(&self, sequence: u64, value: &Value) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::delivery())?;
        if self.abandoned() || state.read_lost { return Err(error::delivery()); }
        let read = state.read.as_ref().filter(|read| read.sequence == sequence).ok_or_else(error::delivery)?;
        let eof = read.offered_eof.ok_or_else(error::delivery)?;
        if value["profile"] != PROFILE || value["handle"].as_str() != Some(self.handle.as_str()) || value["receipt"].as_str() != state.receipt.as_deref() || value["sequence"].as_u64() != Some(sequence) || value["eof"].as_bool() != Some(eof) { return Err(error::delivery()); }
        state.last_delivered = Some(sequence);
        if eof { state.eof_delivered = Some(sequence); }
        state.read = None;
        Ok(())
    }
    pub(super) fn lose_read(&self, sequence: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state.read.as_ref().is_some_and(|read| read.sequence == sequence) { state.read_lost = true; }
        }
        self.abandon();
    }
    pub(super) fn reserve_settlement(&self, receipt: &str, revision: u64, command: settle::Command) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        let sequence = command.sequence;
        if self.abandoned() || self.settled.is_cancelled() || self.stage.load(Ordering::Acquire) != EOF_AWAITING_SETTLEMENT
            || state.failure.is_some() || state.cleanup.is_some() || state.settlement.is_some() || state.retained_owner.is_some() || state.tool_reserved.is_some()
            || !state.creation_delivered || !state.round_accepted || !state.turn_opened || state.read.is_some() || state.read_lost
            || state.receipt.as_deref() != Some(receipt) || revision != self.prepared.revision || state.session_revision != 0
            || state.transport_eof != Some(sequence) || state.eof_delivered != Some(sequence) || state.last_delivered != Some(sequence)
            || sequence.checked_add(1) != Some(state.next_sequence) { return Err(settlement::unconfirmed()); }
        let commands = state.commands.as_ref().filter(|commands| !commands.is_closed()).ok_or_else(settlement::unconfirmed)?.clone();
        state.settlement = Some(SettlementState { sequence, lost: false, completion_reserved: false, completion_accepted: false, commit_entered: false, committed: false, delivered: false });
        self.stage.store(SETTLEMENT_RESERVED, Ordering::Release);
        if commands.try_send(Command::Settle(command)).is_err() {
            if let Some(settlement) = &mut state.settlement { settlement.lost = true; }
            self.abandoned.store(true, Ordering::Release); self.ended.cancel(); return Err(settlement::unconfirmed());
        }
        Ok(())
    }
    pub(super) fn settlement_reserved(&self, sequence: u64) -> bool {
        self.state.lock().is_ok_and(|state| !self.abandoned() && !self.settled.is_cancelled()
            && self.stage.load(Ordering::Acquire) == SETTLEMENT_RESERVED && state.failure.is_none() && state.cleanup.is_none() && state.tool_reserved.is_none()
            && state.settlement.as_ref().is_some_and(|settlement| settlement.sequence == sequence && !settlement.lost && !settlement.completion_reserved))
    }
    /// 只供原Owner在原Source门中消费已核工具证据；不表示最终HTTP交付或实际交接。
    pub(super) fn reserve_tool_evidence(&self, source: &SourceRef, receipt: &str, sequence: u64) -> Result<(), proto::WireError> {
        if !self.source.same_source(source) { return Err(settlement::invalid_evidence()); }
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        if self.abandoned() || self.settled.is_cancelled() || !self.submitted()
            || self.stage.load(Ordering::Acquire) != SETTLEMENT_RESERVED
            || state.failure.is_some() || state.cleanup.is_some() || state.retained_owner.is_some()
            || state.tool_reserved.is_some() || state.stop_attempted || state.event_attempted
            || !state.creation_delivered || !state.round_accepted || !state.running_accepted
            || !state.turn_submitted || !state.turn_opened || state.receipt.as_deref() != Some(receipt)
            || state.session_revision != 0 || state.read.is_some() || state.read_lost
            || state.transport_eof != Some(sequence) || state.eof_delivered != Some(sequence)
            || state.last_delivered != Some(sequence) || sequence >= MAX_READS
            || sequence.checked_add(1) != Some(state.next_sequence)
            || state.settlement.as_ref().is_none_or(|settlement| settlement.sequence != sequence
                || settlement.lost || settlement.completion_reserved || settlement.completion_accepted
                || settlement.commit_entered || settlement.committed || settlement.delivered) {
            return Err(settlement::unconfirmed());
        }
        state.tool_reserved = Some(sequence);
        self.stage.store(TOOL_RESERVED, Ordering::Release);
        Ok(())
    }
    pub(super) fn output_reservation_matches(&self, source: &SourceRef, sequence: u64) -> bool {
        self.source.same_source(source) && self.state.try_lock().is_ok_and(|state| {
            (state.tool_reserved == Some(sequence) || state.settlement.as_ref()
                .is_some_and(|settlement| settlement.sequence == sequence && settlement.committed && settlement.completion_accepted))
                && state.transport_eof == Some(sequence) && state.eof_delivered == Some(sequence) && !state.read_lost
                && !state.stop_attempted
        })
    }

    /// 只读一次消费事实供错误旁路保留原Owner；锁毒也不当作未预约或发送许可。
    pub(super) fn tool_reservation_consumed(&self) -> bool {
        self.state.lock().map(|state| state.tool_reserved.is_some()).unwrap_or(true)
    }
    pub(super) fn mark_completion_submission(&self, sequence: u64) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        if self.abandoned() || self.stage.load(Ordering::Acquire) != SETTLEMENT_RESERVED || state.cleanup.is_some() || state.failure.is_some()
            || state.tool_reserved.is_some() || state.event_attempted || state.stop_attempted || state.session_revision != 0 { return Err(settlement::unconfirmed()); }
        let settlement = state.settlement.as_mut().filter(|settlement| settlement.sequence == sequence && !settlement.lost && !settlement.completion_reserved).ok_or_else(settlement::unconfirmed)?;
        settlement.completion_reserved = true;
        state.event_attempted = true;
        self.stage.store(COMPLETING, Ordering::Release);
        Ok(())
    }
    pub(super) fn completion_reserved(&self) -> bool {
        self.state.lock().map(|state| state.settlement.as_ref().is_some_and(|settlement| settlement.completion_reserved)).unwrap_or(true)
    }
    pub(super) fn note_completion_accepted(&self, sequence: u64) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        let settlement = state.settlement.as_mut().filter(|settlement| settlement.sequence == sequence && settlement.completion_reserved && !settlement.completion_accepted).ok_or_else(settlement::unconfirmed)?;
        settlement.completion_accepted = true;
        Ok(())
    }
    pub(super) fn commit_settlement(&self, sequence: u64) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        if self.abandoned() || self.stage.load(Ordering::Acquire) != COMPLETING || state.cleanup.is_some() || state.failure.is_some()
            || state.session_revision != 0 || state.transport_eof != Some(sequence) || state.eof_delivered != Some(sequence) || state.last_delivered != Some(sequence) { return Err(settlement::unconfirmed()); }
        let settlement = state.settlement.as_mut().filter(|settlement| settlement.sequence == sequence && settlement.completion_accepted && !settlement.commit_entered && !settlement.lost).ok_or_else(settlement::unconfirmed)?;
        settlement.commit_entered = true;
        state.session_revision = 1;
        Ok(())
    }
    pub(super) fn confirm_settlement(&self, sequence: u64) -> Result<Value, proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        // 已完整确认提交允许随后本地收尾；除此之外取消/交付未知绝不产成功回包。
        if self.abandoned() || state.session_revision != 1 || state.cleanup.is_some() || state.failure.is_some() { return Err(settlement::unconfirmed()); }
        let receipt = state.receipt.as_ref().ok_or_else(settlement::unconfirmed)?.clone();
        let settlement = state.settlement.as_mut().filter(|settlement| settlement.sequence == sequence && settlement.completion_accepted && settlement.commit_entered && !settlement.committed && !settlement.lost).ok_or_else(settlement::unconfirmed)?;
        settlement.committed = true;
        self.stage.store(COMPLETED, Ordering::Release);
        Ok(json!({"profile": PROFILE, "handle": self.handle, "receipt": receipt, "disposition": "completed", "state_committed": true, "session_revision": self.prepared.revision + 1}))
    }
    pub(super) fn accept_settlement_delivery(&self, value: &Value) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| settlement::unconfirmed())?;
        if self.abandoned() || state.session_revision != 1 || state.failure.is_some()
            || value["profile"] != PROFILE || value["handle"].as_str() != Some(self.handle.as_str()) || value["receipt"].as_str() != state.receipt.as_deref()
            || value["state_committed"] != true || value["session_revision"].as_u64() != Some(self.prepared.revision + 1) {
            return Err(settlement::unconfirmed());
        }
        if value["disposition"] == "awaiting_tools" {
            if state.tool_reserved.is_none() { return Err(settlement::unconfirmed()); }
            let settlement = state.settlement.as_mut().filter(|settlement| !settlement.delivered && !settlement.lost)
                .ok_or_else(settlement::unconfirmed)?;
            settlement.delivered = true;
            return Ok(());
        }
        if value["disposition"] != "completed" { return Err(settlement::unconfirmed()); }
        let settlement = state.settlement.as_mut().filter(|settlement| settlement.committed && settlement.completion_accepted && !settlement.delivered && !settlement.lost).ok_or_else(settlement::unconfirmed)?;
        settlement.delivered = true;
        Ok(())
    }
    pub(super) fn lose_settlement(&self) {
        if let Ok(mut state) = self.state.lock() { if let Some(settlement) = &mut state.settlement { settlement.lost = true; } }
        self.abandon();
    }
    pub(super) fn tool_ready(&self) -> bool { self.state.try_lock().is_ok_and(|state| {
        state.failure.is_none() && state.cleanup.is_none() && state.tool_reserved.is_some()
            && self.stage.load(Ordering::Acquire) == TOOL_HANDED_OFF && self.settled.is_cancelled()
            && state.settlement.as_ref().is_some_and(|settlement| settlement.delivered && !settlement.lost)
    }) }

    fn blocks_session(&self) -> bool {
        if self.ownership_transferred.load(Ordering::Acquire) { return false; }
        self.state.try_lock().map(|state| state.cleanup != Some(Outcome::NotNeeded)
            || !self.settled.is_cancelled()).unwrap_or(true)
    }

    fn forgettable(&self) -> bool {
        if !self.settled.is_cancelled() || self.watching.load(Ordering::Acquire) && !self.watch_done.is_cancelled() {
            return false;
        }
        if self.ownership_transferred.load(Ordering::Acquire) { return true; }
        let sequence = match self.state.try_lock() {
            Ok(state) if state.retained_owner.is_none() => {
                if state.cleanup == Some(Outcome::Confirmed) || !self.submitted() { return true; }
                if state.cleanup != Some(Outcome::NotNeeded) { return false; }
                state.settlement.as_ref().filter(|settlement| settlement.committed).map(|settlement| settlement.sequence)
            }
            _ => return false,
        };
        sequence.is_some_and(|sequence| self.source.output_slot()
            .map(|output| output.observed(&self.source, self, sequence)).unwrap_or(true))
    }

    fn handoff_matches(&self, next: &SourceRef) -> bool {
        self.handoff_owner.try_lock().is_ok_and(|owner| owner.as_ref()
            .is_some_and(|owner| owner.continuation_matches(next)))
    }

    pub(super) fn keep_handoff_owner(&self, owner: Owner) -> Result<(), Owner> {
        let Ok(mut held) = self.handoff_owner.lock() else { return Err(owner); };
        if held.is_some() { return Err(owner); }
        *held = Some(owner);
        self.stage.store(TOOL_HANDED_OFF, Ordering::Release);
        Ok(())
    }

    pub(super) fn take_handoff_owner(&self, next: &SourceRef, input: &StartInput) -> Result<Owner, proto::WireError> {
        if !self.tool_ready() { return Err(settlement::unconfirmed()); }
        let mut held = self.handoff_owner.try_lock().map_err(|_| settlement::unconfirmed())?;
        held.as_ref().ok_or_else(settlement::unconfirmed)?.validate_continuation(next, input)?;
        self.stage.compare_exchange(TOOL_HANDED_OFF, TOOL_TAKEN, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| settlement::unconfirmed())?;
        let owner = held.take().ok_or_else(settlement::unconfirmed)?;
        self.ownership_transferred.store(true, Ordering::Release);
        Ok(owner)
    }

    pub(super) fn watch_started(&self) -> bool {
        !self.watching.swap(true, Ordering::AcqRel)
    }
    pub(super) fn watch_finished(&self) { self.watch_done.cancel(); }
    pub(super) fn handoff_taken(&self) -> bool { self.ownership_transferred.load(Ordering::Acquire) }
    pub(super) fn take_handoff_cleanup(&self) -> Option<Owner> {
        let mut held = self.handoff_owner.lock().ok()?;
        if self.stage.compare_exchange(TOOL_HANDED_OFF, CLOSING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return None;
        }
        held.take()
    }
    pub(super) fn output_failed(&self) -> bool {
        self.source.output_slot().map(|output| output.failed()).unwrap_or(true)
    }

    pub(super) fn handoff_value(&self, sequence: u64) -> Result<Value, proto::WireError> {
        let mut state = self.state.try_lock().map_err(|_| settlement::unconfirmed())?;
        if state.tool_reserved != Some(sequence) || state.failure.is_some() || self.abandoned()
            || state.session_revision != 0 { return Err(settlement::unconfirmed()); }
        state.session_revision = 1;
        Ok(json!({"profile": PROFILE, "handle": self.handle, "receipt": state.receipt,
            "disposition": "awaiting_tools", "state_committed": true, "session_revision": self.prepared.revision + 1}))
    }

    pub(super) fn accept_inherited_round(&self, receipt: &str) -> Result<(), proto::WireError> {
        if !self.prepared.continuation || self.predecessor.is_none() { return Err(error::conflict()); }
        self.round_accepted(receipt)?;
        let mut state = self.state.lock().map_err(|_| error::unknown())?;
        state.running_submitted = true;
        state.running_accepted = true;
        Ok(())
    }

    pub(super) fn predecessor(&self) -> Option<&Arc<Record>> { self.predecessor.as_ref() }

    pub(super) fn release_call(&self) {
        let awaiting = self.state.try_lock().is_ok_and(|state| state.tool_reserved.is_some()
            && state.session_revision == 1 && state.failure.is_none()
            && state.settlement.as_ref().is_some_and(|settlement| settlement.delivered && !settlement.lost));
        if !awaiting { self.abandon(); }
    }

    pub(super) fn note_failure(&self, failure: proto::WireError) {
        if let Ok(mut state) = self.state.lock() { state.failure.get_or_insert(failure); }
    }
    pub(super) fn mark_cleanup_action(&self, route: Route) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::cleanup())?;
        if state.settlement.as_ref().is_some_and(|settlement| settlement.completion_reserved) { return Err(error::cleanup()); }
        match route {
            Route::Stop if state.turn_submitted && !state.stop_attempted => state.stop_attempted = true,
            Route::Terminal if !state.event_attempted => state.event_attempted = true,
            _ => return Err(error::cleanup()),
        }
        self.stage.store(CLOSING, Ordering::Release);
        Ok(())
    }
    pub(super) fn finish_cleanup(&self, outcome: Outcome) -> Result<(), proto::WireError> {
        let mut state = self.state.lock().map_err(|_| error::cleanup())?;
        if state.cleanup.is_some() { return Err(error::cleanup()); }
        state.cleanup = Some(outcome);
        state.commands = None;
        self.stage.store(LOCAL_FINISHED, Ordering::Release);
        Ok(())
    }
    pub(super) fn retain_lost_owner(&self, owner: Owner) {
        // 同步析构旁路不联网；锁毒也仅保留原材料，不能从毒状态签发成功。
        let mut owner = Some(owner);
        {
            let mut state = match self.state.lock() { Ok(state) => state, Err(poisoned) => poisoned.into_inner() };
            if state.retained_owner.is_none() { state.retained_owner = owner.take(); }
            state.failure.get_or_insert_with(error::unknown);
        }
        drop(owner);
    }
    pub(super) async fn worker_settled(&self) { self.settled.cancelled().await; }
    pub(super) fn worker_finished(&self) { self.settled.cancel(); }
    pub(super) fn note_worker_failure(&self) {
        self.stage.store(WORKER_LOST, Ordering::Release);
        if let Ok(mut state) = self.state.lock() { state.failure.get_or_insert_with(error::unknown); state.commands = None; }
    }
    pub(super) fn worker_lost(&self) { self.note_worker_failure(); self.settled.cancel(); }
    pub(super) fn close_response(&self) -> Result<Value, proto::WireError> {
        if !self.settled.is_cancelled() { return Err(error::cleanup()); }
        if self.state.try_lock().is_ok_and(|state| state.tool_reserved.is_some()
            && state.cleanup.is_none() && state.failure.is_none()
            && state.settlement.as_ref().is_some_and(|settlement| settlement.delivered && !settlement.lost)) {
            return Ok(json!({"profile": PROFILE, "handle": self.handle, "closed": true, "cleanup": "not_needed"}));
        }
        let state = self.state.lock().map_err(|_| error::cleanup())?;
        let outcome = state.cleanup.unwrap_or(if self.submitted() { Outcome::Unconfirmed } else { Outcome::NotNeeded });
        Ok(json!({"profile": PROFILE, "handle": self.handle, "closed": true, "cleanup": outcome.text()}))
    }
}

pub(super) fn nonce() -> Result<String, proto::WireError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| error::unavailable())?;
    if bytes.iter().all(|byte| *byte == 0) { return Err(error::unavailable()); }
    bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
    let hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}
