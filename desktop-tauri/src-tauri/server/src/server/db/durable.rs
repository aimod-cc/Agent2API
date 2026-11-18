use std::sync::atomic::Ordering;

use super::{codearts_slot::{full_transaction, CodeArtsTransaction}, Db};

pub(crate) enum Durable<T, E> {
    Committed(T),
    Rejected(E),
    Unconfirmed,
}

impl Db {
    pub(crate) fn plugin_state_transaction<T, E>(
        &self,
        action: impl FnOnce(&rusqlite::Connection) -> Result<T, E>,
    ) -> Durable<T, E> {
        let state = match self.inner.conn.lock() {
            Ok(state) => state,
            Err(_) => return Durable::Unconfirmed,
        };
        if self.inner.task_guard_unusable.load(Ordering::Acquire) { return Durable::Unconfirmed; }
        match full_transaction(&state, &self.inner.task_guard_unusable, action) {
            CodeArtsTransaction::Committed { value, storage_closed: true } => Durable::Committed(value),
            CodeArtsTransaction::Rejected { error, storage_closed: true } => Durable::Rejected(error),
            _ => Durable::Unconfirmed,
        }
    }
}
