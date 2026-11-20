use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::monitor::Resources;
use super::{AcquireError, CapacityRejection, Class, Limits, Manager, RejectCounts, RejectReason};

#[derive(Default)]
pub(super) struct State {
    pub active: [usize; 7],
    pub memory: usize,
    pub waiters: Vec<Waiter>,
    pub queue_bytes: usize,
    pub rejected: u64,
    pub reject_counts: RejectCounts,
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

fn block_reason(state: &State, class: Class, provider: &str, limits: &Limits) -> Option<RejectReason> {
    if limits.memory_pressure
        && matches!(class, Class::Conversation | Class::Image | Class::Background | Class::Request | Class::ImageRequest)
    {
        return Some(RejectReason::MemoryPressure);
    }
    if limits.cpu_pressure
        && matches!(class, Class::Image | Class::Background | Class::ImageRequest)
        && limits.for_class(class) == 0
    {
        return Some(RejectReason::CpuPressure);
    }
    if state.active[class.index()] >= limits.for_class(class) {
        return Some(RejectReason::ConcurrencyLimit);
    }
    let count = state.providers.get(&(class.index(), provider.to_string())).copied().unwrap_or(0);
    if class == Class::Background && count >= limits.background_per_provider {
        return Some(RejectReason::ProviderLimit);
    }
    if matches!(class, Class::Conversation | Class::Image) {
        let settings = crate::server::config::runtime_capacity::current();
        if settings.settings.provider_limits.get(provider).is_some_and(|limit| count >= *limit) {
            return Some(RejectReason::ProviderLimit);
        }
    }
    None
}

fn selected(state: &State, class: Class, limits: &Limits, now: Instant) -> Option<u64> {
    state.waiters.iter().filter(|waiter| waiter.class == class && waiter.deadline > now
        && block_reason(state, class, &waiter.provider, limits).is_none()).min_by_key(|waiter| {
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

fn remove_waiter(state: &mut State, id: u64) {
    if let Some(index) = state.waiters.iter().position(|waiter| waiter.id == id) {
        let waiter = state.waiters.remove(index);
        state.queue_bytes = state.queue_bytes.saturating_sub(waiter.bytes);
    }
}

fn record_rejection(
    state: &mut State,
    reason: RejectReason,
    class: Class,
    waited: Duration,
    limits: &Limits,
    resources: Resources,
) -> CapacityRejection {
    state.rejected = state.rejected.saturating_add(1);
    state.reject_counts.record(reason);
    CapacityRejection {
        reason,
        class,
        waited_ms: waited.as_millis() as u64,
        active: state.active[class.index()],
        limit: limits.for_class(class),
        queued: state.waiters.len(),
        queue_limit: limits.queue_requests,
        queue_bytes: state.queue_bytes,
        queue_bytes_limit: limits.queue_bytes,
        available_bytes: resources.available_bytes,
        total_bytes: resources.total_bytes,
        used_bytes: resources.container_bytes.unwrap_or(resources.process_bytes),
    }
}

pub async fn acquire_detailed(class: Class, provider: &str, caller: &str, bytes: usize, deadline: Instant,
    cancel: &CancellationToken) -> Result<Ticket, AcquireError> {
    let manager = super::manager().clone();
    let started = Instant::now();
    if cancel.is_cancelled() { return Err(AcquireError::Cancelled); }
    let (limits, resources) = super::snapshot_inputs();
    if started >= deadline {
        let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
        let reason = block_reason(&state, class, provider, &limits).unwrap_or(RejectReason::QueueTimeout);
        return Err(AcquireError::Rejected(record_rejection(&mut state, reason, class, Duration::ZERO, &limits, resources)));
    }
    let mut last_reason;
    let id = {
        let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
        if cancel.is_cancelled() { return Err(AcquireError::Cancelled); }
        let blocked = block_reason(&state, class, provider, &limits);
        if blocked.is_none() && selected(&state, class, &limits, Instant::now()).is_none() {
            return Ok(enter(&mut state, manager.clone(), class, provider.into(), caller.into()));
        }
        last_reason = blocked;
        let reservation = bytes.saturating_add(provider.len()).saturating_add(caller.len()).saturating_add(256);
        let queue_reason = if state.waiters.len() >= limits.queue_requests {
            Some(RejectReason::QueueFull)
        } else if state.queue_bytes.saturating_add(reservation) > limits.queue_bytes {
            Some(RejectReason::QueueMemoryFull)
        } else { None };
        if let Some(reason) = queue_reason {
            return Err(AcquireError::Rejected(record_rejection(&mut state, reason, class, started.elapsed(), &limits, resources)));
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
        let (limits, resources) = super::snapshot_inputs();
        {
            let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(blocked) = block_reason(&state, class, provider, &limits) {
                last_reason = Some(blocked);
            }
            if Instant::now() >= deadline {
                let reason = block_reason(&state, class, provider, &limits).or(last_reason).unwrap_or(RejectReason::QueueTimeout);
                let rejection = record_rejection(&mut state, reason, class, started.elapsed(), &limits, resources);
                remove_waiter(&mut state, id);
                return Err(AcquireError::Rejected(rejection));
            }
            if !cancel.is_cancelled() && selected(&state, class, &limits, Instant::now()) == Some(id) {
                let Some(index) = state.waiters.iter().position(|waiter| waiter.id == id) else {
                    let reason = block_reason(&state, class, provider, &limits).or(last_reason).unwrap_or(RejectReason::QueueTimeout);
                    return Err(AcquireError::Rejected(record_rejection(&mut state, reason, class, started.elapsed(), &limits, resources)));
                };
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
            _ = cancel.cancelled() => return Err(AcquireError::Cancelled),
            _ = tokio::time::sleep_until(deadline) => {
                let (limits, resources) = super::snapshot_inputs();
                let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
                let reason = block_reason(&state, class, provider, &limits).or(last_reason).unwrap_or(RejectReason::QueueTimeout);
                let rejection = record_rejection(&mut state, reason, class, started.elapsed(), &limits, resources);
                remove_waiter(&mut state, id);
                return Err(AcquireError::Rejected(rejection));
            }
            _ = &mut wake => {},
            _ = tokio::time::sleep(Duration::from_millis(250)) => {},
        }
    }
}

pub fn try_acquire(class: Class, provider: &str) -> Result<Ticket, proto::WireError> {
    let manager = super::manager().clone();
    let (limits, resources) = super::snapshot_inputs();
    let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(reason) = block_reason(&state, class, provider, &limits) {
        return Err(record_rejection(&mut state, reason, class, Duration::ZERO, &limits, resources).into_wire_error());
    }
    Ok(enter(&mut state, manager.clone(), class, provider.into(), super::caller()))
}

pub fn record_slot_rejection(class: Class, waited: Duration) -> CapacityRejection {
    let manager = super::manager().clone();
    let (limits, resources) = super::snapshot_inputs();
    let mut state = manager.state.lock().unwrap_or_else(|error| error.into_inner());
    record_rejection(&mut state, RejectReason::PluginSlotLimit, class, waited, &limits, resources)
}
