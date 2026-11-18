//! 固定create/stop和登记收据的有限只读观察；没有普通HTTP队列、重试或任意上行。
//! FrozenHttpClient完整包装始终由原worker持有，直到读取与最终停止全部结束。

mod attach;
mod detail;
mod observation;
mod wire;
pub(super) use observation::{
    ObservationEnd, ObservationFailure, ObservationItem, ObservationWindow, TextChunk,
};

use std::sync::Arc;

use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT},
    Method, Request,
};
use tokio::time::{timeout_at, Instant};

use super::super::conversation::CallGate;

use super::{
    account_snapshot::{FinalAccount, SnapshotMaterial},
    egress_preparation::PreparedEgress,
    input::{Binding, FrozenUserAgent},
    lifecycle::Record,
    policy::{self, CreateBody, ProjectedCookie},
    receipt::{
        self, CreationOutcome, Failure, IncompleteReason, ParsedCreation, ResponseInput,
        StopOutcome,
    },
    registry::{CreatePermit, PreparingGuard, StopPermit},
    status::NotSentReason,
    types::ValidationError,
};

/// 只由本模块签发的借用钥匙，不能由记录/JSON构造。
pub(super) struct EgressUse(());
pub(super) struct AccountUse(());

struct SessionEgress(PreparedEgress);

impl SessionEgress {
    fn client(&self) -> Result<&reqwest::Client, String> {
        self.0.client(EgressUse(()))
    }
}

pub(super) struct Session {
    // 整份来源与客户端按值一起持有，不导出客户端后另配见证。
    frozen: SessionEgress,
    identity: Arc<()>,
    binding: Arc<Binding>,
    cookie: ProjectedCookie,
    user_agent: FrozenUserAgent,
    account: FinalAccount,
    create_prepared: bool,
    stop_prepared: bool,
}

/// 只有真实执行本模块固定create并通过严格parser后才构造；普通解析器不能签发。
pub(super) struct WitnessedCreation {
    parsed: ParsedCreation,
    binding: Arc<Binding>,
    session: Arc<()>,
}

impl WitnessedCreation {
    pub(super) fn http_status(&self) -> u16 {
        self.parsed.http_status()
    }
    pub(super) fn binding(&self) -> &Binding {
        &self.binding
    }
}

pub(super) enum CreateResult {
    Created(WitnessedCreation),
    Rejected(Failure),
    Unknown(Failure),
}

/// 不可克隆，只能由本模块以固定地址和头构造，并消耗一次。
pub(super) struct PreparedCreate {
    request: Request,
    session: Arc<()>,
    identity: Arc<()>,
}

/// 只由确切PreparedCreate派生，不可Clone/serde，也不能从同Session重造。
pub(super) struct CreateWitness {
    session: Arc<()>,
    request: Arc<()>,
}

pub(super) struct PreparedStop {
    request: Request,
    session: Arc<()>,
    deadline: Instant,
}

impl Session {
    pub(super) fn new(
        binding: Arc<Binding>,
        frozen: PreparedEgress,
        account: SnapshotMaterial,
    ) -> Result<Self, ValidationError> {
        if !account.matches_binding(&binding) {
            return Err(unavailable());
        }
        let (account, cookie, user_agent) = account
            .into_session(AccountUse(()))
            .map_err(|_| unavailable())?;
        Ok(Self {
            frozen: SessionEgress(frozen),
            identity: Arc::new(()),
            binding,
            cookie,
            user_agent,
            account,
            create_prepared: false,
            stop_prepared: false,
        })
    }

    /// 只比较锁外构造的确切请求与完整会话，不克隆正文、凭据或客户端。
    pub(super) fn request_witness(
        &self,
        request: &PreparedCreate,
    ) -> Result<CreateWitness, NotSentReason> {
        if !self.create_prepared || !Arc::ptr_eq(&self.identity, &request.session) {
            return Err(NotSentReason::InvalidBinding);
        }
        Ok(CreateWitness {
            session: self.identity.clone(),
            request: request.identity.clone(),
        })
    }

    pub(super) fn check_prepared(
        &self,
        record: &Arc<Record>,
        gate: &CallGate<'_>,
        preparing: &PreparingGuard<'_>,
        request: &PreparedCreate,
        witness: &CreateWitness,
    ) -> Result<(), NotSentReason> {
        if !Arc::ptr_eq(&self.binding, record.binding())
            || !self
                .account
                .matches_binding(&self.binding, record.task_call())
            || !self.matches_request(request, witness)
        {
            return Err(NotSentReason::InvalidBinding);
        }
        self.frozen.0.check(gate, preparing)
    }

    pub(super) fn final_account(&self) -> &FinalAccount {
        &self.account
    }

    fn matches_request(&self, request: &PreparedCreate, witness: &CreateWitness) -> bool {
        self.create_prepared
            && Arc::ptr_eq(&self.identity, &request.session)
            && Arc::ptr_eq(&self.identity, &witness.session)
            && Arc::ptr_eq(&request.identity, &witness.request)
    }

    pub(super) fn prepare_create(
        &mut self,
        body: CreateBody,
    ) -> Result<PreparedCreate, ValidationError> {
        if self.create_prepared {
            return Err(unavailable());
        }
        self.create_prepared = true;
        self.frozen.client().map_err(|_| unavailable())?;
        self.prepare(Method::POST, policy::create_route(self.binding.provider())?, body.into_bytes())
            .map(|request| PreparedCreate {
                request,
                session: self.identity.clone(),
                identity: Arc::new(()),
            })
    }

    pub(super) fn prepare_stop(
        &mut self,
        permit: StopPermit,
    ) -> Result<PreparedStop, ValidationError> {
        if self.stop_prepared {
            return Err(unavailable());
        }
        self.stop_prepared = true;
        let (creation, deadline) = permit.into_parts();
        if creation.binding() != self.binding.as_ref()
            || !Arc::ptr_eq(&creation.session, &self.identity)
        {
            return Err(unavailable());
        }
        self.prepare(
            Method::PUT,
            policy::stop_route(self.binding.provider())?,
            creation.parsed.stop_body()?,
        )
        .map(|request| PreparedStop {
            request,
            deadline,
            session: self.identity.clone(),
        })
    }

    fn prepare(
        &self,
        method: Method,
        route: policy::Route,
        body: Vec<u8>,
    ) -> Result<Request, ValidationError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, self.user_agent.header());
        self.cookie.insert_into(&mut headers);
        let url = url::Url::parse(route.url()).map_err(|_| unavailable())?;
        let mut request = Request::new(method, url);
        *request.headers_mut() = headers;
        *request.body_mut() = Some(body.into());
        Ok(request)
    }

    pub(super) async fn create(
        &self,
        request: PreparedCreate,
        permit: CreatePermit,
    ) -> CreateResult {
        if permit.binding() != self.binding.as_ref()
            || !self.matches_request(&request, permit.request())
        {
            return CreateResult::Unknown(receipt::Failure::executor_unknown());
        }
        let deadline = permit.deadline();
        let response = self
            .exchange(
                request.request,
                deadline,
                super::types::MAX_CREATION_RESPONSE_BYTES,
            )
            .await;
        match receipt::parse_creation(response.input()) {
            CreationOutcome::Parsed(parsed) => CreateResult::Created(WitnessedCreation {
                parsed,
                binding: self.binding.clone(),
                session: self.identity.clone(),
            }),
            CreationOutcome::Rejected(failure) => CreateResult::Rejected(failure),
            CreationOutcome::Unknown(failure) => CreateResult::Unknown(failure),
        }
    }

    pub(super) async fn stop(&self, request: PreparedStop) -> StopOutcome {
        if !Arc::ptr_eq(&request.session, &self.identity) {
            return StopOutcome::Unknown(receipt::Failure::executor_unknown());
        }
        let response = self
            .exchange(
                request.request,
                request.deadline,
                super::types::MAX_STOP_RESPONSE_BYTES,
            )
            .await;
        receipt::parse_stop(response.input())
    }

    async fn exchange(
        &self,
        request: Request,
        deadline: Instant,
        limit: usize,
    ) -> BufferedResponse {
        let mut status = None;
        let result = timeout_at(deadline, async {
            // 仅阶段固定期限能取消这里；不 select 调用取消，也不重新计时。
            if Instant::now() >= deadline {
                return Err(IncompleteReason::Timeout);
            }
            let client = self
                .frozen
                .client()
                .map_err(|_| IncompleteReason::Disconnected)?;
            // create/stop专用发送点；不采用观察的取消策略，不设send重试循环。
            let mut response = client.execute(request).await.map_err(classify)?;
            status = Some(response.status().as_u16());
            if response
                .content_length()
                .is_some_and(|size| size > limit as u64)
            {
                return Err(IncompleteReason::BodyLimit);
            }
            let mut bytes = Vec::with_capacity(limit);
            loop {
                if Instant::now() >= deadline {
                    return Err(IncompleteReason::Timeout);
                }
                let chunk = response.chunk().await.map_err(classify)?;
                if Instant::now() >= deadline {
                    return Err(IncompleteReason::Timeout);
                }
                let Some(chunk) = chunk else { return Ok(bytes) };
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err(IncompleteReason::BodyLimit);
                }
                bytes.extend_from_slice(&chunk);
            }
        })
        .await;
        match result {
            Ok(Ok(bytes)) => BufferedResponse::Complete {
                status: status.unwrap_or(0),
                bytes,
            },
            Ok(Err(reason)) => BufferedResponse::Incomplete { status, reason },
            Err(_) => BufferedResponse::Incomplete {
                status,
                reason: IncompleteReason::Timeout,
            },
        }
    }
}

fn classify(error: reqwest::Error) -> IncompleteReason {
    // 不展示 reqwest 原错误：它可能携带 URL、代理或上游内容。
    if error.is_timeout() {
        IncompleteReason::Timeout
    } else {
        IncompleteReason::Disconnected
    }
}

fn unavailable() -> ValidationError {
    ValidationError::new("冻结出口或固定任务请求不可用，未更换账号或代理")
}

enum BufferedResponse {
    Complete {
        status: u16,
        bytes: Vec<u8>,
    },
    Incomplete {
        status: Option<u16>,
        reason: IncompleteReason,
    },
}

impl BufferedResponse {
    fn input(&self) -> ResponseInput<'_> {
        match self {
            Self::Complete { status, bytes } => ResponseInput::Complete {
                http_status: *status,
                bytes,
            },
            Self::Incomplete { status, reason } => ResponseInput::Incomplete {
                http_status: *status,
                reason: match reason {
                    IncompleteReason::Timeout => IncompleteReason::Timeout,
                    IncompleteReason::Disconnected => IncompleteReason::Disconnected,
                    IncompleteReason::BodyLimit => IncompleteReason::BodyLimit,
                    IncompleteReason::Cancelled => IncompleteReason::Cancelled,
                },
            },
        }
    }
}
