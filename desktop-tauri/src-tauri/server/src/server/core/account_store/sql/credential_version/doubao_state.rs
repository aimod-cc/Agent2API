//! 固定豆包来源与最初状态版本的真实IMMEDIATE CAS；只持原账号锁及同源Db连接锁。
//! Unknown和Official同schema分别解析；Official只能从原Empty快照+按值typed fact写入，不降级/补归属。
//! 大明文/固定值/读取快照由外层RetainedValues持至锁外；事务返回只含小标量。

mod envelope;
mod record;
mod retained_json;
mod transaction;
pub(in crate::server::core::account_store) mod rewards;
pub(in crate::server::core::account_store) use transaction::CommitAttempt;

use rusqlite::Connection;
use serde_json::{json, Value};

use super::{bounded, row_source::RowStamp, storage, Error, StorageFailure};
use crate::server::core::{account_store::credential_version::RowProvider, plugin_host::wasm::OfficialDailyFact};
use record::{FixedState, Memory, RawSlot};

const STATE_KEY: &str = "host:doubaowork:daily-confirmation";
const SCHEMA: u32 = 1;
const MAX_VALUE_BYTES: usize = 1024;
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_STATE_ENTRIES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateFailure {
    WrongProvider, Unavailable, SourceChanged, CandidateChanged, InvalidContainer, InvalidEntry,
    UnsupportedSchema, OfficialConfirmationUnavailable, StateChanged, RevisionExhausted, InvalidClock,
    SnapshotNotEmpty, OfficialAlreadyConfirmed,
}

#[derive(Debug)]
pub(crate) enum StateError { Account(Error), State(StateFailure) }
impl From<Error> for StateError { fn from(error: Error) -> Self { Self::Account(error) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateKind { Empty, Unknown, Official }

/// kind明确区分Empty/Unknown/Official；后两项互斥，None不能单独当作“没有签到”。
pub(crate) struct StateView<'a> {
    pub present: bool,
    pub schema: u32,
    pub revision: u64,
    pub kind: StateKind,
    pub unknown: Option<UnknownView<'a>>,
    pub official: Option<OfficialView<'a>>,
}

pub(crate) struct UnknownView<'a> { pub receipt_id: &'a str, pub snapshot_at: i64, pub day: &'a str }

/// 只表示官网原当日任务已完成；execution保持未知，不包含金额、领取、发送或重发许可。
pub(crate) struct OfficialView<'a> {
    pub receipt_id: &'a str,
    pub snapshot_at: i64,
    pub day: &'a str,
    pub discovery_group: &'a str,
    pub scene: &'a str,
    pub item_code: &'a str,
    pub selected_day: &'a str,
    pub selected_at: i64,
    pub observed_at: i64,
}

pub(in crate::server::core::account_store) struct Snapshot {
    present: bool,
    schema: u32,
    revision: u64,
    memory: Option<Memory>,
    reserved: Memory,
}

impl Snapshot {
    pub(in crate::server::core::account_store) fn view(&self) -> StateView<'_> {
        let mut view = StateView { present: self.present, schema: self.schema, revision: self.revision,
            kind: StateKind::Empty, unknown: None, official: None };
        if let Some(memory) = &self.memory {
            match &memory.official {
                None => {
                    view.kind = StateKind::Unknown;
                    view.unknown = Some(UnknownView { receipt_id: &memory.receipt_id,
                        snapshot_at: memory.snapshot_at, day: &memory.day });
                }
                Some(task) => {
                    view.kind = StateKind::Official;
                    view.official = Some(OfficialView { receipt_id: &memory.receipt_id,
                        snapshot_at: memory.snapshot_at, day: &memory.day, discovery_group: &task.discovery_group,
                        scene: &task.scene, item_code: &task.item_code, selected_day: &task.selected_day,
                        selected_at: task.selected_at, observed_at: task.observed_at });
                }
            }
        }
        view
    }

    pub(in crate::server::core::account_store) fn current_day(&self) -> &str { &self.reserved.day }
    pub(in crate::server::core::account_store) fn advance_unknown(&mut self, revision: u64) -> Result<(), StateError> {
        if self.revision.checked_add(1) != Some(revision) { return Err(state_error(StateFailure::StateChanged)); }
        self.present = true;
        self.revision = revision;
        self.memory = Some(self.reserved.local_copy());
        Ok(())
    }

    fn empty(&self) -> bool {
        !self.present && self.schema == SCHEMA && self.revision == 0 && self.memory.is_none()
    }
}

/// 仅本固定状态使用的外持有收尾槽；字段/来源不公开，没有JSON恢复或任意SQL回调。
/// facade必须在账号/Db锁外构造并销毁，read返回的快照先留此处，COMMIT失败也不锁内drop。
pub(in crate::server::core::account_store) struct RetainedValues {
    raw: RawSlot,
    written: RawSlot,
    fixed: Option<FixedState>,
    after: Option<FixedState>,
    reading: Option<Snapshot>,
    desired: Option<Memory>,
    entry: Option<Value>,
    displaced: Option<Value>,
    displaced_container: Option<Value>,
    output: envelope::Envelope,
}

impl RetainedValues {
    pub(in crate::server::core::account_store) fn for_read() -> Self {
        Self { raw: RawSlot::default(), written: RawSlot::default(), fixed: None, after: None, reading: None,
            desired: None, entry: None, displaced: None, displaced_container: None, output: envelope::Envelope::default() }
    }

    pub(in crate::server::core::account_store) fn for_unknown(snapshot: &Snapshot) -> Result<Self, StateError> {
        Self::prepared(snapshot, snapshot.reserved.local_copy())
    }

    pub(in crate::server::core::account_store) fn for_official(
        snapshot: &Snapshot, fact: &OfficialDailyFact,
    ) -> Result<Self, StateError> {
        Self::prepared(snapshot, Memory::from_official(&snapshot.reserved, fact)?)
    }

    fn prepared(snapshot: &Snapshot, memory: Memory) -> Result<Self, StateError> {
        let revision = snapshot.revision.checked_add(1).ok_or_else(|| state_error(StateFailure::RevisionExhausted))?;
        let mut retained = Self::for_read();
        let value = memory.value();
        retained_json::check(&value, MAX_VALUE_BYTES)?;
        retained.entry = Some(json!({ "schema": SCHEMA, "revision": revision, "value": value }));
        retained.desired = Some(memory);
        // 输出缓冲在锁外预留原上限，编码/加密追加不丢弃旧明文分配。
        retained.output.reserve_write();
        Ok(retained)
    }

    pub(in crate::server::core::account_store) fn take_read(&mut self) -> Option<Snapshot> { self.reading.take() }
}

#[must_use]
pub(in crate::server::core::account_store) enum Outcome<T> {
    NotEntered { error: StateError, storage_closed: bool },
    Rejected { error: StateError, storage_closed: bool },
    CommitUnknown { error: StateError, storage_closed: bool },
    Committed(T),
}

pub(in crate::server::core::account_store) fn read(
    conn: &mut Connection, key: &[u8; 32], stamp: &RowStamp,
    retained: &mut RetainedValues, attempt: &CommitAttempt,
) -> Outcome<()> {
    if stamp.provider() != RowProvider::DoubaoWork {
        return Outcome::NotEntered { error: state_error(StateFailure::WrongProvider), storage_closed: true };
    }
    let tx = match transaction::begin(conn, attempt) { Ok(tx) => tx, Err(outcome) => return outcome };
    let result = (|| {
        record::load_raw_into(&tx, key, stamp, &mut retained.raw)?;
        retained.raw.verify(&tx, stamp)?;
        retained.fixed = Some(record::fixed_state(&retained.raw.value)?);
        let fixed = retained.fixed.as_mut().ok_or_else(|| state_error(StateFailure::Unavailable))?;
        let reserved = Memory::reserve()?;
        retained.reading = Some(Snapshot { present: fixed.present, schema: fixed.schema, revision: fixed.revision,
            memory: fixed.memory.take(), reserved });
        Ok(())
    })();
    transaction::finish(tx, result, attempt)
}

pub(in crate::server::core::account_store) fn save(
    conn: &mut Connection, key: &[u8; 32], stamp: &RowStamp, snapshot: &Snapshot,
    retained: &mut RetainedValues, attempt: &CommitAttempt,
) -> Outcome<u64> {
    write(conn, key, stamp, snapshot, retained, attempt, WriteKind::Unknown)
}

pub(in crate::server::core::account_store) fn save_official(
    conn: &mut Connection, key: &[u8; 32], stamp: &RowStamp, snapshot: &Snapshot, fact: &OfficialDailyFact,
    retained: &mut RetainedValues, attempt: &CommitAttempt,
) -> Outcome<u64> {
    write(conn, key, stamp, snapshot, retained, attempt, WriteKind::Official(fact))
}

enum WriteKind<'a> { Unknown, Official(&'a OfficialDailyFact) }

fn write(
    conn: &mut Connection, key: &[u8; 32], stamp: &RowStamp, snapshot: &Snapshot,
    retained: &mut RetainedValues, attempt: &CommitAttempt, kind: WriteKind<'_>,
) -> Outcome<u64> {
    if stamp.provider() != RowProvider::DoubaoWork {
        return Outcome::NotEntered { error: state_error(StateFailure::WrongProvider), storage_closed: true };
    }
    let tx = match transaction::begin(conn, attempt) { Ok(tx) => tx, Err(outcome) => return outcome };
    let result = (|| {
        record::load_raw_into(&tx, key, stamp, &mut retained.raw)?;
        retained.raw.verify(&tx, stamp)?;
        retained.fixed = Some(record::fixed_state(&retained.raw.value)?);
        let fixed = retained.fixed.as_ref().ok_or_else(|| state_error(StateFailure::Unavailable))?;
        if fixed.present != snapshot.present || fixed.schema != snapshot.schema
            || fixed.revision != snapshot.revision || fixed.memory != snapshot.memory {
            return Err(state_error(StateFailure::StateChanged));
        }
        let desired = retained.desired.as_ref().ok_or_else(|| state_error(StateFailure::CandidateChanged))?;
        match kind {
            WriteKind::Unknown => {
                if fixed.memory.as_ref().is_some_and(|memory| memory.day == snapshot.reserved.day && memory.official.is_some())
                    || desired.official.is_some() { return Err(state_error(StateFailure::OfficialAlreadyConfirmed)); }
                if desired != &snapshot.reserved { return Err(state_error(StateFailure::CandidateChanged)); }
            }
            WriteKind::Official(fact) => {
                if !desired.matches_official(fact) || desired.receipt_id != snapshot.reserved.receipt_id
                    || desired.snapshot_at != snapshot.reserved.snapshot_at || desired.day != snapshot.reserved.day {
                    return Err(state_error(StateFailure::CandidateChanged));
                }
            }
        }
        let revision = snapshot.revision.checked_add(1).ok_or_else(|| state_error(StateFailure::RevisionExhausted))?;
        // desired只借这个字段，其它工作槽分别借用，不能通过整个retained丢弃原快照/事实。
        let raw = &mut retained.raw.value;
        record::put_fixed(raw, &mut retained.entry, &mut retained.displaced, &mut retained.displaced_container)?;
        record::update_raw(&tx, key, stamp, raw, &mut retained.output)?;
        record::load_raw_into(&tx, key, stamp, &mut retained.written)?;
        if &retained.written.value != raw { return Err(Error::Storage(StorageFailure::Write).into()); }
        retained.written.verify(&tx, stamp)?;
        retained.after = Some(record::fixed_state(&retained.written.value)?);
        let after = retained.after.as_ref().ok_or_else(|| state_error(StateFailure::Unavailable))?;
        if !after.present || after.schema != SCHEMA || after.revision != revision
            || after.memory.as_ref() != Some(desired) {
            return Err(Error::Storage(StorageFailure::Write).into());
        }
        Ok(revision)
    })();
    transaction::finish(tx, result, attempt)
}

pub(in crate::server::core::account_store) fn public_status(entry: &Value) -> Option<Value> {
    let (_, schema) = record::entry_header(entry).ok()?;
    if schema != SCHEMA { return None; }
    let memory = Memory::read(&entry["value"]).ok()?;
    if memory.day != crate::server::core::beijing::today_key() { return None; }
    let completed = memory.official.is_some();
    let at = memory.official.as_ref().map_or(memory.snapshot_at, |task| task.observed_at);
    Some(json!({
        "at": at, "completed": completed, "completedCount": u8::from(completed), "totalCount": 1,
        "sourcesComplete": completed, "status": if completed { "done" } else { "confirming" },
        "queryOnly": true, "retryAt": null,
        "message": if completed { "豆包工作今日签到已由官方确认；本次收益未查询，不补零或猜测奖励" }
            else { "豆包工作原签到结果仍待确认；本次仅回查，没有再次领取，原待确认记录保持不变" },
    }))
}

fn state_error(reason: StateFailure) -> StateError { StateError::State(reason) }
