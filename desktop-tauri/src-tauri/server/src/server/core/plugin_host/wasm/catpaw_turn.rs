//! CatPaw 固定 round -> running -> turn 的唯一宿主所有者与有界读流。
//! 关闭只消费原受理回执；结算只支持本次无工具新轮，不开放工具或会话续接。

mod catalog;
mod cleanup;
mod handoff_cleanup;
mod close;
mod error;
mod input;
mod json_text;
mod local_gate;
mod message;
mod network;
mod owner;
pub(in crate::server) mod output;
mod output_view;
mod protocol;
mod read;
mod reader;
mod registry;
pub(super) mod service;
mod settle;
mod settlement;
mod source;
mod tool_evidence;
mod worker;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use std::sync::Arc;

use super::conversation::SourceRef;
use super::catpaw_account::Account as RowSource;

pub(super) use close::CloseWait;
pub(super) use read::ReadWait;
pub(super) use registry::Registry;
pub(super) use settle::SettlementWait;
pub(super) use worker::CreationWait;

pub(super) const PROFILE: &str = "catpaw.turn.v1";
const PROVIDER: &str = "catpaw";

/// 本地受控表只能由原 Shared 创建/取得；调用方不能用 JSON 选择另一个表或账号。
pub(super) fn prepare(
    registry: &Arc<Registry>,
    source: SourceRef,
    account: RowSource,
    params: Value,
) -> Result<Value, proto::WireError> {
    registry::prepare(registry, source, account, params)
}

/// 同步建立唯一 worker 和退出所有者；创建 future 第一次 poll 前完成登记。
/// 等待端取消不 abort 已提交的 round；后续网络/响应一直属于原 Owner。
pub(super) fn start(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<CreationWait, proto::WireError> {
    worker::start(registry, source, params)
}

/// 同步占用从0严格递增的唯一读预约，Future未poll也不可重新领取。
pub(super) fn read(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<ReadWait, proto::WireError> {
    read::read(registry, source, params)
}

/// 只关闭原SourceRef的原句柄；等待取消不会撤销已发停止或终态请求。
pub(super) fn close(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<CloseWait, proto::WireError> {
    close::close(registry, source, params)
}

/// 同步消费唯一结算预约，原worker独立核本次原字节证据，只完成无工具新轮。
pub(super) fn settle(
    registry: &Arc<Registry>,
    source: &SourceRef,
    params: Value,
) -> Result<SettlementWait, proto::WireError> {
    settle::settle(registry, source, params)
}
