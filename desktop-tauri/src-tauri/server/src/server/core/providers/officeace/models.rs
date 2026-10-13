//! OfficeAce 模型目录：`GET {网关}/v1/models`（OpenAI 格式）+ 持久化缓存。
//!
//! ── 上游形态（依据：officeace2api 的 `server.mjs` 用同一凭据打
//! `{baseUrl}/models`；桌面端 `models.json` 里每条也带 `model_name`）──
//! 标准 OpenAI `{data:[{id, object, created, owned_by}]}`。**只要网关 Basic
//! 凭据**，不需要控制面签名（那套只服务额度/签到）。
//!
//! ── 实测口径：目录 ≠ 可用 ────────────────────────────────────
//! officeace2api 实测：上游目录 34 个名，两个号实际只有 11 个能用，其余回
//! `81004`（没权限）/ `81009`（名字不认）。所以**目录照收**，把「哪些真能用」
//! 交给上游在转发时报错（它的 `model_not_available` 分类），本模块不猜、
//! 也不自作主张过滤 —— 过滤错了会把用户能用的模型挡在门外。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 与各家同款：内存缓存 + `providers::catalog_cache` 落盘（进程重启后读回）。
//! 自动路径 10 分钟 TTL；用户手动「刷新模型清单」时 `force = true` 绕过。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::server::core::providers::catalog_cache;
use crate::server::errors::GatewayError;

use super::credentials::{from_record, OfficeAceCredential};

/// 本家在 `catalog_cache` 里的 scope 名
pub const SCOPE: &str = catalog_cache::SCOPE_OFFICEACE;

/// 自动路径的缓存 TTL（与各家同档）
const TTL_MS: i64 = 10 * 60 * 1000;

/// 进程内缓存
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<CatalogState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

fn with_state<R>(f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mutex = state();
    let mut guard = mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(SCOPE) {
            *guard = Some(CatalogState {
                models: cached.models,
                fetched_at: cached.fetched_at,
            });
        }
    }
    f(&mut guard)
}

/// 当前清单（内存 → 落盘缓存；都没有时为空，由刷新路径填）。
pub fn list() -> Vec<Value> {
    with_state(|slot| slot.as_ref().map(|state| state.models.clone()).unwrap_or_default())
}

/// 这份清单是否来自远程拉取（界面「来源」列的判据）。
pub fn remote_refreshed() -> bool {
    with_state(|slot| {
        slot.as_ref()
            .map(|state| !state.models.is_empty())
            .unwrap_or(false)
    })
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）。
pub fn last_refreshed_at() -> i64 {
    with_state(|slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 每模型倍率的**最后一份非空读数**（`model_name` 小写 → `cost_coefficient`）。
///
/// 为什么要单独留一份，而不是「每次刷新都现取」：倍率走的是控制面那条 V11 签名链
/// （临时 AK/SK，约 2 小时过期），而模型目录走的是**不过期**的网关 Basic —— 两条链寿命不同。
/// 一过期就「取不到 ⇒ 清空」，会让已经有读数的列变成一片 `—`，看起来像「上游不给倍率」，
/// 而真实情况只是我们手上那把临时凭据到期了。宁可旧一点也不假空。
///
/// ⚠️ 这份兜底只在**同一进程内**有效：重启后表是空的，若那一发恰好取不到，这一轮
/// 目录就没有倍率（下一次取到即恢复）。没有把它落到 kv，是因为自动续期正常时这条
/// 路径走不到；真要固化，随目录一起持久化 `credits` 就是它的落点（`attach_rates`
/// 在 `catalog_cache::save` 之前跑，成功那一轮的读数本来就跟着目录落盘了）。
static RATES: OnceLock<Mutex<Vec<(String, f64)>>> = OnceLock::new();

fn rates_slot() -> &'static Mutex<Vec<(String, f64)>> {
    RATES.get_or_init(|| Mutex::new(Vec::new()))
}

fn current_rates() -> Vec<(String, f64)> {
    rates_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 记下这份新读数并返回「本次该用哪一份」：**空表不许把已有读数抹掉**
/// （上游整个挂了与真的没有倍率，回的都是空数组，分不清就不该当"没有"处理）。
fn remember_rates(rows: Vec<(String, f64)>) -> Vec<(String, f64)> {
    if rows.is_empty() {
        return current_rates();
    }
    let mut guard = rates_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = rows.clone();
    rows
}

/// 解析 `{models: [{model_name, cost_coefficient, …}]}`。
///
/// `cost_coefficient` 两种形态都收（JSON 数字与 `"0.30"` 字符串）：上游这条链里
/// 「数值其实是字符串」是**已经踩过一次**的形态（订阅/额度那批字段就是字符串，
/// 见 `subscription.rs`），而倍率解不出来只会静默少一列，不会报错。
/// 没有 `model_name`、数解不出来或**负数**的条目丢掉 —— 负数不是倍率，是脏数据。
pub fn parse_rates(payload: &Value) -> Vec<(String, f64)> {
    let rows = payload.get("models").and_then(Value::as_array);
    let mut out: Vec<(String, f64)> = Vec::new();
    for row in rows.into_iter().flatten() {
        let Some(name) = row.get("model_name").and_then(Value::as_str) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let value = match row.get("cost_coefficient") {
            Some(Value::Number(number)) => number.as_f64(),
            Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
            _ => None,
        };
        let Some(value) = value.filter(|value| value.is_finite() && *value >= 0.0) else {
            continue;
        };
        out.push((name.to_lowercase(), value));
    }
    out
}

/// 把倍率并进目录条目：写 `credits` 文本（界面那一列读它，与别家同一键同名同格式）。
///
/// ── 只按**精确 id** 匹配，不做前缀借用 ──────────────────────
/// 参考实现的面板 `rateFor()` 允许「表里没有这个 id 就蹭同名前缀」，它自己都警告这
/// 样出来的数是「看起来合理但是错的」（`glm-5.3` 蹭到 `glm-5` 的倍率）。倍率是给人
/// 决定用哪个模型的依据，**错一个数量级比空着坏得多** —— 所以这里宁可留 `—`。
/// 大小写不敏感（目录 id 与倍率表的命名偶有大小写差），分隔符不参与匹配。
pub fn attach_rates(models: Vec<Value>, rates: &[(String, f64)]) -> Vec<Value> {
    if rates.is_empty() {
        return models;
    }
    models
        .into_iter()
        .map(|mut model| {
            let id = model
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if let Some((_, value)) = rates.iter().find(|(name, _)| *name == id) {
                if let Some(object) = model.as_object_mut() {
                    object.insert(
                        "credits".to_string(),
                        Value::String(credits_text(*value)),
                    );
                }
            }
            model
        })
        .collect()
}

/// 倍率数字 → 界面文本（`x0.7 credits`）：整数不带小数点，其余最多两位并裁尾零。
/// 两位是必要的 —— 实测读数里有 `0.25` 这种，一位会显示成 `0.3`。
/// 与 `trae::models::credits_text`、`catpaw::catalog::rate_text` 同一取向（本仓第 3 份，
/// 三家的上游给的形态各不相同，先各自保留；要合并得连键名口径一起谈）。
fn credits_text(rate: f64) -> String {
    let rounded = (rate * 100.0).round() / 100.0;
    let plain = if (rounded - rounded.trunc()).abs() < 1e-9 {
        format!("{}", rounded.trunc() as i64)
    } else {
        format!("{rounded:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };
    format!("x{plain} credits")
}

/// 把一个上游条目映成目录条目（保留上游给的字段，缺的补 `officeace`）。
fn map_entry(entry: &Value) -> Option<Value> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if id.is_empty() {
        return None;
    }
    let mut out = json!({
        "name": id,
        "display_name": entry
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(id),
        "description": "",
        "context_length": entry
            .get("context_window")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        // 目录不给输出上限（officeace2api 的 `/v1/models` 也没这个字段）；
        // 留 0 = 「未声明」，界面显示 `—`，不编造。
        "max_output_tokens": 0,
        "credit_display": "",
        "id": id,
    });
    if let Some(object) = out.as_object_mut() {
        // `owned_by` 原样透传（我们的目录形状里叫 `owned_by` 的不多，留着不碍事）
        if let Some(owner) = entry.get("owned_by").and_then(Value::as_str) {
            object.insert("owned_by".to_string(), Value::String(owner.to_string()));
        }
        // 能力位：上游这条链**不报**（officeace2api 的目录也不带 vision/tool 位），
        // 所以不写 `supportsImages` —— 缺席 = 「不知道」，闸门据此不拦（见
        // `catalog/routing.rs` 的 `entry_refuses_images` 与它的模块头）。
        if let Some(developer) = entry.get("developer").and_then(Value::as_str) {
            object.insert("developer".to_string(), Value::String(developer.to_string()));
        }
    }
    Some(out)
}

/// 解析 `{data:[…]}` 或裸数组。
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let rows = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(map_entry).collect()
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
pub async fn refresh(credential: &OfficeAceCredential, force: bool) -> ModelRefreshOutcome {
    let now = crate::server::logging::now_ms();
    if !force {
        let fresh = with_state(|slot| {
            slot.as_ref()
                .map(|state| !state.models.is_empty() && now - state.fetched_at < TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    let url = format!("{}/models", super::chat::normalize_base(&credential.base_url));
    if url == "/models" {
        return ModelRefreshOutcome::failed("OfficeAce 账号缺少模型网关基址");
    }
    let headers = vec![
        (
            "Authorization".to_string(),
            super::chat::basic_authorization(&credential.model_app_key, &credential.model_app_secret),
        ),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let response = match crate::server::core::auth_http::send_raw(
        "GET",
        &url,
        None,
        &headers,
        None,
        Some(20_000),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let text = if error.is_timeout() {
                "OfficeAce 模型目录查询超时".to_string()
            } else {
                format!("OfficeAce 模型目录查询失败：{error}")
            };
            return ModelRefreshOutcome::failed(text);
        }
    };
    if !response.ok {
        return ModelRefreshOutcome::failed(format!(
            "OfficeAce 模型目录返回 HTTP {}（网关不接受这条凭据？）",
            response.status
        ));
    }
    let models = parse_models(&response.payload.unwrap_or(Value::Null));
    if models.is_empty() {
        return ModelRefreshOutcome::failed("OfficeAce 模型目录为空或格式不认识（保留现有清单）");
    }
    // 倍率：另一条链（控制面签名），失败就沿用最后一份读数，别抹空（见 `RATES`）。
    let rates = match super::subscription::fetch_model_token_rates(credential).await {
        Ok(payload) => remember_rates(parse_rates(&payload)),
        Err(error) => {
            crate::server::logging::verbose(
                "[OfficeAce]",
                &format!("模型倍率没取到，沿用上一份读数：{}", error.message),
            );
            current_rates()
        }
    };
    let models = attach_rates(models, &rates);
    // 一条事实留痕：上游给了几条读数、几条对上了目录。少了这条，「只有 4 个模型显示倍率」
    // 就分不清是**上游只发布这几个**还是**两边命名对不上**（后者才需要改匹配规则）。
    let with_rate = models.iter().filter(|model| model.get("credits").is_some()).count();
    crate::server::logging::log(
        "[OfficeAce]",
        &format!(
            "模型倍率：上游给了 {} 条读数，对上目录 {} 条（目录共 {} 条）",
            rates.len(),
            with_rate,
            models.len()
        ),
    );
    let count = models.len();
    with_state(|slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
        });
    });
    catalog_cache::save(SCOPE, &models, now);
    // 目录刷新成功后按需起一轮可用性探测（有 6 小时 TTL + 在途标记，不会因为
    // 目录被反复拉而反复打上游；`force` 由手动「刷新模型清单」透传，给用户一个
    // 「重新探一次」的入口）。探测结论由 `adapter::advertise_models` 用来收窄
    // 对外清单 —— 见 `probe` 模块头。
    super::probe::maybe_spawn(credential, &models, force);
    ModelRefreshOutcome::refreshed(count)
}

/// 从账号记录直接刷（适配器用）。
pub async fn refresh_from_record(record: Option<&Value>, force: bool) -> ModelRefreshOutcome {
    let credential = match from_record(record) {
        Ok(credential) => credential,
        Err(error) => return ModelRefreshOutcome::failed(error.message),
    };
    refresh(&credential, force).await
}

/// 目录为空时的兜底错误（供适配器用）。
pub fn unavailable_error() -> GatewayError {
    GatewayError::with_status(503, "OfficeAce 模型目录尚未拉取：请先在模型管理页点「刷新模型清单」")
}

/// 模型目录刷新结果（与各家同一类型）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_shape_and_skips_entries_without_id() {
        let payload = json!({
            "object": "list",
            "data": [
                {"id": "glm-5.3", "object": "model", "owned_by": "officeace", "display_name": "GLM 5.3", "context_window": 198000},
                {"object": "model"},
                {"id": "  "},
                {"id": "deepseek-v4-flash-0731"}
            ]
        });
        let models = parse_models(&payload);
        assert_eq!(2, models.len(), "只收有 id 的两条");
        assert_eq!("glm-5.3", models[0]["id"]);
        assert_eq!("GLM 5.3", models[0]["display_name"]);
        assert_eq!(198000, models[0]["context_length"]);
        assert_eq!("officeace", models[0]["owned_by"]);
        // 没给 display_name 时回落 id
        assert_eq!("deepseek-v4-flash-0731", models[1]["display_name"]);
        // 不写能力位：缺席 = 不知道（闸门不据此拦）
        assert!(models[0].get("supportsImages").is_none());
        assert_eq!(0, models[0]["max_output_tokens"], "目录不给输出上限，留 0");
    }

    #[test]
    fn accepts_a_bare_array_too() {
        let payload = json!([{"id": "a"}, {"id": "b"}]);
        assert_eq!(2, parse_models(&payload).len());
    }

    /// 倍率表的读法：两种数值形态都收，脏数据丢掉，`model_name` 归一小写。
    #[test]
    fn rates_read_both_number_and_string_shapes() {
        let payload = json!({"models": [
            {"model_name": " GLM-5.2 ", "cost_coefficient": 0.7},
            {"model_name": "deepseek-v4.1-flash", "cost_coefficient": "0.25"},
            {"model_name": "kimi-k2.6", "cost_coefficient": 0},
            {"model_name": "", "cost_coefficient": 1},
            {"cost_coefficient": 1},
            {"model_name": "no-rate"},
            {"model_name": "negative", "cost_coefficient": -1},
            {"model_name": "garbage", "cost_coefficient": "n/a"},
        ]});
        let rates = parse_rates(&payload);
        let names: Vec<&str> = rates.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            vec!["glm-5.2", "deepseek-v4.1-flash", "kimi-k2.6"],
            names,
            "空名/缺字段/负数/非数字都不该产出读数：{rates:?}"
        );
        assert_eq!(Some(&0.7), rates.iter().find(|(name, _)| name == "glm-5.2").map(|(_, value)| value));
        // `0` 是合法读数（免费/限免那一档），不许被「非零才算有」滤掉
        assert!(rates.iter().any(|(name, value)| name == "kimi-k2.6" && *value == 0.0));
    }

    /// 匹配只认精确 id：表里没有 `glm-5.3` 就**不许**蹭 `glm-5` 的读数
    /// （参考实现的面板允许借用并自己警告那样会显示一个看起来合理的错倍率）。
    #[test]
    fn rates_match_exact_ids_only_and_absent_means_no_key() {
        let rates = vec![("glm-5.2".to_string(), 0.75f64)];
        let models = parse_models(&json!({"data": [
            {"id": "glm-5.2"},
            {"id": "glm-5.3"},
            {"id": "GLM-5.2"},
        ]}));
        let merged = attach_rates(models, &rates);
        assert_eq!(
            Some("x0.75 credits"),
            merged[0].get("credits").and_then(Value::as_str),
            "精确命中要落到 credits 上：{}",
            merged[0]
        );
        assert!(
            merged[1].get("credits").is_none(),
            "没有这条 id 就不许借用，留 `—`：{}",
            merged[1]
        );
        assert!(
            merged[2].get("credits").is_some(),
            "大小写差不该让它落到没匹配上：{}",
            merged[2]
        );
        // 空表 ⇒ 原样返回（连"抹掉已有 credits"都不做，读数由 `remember_rates` 兜）
        let untouched = attach_rates(merged.clone(), &[]);
        assert_eq!(
            merged[0].get("credits"),
            untouched[0].get("credits"),
            "空倍率表不该改动条目"
        );
    }

    /// `x1` / `x0` / 两位小数 / 裁尾零：格式与别家那一列对齐。
    #[test]
    fn the_rate_text_keeps_two_decimals_and_trims_zeros() {
        assert_eq!("x1 credits", credits_text(1.0));
        assert_eq!("x0 credits", credits_text(0.0));
        assert_eq!("x0.25 credits", credits_text(0.25));
        assert_eq!("x0.2 credits", credits_text(0.2));
        assert_eq!("x0.08 credits", credits_text(0.083));
    }

    /// 空读数不许把已有读数抹掉（STS 一过期就整列变 `—` 是假空）。
    #[test]
    fn an_empty_rate_table_keeps_the_last_reading() {
        let fresh = vec![("glm-5.1".to_string(), 0.5f64)];
        assert_eq!(1, remember_rates(fresh).len());
        assert_eq!(1, remember_rates(Vec::new()).len(), "空表要回上一份，不是空");
        assert_eq!("glm-5.1", current_rates()[0].0);
    }
}
