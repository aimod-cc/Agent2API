//! `/api/accounts/usage` 的实现（**薄壳**：查询逻辑已下沉到
//! `core::usage_query`，见那里的模块头）。
//!
//! 本文件只做三件事：解析 `?id=`、调 core 拿结果、把 `TargetError` 转成管理
//! 信封的响应。目标集合解析、跨账号并发、单账号失败收敛、401 的刷新重试全在 core
//! ——「每账号自动查询」的心跳循环走同一份逻辑，而它不认识 axum。
//!
//! ── `?id=` 与批量是两条语义 ─────────────────────────────
//! 不带 `id` = 批量（目标集合是「全部**可用**账号」，供工具栏的「查询余额」）；
//! 带 `id` = 查这一个账号（账号页每一行的「余额」按钮走这条）。
//! **两条都不看启用状态**：禁用只表示「不参与转发」，与其余额能否查无关 ——
//! 按启用状态把批量挡掉，界面只会让那些行永远停在「未查询」，用户只能逐个手点
//! （那正是「自动查询看起来没生效」的来源）。凭证不完整的账号（available:false）
//! 仍在批量目标之外，`skipped` 记的就是它们的数量。
//! 判据落在 `core::usage_query::query_all` 的 `id` 参数上（那一段有完整说明）。
//!
//! ── 额外的一条：最近查询结果的快照 ─────────────────────────
//! `GET /api/accounts/usage/snapshot` 读记录表里各账号的最近结论（含失败行），
//! 供界面在**不点按钮**的情况下跟上自动查询的节奏（每账号到期的写法下，
//! 界面轮询一份快照就能拿到全部账号的最新读数）。

use axum::response::Response;

use crate::server::core::usage_query;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, query_param};
use crate::server::ServerState;

/// GET /api/accounts/usage
///
/// 逐账号并发查询余额 / 积分汇总（`{ results: [{id,name,usage,error,code?}], skipped }`）。
/// 用户手动点「查询余额」走这条；`?id=` 则只查那一个账号（单行「余额」按钮）。
/// 查询行为见 `core::usage_query::query_all`。
pub async fn accounts_usage(state: &ServerState, query: &str) -> Response {
    // 空串与缺失同义（前端 `''` 时不带 id），所以这里 filter 掉空值再往下传
    let id = query_param(query, "id").filter(|value| !value.is_empty());
    match usage_query::query_all(state.store(), id.as_deref()).await {
        Ok(report) => ok_json(report),
        Err(error) => management_error(error.status_code as i32, error.message),
    }
}

/// GET /api/accounts/usage/snapshot
///
/// 各账号最近一次查询结果的快照
/// （`{ at, results: [{id,name,usage,error,code?,at}], skipped }`，`at` 是毫秒
/// 时间戳、0 表示还没查过；行上的 `at` 是**这一行**的结论时刻）。
///
/// 形状与 `/api/accounts/usage` 的响应兼容（每行多一个自己的 `at`）：
/// 前端因此可以用同一个 `applyBalances` 写缓存，不必为「手动」与「自动」两条
/// 来源各写一套解析。行级 `at` 是按账号到期查询的要求 —— 各行的结论时刻天然
/// 不同，失败行的时效判定必须按行算。
///
/// 读快照要带上账号存储：出口会丢掉「账号已删除」的行、以及「比账号记录还旧」
/// 的失败行（见 `core::usage_query::snapshot` 的说明）。
pub async fn accounts_usage_snapshot(state: &ServerState) -> Response {
    ok_json(usage_query::snapshot(state.store()))
}
