//! 创建/停止响应的纯解析器。ParsedCreation 不是 CreatedReceipt：它没有调用、
//! 账号、出口、批准信息或真实发送证明。执行器须确认真实发送并保存清理责任后
//! 才可产生内部权限收据；没有 ABI 入口，也不提供从任意 taskId 恢复接口。
//! 官方停止只确认接口受理，VM 回收错误可被上游记录后忽略，不能称已确定清理。

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    policy,
    strict_json::{self, Object},
    types::{self, NonZeroUuid, ValidationError},
};

/// 调用方必须传真实传输结果；这里的类型不能证明网络或账号授权。
/// 不接收错误原文，超时/断网只使用固定分类。
pub(super) enum ResponseInput<'a> {
    Complete {
        http_status: u16,
        bytes: &'a [u8],
    },
    Incomplete {
        http_status: Option<u16>,
        reason: IncompleteReason,
    },
}

pub(super) enum IncompleteReason {
    Timeout,
    Disconnected,
    BodyLimit,
    Cancelled,
}

pub(super) enum CreationOutcome {
    Parsed(ParsedCreation),
    Rejected(Failure),
    Unknown(Failure),
}

/// 仅 parser 能构造：私有 ID，不实现 Clone/Debug/Serialize/Deserialize。
/// 非2xx、结构模糊或缺少成功码时不会保留任何可用于停止的编号。
pub(super) struct ParsedCreation {
    id: NonZeroUuid,
    http_status: u16,
}

pub(super) struct Failure {
    http_status: Option<u16>,
    business_code: Option<i64>,
    message: &'static str,
}

impl Failure {
    fn new(http_status: Option<u16>, business_code: Option<i64>, message: &'static str) -> Self {
        Self {
            http_status,
            business_code,
            message,
        }
    }

    fn projection(&self, state: &'static str) -> Value {
        projection(state, self.http_status, self.business_code, self.message)
    }

    pub(super) fn executor_unknown() -> Self {
        Self::new(
            None,
            None,
            "受控执行绑定异常，未提交后续请求；远端状态未确认且不自动重放",
        )
    }

    pub(super) fn http_status(&self) -> Option<u16> {
        self.http_status
    }
    pub(super) fn business_code(&self) -> Option<i64> {
        self.business_code
    }
    pub(super) fn message(&self) -> &'static str {
        self.message
    }
}

fn projection(
    state: &'static str,
    status: Option<u16>,
    code: Option<i64>,
    message: &'static str,
) -> Value {
    json!({ "state": state, "httpStatus": status, "businessCode": code, "message": message })
}

impl CreationOutcome {
    /// 公开白名单故意不含 taskId、opaque收据、正文、Set-Cookie或响应原文。
    pub(super) fn projection(&self) -> Value {
        match self {
            Self::Parsed(creation) => projection(
                "parsed",
                Some(creation.http_status),
                Some(0),
                "创建响应符合固定成功结构；仅完成解析，尚未授予任务或清理权限",
            ),
            Self::Rejected(failure) => failure.projection("rejected"),
            Self::Unknown(failure) => failure.projection("unknown"),
        }
    }
}

impl ParsedCreation {
    pub(super) fn http_status(&self) -> u16 {
        self.http_status
    }

    /// 仅从此内部已验证创建编号构造固定停止正文；构造不等于允许发送。
    pub(super) fn stop_body(&self) -> Result<Vec<u8>, ValidationError> {
        serde_json::to_vec(&json!({ "id": self.id.as_str() }))
            .map_err(|_| ValidationError::new("固定停止正文构造失败，未替换任务编号"))
    }

    pub(super) fn detail_route(&self, provider: &str) -> Result<policy::Route, ValidationError> {
        policy::detail_route(provider, &self.id)
    }
    pub(super) fn stream_route(&self, provider: &str) -> Result<policy::Route, ValidationError> {
        policy::stream_route(provider, &self.id)
    }

    /// 只比对编号，不向读取者导出UUID；匹配结果自身不构成任何观察许可。
    pub(super) fn matches_detail(
        &self,
        detail: &super::observe::DetailObservation,
    ) -> Option<bool> {
        detail.matches_id(&self.id)
    }
}

#[derive(Deserialize)]
struct CreationEnvelope {
    // 第一遍已拒绝重复键/无效类型；第二遍只读必需字段，忽略其他敏感响应值。
    data: Object<CreationData>,
}

#[derive(Deserialize)]
struct CreationData {
    id: String,
}

fn valid_status(status: u16) -> bool {
    (100..600).contains(&status)
}
fn success_status(status: u16) -> bool {
    (200..300).contains(&status)
}

fn incomplete(http_status: Option<u16>, reason: IncompleteReason) -> Failure {
    let message = match reason {
        IncompleteReason::Timeout => "上游请求超时，结果未知；没有可授予权限的完整回应",
        IncompleteReason::Disconnected => "上游连接或正文读取中断，结果未知",
        IncompleteReason::BodyLimit => "上游回应超过受控大小，结果未知",
        IncompleteReason::Cancelled => "请求等待已中断，结果未知；不能据此认定上游未执行操作",
    };
    Failure::new(
        http_status.filter(|status| valid_status(*status)),
        None,
        message,
    )
}

pub(super) fn parse_creation(input: ResponseInput<'_>) -> CreationOutcome {
    let (status, bytes) = match input {
        ResponseInput::Complete { http_status, bytes } => (http_status, bytes),
        ResponseInput::Incomplete {
            http_status,
            reason,
        } => {
            return CreationOutcome::Unknown(incomplete(http_status, reason));
        }
    };
    if !valid_status(status) {
        return CreationOutcome::Unknown(Failure::new(
            None,
            None,
            "传输层未提供有效 HTTP 状态，创建结果未知",
        ));
    }
    let shape = strict_json::validate(bytes, types::MAX_CREATION_RESPONSE_BYTES);
    // HTTP失败优先拒绝，body即使看似成功也不解析或保留编号。
    // Rejected只表示本次回应拒绝成功判定，不承诺上游没有副作用或允许自动重放。
    if !success_status(status) {
        let code = if shape.is_ok() {
            strict_json::code(bytes)
        } else {
            None
        };
        return CreationOutcome::Rejected(Failure::new(
            Some(status),
            code,
            "创建请求被 HTTP 明确拒绝，未授予任务编号或清理权限",
        ));
    }
    if let Err(error) = shape {
        return CreationOutcome::Unknown(Failure::new(
            Some(status),
            None,
            match error {
                strict_json::JsonError::TooLarge => "创建回应超过 1MiB，结果未知，未提取任务编号",
                strict_json::JsonError::Invalid => {
                    "创建回应不是严格 JSON、过深或含重复字段，结果未知"
                }
            },
        ));
    }
    let Some(code) = strict_json::code(bytes) else {
        return CreationOutcome::Unknown(Failure::new(
            Some(status),
            None,
            "创建回应缺少整数业务码，结果未知；字符串、布尔或小数不能当成功",
        ));
    };
    if code != 0 {
        return CreationOutcome::Rejected(Failure::new(
            Some(status),
            Some(code),
            if code == 10811 {
                "上游任务名额已占用；未枚举或停止其他任务"
            } else {
                "创建回应包含明确失败业务码，未授予清理权限"
            },
        ));
    }
    let id = serde_json::from_slice::<Object<CreationEnvelope>>(bytes)
        .ok()
        .and_then(|response| NonZeroUuid::parse(&response.0.data.0.id));
    match id {
        Some(id) => CreationOutcome::Parsed(ParsedCreation {
            id,
            http_status: status,
        }),
        None => CreationOutcome::Unknown(Failure::new(
            Some(status),
            Some(code),
            "创建回应没有合法的 /data/id 非零UUID，结果未知；不尝试任何别名或任务列表",
        )),
    }
}

pub(super) enum StopOutcome {
    Acknowledged { http_status: u16 },
    Rejected(Failure),
    Unknown(Failure),
}

impl StopOutcome {
    pub(super) fn projection(&self) -> Value {
        match self {
            Self::Acknowledged { http_status } => projection(
                "acknowledged",
                Some(*http_status),
                Some(0),
                "停止接口已确认受理；不能据此证明执行环境已回收",
            ),
            Self::Rejected(failure) => failure.projection("rejected"),
            Self::Unknown(failure) => failure.projection("unknown"),
        }
    }
}

pub(super) fn parse_stop(input: ResponseInput<'_>) -> StopOutcome {
    let (status, bytes) = match input {
        ResponseInput::Complete { http_status, bytes } => (http_status, bytes),
        ResponseInput::Incomplete {
            http_status,
            reason,
        } => {
            return StopOutcome::Unknown(incomplete(http_status, reason));
        }
    };
    if !valid_status(status) {
        return StopOutcome::Unknown(Failure::new(
            None,
            None,
            "传输层未提供有效 HTTP 状态，停止结果未知",
        ));
    }
    let shape = strict_json::validate(bytes, types::MAX_STOP_RESPONSE_BYTES);
    if !success_status(status) {
        let code = if shape.is_ok() {
            strict_json::code(bytes)
        } else {
            None
        };
        return StopOutcome::Rejected(Failure::new(
            Some(status),
            code,
            "停止请求被 HTTP 明确拒绝，没有确认清理",
        ));
    }
    if let Err(error) = shape {
        return StopOutcome::Unknown(Failure::new(
            Some(status),
            None,
            match error {
                strict_json::JsonError::TooLarge => "停止回应超过 64KiB，结果未知，不自动重试",
                strict_json::JsonError::Invalid => {
                    "停止回应不是严格 JSON、过深或含重复字段，结果未知"
                }
            },
        ));
    }
    match strict_json::code(bytes) {
        Some(0) => StopOutcome::Acknowledged {
            http_status: status,
        },
        Some(code) => StopOutcome::Rejected(Failure::new(
            Some(status),
            Some(code),
            "停止接口返回失败业务码，没有确认清理",
        )),
        None => StopOutcome::Unknown(Failure::new(
            Some(status),
            None,
            "停止回应缺少整数成功业务码，结果未知",
        )),
    }
}
