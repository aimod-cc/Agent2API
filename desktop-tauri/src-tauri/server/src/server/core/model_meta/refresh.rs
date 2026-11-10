//! models.dev 的联网刷新：拉两份官方 JSON → 压成同一个精简索引 → 落库并换内存。
//!
//! ── 压缩口径的唯一事实来源在脚本里 ──────────────────────────
//! 索引的文件形态由 `scripts/sync-modelsdev.mjs` 定义（内嵌快照就是它产出的），
//! 本模块是那份口径的**运行期孪生实现**：同一套「收录哪些模型 / 档位取哪一条 /
//! 索引键怎么归一」。两处必须逐字一致 —— 分叉的症状是「刷新一次之后档位全变回
//! 通用表」，不报错、不抛异常，只是悄悄退化。改这里就要改那边，反之亦然。
//!
//! ── 为什么运行期还要再拉一次（已有内嵌快照）─────────────────
//! 内嵌快照是发版那一刻的数据。models.dev 是社区库，新模型与档位调整都在持续
//! 发生，而我们的上游清单是实时刷新的 —— 只用内嵌快照就会出现「上游已经上了
//! 新模型、元数据里却没有它」。因此与各家模型目录同一节奏：定时刷新 + 手动可
//! 提前，成功即覆盖，失败保留旧值（不退回更旧的内嵌快照）。
//!
//! ── 出网走公网那条链路（不新增配置键）───────────────────────
//! `update::client::fetch_public` —— 与「软件更新」同一条出口解析（设置的更新
//! 出口 > Clash 混合端口 > 直连）。它回答的问题就是「这台机器的公网怎么出去」，
//! 与访问哪个公网站点无关；为 models.dev 单开一个出口设置只会多一个没人配、
//! 出问题时又必须配对的旋钮。
//!
//! ── 排期与失败冷却 ──────────────────────────────────────────
//! 与各家目录刷新共用 `task_state` 的那套：键 `modelRefresh:modelsdev`、间隔取
//! 「模型目录刷新」任务配置的间隔、失败按 `task_state` 的指数退避冷却、手动触发
//! 越过冷却（`ManualBackoff::Bypass`：用户按按钮往往正是刚把网络问题修好）。

use std::collections::HashMap;
use std::io::Read;

use flate2::read::GzDecoder;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::core::update::client;
use crate::server::logging;

use super::snapshot::{self, Record, Snapshot};

const API_URL: &str = "https://models.dev/api.json";
const MODELS_URL: &str = "https://models.dev/models.json";

/// 单次请求总超时（两个官方文件都较大，国内网络经代理经常需要一分钟以上；
/// 与 `scripts/sync-modelsdev.mjs` 保持两分钟，超时即失败、不产半份索引）。
const REQUEST_TIMEOUT_MS: u64 = 120_000;

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
    let backoff = if manual {
        ManualBackoff::Bypass
    } else {
        ManualBackoff::Respect
    };
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
                if manual {
                    state.waiting_message_bypassing()
                } else {
                    state.waiting_message()
                }
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
    let outcome = fetch_and_reduce().await;
    match outcome {
        Ok((snapshot, count)) => {
            snapshot::write_cache(&snapshot::serialize(&snapshot));
            snapshot::replace(snapshot);
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

/// 拉两份官方 JSON 并压成索引。失败时返回**给用户看的原因**（不抛错：这条链路
/// 的每一处失败都已经是一条界面文案，再包一层错误类型没有额外信息）。
async fn fetch_and_reduce() -> Result<(Snapshot, usize), String> {
    let (api, models) = tokio::try_join!(fetch_json(API_URL), fetch_json(MODELS_URL))?;
    let built = reduce(&api, &models).ok_or_else(|| "models.dev 返回的模型条目为空".to_string())?;
    let count = built.len();
    Ok((built, count))
}

async fn fetch_json(url: &str) -> Result<Value, String> {
    let response = client::fetch_public(
        url,
        &[
            ("accept".to_string(), "application/json".to_string()),
            // `models.dev` 的两个官方文件较大，节点通常支持 gzip；
            // 网关的 reqwest 未启用自动解压特性，因此在读完响应后显式解码
            ("accept-encoding".to_string(), "gzip".to_string()),
        ],
        Some(REQUEST_TIMEOUT_MS),
    )
    .await?;
    if !response.status().is_success() {
        return Err(format!("{url} 返回 HTTP {}", response.status().as_u16()));
    }
    let content_encoding = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = response.bytes().await.map_err(|error| {
        if error.is_timeout() {
            format!("{url} 响应读取超时（超过 {} 秒）", REQUEST_TIMEOUT_MS / 1000)
        } else {
            format!("{url} 响应读取失败：{error}")
        }
    })?;
    decode_json_body(url, &content_encoding, bytes.as_ref())
}

fn decode_json_body(
    url: &str,
    content_encoding: &str,
    bytes: &[u8],
) -> Result<Value, String> {
    let decoded = match content_encoding.trim() {
        "" | "identity" => bytes.to_vec(),
        "gzip" => {
            let mut decoded = Vec::new();
            GzDecoder::new(bytes)
                .read_to_end(&mut decoded)
                .map_err(|error| format!("{url} gzip 响应解压失败：{error}"))?;
            decoded
        }
        encoding => return Err(format!("{url} 返回不支持的压缩编码：{encoding}")),
    };
    serde_json::from_slice::<Value>(&decoded).map_err(|error| {
        format!(
            "{url} 响应不是合法 JSON（收到 {} 字节）：{error}",
            decoded.len()
        )
    })
}

/// 两份官方 JSON → 精简索引（与 `scripts/sync-modelsdev.mjs` 逐字同口径）。
fn reduce(api: &Value, models: &Value) -> Option<Snapshot> {
    let mut records: Vec<Record> = Vec::new();
    let mut by_canonical: HashMap<String, usize> = HashMap::new();
    // ① canonical 条目的能力位与上下限
    for (canonical, entry) in models.as_object()? {
        if !is_chat_model(entry) {
            continue;
        }
        if snapshot::key_of(canonical).is_empty() {
            continue;
        }
        let input = entry
            .pointer("/modalities/input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has = |name: &str| input.iter().any(|item| item.as_str() == Some(name));
        by_canonical.insert(canonical.to_ascii_lowercase(), records.len());
        records.push(Record {
            canonical: canonical.clone(),
            name: entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
            reasoning: entry.get("reasoning").and_then(Value::as_bool),
            tool_call: entry.get("tool_call").and_then(Value::as_bool),
            images: Some(has("image")),
            video: Some(has("video")),
            context: limit_of(entry.pointer("/limit/context")),
            output: limit_of(entry.pointer("/limit/output")),
            levels: Vec::new(),
        });
    }
    // ② provider 条目的命名档位（lab 家优先，其次 provider id 字典序）
    let mut picks: HashMap<String, (Vec<String>, bool, String)> = HashMap::new();
    let mut providers: Vec<(&String, &Value)> = api.as_object()?.iter().collect();
    providers.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (provider_id, provider) in providers {
        let Some(entries) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for entry in entries.values() {
            let canonical = entry
                .get("canonical_model_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| entry.get("id").and_then(Value::as_str))
                .unwrap_or_default();
            if snapshot::key_of(canonical).is_empty() {
                continue;
            }
            let pick_key = canonical.to_ascii_lowercase();
            let Some(levels) = effort_levels(entry) else {
                continue;
            };
            let is_lab = canonical.split('/').next() == Some(provider_id.as_str());
            match picks.get(&pick_key) {
                // 实验室自述的那条优先；两条都不是 lab 时按 provider 名序取第一条
                Some((_, true, _)) => continue,
                Some((_, false, _)) if !is_lab => continue,
                _ => picks.insert(pick_key, (levels, is_lab, canonical.to_string())),
            };
        }
    }
    for (levels, _, canonical) in picks.into_values() {
        if let Some(index) = by_canonical.get(&canonical.to_ascii_lowercase()).copied() {
            if let Some(record) = records.get_mut(index) {
                record.levels = levels;
            }
        }
    }
    if records.is_empty() {
        return None;
    }
    Some(Snapshot::new(records, logging::now_ms(), "network"))
}

/// 正整数上限，其余（0 / 负数 / 小数 / 非数字）→ `None`（=「这份数据没说」）。
fn limit_of(value: Option<&Value>) -> Option<u64> {
    let number = value?.as_f64()?;
    if !number.is_finite() || number < 1.0 {
        return None;
    }
    Some(number.trunc() as u64)
}

/// 只收录**能出文本**的对话模型（embedding / reranking / 生图生视频都不要）。
fn is_chat_model(entry: &Value) -> bool {
    if let Some(kind) = entry.get("type").and_then(Value::as_str) {
        if kind != "chat" {
            return false;
        }
    }
    match entry
        .pointer("/modalities/output")
        .and_then(Value::as_array)
    {
        Some(output) => output.iter().any(|item| item.as_str() == Some("text")),
        None => true,
    }
}

/// 从一条 provider 条目里读**命名档位**；只有 `effort` 那一种形态算数
/// （`toggle` / 空数组 = 没给命名档位 → `None`，网关侧回落通用表）。
fn effort_levels(entry: &Value) -> Option<Vec<String>> {
    let options = entry.get("reasoning_options").and_then(Value::as_array)?;
    let effort = options
        .iter()
        .find(|option| option.get("type").and_then(Value::as_str) == Some("effort"))?;
    let levels: Vec<String> = effort
        .get("values")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|level| !level.is_empty())
        .map(str::to_string)
        .collect();
    (!levels.is_empty()).then_some(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    #[test]
    fn gzip_json_is_decoded_before_parsing() {
        let body = br#"{"models":{"demo/model":{"name":"Demo"}}}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body).expect("gzip input");
        let compressed = encoder.finish().expect("gzip finish");
        let value = decode_json_body("https://models.dev/models.json", "gzip", &compressed)
            .expect("gzip JSON should decode");
        assert_eq!(value["models"]["demo/model"]["name"], "Demo");
    }

    #[test]
    fn unsupported_content_encoding_is_reported() {
        let error = decode_json_body("https://models.dev/models.json", "br", b"{}")
            .expect_err("unsupported encoding must fail");
        assert!(error.contains("不支持的压缩编码"));
    }
}
