use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_CHANGES: usize = 4096;

#[derive(Default)]
struct Journal {
    revision: u64,
    reset_at: u64,
    rows: VecDeque<(u64, i64)>,
}

#[derive(Default)]
pub(super) struct AccountRevisions {
    revision: AtomicU64,
    journal: Mutex<Journal>,
}

pub(crate) struct AccountChanges {
    pub revision: u64,
    pub rows: Option<Vec<i64>>,
}

impl AccountRevisions {
    pub fn current(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    // 钩子在 SQL 提交前触发；回滚也保留失效信号，读取方只缓存锁外取得的已提交快照。
    pub fn changed(&self, rowid: i64) {
        let mut journal = self.journal.lock().unwrap_or_else(|error| error.into_inner());
        journal.revision = journal.revision.wrapping_add(1);
        let revision = journal.revision;
        journal.rows.push_back((revision, rowid));
        while journal.rows.len() > MAX_CHANGES {
            if let Some((revision, _)) = journal.rows.pop_front() {
                journal.reset_at = revision;
            }
        }
        self.revision.store(revision, Ordering::Release);
    }

    pub fn invalidate(&self) {
        let mut journal = self.journal.lock().unwrap_or_else(|error| error.into_inner());
        journal.revision = journal.revision.wrapping_add(1);
        journal.reset_at = journal.revision;
        journal.rows.clear();
        self.revision.store(journal.revision, Ordering::Release);
    }

    pub fn since(&self, revision: Option<u64>) -> AccountChanges {
        let journal = self.journal.lock().unwrap_or_else(|error| error.into_inner());
        let rows = revision.filter(|revision| *revision >= journal.reset_at)
            .map(|revision| journal.rows.iter()
                .filter(|(at, _)| *at > revision)
                .map(|(_, rowid)| *rowid)
                .collect::<BTreeSet<_>>().into_iter().collect());
        AccountChanges { revision: journal.revision, rows }
    }
}
