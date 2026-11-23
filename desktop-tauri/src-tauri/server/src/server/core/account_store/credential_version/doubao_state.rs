//! 固定豆包每日确认记忆；只写宿主host:键，不授予签到发送/领取/任意状态权限。
//! SnapshotReceipt与DailySource持同一个私有Arc<RowSource>；官方事实只能消费原Empty快照与typed fact。
//! Unknown保留旧格式；Official仅表示官网原当日任务已完成，原执行仍未知，无金额/重发许可。
//! 本层异常/原新明文/快照/Fact持至原账号与Db锁外收尾，不承诺exactly-once或设备外耐久。

pub(in crate::server::core) mod rewards;

use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{atomic::{AtomicBool, Ordering}, Arc},
};

use super::{cached_key, RowProvider, RowSource};
use super::super::sql::credential_version::doubao_state as state_sql;
use crate::server::{core::plugin_host::wasm::OfficialDailyFact, db::CredentialSourceError};

pub(crate) use state_sql::{OfficialView, StateError, StateFailure, StateKind, StateView, UnknownView};

struct SnapshotIdentity { daily_source_issued: AtomicBool }

#[must_use]
pub(crate) struct DailySource { original: Arc<RowSource>, identity: Arc<SnapshotIdentity> }
impl DailySource { pub(crate) fn account(&self) -> &RowSource { self.original.as_ref() } }

/// 唯一真实读取回执；不提供Clone/serde、Db/version getter或从view恢复来源的入口。
#[must_use]
pub(crate) struct SnapshotReceipt {
    original: Arc<RowSource>,
    snapshot: state_sql::Snapshot,
    identity: Arc<SnapshotIdentity>,
    pending_reserved: bool,
}

#[must_use]
pub(crate) struct UnknownCandidate { identity: Arc<SnapshotIdentity> }

enum WriteTarget<'a> { Unknown, Official(&'a OfficialDailyFact), Clear }

/// 只表明本地固定记忆的COMMIT；官方分支也不表示本次签到/领取/金额或发送成功。
#[must_use]
pub(crate) struct CommitReceipt { revision: u64, _snapshot: Arc<SnapshotIdentity> }
impl CommitReceipt {
    pub(crate) fn revision(&self) -> u64 { self.revision }
    pub(crate) fn updated(&self) -> bool { true }
}

#[must_use]
pub(crate) enum SaveOutcome {
    NotEntered { error: StateError, storage_closed: bool },
    Rejected { error: StateError, storage_closed: bool },
    CommitUnknown { error: StateError, storage_closed: bool },
    Committed(CommitReceipt),
}

#[must_use]
pub(crate) enum SnapshotOutcome {
    NotEntered { error: StateError, storage_closed: bool },
    Rejected { error: StateError, storage_closed: bool },
    CommitUnknown { error: StateError, storage_closed: bool },
    Read(SnapshotReceipt),
}

impl RowSource {
    /// 同原来源读取，读取事务只返回小标量；真实快照先留外持有槽至整个Db借用结束。
    pub(crate) fn read_doubao_state(self) -> SnapshotOutcome {
        if self.provider() != RowProvider::DoubaoWork {
            return SnapshotOutcome::NotEntered { error: StateError::State(StateFailure::WrongProvider), storage_closed: true };
        }
        let key = match cached_key() {
            Ok(key) => key,
            Err(error) => return SnapshotOutcome::NotEntered { error: StateError::Account(error), storage_closed: true },
        };
        let mut retained = state_sql::RetainedValues::for_read();
        let attempt = state_sql::CommitAttempt::new();
        let mut observed = None;
        let mut payload: Option<Box<dyn Any + Send>> = None;
        let work = catch_unwind(AssertUnwindSafe(|| {
            let _account = self.store.guard();
            let db = self.store.database().ok_or(CredentialSourceError::Unavailable)?;
            let _ = db.commit_credential_source(&self.source, |conn| {
                match catch_unwind(AssertUnwindSafe(|| state_sql::read(conn, &key, &self.inner, &mut retained, &attempt))) {
                    Ok(outcome) => { observed = Some(outcome); }
                    Err(error) => { payload = Some(error); }
                }
                Ok::<_, std::convert::Infallible>(())
            })?;
            Ok::<_, CredentialSourceError>(())
        }));
        // 到这里原账号/Db锁和隔离检查已结束，才拆异常/失败回执或最初快照。
        let source = match work {
            Ok(source) if payload.is_none() => source,
            Ok(_) => Err(CredentialSourceError::Unavailable),
            Err(error) => { drop(error); Err(CredentialSourceError::Unavailable) }
        };
        drop(payload);
        let storage_closed = observed.as_ref().is_some_and(closed);
        if let Err(error) = source {
            let (error, _) = source_error(error);
            return read_failure(&attempt, error, storage_closed);
        }
        match observed {
            Some(state_sql::Outcome::Committed(())) => match retained.take_read() {
                Some(snapshot) => SnapshotOutcome::Read(SnapshotReceipt { original: Arc::new(self), snapshot,
                    identity: Arc::new(SnapshotIdentity { daily_source_issued: AtomicBool::new(false) }), pending_reserved: false }),
                None => SnapshotOutcome::CommitUnknown { error: StateError::State(StateFailure::Unavailable), storage_closed: true },
            },
            Some(state_sql::Outcome::NotEntered { error, storage_closed }) => SnapshotOutcome::NotEntered { error, storage_closed },
            Some(state_sql::Outcome::Rejected { error, storage_closed }) => SnapshotOutcome::Rejected { error, storage_closed },
            Some(state_sql::Outcome::CommitUnknown { error, storage_closed }) => SnapshotOutcome::CommitUnknown { error, storage_closed },
            None => read_failure(&attempt, StateError::State(StateFailure::Unavailable), false),
        }
    }
}

impl SnapshotReceipt {
    pub(crate) fn state(&self) -> StateView<'_> { self.snapshot.view() }

    pub(crate) fn pending_reserved(&self) -> bool { self.pending_reserved }

    pub(crate) fn pending_today(&self) -> bool {
        self.state().unknown.as_ref().is_some_and(|view| view.day == self.snapshot.current_day())
    }

    /// 旧Unknown/Official均不取得Empty票据；失败/取消不退还一次机会。
    pub(crate) fn take_daily_source(&self) -> Result<DailySource, StateError> {
        if self.original.provider() != RowProvider::DoubaoWork { return Err(StateError::State(StateFailure::WrongProvider)); }
        if self.state().schema != 1 { return Err(StateError::State(StateFailure::OfficialConfirmationUnavailable)); }
        self.identity.daily_source_issued.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| StateError::State(StateFailure::CandidateChanged))?;
        Ok(DailySource { original: self.original.clone(), identity: self.identity.clone() })
    }

    pub(crate) fn matches_daily_source(&self, source: &DailySource) -> bool {
        Arc::ptr_eq(&self.original, &source.original) && Arc::ptr_eq(&self.identity, &source.identity)
            && self.identity.daily_source_issued.load(Ordering::Acquire)
    }

    pub(crate) fn unknown_candidate(&self) -> UnknownCandidate { UnknownCandidate { identity: self.identity.clone() } }

    pub(crate) fn matches_doubao_account(&self, account: &str) -> bool {
        self.original.provider() == RowProvider::DoubaoWork && self.original.account() == account
    }

    /// 未知保存不降级已Official记忆；原快照版本冲突不重读/替换或按日期清状态。
    pub(crate) fn save_unknown(self, candidate: UnknownCandidate) -> SaveOutcome {
        if !Arc::ptr_eq(&self.identity, &candidate.identity) {
            return refused(StateFailure::CandidateChanged);
        }
        if self.state().official.as_ref().is_some_and(|view| view.day == self.snapshot.current_day()) {
            return refused(StateFailure::OfficialAlreadyConfirmed);
        }
        self.write(WriteTarget::Unknown)
    }

    /// 仅在本次签到明确被官方拒绝且回查未完成时撤回本轮暂存的 Pending 记录。
    pub(crate) fn clear_pending(self, candidate: UnknownCandidate) -> SaveOutcome {
        if !self.pending_reserved || !Arc::ptr_eq(&self.identity, &candidate.identity) {
            return refused(StateFailure::CandidateChanged);
        }
        if self.state().official.as_ref().is_some_and(|view| view.day == self.snapshot.current_day()) {
            return refused(StateFailure::OfficialAlreadyConfirmed);
        }
        self.write(WriteTarget::Clear)
    }

    /// 唯一固定官方保存入口，按值消费真实Fact与原Empty快照；无bool/JSON/view构造旁路。
    /// Fact持到真实保存/异常锁外收尾，COMMIT只表示原当日任务官网状态已记住。
    pub(in crate::server::core) fn save_official(self, fact: OfficialDailyFact) -> SaveOutcome {
        if !fact.matches_snapshot(&self) { return refused(StateFailure::CandidateChanged); }
        self.write(WriteTarget::Official(&fact))
    }

    pub(in crate::server::core) fn checkpoint_pending(&mut self) -> Result<(), StateError> {
        if self.state().unknown.as_ref().is_some_and(|view| view.day == self.snapshot.current_day())
            || self.state().official.as_ref().is_some_and(|view| view.day == self.snapshot.current_day()) {
            return Err(StateError::State(StateFailure::CandidateChanged));
        }
        match self.write(WriteTarget::Unknown) {
            SaveOutcome::Committed(receipt) => {
                self.pending_reserved = true;
                self.snapshot.advance_unknown(receipt.revision())
            }
            SaveOutcome::CommitUnknown { .. } => {
                self.pending_reserved = true;
                Err(StateError::State(StateFailure::Unavailable))
            }
            SaveOutcome::NotEntered { .. } | SaveOutcome::Rejected { .. } => Err(StateError::State(StateFailure::Unavailable)),
        }
    }

    fn write(&self, target: WriteTarget<'_>) -> SaveOutcome {
        let key = match cached_key() {
            Ok(key) => key,
            Err(error) => return SaveOutcome::NotEntered { error: StateError::Account(error), storage_closed: true },
        };
        let prepared = match target {
            WriteTarget::Official(fact) => state_sql::RetainedValues::for_official(&self.snapshot, fact),
            WriteTarget::Unknown => state_sql::RetainedValues::for_unknown(&self.snapshot),
            WriteTarget::Clear => state_sql::RetainedValues::for_clear(&self.snapshot),
        };
        let mut retained = match prepared {
            Ok(retained) => retained,
            Err(error) => return SaveOutcome::NotEntered { error, storage_closed: true },
        };
        let attempt = state_sql::CommitAttempt::new();
        let mut observed = None;
        let mut payload: Option<Box<dyn Any + Send>> = None;
        let work = catch_unwind(AssertUnwindSafe(|| {
            let _account = self.original.store.guard();
            let db = self.original.store.database().ok_or(CredentialSourceError::Unavailable)?;
            let _ = db.commit_credential_source(&self.original.source, |conn| {
                let action = catch_unwind(AssertUnwindSafe(|| match target {
                    WriteTarget::Official(fact) => state_sql::save_official(conn, &key, &self.original.inner, &self.snapshot, fact, &mut retained, &attempt),
                    WriteTarget::Unknown => state_sql::save(conn, &key, &self.original.inner, &self.snapshot, &mut retained, &attempt),
                    WriteTarget::Clear => state_sql::clear(conn, &key, &self.original.inner, &self.snapshot, &mut retained, &attempt),
                }));
                match action { Ok(outcome) => observed = Some(outcome), Err(error) => payload = Some(error) }
                Ok::<_, std::convert::Infallible>(())
            })?;
            Ok::<_, CredentialSourceError>(())
        }));
        // observed/retained/fact/self全在锁外所有者中；原Db来源复核失败不在Db锁内丢真实结果。
        let source = match work {
            Ok(source) if payload.is_none() => source,
            Ok(_) => Err(CredentialSourceError::Unavailable),
            Err(error) => { drop(error); Err(CredentialSourceError::Unavailable) }
        };
        drop(payload);
        let storage_closed = observed.as_ref().is_some_and(closed);
        if let Err(error) = source {
            let (error, _) = source_error(error);
            return save_failure(&attempt, error, storage_closed);
        }
        match observed {
            Some(state_sql::Outcome::Committed(revision)) => SaveOutcome::Committed(CommitReceipt { revision, _snapshot: self.identity.clone() }),
            Some(state_sql::Outcome::NotEntered { error, storage_closed }) => SaveOutcome::NotEntered { error, storage_closed },
            Some(state_sql::Outcome::Rejected { error, storage_closed }) => SaveOutcome::Rejected { error, storage_closed },
            Some(state_sql::Outcome::CommitUnknown { error, storage_closed }) => SaveOutcome::CommitUnknown { error, storage_closed },
            None => save_failure(&attempt, StateError::State(StateFailure::Unavailable), false),
        }
    }
}

fn empty(state: &StateView<'_>) -> bool {
    !state.present && state.schema == 1 && state.revision == 0 && state.kind == StateKind::Empty
        && state.unknown.is_none() && state.official.is_none()
}
fn closed<T>(outcome: &state_sql::Outcome<T>) -> bool {
    match outcome {
        state_sql::Outcome::Committed(_) => true,
        state_sql::Outcome::NotEntered { storage_closed, .. } | state_sql::Outcome::Rejected { storage_closed, .. }
        | state_sql::Outcome::CommitUnknown { storage_closed, .. } => *storage_closed,
    }
}
fn save_failure(attempt: &state_sql::CommitAttempt, error: StateError, storage_closed: bool) -> SaveOutcome {
    if attempt.attempted() { SaveOutcome::CommitUnknown { error, storage_closed } }
    else if attempt.entered() { SaveOutcome::Rejected { error, storage_closed } }
    else { SaveOutcome::NotEntered { error, storage_closed } }
}
fn read_failure(attempt: &state_sql::CommitAttempt, error: StateError, storage_closed: bool) -> SnapshotOutcome {
    if attempt.attempted() { SnapshotOutcome::CommitUnknown { error, storage_closed } }
    else if attempt.entered() { SnapshotOutcome::Rejected { error, storage_closed } }
    else { SnapshotOutcome::NotEntered { error, storage_closed } }
}
fn refused(failure: StateFailure) -> SaveOutcome {
    SaveOutcome::NotEntered { error: StateError::State(failure), storage_closed: true }
}
fn source_error(error: CredentialSourceError) -> (StateError, bool) {
    match error {
        CredentialSourceError::Unavailable => (StateError::State(StateFailure::Unavailable), false),
        CredentialSourceError::Changed => (StateError::State(StateFailure::SourceChanged), false),
    }
}
