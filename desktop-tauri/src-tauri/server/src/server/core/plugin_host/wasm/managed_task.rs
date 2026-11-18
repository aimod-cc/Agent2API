//! 固定受控任务内部模块，服务实现未接准入/ABI，不是已开放的插件能力。
//! 只覆盖 monkeycode.cn.task.v1 / monkeycode / conversation；不注册能力、
//! 不从 JSON 生成权限。可选声明与旧授权撤销不开放执行器。
//! 账号来源/内部装配已接线；真实审批入口和任务ABI仍关闭，服务代码不自行授予执行权。
//!
//! 协议事实已对照原生 providers/monkeycode/task.rs 及官方固定提交：
//! https://github.com/chaitin/MonkeyCode/blob/ad169c9b946ea04a1fc19a7abff9815d38116a73/backend/domain/task.go
//! https://github.com/chaitin/MonkeyCode/blob/ad169c9b946ea04a1fc19a7abff9815d38116a73/backend/biz/task/handler/v1/task.go
//! https://github.com/chaitin/MonkeyCode/blob/ad169c9b946ea04a1fc19a7abff9815d38116a73/backend/biz/task/usecase/task.go
//! 本模块新写封闭校验，不搬上游任务业务、模型选择或 ACP 驱动。

mod account_snapshot;
pub(crate) mod approval_fence;
pub(crate) mod declaration;
mod egress_preparation;
mod executor;
mod input;
mod install_binding;
mod lifecycle;
mod network;
mod observe;
mod ordinary;
pub(super) use ordinary::OrdinaryPolicy;
mod permission_install;
mod permission_store;
pub(crate) mod permissions;
pub(super) mod policy;
pub(super) mod receipt;
mod registry;
pub(super) use registry::PreparingGuard;
pub(in crate::server::core::plugin_host) use registry::shutdown;
mod runtime_slot;
pub(in crate::server::core::plugin_host::wasm) mod service;
mod status;
pub(super) use lifecycle::Record;
pub(super) use runtime_slot::ExecutorSlot;
pub(super) use status::{EndReason, NotSentReason};
mod strict_json;
pub(super) mod types;

pub(crate) fn account_concurrency_limit(provider: &str) -> Option<u64> {
    policy::profile(provider).map(|_| types::UNSETTLED_PER_ACCOUNT as u64)
}
