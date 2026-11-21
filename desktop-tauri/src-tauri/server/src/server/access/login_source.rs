use axum::http::HeaderMap;
use tokio_util::sync::CancellationToken;

use super::local_session;

#[derive(Clone)]
pub(crate) enum Source {
    Local(local_session::Source),
    Panel { digest: String, session: String, expires_at: i64, revoked: CancellationToken },
}

impl Source {
    pub(crate) fn capture(headers: &HeaderMap) -> Option<Self> {
        let candidates = super::access_candidates(headers);
        for token in &candidates {
            if let Some(source) = local_session::capture(token) { return Some(Self::Local(source)); }
        }
        let table = super::access_tokens().try_lock().ok()?;
        for token in candidates {
            let digest = super::sha256_hex(&token);
            if let Some((session, expires_at)) = table.get(&digest).filter(|(_, at)| *at > super::now_ms()) {
                return Some(Self::Panel { digest, session: session.clone(), expires_at: *expires_at, revoked: CancellationToken::new() });
            }
        }
        None
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Local(a), Self::Local(b)) => a.same(b),
            (Self::Panel { digest: a, session: sa, expires_at: ea, .. }, Self::Panel { digest: b, session: sb, expires_at: eb, .. }) => a == b && sa == sb && ea == eb,
            _ => false,
        }
    }

    pub(crate) fn revocation(&self) -> CancellationToken {
        match self { Self::Local(source) => source.revocation(), Self::Panel { revoked, .. } => revoked.clone() }
    }

    pub(crate) fn refresh_revocation(&self) {
        let Self::Panel { digest, session, expires_at, revoked } = self else { return; };
        match super::access_tokens().try_lock() {
            Ok(table) => {
                if *expires_at <= super::now_ms() || table.get(digest).is_none_or(|(id, at)| id != session || at != expires_at) {
                    revoked.cancel();
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {},
            Err(std::sync::TryLockError::Poisoned(_)) => revoked.cancel(),
        }
    }

    pub(crate) fn is_current(&self) -> bool { self.with_current(|| ()).is_some() }

    pub(crate) fn with_current<R>(&self, action: impl FnOnce() -> R) -> Option<R> {
        match self {
            Self::Local(source) => source.with_current(action),
            Self::Panel { digest, session, expires_at, revoked } => {
                if revoked.is_cancelled() { return None; }
                let table = super::access_tokens().try_lock().ok()?;
                if *expires_at <= super::now_ms() || table.get(digest).is_none_or(|(id, at)| id != session || at != expires_at) {
                    revoked.cancel();
                    return None;
                }
                Some(action())
            }
        }
    }
}
