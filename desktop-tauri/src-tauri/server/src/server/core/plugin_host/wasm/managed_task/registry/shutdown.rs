//! 固定任务scope的两阶段关闭：按typed宿主身份选取，先全量通知再等待最早绝对截止。
//! 本地排空不等于远端回收；abort仅是请求中断，仍以running登记真正移除为准。

use std::sync::{Arc, Mutex};

use tokio::time::{sleep_until, Instant};

use crate::server::core::plugin_host::{wasm::context::Shared, Host};

use super::super::{
    input,
    lifecycle::Record,
    status::{EndReason, Phase, ResultKind, Snapshot},
};
use super::{changed, lock, Scope};

#[derive(Clone, Copy)]
pub(in crate::server::core::plugin_host) enum ShutdownReason {
    HostShutdown,
    RuntimeReplaced,
}

impl ShutdownReason {
    pub(super) fn reason(self) -> EndReason {
        match self {
            Self::HostShutdown => EndReason::HostShutdown,
            Self::RuntimeReplaced => EndReason::RuntimeReplaced,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Window {
    pub(super) deadline: Instant,
}

#[derive(Default)]
pub(super) struct Completed {
    stop_acknowledged: usize,
    remote_unknown: usize,
    deadline_reached: bool,
}

impl Completed {
    pub(super) fn record(&mut self, snapshot: &Snapshot, expired: bool) {
        self.deadline_reached |= expired;
        if snapshot
            .stopping()
            .is_some_and(|stop| stop.kind() == ResultKind::Acknowledged)
        {
            self.stop_acknowledged = self.stop_acknowledged.saturating_add(1);
        } else if snapshot.submitted() {
            self.remote_unknown = self.remote_unknown.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::server::core::plugin_host) struct DrainReport {
    pub(in crate::server::core::plugin_host) unfinished: usize,
    pub(in crate::server::core::plugin_host) deadline_reached: bool,
    pub(in crate::server::core::plugin_host) creating: usize,
    pub(in crate::server::core::plugin_host) owned: usize,
    pub(in crate::server::core::plugin_host) stopping: usize,
    pub(in crate::server::core::plugin_host) stop_acknowledged: usize,
    pub(in crate::server::core::plugin_host) remote_unknown: usize,
}

impl DrainReport {
    pub(in crate::server::core::plugin_host) fn reached_terminal(&self) -> bool {
        self.unfinished == 0
    }
}

struct Held {
    scopes: Vec<Arc<Scope>>,
    records: Vec<Arc<Record>>,
    // 旧Shared也保留到实际worker退场；不携带Session/client或任务编号。
    shared: Vec<Arc<Shared>>,
}

struct State {
    held: Option<Held>,
    final_report: DrainReport,
}

pub(in crate::server::core::plugin_host) struct DrainBatch {
    state: Mutex<State>,
}

/// Host生命周期锁内的第一阶段，只写scope/record结束意图，不碰Shared或全局宿主表。
pub(in crate::server::core::plugin_host) fn signal_host(
    host: &Host,
    reason: ShutdownReason,
    deadline: Instant,
) {
    let table = lock();
    let scopes: Vec<_> = table
        .scopes
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter(|scope| scope.binding.belongs_to_host(host))
        .collect();
    for scope in &scopes {
        scope.stop_at(reason.reason(), deadline);
    }
    for running in table.running.iter().filter(|running| {
        scopes
            .iter()
            .any(|scope| Arc::ptr_eq(scope, running.record.scope()))
    }) {
        running.record.request_stop(
            running
                .record
                .scope()
                .end_reason()
                .unwrap_or(reason.reason()),
        );
    }
    drop(table);
    changed().notify_waiters();
}

pub(in crate::server::core::plugin_host) fn host_batch(
    host: &Host,
    reason: ShutdownReason,
    deadline: Instant,
) -> Arc<DrainBatch> {
    begin(
        |scope| scope.binding.belongs_to_host(host),
        reason,
        deadline,
    )
}

pub(in crate::server::core::plugin_host) fn registered_hosts() -> Vec<Arc<Host>> {
    let table = lock();
    let mut hosts = Vec::new();
    for host in table
        .scopes
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter_map(|scope| scope.binding.shutdown_host())
    {
        if !hosts.iter().any(|other| Arc::ptr_eq(other, &host)) {
            hosts.push(host);
        }
    }
    hosts
}

pub(in crate::server::core::plugin_host) fn all_batch(deadline: Instant) -> Arc<DrainBatch> {
    begin(|_| true, ShutdownReason::HostShutdown, deadline)
}

fn begin(
    select: impl Fn(&Scope) -> bool,
    reason: ShutdownReason,
    deadline: Instant,
) -> Arc<DrainBatch> {
    let held = {
        let table = lock();
        let scopes: Vec<_> = table
            .scopes
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .filter(|scope| select(scope))
            .collect();
        // 同一Table锁阻止新scope/record插入；调用方已先封闭Host或全局准入。
        for scope in &scopes {
            scope.stop_at(reason.reason(), deadline);
        }
        let records: Vec<_> = table
            .running
            .iter()
            .filter(|item| {
                scopes
                    .iter()
                    .any(|scope| Arc::ptr_eq(scope, item.record.scope()))
            })
            .map(|item| item.record.clone())
            .collect();
        for record in &records {
            record.request_stop(record.scope().end_reason().unwrap_or(reason.reason()));
        }
        let mut shared = Vec::new();
        for runtime in scopes
            .iter()
            .filter_map(|scope| scope.binding.shutdown_shared())
        {
            if !shared.iter().any(|other| Arc::ptr_eq(other, &runtime)) {
                shared.push(runtime);
            }
        }
        Held {
            scopes,
            records,
            shared,
        }
    };
    // 不在受控表锁里cancel Shared；更不能从表锁反向取得Host/运行时全局表锁。
    for shared in &held.shared {
        shared.managed_tasks.close_for_shutdown(reason, deadline);
        shared.cancel.cancel();
        shared.verification_slots.close();
    }
    changed().notify_waiters();
    Arc::new(DrainBatch {
        state: Mutex::new(State {
            held: Some(held),
            final_report: DrainReport::default(),
        }),
    })
}

impl DrainBatch {
    pub(in crate::server::core::plugin_host) fn tighten(
        &self,
        reason: ShutdownReason,
        deadline: Instant,
    ) {
        let scopes = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .held
            .as_ref()
            .map(|held| held.scopes.clone())
            .unwrap_or_default();
        {
            let _table = lock();
            for scope in scopes {
                scope.stop_at(reason.reason(), deadline);
            }
        }
        changed().notify_waiters();
    }

    pub(in crate::server::core::plugin_host) fn report(&self) -> DrainReport {
        let (scopes, _records) = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(held) = &state.held else {
                return state.final_report;
            };
            (held.scopes.clone(), held.records.clone())
        };
        let mut report = DrainReport::default();
        {
            let table = lock();
            for scope in &scopes {
                let completed = scope
                    .completed
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                report.stop_acknowledged = report
                    .stop_acknowledged
                    .saturating_add(completed.stop_acknowledged);
                report.remote_unknown = report
                    .remote_unknown
                    .saturating_add(completed.remote_unknown);
                report.deadline_reached |= completed.deadline_reached;
            }
            for running in table.running.iter().filter(|item| {
                scopes
                    .iter()
                    .any(|scope| Arc::ptr_eq(scope, item.record.scope()))
            }) {
                report.unfinished += 1;
                let snapshot = running.record.snapshot();
                report.deadline_reached |= running.record.drain_expired();
                match snapshot.phase() {
                    Phase::Creating => report.creating += 1,
                    Phase::Owned => report.owned += 1,
                    Phase::Stopping => report.stopping += 1,
                    _ => {}
                }
                if snapshot
                    .stopping()
                    .is_some_and(|stop| stop.kind() == ResultKind::Acknowledged)
                {
                    report.stop_acknowledged += 1;
                } else if snapshot.submitted() {
                    report.remote_unknown += 1;
                }
            }
        }
        if report.reached_terminal() {
            let released = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                state.final_report = report;
                state.held.take()
            };
            // 释放Shared可能触发ExecutorSlot::Drop，必须在任何登记表/本批锁之外。
            drop(released);
        }
        report
    }

    pub(in crate::server::core::plugin_host) async fn wait(&self) -> DrainReport {
        loop {
            let notified = changed().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let report = self.report();
            if report.reached_terminal() {
                return report;
            }
            let scopes = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .held
                .as_ref()
                .map(|held| held.scopes.clone())
                .unwrap_or_default();
            let now = Instant::now();
            let mut next = None;
            let aborts = {
                let mut table = lock();
                let mut aborts = Vec::new();
                for running in &mut table.running {
                    if !scopes
                        .iter()
                        .any(|scope| Arc::ptr_eq(scope, running.record.scope()))
                    {
                        continue;
                    }
                    let deadline = running
                        .record
                        .scope()
                        .shutdown
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .map(|window| window.deadline)
                        .unwrap_or(now);
                    if deadline <= now {
                        running.record.request_stop(EndReason::DrainTimeout);
                        if !running.drain_abort_sent {
                            if let Some(abort) = &running.abort {
                                aborts.push(abort.clone());
                                running.drain_abort_sent = true;
                            }
                        }
                    } else {
                        next =
                            Some(next.map_or(deadline, |current: Instant| current.min(deadline)));
                    }
                }
                aborts
            };
            for abort in aborts {
                abort.abort();
            }
            // 较早关闭的scope到限不提前放弃同批其他scope；仍按各自最早截止并行推进。
            let Some(deadline) = next else {
                return self.report();
            };
            tokio::select! {
                biased;
                _ = &mut notified => {},
                _ = sleep_until(deadline) => {},
            }
        }
    }
}

pub(super) fn bounded_deadline(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + input::DRAIN_TIMEOUT)
}
