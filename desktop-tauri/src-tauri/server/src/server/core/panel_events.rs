use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::watch;

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Revisions {
    pub accounts: u64,
    pub balances: u64,
    pub usage: u64,
    pub connections: u64,
    pub proxies: u64,
}

pub(crate) enum Change {
    Accounts,
    Balances,
    Usage,
    Connections,
    Proxies,
}

fn channel() -> &'static watch::Sender<Revisions> {
    static EVENTS: OnceLock<watch::Sender<Revisions>> = OnceLock::new();
    EVENTS.get_or_init(|| watch::channel(Revisions::default()).0)
}

pub(crate) fn notify(change: Change) {
    channel().send_modify(|revisions| {
        let revision = match change {
            Change::Accounts => &mut revisions.accounts,
            Change::Balances => &mut revisions.balances,
            Change::Usage => &mut revisions.usage,
            Change::Connections => &mut revisions.connections,
            Change::Proxies => &mut revisions.proxies,
        };
        *revision = revision.wrapping_add(1);
    });
}

pub(crate) fn subscribe() -> watch::Receiver<Revisions> {
    channel().subscribe()
}

fn logs_channel() -> &'static watch::Sender<u64> {
    static LOGS: OnceLock<watch::Sender<u64>> = OnceLock::new();
    LOGS.get_or_init(|| watch::channel(0).0)
}

fn requests_channel() -> &'static watch::Sender<u64> {
    static REQUESTS: OnceLock<watch::Sender<u64>> = OnceLock::new();
    REQUESTS.get_or_init(|| watch::channel(0).0)
}

pub(crate) fn notify_logs() {
    logs_channel().send_modify(|revision| {
        *revision = revision.wrapping_add(1);
    });
}

pub(crate) fn subscribe_logs() -> watch::Receiver<u64> {
    logs_channel().subscribe()
}

pub(crate) fn notify_requests() {
    requests_channel().send_modify(|revision| {
        *revision = revision.wrapping_add(1);
    });
}

pub(crate) fn subscribe_requests() -> watch::Receiver<u64> {
    requests_channel().subscribe()
}
