use std::sync::{Arc, Mutex};

use agent2api_provider_protocol as proto;

use super::{Manager, MIB};

struct MemoryState {
    manager: Arc<Manager>,
    bytes: Mutex<usize>,
}

impl Drop for MemoryState {
    fn drop(&mut self) {
        let bytes = *self.bytes.lock().unwrap_or_else(|error| error.into_inner());
        let mut state = self.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        state.memory = state.memory.saturating_sub(bytes);
        drop(state);
        self.manager.changed.notify_waiters();
    }
}

#[derive(Clone)]
pub struct MemoryBudget { state: Arc<MemoryState> }

pub struct Allocation { budget: MemoryBudget }

impl Default for MemoryBudget {
    fn default() -> Self { Self::new() }
}

impl MemoryBudget {
    pub fn new() -> Self {
        Self { state: Arc::new(MemoryState { manager: super::manager().clone(), bytes: Mutex::new(0) }) }
    }

    pub fn resize(&self, bytes: usize) -> Result<(), proto::WireError> {
        let limits = super::limits();
        let mut local = self.state.bytes.lock().unwrap_or_else(|error| error.into_inner());
        let mut state = self.state.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        let total = state.memory.saturating_sub(*local).saturating_add(bytes);
        let reserved = (limits.memory_bytes / 16).min(64 * MIB);
        if bytes > *local && total > limits.memory_bytes.saturating_sub(reserved) { return Err(super::exhausted()); }
        state.memory = total;
        *local = bytes;
        Ok(())
    }

    pub fn grow(&self, additional: usize) -> Result<(), proto::WireError> {
        let limits = super::limits();
        let mut local = self.state.bytes.lock().unwrap_or_else(|error| error.into_inner());
        let mut state = self.state.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.memory.saturating_add(additional) > limits.memory_bytes { return Err(super::exhausted()); }
        state.memory += additional;
        *local += additional;
        Ok(())
    }

    pub fn shrink(&self, bytes: usize) {
        let mut local = self.state.bytes.lock().unwrap_or_else(|error| error.into_inner());
        let removed = bytes.min(*local);
        *local -= removed;
        let mut state = self.state.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        state.memory = state.memory.saturating_sub(removed);
        drop(state);
        self.state.manager.changed.notify_waiters();
    }

    pub fn bytes(&self) -> usize { *self.state.bytes.lock().unwrap_or_else(|error| error.into_inner()) }
}

impl Allocation {
    pub fn new(bytes: usize) -> Result<Self, proto::WireError> {
        let budget = MemoryBudget::new();
        budget.grow(bytes)?;
        Ok(Self { budget })
    }

    pub fn bytes(&self) -> usize { self.budget.bytes() }

    pub fn shrink_to(&self, bytes: usize) { self.budget.shrink(self.budget.bytes().saturating_sub(bytes)); }
}
