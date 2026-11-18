//! 错误不含令牌、原始上游正文或调用者 JSON；发送事实另由原 Record 保存。
use agent2api_provider_protocol as proto;

pub(super) fn invalid() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 固定轮次材料无效或与本次准备不一致，未发送",
        "catpaw_turn_invalid_input",
    )
}
pub(super) fn budget() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 轮次材料超过有界大小、深度或数量限制，未发送",
        "catpaw_turn_resource_limit",
    )
}
pub(super) fn unavailable() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 原调用、账号来源或执行体已失效，未交付替代凭据或自动重发",
        "catpaw_turn_source_unavailable",
    )
}
pub(super) fn busy() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本客户端会话已有真实轮次，当前不支持替换或工具续接，未发送新轮次",
        "catpaw_turn_session_busy",
    )
}
pub(super) fn ended() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本次等待已取消或调用结束；已提交结果仍由原所有者承担，未自动重发",
        "catpaw_turn_call_ended",
    )
}
pub(super) fn conflict() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本次轮次准备已被消费或状态已变化，未重复提交",
        "catpaw_turn_already_started",
    )
}
pub(super) fn unknown() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本次上游提交结果未确认，未交付成功或自动重发",
        "catpaw_turn_submission_unconfirmed",
    )
}
pub(super) fn rejected() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 上游拒绝了本次固定轮次请求，未自动重发或替换会话",
        "catpaw_turn_upstream_rejected",
    )
}
pub(super) fn stream() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw turn 未返回本次固定请求的 HTTP 200 SSE 流，未交付成功",
        "catpaw_turn_stream_unconfirmed",
    )
}
pub(super) fn read_conflict() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 读取序号、原回执或状态冲突，未并发读取、重复读取或跳号",
        "catpaw_turn_read_conflict",
    )
}
pub(super) fn delivery() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本笔流数据交付未确认，原预约已消费，未重读或补发",
        "catpaw_turn_read_delivery_unconfirmed",
    )
}
pub(super) fn protocol() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 原上游流中断、错误、超预算或终态不完整，未确认本轮成功",
        "catpaw_turn_protocol_unconfirmed",
    )
}
pub(super) fn cleanup() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 本次原轮次停止或终态受理未确认，未猜测回收或自动重试",
        "catpaw_turn_cleanup_unconfirmed",
    )
}
