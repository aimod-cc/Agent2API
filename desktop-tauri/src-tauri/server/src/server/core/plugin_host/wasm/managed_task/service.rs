//! 固定MonkeyCode任务服务实现；未登记ABI/能力，声明和旧磁盘grant都不能授予执行批准。
//! 单个Context只保留本次call/随机句柄/Started/观察端；网络与停止责任仍在原worker。

use std::future::Future;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::super::{context::Context, conversation::TaskCallRef};
use super::{
    account_snapshot::{AccountSnapshot, SnapshotError},
    executor::Delivery,
    lifecycle::Observation,
    network::{ObservationEnd, ObservationFailure, ObservationItem, TextChunk},
    observe::{Interaction, TextKind},
    policy::{self, CreateBody},
    runtime_slot::{self, StartView, Started},
    status::{NotSentReason, Phase, ResultKind, StageResult},
};
use crate::server::core::account_store::{plugin_accounts, AccountStore};

const CAPABILITY: &str = "http.managed-task.v1";
const CREATE: &str = "managed_task.create";
const OBSERVE: &str = "managed_task.observe";
const READ: &str = "managed_task.read";
const STATUS: &str = "managed_task.status";
const CLOSE: &str = "managed_task.close";
const HANDLE_BYTES: usize = 32;

/// 不实现Clone/serde，不接句柄/账号/运行实例导入；始终由原Context持有。
#[derive(Default)]
pub(in crate::server::core::plugin_host::wasm) struct Owner {
    claimed: bool,
    cancelled: bool,
    call: Option<TaskCallRef>,
    id: Option<String>,
    observation_claimed: bool,
    end: Option<ObservationEnd>,
    failure: Option<LocalFailure>,
    // 字段顺序让观察端先释放，随后Started内的真实收据和原Invocation再释放。
    observation: Option<Observation>,
    started: Option<Started>,
}

impl Owner {
    /// 取消只置原任务结束意图/释放接收端；从不abort或重建Creating/Stopping的future。
    pub(in crate::server::core::plugin_host::wasm) fn cancel(&mut self) {
        self.cancelled = true;
        if let Some(started) = &self.started {
            started.cancel();
        }
        self.observation = None;
        if self.started.is_none() && self.claimed && self.failure.is_none() {
            self.failure = Some(LocalFailure {
                message: "当前创建准备或等待已取消，未重新取得调用来源或创建机会",
                replay_forbidden: false,
            });
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct LocalFailure {
    message: &'static str,
    replay_forbidden: bool,
}

enum Command {
    Create(CreateBody),
    Observe(String),
    Read(String),
    Status(String),
    Close(String),
}

impl Command {
    fn parse(method: &str, params: &Value) -> Result<Self, proto::WireError> {
        if method != CREATE && !matches!(method, OBSERVE | READ | STATUS | CLOSE) {
            return Err(proto::WireError::with_code(
                "受控任务服务方法不存在",
                proto::WireError::NO_METHOD,
            ));
        }
        let fields = params
            .as_object()
            .ok_or_else(|| invalid("受控任务参数必须是固定JSON对象"))?;
        if method == CREATE {
            if fields.len() != 1 || !fields.contains_key("body") {
                return Err(invalid(
                    "创建任务只接受body，不接受账号、地址、请求头或任务编号",
                ));
            }
            // body保留原始JSON文本，严格解析不会先把重复键折成一个Value。
            let bytes = fields["body"]
                .as_str()
                .ok_or_else(|| invalid("任务body必须是固定任务结构的原始JSON文本"))?
                .as_bytes();
            return CreateBody::parse(bytes)
                .map(Self::Create)
                .map_err(|error| invalid(error.message()));
        }
        if fields.len() != 1 || !fields.contains_key("id") {
            return Err(invalid("任务读取或关闭只接受本次调用的id"));
        }
        let id = fields["id"]
            .as_str()
            .filter(|id| {
                id.len() == 3 + HANDLE_BYTES * 2
                    && id.starts_with("mt-")
                    && id[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| invalid("任务id不是本次宿主交付的有效句柄"))?
            .to_string();
        match method {
            OBSERVE => Ok(Self::Observe(id)),
            READ => Ok(Self::Read(id)),
            STATUS => Ok(Self::Status(id)),
            CLOSE => Ok(Self::Close(id)),
            _ => Err(proto::WireError::with_code(
                "受控任务服务方法不存在",
                proto::WireError::NO_METHOD,
            )),
        }
    }

    fn cancels_on_drop(&self) -> bool {
        matches!(self, Self::Create(_) | Self::Observe(_) | Self::Read(_))
    }
}

/// 守卫在返回Future之前构造；从未poll、中途取消或陷阱都不能回补已领取的创建机会。
struct Request<'a> {
    context: &'a mut Context,
    cancel_on_drop: bool,
}

impl Request<'_> {
    fn finish(
        mut self,
        result: Result<Value, proto::WireError>,
    ) -> Result<Value, proto::WireError> {
        self.cancel_on_drop = false;
        result
    }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.context.task_service.cancel();
        }
    }
}

impl Context {
    /// 主分派可直接await；同步部分先校验固定参数并占一次call机会，不持任何门跨await。
    pub(in crate::server::core::plugin_host::wasm) fn managed_task_service<'a>(
        &'a mut self,
        method: &str,
        params: &Value,
    ) -> impl Future<Output = Result<Value, proto::WireError>> + 'a {
        let command = Command::parse(method, params).and_then(|command| {
            self.check_task_context()?;
            match &command {
                Command::Create(_) => {
                    if self.task_service.claimed || self.task_service.cancelled {
                        return Err(forbidden("本次调用的唯一创建机会已消耗，不能重新创建"));
                    }
                    // 在task_call_ref之前同步领取；即使取得来源失败或返回Future未poll也不回补。
                    self.task_service.claimed = true;
                    let call = self.task_call_ref().map_err(|reason| {
                        self.task_service.failure = Some(LocalFailure {
                            message: not_sent_message(reason),
                            replay_forbidden: matches!(
                                reason,
                                NotSentReason::AlreadySubmitted | NotSentReason::AlreadyPreparing
                            ),
                        });
                        forbidden(not_sent_message(reason))
                    })?;
                    self.task_service.call = Some(call);
                }
                Command::Observe(id)
                | Command::Read(id)
                | Command::Status(id)
                | Command::Close(id) => self.check_task_resource(id)?,
            }
            Ok(command)
        });
        let cancel_on_drop = command.as_ref().is_ok_and(Command::cancels_on_drop);
        let mut request = Request {
            context: self,
            cancel_on_drop,
        };
        async move {
            let result = match command {
                Err(error) => Err(error),
                Ok(Command::Create(body)) => request.context.task_create(body).await,
                Ok(Command::Observe(id)) => request.context.task_observe(&id),
                Ok(Command::Read(id)) => request.context.task_read(&id).await,
                Ok(Command::Status(id)) => {
                    request.context.check_task_resource(&id)?;
                    Ok(task_status(&request.context.task_service))
                }
                Ok(Command::Close(id)) => request.context.task_close(&id),
            };
            request.finish(result)
        }
    }

    fn check_task_context(&self) -> Result<(), proto::WireError> {
        self.permission(CAPABILITY)?;
        if policy::profile(&self.shared.options.provider).is_none()
            || self.verification.is_some()
            || self.managed.is_some()
            || self.checkin_call.is_some()
        {
            return Err(forbidden("受控任务只允许当前MonkeyCode国内真实对话调用"));
        }
        let declaration = self
            .shared
            .options
            .manifest
            .managed_task
            .as_ref()
            .ok_or_else(|| forbidden("当前插件没有固定MonkeyCode任务声明"))?;
        if !declaration
            .matches_call(&self.shared.options.provider, policy::ENTRY)
            .map_err(|_| forbidden("当前任务声明不属于固定对话profile"))?
        {
            return Err(forbidden("当前插件未请求固定MonkeyCode任务profile"));
        }
        self.conversation
            .as_ref()
            .ok_or_else(|| forbidden("当前操作没有真实对话来源，未授予任务权限"))?
            .validate_context(&self.shared, &self.account)
    }

    fn check_task_resource(&self, id: &str) -> Result<(), proto::WireError> {
        self.check_task_context()?;
        if self.task_service.id.as_deref() != Some(id) {
            return Err(forbidden("任务句柄不属于当前调用"));
        }
        let call = self
            .task_service
            .call
            .as_ref()
            .ok_or_else(|| forbidden("任务句柄没有原调用来源"))?;
        let started = self
            .task_service
            .started
            .as_ref()
            .ok_or_else(|| forbidden("任务句柄没有原登记所有者"))?;
        if call.identity().provider() != self.shared.options.provider
            || call.identity().account() != self.account
            || !started.belongs_to(call)
        {
            return Err(forbidden("任务句柄与原账号或运行调用不一致"));
        }
        // 字符串相同仍不足以授权；借原门核对实际Shared/Host/Runtime/call，不重新签发。
        call.with_gate(|gate| {
            if !std::ptr::eq(gate.shared(), self.shared.as_ref()) || !gate.matches(call) {
                return Err(NotSentReason::InvalidBinding);
            }
            Ok(())
        })
        .map_err(|_| forbidden("任务原调用已失效或运行实例已变化"))
    }

    async fn task_create(&mut self, body: CreateBody) -> Result<Value, proto::WireError> {
        let call = self
            .task_service
            .call
            .as_ref()
            .ok_or_else(|| forbidden("本次创建没有已领取的真实调用来源"))?
            .clone();
        let store = match plugin_accounts::shared() {
            Some(store) => store,
            None => return Ok(self.task_not_sent("原账号存储尚未就绪，未创建任务")),
        };
        let account = match read_account(&call, &store).await {
            Ok(account) => account,
            Err(error) => return Ok(self.task_not_sent(error.message())),
        };
        let id = match random_handle() {
            Ok(id) => id,
            Err(_) => return Ok(self.task_not_sent("宿主随机句柄不可用，未创建任务")),
        };
        let admitted = match runtime_slot::admit(&call, account, body) {
            Ok(admitted) => admitted,
            Err(reason) => {
                self.task_service.failure = Some(LocalFailure {
                    message: not_sent_message(reason),
                    replay_forbidden: matches!(
                        reason,
                        NotSentReason::AlreadySubmitted | NotSentReason::AlreadyPreparing
                    ),
                });
                return Ok(task_status(&self.task_service));
            }
        };
        // admit内部全部门已释放；原Invocation整体移入Owner，不创建临时Started或二元组。
        self.task_service.started = Some(admitted.start());
        self.task_service.id = Some(id);
        if let Some(started) = self.task_service.started.as_mut() {
            // 真正调用结果和receipt留在Started；局部只保留不含来源/凭据的真实NotSent原因。
            let frozen = started.snapshot().replay_forbidden();
            let failure = match started.wait().await {
                StartView::NotSent {
                    reason,
                    replay_forbidden,
                } => Some(LocalFailure {
                    message: not_sent_message(reason),
                    replay_forbidden,
                }),
                StartView::Delivery(Delivery::NotSent(reason)) => Some(LocalFailure {
                    message: not_sent_message(*reason),
                    replay_forbidden: frozen,
                }),
                _ => None,
            };
            self.task_service.failure = failure;
        }
        Ok(task_status(&self.task_service))
    }

    fn task_not_sent(&mut self, message: &'static str) -> Value {
        self.task_service.failure = Some(LocalFailure {
            message,
            replay_forbidden: false,
        });
        task_status(&self.task_service)
    }

    fn task_observe(&mut self, id: &str) -> Result<Value, proto::WireError> {
        self.check_task_resource(id)?;
        if self.task_service.observation_claimed || self.task_service.cancelled {
            return Err(forbidden(
                "本次任务观察机会已消耗或消费者已取消，不能重新连接",
            ));
        }
        // 尝试观察也只有一次，registry拒绝不会恢复机会。
        self.task_service.observation_claimed = true;
        let call = self.task_service.call.as_ref().ok_or_else(no_receipt)?;
        let started = self.task_service.started.as_ref().ok_or_else(no_receipt)?;
        self.task_service.observation = Some(started.observe(call).map_err(observation_error)?);
        Ok(json!({ "id": id, "status": "observing", "done": false, "replay_forbidden": true }))
    }

    async fn task_read(&mut self, id: &str) -> Result<Value, proto::WireError> {
        self.check_task_resource(id)?;
        if let Some(end) = self.task_service.end {
            return Ok(end_json(id, end, &self.task_service));
        }
        let observation = self
            .task_service
            .observation
            .as_mut()
            .ok_or_else(|| forbidden("本次任务尚未授权观察或观察已取消"))?;
        match observation.next().await {
            ObservationItem::Text(TextChunk::Text { kind, text }) => Ok(json!({
                "id": id, "status": "text", "kind": match kind {
                    TextKind::Message => "message", TextKind::Thought => "thought",
                },
                "text": text, "done": false, "replay_forbidden": true,
            })),
            ObservationItem::Text(TextChunk::Tool(tool)) => Ok(json!({
                "id": id, "status": "tool", "event": tool.event(), "tool": tool.projection(),
                "done": false, "replay_forbidden": true,
            })),
            ObservationItem::End(end) => {
                self.task_service.end = Some(end);
                // 真实观察终态已保留；释放接收端只置意图，原worker继续同一次停止和结清。
                self.task_service.observation = None;
                Ok(end_json(id, end, &self.task_service))
            }
        }
    }

    fn task_close(&mut self, id: &str) -> Result<Value, proto::WireError> {
        self.check_task_resource(id)?;
        let call = self.task_service.call.as_ref().ok_or_else(no_receipt)?;
        let started = self.task_service.started.as_ref().ok_or_else(no_receipt)?;
        let close_requested = started.close_receipt(call);
        if close_requested {
            self.task_service.observation = None;
        }
        Ok(json!({
            "id": id, "close_requested": close_requested,
            "task": task_status(&self.task_service),
        }))
    }
}

async fn read_account(
    call: &TaskCallRef,
    store: &AccountStore,
) -> Result<super::account_snapshot::SnapshotMaterial, SnapshotError> {
    AccountSnapshot::read(call, store)
        .await?
        .into_material()
        .await
}

fn random_handle() -> Result<String, ()> {
    let mut bytes = [0u8; HANDLE_BYTES];
    getrandom::getrandom(&mut bytes).map_err(|_| ())?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(());
    }
    Ok(format!(
        "mt-{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn task_status(owner: &Owner) -> Value {
    let Some(started) = &owner.started else {
        let message = owner.failure.as_ref().map_or(
            "创建准备尚未结束，未确认任务提交",
            |failure| failure.message,
        );
        let replay_forbidden = owner
            .failure
            .as_ref()
            .is_some_and(|failure| failure.replay_forbidden);
        return json!({
            "id": null, "status": "not_sent", "phase": "not_sent", "submitted": false,
            "replay_forbidden": replay_forbidden, "retry_allowed": false,
            "creation": { "status": "not_sent", "http_status": null, "business_code": null, "message": message },
            "stop": null, "done": owner.failure.is_some() || owner.cancelled,
        });
    };
    let snapshot = started.snapshot();
    let creation = snapshot.creation();
    let has_receipt = owner
        .call
        .as_ref()
        .is_some_and(|call| started.has_receipt(call));
    let status = if !snapshot.submitted() {
        "not_sent"
    } else if creation.kind() == ResultKind::Rejected {
        "rejected"
    } else if has_receipt && creation.kind() == ResultKind::Created {
        "created"
    } else {
        "unknown"
    };
    let stop = snapshot.stopping().map(|result| {
        let mut value = stage_json(result);
        value["remote_destroyed"] = Value::Null;
        value
    });
    let creation_json = if !snapshot.submitted() {
        owner.failure.as_ref().map_or_else(
            || stage_json(creation),
            |failure| {
                json!({
                    "status": "not_sent", "http_status": null, "business_code": null,
                    "message": failure.message,
                })
            },
        )
    } else {
        stage_json(creation)
    };
    let replay_forbidden = snapshot.replay_forbidden()
        || owner
            .failure
            .as_ref()
            .is_some_and(|failure| failure.replay_forbidden);
    json!({
        "id": owner.id, "status": status, "phase": match snapshot.phase() {
            Phase::NotSent => "not_sent", Phase::Preparing => "preparing", Phase::Creating => "creating",
            Phase::Owned => "owned", Phase::Stopping => "stopping", Phase::Finished => "finished",
        },
        "submitted": snapshot.submitted(), "replay_forbidden": replay_forbidden,
        "retry_allowed": false, "creation": creation_json, "stop": stop,
        "done": snapshot.phase() == Phase::Finished || (!snapshot.submitted() && owner.failure.is_some()),
    })
}

fn stage_json(result: StageResult) -> Value {
    json!({
        "status": match result.kind() {
            ResultKind::NotSent => "not_sent", ResultKind::Pending => "pending", ResultKind::Created => "created",
            ResultKind::Acknowledged => "acknowledged", ResultKind::Rejected => "rejected", ResultKind::Unknown => "unknown",
        },
        "http_status": result.http_status(), "business_code": result.business_code(), "message": result.message(),
    })
}

fn end_json(id: &str, end: ObservationEnd, owner: &Owner) -> Value {
    let (outcome, interaction, message) = match end {
        ObservationEnd::RoundEnded => (
            "round_ended",
            None,
            "本轮结束消息已确认；任务停止和执行环境回收仍按独立真实结果报告",
        ),
        ObservationEnd::NeedsInteraction(interaction) => (
            "needs_interaction",
            Some(match interaction {
                Interaction::Permission => "permission",
                Interaction::Question => "question",
            }),
            "任务需要尚未接入的真人交互，未自动批准或答复，原worker继续本次收尾",
        ),
        ObservationEnd::Failed(error) => ("failed", None, error.message()),
    };
    json!({
        "id": id, "status": "end", "outcome": outcome, "interaction": interaction,
        "message": message, "done": true, "replay_forbidden": true, "task": task_status(owner),
    })
}

fn not_sent_message(reason: NotSentReason) -> &'static str {
    match reason {
        NotSentReason::AdmissionClosed => "当前安装没有真实有效的任务批准，未创建任务",
        NotSentReason::InvalidBinding => "原账号、调用或安装来源不一致，未创建任务",
        NotSentReason::Capacity => "本次受控任务名额已满，未创建任务或排队重试",
        NotSentReason::AlreadySubmitted => "本次任务创建机会已消耗，未再次创建",
        NotSentReason::AlreadyPreparing => "本次调用已有任务准备，未重新取得出口或创建机会",
        NotSentReason::CallEnded => "当前真实对话已结束，未创建任务",
        NotSentReason::PreparationExpired => "任务准备超过首次固定期限，未重新计时或创建任务",
        NotSentReason::RuntimeUnavailable => "原受控任务运行环境不可用，未创建任务或更换实例",
        NotSentReason::RandomUnavailable => "宿主随机材料不可用，未创建任务",
        NotSentReason::RequestUnavailable => "原账号材料或固定任务请求不可用，未创建任务",
    }
}

fn invalid(message: &'static str) -> proto::WireError {
    proto::WireError::with_code(message, "bad_params")
}

fn forbidden(message: &'static str) -> proto::WireError {
    proto::WireError::with_code(message, "forbidden")
}

fn no_receipt() -> proto::WireError {
    forbidden("没有本次真实创建收据，未授权任务观察或关闭")
}

fn observation_error(error: ObservationFailure) -> proto::WireError {
    proto::WireError::with_code(error.message(), "managed_task_observation_unavailable")
}
