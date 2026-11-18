use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Class, Limits, Manager};

#[derive(Default)]
pub(super) struct State {
    pub active: [usize; 7],
    pub memory: usize,
    pub waiters: Vec<Waiter>,
    pub queue_bytes: usize,
    pub rejected: u64,
    sequence: u64,
    providers: HashMap<(usize, String), usize>,
    callers: HashMap<(usize, String), usize>,
}

pub(super) struct Waiter {
    id: u64,
    class: Class,
    provider: String,
    caller: String,
    bytes: usize,
    deadline: Instant,
}

pub struct Ticket {
    manager: Arc<Manager>,
    class: Class,
    provider: String,
    caller: String,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        let index = self.class.index();
        state.active[index] = state.active[index].saturating_sub(1);
        decrement(&mut state.providers, (index, self.provider.clone()));
        decrement(&mut state.callers, (index, self.caller.clone()));
        drop(state);
        self.manager.changed.notify_waiters();
    }
}

fn decrement(table: &mut HashMap<(usize, String), usize>, key: (usize, String)) {
    if let Some(count) = table.get_mut(&key) {
        *count = count.saturating_sub(1);
        if *count == 0 { table.remove(&key); }
    }
}

struct Waiting { manager: Arc<Manager>, id: u64 }
impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(index) = state.waiters.iter().position(|waiter| waiter.id == self.id) {
            let waiter = state.waiters.remove(index);
            state.queue_bytes = state.queue_bytes.saturating_sub(waiter.bytes);
        }
        drop(state);
        self.manager.changed.notify_waiters();
    }
}

fn allowed(state: &State, class: Class, provider: &str, limits: &Limits) -> bool {
    if state.active[class.index()] >= limits.for_class(class) { return false; }
    if limits.memory_pressure && matches!(class, Class::Conversation | Class::Image | Class::Background | Class::Request | Class::ImageRequest) { return false; }
    let count = state.providers.get(&(class.index(), provider.to_string())).copied().unwrap_or(0);
    if class == Class::Background && count >= limits.background_per_provider { return false; }
    if matches!(class, Class::Conversation | Class::Image) {
        let settings = crate::server::config::runtime_capacity::current();
        if settings.settings.provider_limits.get(provider).is_some_and(|limit| count >= *limit) { return false; }
    }
    true
}

fn selected(state: &State, class: Class, limits: &Limits, now: Instant) -> Option<u64> {
    state.waiters.iter().filter(|waiter| waiter.class == class && waiter.deadline > now
        && allowed(state, class, &waiter.provider, limits)).min_by_key(|waiter| {
        (state.callers.get(&(class.index(), waiter.caller.clone())).copied().unwrap_or(0), waiter.id)
    }).map(|waiter| waiter.id)
}

fn enter(state: &mut State, manager: Arc<Manager>, class: Class, provider: String, caller: String) -> Ticket {
    let index = class.index();
    state.active[index] += 1;
    *state.providers.entry((index, provider.clone())).or_insert(0) += 1;
    *state.callers.entry((index, caller.clone())).or_insert(0) += 1;
    Ticket { manager, class, provider, caller }
}

pub async fn acquire(class: Class, provider: &str, caller: &str, bytes: usize, deadline: Instant,
    cancel: &CancellationToken) -> Result<Ticket, proto::WireError> {
    let manager = super::manager().clone();
    let now = Instant::now();
    if cancel.is_cancelled() { return Err(proto::WireError::with_code("容量等待已取消", proto::WireError::ABORTED)); }
    if now >= deadline { return Err(super::busy()); }
    let limits = super::limits();
    let id = {
        let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
        if cancel.is_cancelled() { return Err(proto::WireError::with_code("容量等待已取消", proto::WireError::ABORTED)); }
        if allowed(&state, class, provider, &limits) && selected(&state, class, &limits, Instant::now()).is_none() {
            return Ok(enter(&mut state, manager.clone(), class, provider.into(), caller.into()));
        }
        let reservation = bytes.saturating_add(provider.len()).saturating_add(caller.len()).saturating_add(256);
        if state.waiters.len() >= limits.queue_requests || state.queue_bytes.saturating_add(reservation) > limits.queue_bytes {
            state.rejected = state.rejected.saturating_add(1);
            return Err(super::busy());
        }
        state.sequence = state.sequence.wrapping_add(1);
        let id = state.sequence;
        state.waiters.push(Waiter { id, class, provider: provider.into(), caller: caller.into(), bytes: reservation, deadline });
        state.queue_bytes += reservation;
        id
    };
    let waiting = Waiting { manager: manager.clone(), id };
    loop {
        let wake = manager.changed.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let limits = super::limits();
        {
            let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
            if Instant::now() >= deadline {
                state.rejected = state.rejected.saturating_add(1);
                return Err(super::busy());
            }
            if !cancel.is_cancelled() && selected(&state, class, &limits, Instant::now()) == Some(id) {
                let index = state.waiters.iter().position(|waiter| waiter.id == id).ok_or_else(super::busy)?;
                let waiter = state.waiters.remove(index);
                state.queue_bytes = state.queue_bytes.saturating_sub(waiter.bytes);
                let ticket = enter(&mut state, manager.clone(), class, waiter.provider, waiter.caller);
                drop(state);
                drop(waiting);
                manager.changed.notify_waiters();
                return Ok(ticket);
            }
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(proto::WireError::with_code("容量等待已取消", proto::WireError::ABORTED)),
            _ = tokio::time::sleep_until(deadline) => return Err(super::busy()),
            _ = &mut wake => {},
            _ = tokio::time::sleep(Duration::from_millis(250)) => {},
        }
    }
}

pub fn try_acquire(class: Class, provider: &str) -> Result<Ticket, proto::WireError> {
    let manager = super::manager().clone();
    let limits = super::limits();
    let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
    if !allowed(&state, class, provider, &limits) {
        state.rejected = state.rejected.saturating_add(1);
        return Err(super::busy());
    }
    Ok(enter(&mut state, manager.clone(), class, provider.into(), super::caller()))
}
