//! 固定GET：只借用登记中的真实创建结果，不取走stop责任，不重试失败的HTTP。

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use reqwest::{
    header::{HeaderValue, ORIGIN, REFERER},
    Method, Request,
};
use tokio::time::{sleep_until, Instant};

use super::super::{
    observe::{self, DetailObservation, DetailState},
    policy, registry,
};
use super::{
    observation::{
        ObservationFailure, Observer, DETAIL_INTERVAL, DETAIL_REQUEST_TIMEOUT, MAX_DETAIL_REQUESTS,
        MAX_DETAIL_TOTAL_BYTES,
    },
    Session, WitnessedCreation,
};

pub(super) async fn wait_ready(
    session: &Session,
    observer: &mut Observer<'_>,
) -> Result<(), ObservationFailure> {
    for _ in 0..MAX_DETAIL_REQUESTS {
        observer.check(observer.window.ready, ObservationFailure::ReadinessTimeout)?;
        let now = Instant::now();
        let deadline = observer.window.ready.min(now + DETAIL_REQUEST_TIMEOUT);
        let timeout = if deadline == observer.window.ready {
            ObservationFailure::ReadinessTimeout
        } else {
            ObservationFailure::DetailTimeout
        };
        let remaining = MAX_DETAIL_TOTAL_BYTES.saturating_sub(observer.detail_bytes);
        if remaining == 0 {
            return Err(ObservationFailure::DetailBytes);
        }
        let received = AtomicUsize::new(0);
        let result = observer
            .wait(deadline, timeout, async {
                // 这段同步确认紧邻第一次execute；不先拿许可跨await排队。结束CAS先赢就不发。
                let (request, _read) = registry::observation_read(
                    observer.record,
                    observer.permit,
                    false,
                    |creation| request(session, creation),
                )?;
                let client = session
                    .frozen
                    .client()
                    .map_err(|_| ObservationFailure::Unavailable)?;
                let mut response = observer
                    .wait(deadline, timeout, async {
                        client.execute(request).await.map_err(transport)
                    })
                    .await?;
                let status = response.status().as_u16();
                let limit = observe::MAX_DETAIL_BYTES.min(remaining);
                if response
                    .content_length()
                    .is_some_and(|bytes| bytes > limit as u64)
                {
                    return Err(ObservationFailure::DetailBytes);
                }
                let mut bytes = Vec::new();
                loop {
                    // 每块响应体都重新select结束/期限；大量立即就绪chunk也不能饿死停止。
                    let chunk = observer
                        .wait(deadline, timeout, async {
                            let chunk = response.chunk().await.map_err(transport)?;
                            if let Some(chunk) = &chunk {
                                received.store(
                                    received.load(Ordering::Relaxed).saturating_add(chunk.len()),
                                    Ordering::Relaxed,
                                );
                            }
                            Ok(chunk)
                        })
                        .await?;
                    let Some(chunk) = chunk else {
                        break;
                    };
                    if chunk.len() > limit.saturating_sub(bytes.len()) {
                        return Err(ObservationFailure::DetailBytes);
                    }
                    bytes.extend_from_slice(&chunk);
                    observer
                        .wait(deadline, timeout, async {
                            tokio::task::yield_now().await;
                            Ok(())
                        })
                        .await?;
                }
                Ok((status, bytes))
            })
            .await;
        // 部分响应、无效/未知回应同样扣预算，不只统计被解释为成功的正文。
        observer.detail_bytes = observer
            .detail_bytes
            .saturating_add(received.load(Ordering::Relaxed));
        let (status, bytes) = result?;
        let detail = observe::parse_detail(status, &bytes).map_err(ObservationFailure::Invalid)?;
        observer.check(observer.window.ready, ObservationFailure::ReadinessTimeout)?;
        matches_registered(session, observer, &detail)?;
        match detail.state() {
            DetailState::NeedsAttach(_) => return Ok(()),
            DetailState::Failed(failure) => return Err(ObservationFailure::DetailFailed(failure)),
            DetailState::Unknown(reason) => return Err(ObservationFailure::DetailUnknown(reason)),
            DetailState::Waiting => {}
        }
        // 上次完整GET之后至少2秒，不因响应快或排队命令而增加请求频率。
        let next = Instant::now() + DETAIL_INTERVAL;
        observer
            .wait(
                observer.window.ready,
                ObservationFailure::ReadinessTimeout,
                async {
                    sleep_until(next).await;
                    Ok(())
                },
            )
            .await?;
    }
    Err(ObservationFailure::DetailRequests)
}

fn request(session: &Session, creation: &WitnessedCreation) -> Result<Request, ObservationFailure> {
    if creation.binding() != session.binding.as_ref()
        || !Arc::ptr_eq(&creation.session, &session.identity)
    {
        return Err(ObservationFailure::Unavailable);
    }
    let mut request = session
        .prepare(Method::GET, creation.parsed.detail_route(session.binding.provider()).map_err(|_| ObservationFailure::Unavailable)?, Vec::new())
        .map_err(|_| ObservationFailure::Unavailable)?;
    *request.body_mut() = None;
    request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static(policy::origin(session.binding.provider()).ok_or(ObservationFailure::Unavailable)?));
    request.headers_mut().insert(
        REFERER,
        HeaderValue::from_str(&format!("{}/", policy::origin(session.binding.provider()).ok_or(ObservationFailure::Unavailable)?))
            .map_err(|_| ObservationFailure::Unavailable)?,
    );
    Ok(request)
}

fn matches_registered(
    session: &Session,
    observer: &Observer<'_>,
    detail: &DetailObservation,
) -> Result<(), ObservationFailure> {
    // 无网络、无await；UUID仍在真实创建对象内部，仅返回相等与否。
    let state = observer.record.lock();
    if observer.record.end_reason().is_some() {
        return Err(ObservationFailure::Closed);
    }
    let Some(creation) = state.creation.as_ref() else {
        return Err(ObservationFailure::Unavailable);
    };
    if creation.binding() != session.binding.as_ref()
        || !Arc::ptr_eq(&creation.session, &session.identity)
    {
        return Err(ObservationFailure::Unavailable);
    }
    if creation.parsed.matches_detail(detail) == Some(false) {
        return Err(ObservationFailure::Identity);
    }
    Ok(())
}

fn transport(error: reqwest::Error) -> ObservationFailure {
    if error.is_timeout() {
        ObservationFailure::DetailTimeout
    } else {
        ObservationFailure::Transport
    }
}
