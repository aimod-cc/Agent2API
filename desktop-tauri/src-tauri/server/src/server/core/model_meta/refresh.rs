//! models.dev 模型元数据的联网刷新：从官网 CDN（兜底官网 API）拉取精简索引 → 落库并换内存。
//!
//! ── 同源生成与分发（官网后台定时同步）──────────────────────────
//! 管理后台定时从 `models.dev` 拉取原始数据，在服务端同时生成并上传到官网 CDN：
//!   · 精简能力与档位表（`models-metadata.json`，本模块拉取，约 90 KB）
//!   · 完整公共模型目录（`models-catalog.json`，脚本插件按需拉取）
//! 客户端不再直连 `models.dev` 下载几 MB 的原始文件，也不必在本地重复做提炼计算。
//!
//! ── 为什么运行期还要再拉一次（已有内嵌快照）─────────────────
//! 内嵌快照是发版那一刻的数据。models.dev 是社区库，新模型与档位调整都在持续
//! 发生，而我们的上游清单是实时刷新的 —— 只用内嵌快照就会出现「上游已经上了
//! 新模型、元数据里却没有它」。因此与各家模型目录同一节奏：定时刷新 + 手动可
//! 提前，成功即覆盖，失败保留旧值（不退回更旧的内嵌快照）。
//!
//! ── 出网走官网受控分发链路 ──────────────────────────────────
//! `update::website::fetch_json_with_fallback` —— 优先读 `cdn.agent2api.dev`，
//! 失败时自动回退 `agent2api.dev`，复用「软件更新」同一条出口候选解析与域名安全校验。
//!
//! ── 排期与失败冷却 ──────────────────────────────────────────
//! 与各家目录刷新共用 `task_state` 的那套：键 `modelRefresh:modelsdev`、间隔取
//! 「模型目录刷新」任务配置的间隔、失败按 `task_state` 的指数退避冷却、手动触发
//! 越过冷却（`ManualBackoff::Bypass`：用户按按钮往往正是刚把网络问题修好）。

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::core::update::website;
use crate::server::logging;

use super::snapshot::{self, Snapshot};

/// 单次请求总超时。
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 两次刷新之间的最小间隔（防连点 / 防两个进程并排打；手动也不越过）。
const MIN_GAP_MS: i64 = 5_000;

/// 排期键（`task_state`）：与各家的 `modelRefresh:<provider>` 同一命名空间，
/// 于「重置模型刷新间隔」时能被一并重排（见 `providers::catalog_refresh::reschedule`）。
pub(crate) const SCHEDULE_SCOPE: &str = "modelsdev";

/// 刷新一次并把结果包成**一行**（字段与各家目录刷新的结果行同形，见
/// `providers::adapter::refresh_implemented_forced` 的模块头）。
///
/// `interval_ms` 是本轮之后要排的间隔（调用方从「模型目录刷新」任务的配置读，
/// 与各家目录刷新共用同一个间隔旋钮）。
pub async fn refresh(manual: bool, interval_ms: i64) -> Value {
    let mut row = json!({
        // 伪 provider：它不是一家提供商（没有账号、没有上游目录），但结果行沿用
        // 与各家目录刷新同一套字段，界面才能用同一套徽章渲染（见 `api::model_meta`）。
        "provider": SCHEDULE_SCOPE,
        "providerLabel": "models.dev 模型库",
        "refreshedAt": snapshot::origin().1,
    });
    if !config::models_dev_enabled() {
        row["status"] = json!("skipped");
        row["message"] = json!("已在设置里关闭 models.dev 模型元数据");
        return row;
    }
    let backoff = if manual { ManualBackoff::Bypass } else { ManualBackoff::Respect };
    let guard = match task_state::claim(
        &format!("modelRefresh:{SCHEDULE_SCOPE}"),
        interval_ms,
        manual,
        backoff,
        MIN_GAP_MS,
    ) {
        Ok(Claim::Acquired(guard)) => guard,
        Ok(Claim::Deferred(state)) => {
            row["status"] = json!("skipped");
            // 手动触发越过冷却（`ManualBackoff::Bypass`），这时再说「冷却中」
            // 会与「按钮其实能立刻重试」自相矛盾 —— 与各家目录刷新同一口径，
            // 见 `providers::catalog_refresh::deferred_message` 的说明。
            row["message"] = json!(if state.retry_at > logging::now_ms() || state.running() {
                if manual { state.waiting_message_bypassing() } else { state.waiting_message() }
            } else {
                "沿用缓存，尚未到刷新时间".to_string()
            });
            row["retryAt"] = json!(state.retry_at);
            return row;
        }
        Err(error) => {
            row["status"] = json!("failed");
            row["message"] = json!(error);
            return row;
        }
    };
    let outcome = fetch_snapshot().await;
    match outcome {
        Ok((snapshot, count)) => {
            snapshot::write_cache(&snapshot::serialize(&snapshot));
            snapshot::replace(snapshot);
            // 若用户启用了脚本插件或本地已有公共模型目录缓存，同一轮顺带触发从官网同步
            crate::server::core::plugin_host::script::refresh_model_catalog_if_active(manual);
            let summary = format!("models.dev 元数据已更新（{count} 条）");
            logging::log("[Models]", &summary);
            row["status"] = json!("refreshed");
            row["count"] = json!(count);
            row["refreshedAt"] = json!(logging::now_ms());
            let _ = guard.finish(true, summary, None, 0, interval_ms);
        }
        Err(message) => {
            row["status"] = json!("failed");
            row["message"] = json!(message);
            let _ = guard.finish(false, message, None, 0, interval_ms);
        }
    }
    row
}

/// 从官网 CDN（兜底官网 API）拉取精简索引快照。
async fn fetch_snapshot() -> Result<(Snapshot, usize), String> {
    let response = website::fetch_json_with_fallback(
        website::CDN_MODELS_METADATA_URL,
        website::API_MODELS_METADATA_URL,
        REQUEST_TIMEOUT_MS,
    )
    .await
    .map_err(|error| error.message)?;
    if !response.status.is_success() {
        return Err(format!("官网模型元数据服务返回 HTTP {}", response.status.as_u16()));
    }
    let built = snapshot::parse_value(&response.value, "network")
        .ok_or_else(|| "官网返回的 models.dev 模型条目为空或格式无效".to_string())?;
    let count = built.len();
    Ok((built, count))
}
