use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::server::core::panel_events::{self, Revisions};
use crate::server::{access, api::model_access, ServerState};

const HEARTBEAT: Duration = Duration::from_secs(15);
const COALESCE: Duration = Duration::from_millis(150);

mod deadline;
mod external;

struct Events {
    external: external::ExternalChanges,
    store: crate::server::core::account_store::AccountStore,
    deadline: Option<i64>,
    deadline_revision: (u64, u64, u64),
    headers: HeaderMap,
    revisions: watch::Receiver<Revisions>,
    stopping: CancellationToken,
    initial: bool,
}

fn authorized(headers: &HeaderMap) -> bool {
    if access::session_valid(headers) {
        return true;
    }
    if let Ok(access) = model_access::authenticate(headers, "") {
        return !access.scope.as_ref().is_some_and(|scope|
            scope.restricts_models() || scope.restricts_providers());
    }
    !access::panel_gate() && !access::panel_auth_enabled()
        && crate::server::config::current().active_api_keys().is_empty()
}

pub async fn events(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return crate::server::errors::panel_login_required_response(&headers);
    }
    let stopping = state.panel_events_stop.clone();
    let external = crate::server::core::blocking::in_place(|| external::ExternalChanges::new(state.db().cloned()));
    let events = Events { external, store: state.store().clone(), deadline: None, deadline_revision: (0, 0, 0), headers, stopping,
        revisions: panel_events::subscribe(), initial: true };
    let stream = stream::unfold(events, |mut current| async move {
        if current.stopping.is_cancelled() || !authorized(&current.headers) {
            return None;
        }
        if current.initial {
            current.initial = false;
            current.deadline = crate::server::core::blocking::in_place(|| deadline::next_change(&current.store));
            let revisions = *current.revisions.borrow_and_update();
            current.deadline_revision = (revisions.accounts, revisions.balances, revisions.usage);
            let event = Event::default().event("ready").json_data(revisions).ok()?;
            return Some((Ok::<_, Infallible>(event), current));
        }
        let wait = current.deadline.map(|at| Duration::from_millis(at.saturating_sub(crate::server::logging::now_ms()).max(1) as u64))
            .unwrap_or(HEARTBEAT).min(HEARTBEAT);
        let mut changed = tokio::select! {
            biased;
            _ = current.stopping.cancelled() => return None,
            result = current.revisions.changed() => {
                if result.is_err() { return None; }
                true
            },
            _ = tokio::time::sleep(wait) => {
                let elapsed = current.deadline.is_some_and(|at| at <= crate::server::logging::now_ms());
                if elapsed {
                    panel_events::notify(panel_events::Change::Accounts);
                    panel_events::notify(panel_events::Change::Balances);
                    panel_events::notify(panel_events::Change::Usage);
                }
                elapsed
            },
        };
        if !changed && crate::server::core::blocking::in_place(|| current.external.changed()) {
            changed = true;
            panel_events::notify(panel_events::Change::Accounts);
            panel_events::notify(panel_events::Change::Balances);
            panel_events::notify(panel_events::Change::Usage);
            panel_events::notify(panel_events::Change::Proxies);
        }
        if changed {
            tokio::select! {
                biased;
                _ = current.stopping.cancelled() => return None,
                _ = tokio::time::sleep(COALESCE) => {},
            }
        }
        if !authorized(&current.headers) {
            return None;
        }
        let event = if changed {
            let revisions = *current.revisions.borrow_and_update();
            let deadline_revision = (revisions.accounts, revisions.balances, revisions.usage);
            if current.deadline_revision != deadline_revision {
                current.deadline_revision = deadline_revision;
                current.deadline = crate::server::core::blocking::in_place(|| deadline::next_change(&current.store));
            }
            Event::default().event("change").json_data(revisions).ok()?
        } else {
            Event::default().event("heartbeat").data("{}")
        };
        Some((Ok::<_, Infallible>(event), current))
    });
    let mut response = Sse::new(stream).into_response();
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}
