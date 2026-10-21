//! 报表计算 —— 对内存快照的**纯函数**变换，不碰磁盘、不持锁。
//!
//! 为什么单独一层：报表口径是本切片里最容易出细节错的部分（零除、补零、
//! 连续天数、并列比较、时区边界），集中在一起才能一眼看清「同一天/同一小时」
//! 的口径是否处处一致；也便于后续改动时只碰这一处。
//!
//! ── 数据来源的分工 ──────────────────────────────────────────
//!   - `overview` / `dailyTrend` / `heatmap` / `streak` 走**聚合**：
//!     这些口径要跨年（热力图固定 365 天、`all` 可能一年以上），
//!     而明细只留 30 天，靠明细算会越算越少。
//!   - `cacheRates` 走**SQL 侧聚合**（`sql::sum_cache_windows` 一次算出四档
//!     窗口的合计，本层只做零除与百分比）：四档窗口最长 7 天，而「按窗口求和」
//!     没有精度损失 —— 把窗口内的明细搬回内存再遍历四遍，算出来的是一样的数。
//!   - `cacheTrend24h` 走**明细**：要精确到本地整点，按天的聚合行给不出这种
//!     精度；也只取近 24 小时的窄列（`sql::select_summary_rows`）。

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
use serde_json::{json, Value};

use super::clock::{date_key, hour_floor, hour_key, ms_to_local, now_ms};
use super::record::{AccountAccum, DailyEntry, ModelAccum, ProviderAccum, RequestEntry};

/// 未知 provider 的展示名（`id` 为空串的那一组：旧版本写出的明细、
/// 以及「一次都没发出去就失败」的请求）。前端也会用同样的文案做降级显示。
pub(super) const UNKNOWN_PROVIDER_LABEL: &str = "未知";

/// 未知账号的展示名（没有账号身份的那一组）。
///
/// 与 provider 那条是**两个独立的常量**而不是共用一个：两者的「未知」
/// 成因不同（前者是没记下承载的家，后者是没走账号列表），文案将来若分叉，
/// 共用会让改一处等于改两处。
pub(super) const UNKNOWN_ACCOUNT_LABEL: &str = "未知账号";

/// 热力图固定返回的天数（与 range 解耦）
pub(super) const HEATMAP_DAYS: i64 = 365;

/// 报表要读的**明细窄列**（`sql::SUMMARY_COLUMNS` 的结构形态）。
///
/// 为什么要有这个独立类型（不复用 `RequestEntry`）：报表对明细的消费只有
/// 「按整点归桶 + 按模型累计」两件事，用到的字段就是这 6 个。带上完整
/// `RequestEntry` 意味着读取侧要按 25 列去取行（`route` / `attempt_details`
/// 都在里面），而这份数据一个字段都用不上 —— 类型窄一分，读取那一侧就没法
/// 「顺手多取几列」（见 `sql::select_summary_rows` 与 `db::schema` 的 v15 段）。
#[derive(Clone, Debug)]
pub(super) struct SummaryRow {
    pub ts: i64,
    pub model: String,
    /// 上游实际发出的模型名（空串 = 一次都没发出去），统计键的回落见
    /// [`model_stat_key`]
    pub upstream_model: String,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub prompt_tokens: i64,
}

/// 四档缓存命中率窗口：**JSON 键名 + 窗口毫秒**。
///
/// 两件事绑在一处是必须的：`cache_rates` 按这个数组的顺序取出 SQL 侧那四组
/// 合计（`sql::sum_cache_windows` 的返回顺序与入参一一对应），键名又直接是
/// 前端读的 `cacheRates.last10m` 等字段 —— 分成两个数组（一个放键名、一个放
/// 毫秒值）迟早会漂成「第三档的键配了第二档的窗口」，且极难从界面上看出来。
pub(super) const CACHE_WINDOWS: [(&str, i64); 4] = [
    ("last10m", 10 * 60_000),
    ("last1h", 60 * 60_000),
    ("last24h", 24 * 60 * 60_000),
    ("last7d", 7 * 24 * 60 * 60_000),
];

/// 逐天序列 / 连续天数的循环上限（兜底手改出来的超长区间）
const MAX_TREND_DAYS: i64 = 4000;

/// 合法的时间区间取值
const RANGE_TODAY: &str = "today";
/// 「昨天」：本地自然日的昨天 0 点至 24 点（与 `today` 共用同一套日界线）
const RANGE_YESTERDAY: &str = "yesterday";
const RANGE_7: &str = "7";
const RANGE_30: &str = "30";
const RANGE_MONTH: &str = "month";
const RANGE_ALL: &str = "all";
/// 自定义区间：起止日由查询串的 `startDate` / `endDate` 给出（路由层已校验），
/// 存储层只负责把这一档与那两个日期对上。
pub(super) const RANGE_CUSTOM: &str = "custom";

/// range 归一：非法值按 `"7"`（取舍见 `usage_summary` 的注释）
pub(super) fn normalize_range(range: &str) -> &'static str {
    match range.trim() {
        RANGE_TODAY => RANGE_TODAY,
        RANGE_YESTERDAY => RANGE_YESTERDAY,
        RANGE_30 => RANGE_30,
        RANGE_MONTH => RANGE_MONTH,
        RANGE_ALL => RANGE_ALL,
        RANGE_CUSTOM => RANGE_CUSTOM,
        _ => RANGE_7,
    }
}

/// 区间边界（闭合的本地日期键 `[startDate, endDate]`）。
///
/// - `today` 就是今天；`yesterday` 是昨天一整天（昨天 0 点 ~ 昨天 24 点，
///   与 `today` 共用同一套本地日界线）
/// - `7` / `30` 从今天往前推（含今天，所以减 days-1）
/// - `month` 为本月 1 号到今天（本地时区）
/// - `all` 取聚合里最早的一天；**完全没有数据**时给「今天往前 29 天」，
///   让前端拿到一个合法区间而不是空串或 null
/// - `custom` 用调用方给出的两个日期（路由层已解析校验 `YYYY-MM-DD`）；
///   `custom` 却没带日期（不该出现，路由层会先拦）时按 `7` 兜底 ——
///   宁可给一个能显示的口径，也不要让整页报表 500
///
/// 归一的两条防御（都只兜手改查询串 / 时钟回拨这类外部输入）：
///   - `start > end` 时交换：反向区间会让前端画出负长度时间轴；
///   - `end` 夹到今天：未来的日期没有任何数据，如实回显一个「比今天晚」的
///     区间只会让图例与读数看起来像坏了（聚合里本来就不会有未来的行）。
pub(super) fn range_bounds(
    range: &str,
    daily: &BTreeMap<String, DailyEntry>,
    today: NaiveDate,
    custom: Option<(NaiveDate, NaiveDate)>,
) -> (String, String) {
    if range == RANGE_CUSTOM {
        if let Some((custom_start, custom_end)) = custom {
            let (custom_start, custom_end) = if custom_start > custom_end {
                (custom_end, custom_start)
            } else {
                (custom_start, custom_end)
            };
            let end = custom_end.min(today);
            let start = custom_start.min(end);
            return (date_key(start), date_key(end));
        }
    }
    // 「昨天」是唯一 end 不是今天的档位：单日区间 = 昨天 0 点到昨天 24 点。
    // 先于 `end = today` 处理，免得把今天的行算进来。
    if range == RANGE_YESTERDAY {
        let yesterday = today - ChronoDuration::days(1);
        return (date_key(yesterday), date_key(yesterday));
    }
    let end = today;
    let start = match range {
        RANGE_TODAY => today,
        RANGE_30 => today - ChronoDuration::days(29),
        RANGE_MONTH => today.with_day(1).unwrap_or(today),
        RANGE_ALL => daily
            .keys()
            .next()
            .and_then(|key| NaiveDate::parse_from_str(key, "%Y-%m-%d").ok())
            .unwrap_or_else(|| today - ChronoDuration::days(29)),
        // RANGE_7 及任何落到这里的值
        _ => today - ChronoDuration::days(6),
    };
    // 极端情况：聚合里有「未来」的行（系统时钟被往前调过），start 不能晚于 end，
    // 否则区间是反向的，前端画图会出现负长度时间轴
    let start = if start > end { end } else { start };
    (date_key(start), date_key(end))
}

/// 区间内的总量与按模型 / 按 provider / 按账号累计（overview 的原料）
pub(super) struct RangeTotals {
    pub requests: i64,
    pub successful: i64,
    pub tokens: i64,
    pub active_days: i64,
    pub model_totals: Vec<ModelAccum>,
    pub provider_totals: Vec<ProviderAccum>,
    pub account_totals: Vec<AccountAccum>,
}

/// 按区间累计聚合行。`BTreeMap::range` 直接按日期键取闭区间 ——
/// 定长日期串的字典序即时间序，这是选 `BTreeMap` 的直接收益。
pub(super) fn range_totals(
    daily: &BTreeMap<String, DailyEntry>,
    start: &str,
    end: &str,
) -> RangeTotals {
    let mut totals = RangeTotals {
        requests: 0,
        successful: 0,
        tokens: 0,
        active_days: 0,
        model_totals: Vec::new(),
        provider_totals: Vec::new(),
        account_totals: Vec::new(),
    };
    // 闭区间 [start, end]：定长日期串的字典序即时间序，所以直接按键取范围。
    // 这里构造两个 String 边界是有意的取舍 —— 报表调用频率是「用户点一下」级别，
    // 两次小分配远不如让 `range` 的边界类型一目了然重要。
    for (_, day) in daily.range(start.to_string()..=end.to_string()) {
        totals.requests += day.requests;
        totals.successful += day.successful;
        totals.tokens += day.tokens;
        // activeDays 只数**区间内**有请求的天数（补零的日子不算「活跃」）
        if day.requests > 0 {
            totals.active_days += 1;
        }
        for acc in &day.model_tokens {
            push_model_accum(&mut totals.model_totals, &acc.model, acc.tokens, acc.requests);
        }
        for acc in &day.provider_stats {
            push_provider_accum(
                &mut totals.provider_totals,
                &acc.provider,
                acc.requests,
                acc.successful,
                acc.tokens,
            );
        }
        for acc in &day.account_stats {
            push_account_accum(
                &mut totals.account_totals,
                &acc.account_id,
                &acc.account_name,
                acc.requests,
                acc.successful,
                acc.tokens,
            );
        }
    }

    // ── 旧聚合行的余量归属 ──────────────────────────────────────
    // 升级前写出的聚合行没有 `providerStats` 键（读入后是空表），那些天的请求
    // 因此没有任何 provider 组覆盖。这里把「未被任何组覆盖的余量」补进空 id 组：
    //   ① 各 provider 之和恒等于区间总量 —— 报表之间能对账，前端把
    //      `providers` 与 `overview.requests` 并排显示时不会出现对不上的数；
    //   ② 旧数据落进「未知」组而不是被静默丢掉 —— 契约要求不得给旧数据
    //      **猜**值（不许补 workbuddy），但「未知」正是它的真实归属，不是猜。
    // 先求和再借用 `&mut`：闭包持着不可变借用时不能同时改这张表。
    let covered_requests: i64 = totals.provider_totals.iter().map(|item| item.requests).sum();
    let covered_successful: i64 = totals.provider_totals.iter().map(|item| item.successful).sum();
    let covered_tokens: i64 = totals.provider_totals.iter().map(|item| item.tokens).sum();
    // `max(0)` 兜住手改文件造成的「拆分比总量还多」；余量全零时这里仍会建出
    // 一个空组，但 `build_providers` 会把全零的组滤掉（见那边的 `requests > 0
    // || tokens > 0`），所以不会凭空冒出空的「未知」项
    push_provider_accum(
        &mut totals.provider_totals,
        "",
        (totals.requests - covered_requests).max(0),
        (totals.successful - covered_successful).max(0),
        (totals.tokens - covered_tokens).max(0),
    );

    // ── 账号维度同理 ────────────────────────────────────────────
    // `accountStats` 是本次新增的键，**所有**已有聚合行都没有它，所以这一段
    // 在这里比 provider 那一段更常命中：升级后第一次打开报表，区间内几乎全部
    // 请求都会落进「未知账号」组。这是如实反映「那些行没记账号身份」，
    // 而不是我们丢数据 —— 明细里其实有，但聚合要跨过年份，只能按聚合算。
    let covered_requests: i64 = totals.account_totals.iter().map(|item| item.requests).sum();
    let covered_successful: i64 = totals.account_totals.iter().map(|item| item.successful).sum();
    let covered_tokens: i64 = totals.account_totals.iter().map(|item| item.tokens).sum();
    push_account_accum(
        &mut totals.account_totals,
        "",
        "",
        (totals.requests - covered_requests).max(0),
        (totals.successful - covered_successful).max(0),
        (totals.tokens - covered_tokens).max(0),
    );
    totals
}

/// 把一次请求（或一天的累计）并进按模型的累计表。
/// 线性查找即可：模型数量是个位到十位级，建 HashMap 反而更慢。
pub(super) fn push_model_accum(list: &mut Vec<ModelAccum>, model: &str, tokens: i64, requests: i64) {
    match list.iter_mut().find(|item| item.model == model) {
        Some(item) => {
            item.tokens += tokens;
            item.requests += requests;
        }
        None => list.push(ModelAccum {
            model: model.to_string(),
            requests,
            tokens,
        }),
    }
}

/// 报表按模型聚合用的**统计键**：上游真名优先，请求名回落。
///
/// ── 为什么不能直接用 `model` ────────────────────────────────
/// 映射语义重做后（见 `pipeline::resolve_model` 的说明），请求名全程保持
/// 客户端原值，改写下沉到发送侧按家进行（`payload::send_body` →
/// `catalog::wire_target_for_provider`）。于是 `model`（请求侧解析名）在
/// 客户端点名映射别名时就是**别名本身** —— 报表若按它聚合，同一个上游模型
/// 会被拆成「真名 + 各家别名」好几行。而映射只是代名，实际请求的仍是上游
/// 那一个模型：`upstream_model`（telemetry 在发送侧采集的最终下发名）才是
/// 「用量花在哪个模型上」的正确答案。
///
/// ── 回落 ──────────────────────────────────────────────────
/// 转发前就失败的请求一次都没发出去，`upstream_model` 为空串 —— 请求名是
/// 此时我们所知的最好信息（token 已清零，只影响请求数维度）。还没有
/// `upstreamModel` 键的旧明细同样走回落：这不是「给旧数据猜值」，空串回落
/// 保留的是明细里本来就有的信息。
///
/// 口径的落点有三个（各维求和 = 当天总量的前提就在这里）：日报折算
/// （`aggregate::fold_into_daily`）、近 24 小时趋势（本文件的 `cache_trend_24h`）
/// 与整行重算（`aggregate::rebuild_day` 经前者），三处**必须调同一个函数**。
///
/// ── 为什么接两个 &str 而不是 &RequestEntry ───────────────────
/// 同一口径现在要服务两种行：完整的 `RequestEntry`（按天重算）与报表的窄行
/// [`SummaryRow`]（近 24 小时趋势，只读 6 列，见 `sql::select_summary_rows`）。
/// 若签名收 `&RequestEntry`，窄行那一侧就只能自己再写一遍 `if upstream 空就
/// 回落` —— 那正是这个函数要防的「同一口径两处维护」。取字段而不是取整行，
/// 两种调用方都能直接用。
pub(super) fn model_stat_key<'a>(model: &'a str, upstream_model: &'a str) -> &'a str {
    if upstream_model.is_empty() {
        model
    } else {
        upstream_model
    }
}

/// 模型分解 → 报表 JSON 数组（按天柱状图与 24 小时折线的悬停读数用）。
///
/// 形状 `[{model, label, tokens, requests}]`，按 tokens 降序（并列比 requests，
/// 再并列比名字，保证同一份数据多次调用顺序一致）。
///
/// 与 `build_models` 的两处口径差异：
///   - 全零组剔除的判据是 `tokens > 0`（这里只给悬停读数用，一条 token 为 0
///     的模型没有可读的量；请求数维度它是过程量，见 `fold_into_daily` 的说明）；
///   - 空模型名照常返回（label 为「未知模型」）：它代表的真实用量不该从读数里
///     消失，否则各行之和与当天总量对不上。
pub(super) fn model_breakdown_json(list: &[ModelAccum]) -> Vec<Value> {
    let mut rows: Vec<&ModelAccum> = list.iter().filter(|item| item.tokens > 0).collect();
    rows.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then(right.requests.cmp(&left.requests))
            .then(left.model.cmp(&right.model))
    });
    rows.into_iter()
        .map(|item| {
            json!({
                "model": item.model,
                "label": if item.model.is_empty() {
                    UNKNOWN_MODEL_LABEL.to_string()
                } else {
                    item.model.clone()
                },
                "tokens": item.tokens,
                "requests": item.requests,
            })
        })
        .collect()
}

/// 把一批（一条或一天的）请求并进按 provider 的累计表。
///
/// 与 `push_model_accum` 同一取舍：provider 数量是个位数，线性查找比 HashMap
/// 更快也更省（不必分配键）。`provider` 为空串时**照常建组**，不丢弃 ——
/// 旧数据与「转发前失败」的请求都落在这一组里，丢掉它们会让
/// 「各 provider 之和」对不上区间总量。
pub(super) fn push_provider_accum(
    list: &mut Vec<ProviderAccum>,
    provider: &str,
    requests: i64,
    successful: i64,
    tokens: i64,
) {
    match list.iter_mut().find(|item| item.provider == provider) {
        Some(item) => {
            item.requests += requests;
            item.successful += successful;
            item.tokens += tokens;
        }
        None => list.push(ProviderAccum {
            provider: provider.to_string(),
            requests,
            successful,
            tokens,
        }),
    }
}

/// 把一批（一条或一天的）请求并进按账号的累计表。
///
/// ── 身份只认 account_id ─────────────────────────────────────
/// 匹配按 `account_id` 走，**名字不参与判等**：账号可以在账号页改名，
/// 按名字匹配会让同一个账号在改名前后的两条聚合行分家，报表里出现两个半份的
/// 「同一账号」。名字在这里只承担一件事：**首次建组时留下的展示名快照**。
/// 后续同一账号的累计里若带了新名字，就更新快照（账号改名后报表跟着显示新名字，
/// 这也正是账号页的观感），而不是保留旧名。
///
/// ── 空 id 的处理 ─────────────────────────────────────────────
/// 与 `push_provider_accum` 同一取舍：`account_id` 为空串时**照常建组**，不丢弃 ——
/// 走默认登录态转发的请求、以及旧版本写出的行都落在这一组里，
/// 丢掉它们会让「各账号之和」对不上区间总量。
pub(super) fn push_account_accum(
    list: &mut Vec<AccountAccum>,
    account_id: &str,
    account_name: &str,
    requests: i64,
    successful: i64,
    tokens: i64,
) {
    match list.iter_mut().find(|item| item.account_id == account_id) {
        Some(item) => {
            item.requests += requests;
            item.successful += successful;
            item.tokens += tokens;
            // 名字只做「快照刷新」：非空才覆盖，绝不清空已有的名字
            // （某些请求拿不到名字而另一些拿到了，取有名字的那一份）
            if !account_name.is_empty() {
                item.account_name = account_name.to_string();
            }
        }
        None => list.push(AccountAccum {
            account_id: account_id.to_string(),
            account_name: account_name.to_string(),
            requests,
            successful,
            tokens,
        }),
    }
}

/// provider id → 展示 label。
///
/// 直接委托 `providers::label_of` —— 「id → 给人看的名字」的唯一入口，报表层
/// **不得**自建换算（注册表是 provider 身份的唯一事实来源，两处映射迟早漂移，
/// 而这类漂移不会报错，只会让报表里的名字与账号页对不上）。入口统一处理两类
/// 身份：内置 provider 走注册表 `meta().label`；`custom-` 前缀的自定义提供商
/// 查存储里用户起的名字 —— 那是运行期数据（config 内存快照，不碰磁盘），
/// 只走注册表的话会被原样回显成 `custom-…`。
///
/// 未登记 / 已删除的 id 仍由入口原样回显：显示一个陌生的英文 id 好过显示空白，
/// 至少能提示「这是谁」。空串同样原样返回（空 label），由展示层决定用什么
/// 占位（当前约定是「未知」/「—」）—— 这一层只回答「id 对应什么名字」，
/// 不做展示决策。
pub(super) fn provider_label(id: &str) -> String {
    crate::server::core::providers::label_of(id)
}

/// 区间级的按 provider 汇总 → `/api/stats/summary` 的 `providers` 数组。
///
/// 排序：`requests` 降序（契约要求）；并列时按 `totalTokens` 降序、
/// 再并列按 id 升序 —— 后两级纯粹为了让结果稳定（同一份数据多次调用顺序一致），
/// 前端做「只显示前 N 家」的截断时不会每次截到不同的行。
///
/// 空 id 那一组的 label 用「未知」而不是空串：这份数组是**给图表用的**，
/// 每段都要有可显示的图例名；空 label 在图例里就是个看不见的空白项。
/// （明细行那边的空 provider 反而保持空 label，好让展示层用「—」占位 ——
/// 两边形态不同是因为用途不同，不是遗漏。）
pub(super) fn build_providers(totals: &[ProviderAccum]) -> Vec<Value> {
    // 全零的组不返回：它只会出现在手改过的文件里，展示一个「0 次请求」的
    // 图例项没有意义（正常写入路径不会造出这种组）
    let mut rows: Vec<&ProviderAccum> = totals
        .iter()
        .filter(|item| item.requests > 0 || item.tokens > 0)
        .collect();
    rows.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then(right.tokens.cmp(&left.tokens))
            .then(left.provider.cmp(&right.provider))
    });
    rows.into_iter()
        .map(|item| {
            json!({
                "id": item.provider,
                "label": if item.provider.is_empty() {
                    UNKNOWN_PROVIDER_LABEL.to_string()
                } else {
                    provider_label(&item.provider)
                },
                "requests": item.requests,
                // failures 由减法得出而不是另存一列：见 record::ProviderAccum 的注释。
                // max(0) 兜住手改文件造成的负数
                "success": item.successful,
                "failures": (item.requests - item.successful).max(0),
                "totalTokens": item.tokens,
            })
        })
        .collect()
}

/// 未知模型的展示名（模型名为空串的那一组）。
///
/// 与 provider / 账号那两条是**三个独立的常量**：三者的「未知」成因各不相同
/// （没记下承载的家 / 没走账号列表 / 没记下模型名），文案将来若分叉，
/// 共用会让改一处等于改两处。
pub(super) const UNKNOWN_MODEL_LABEL: &str = "未知模型";

/// 区间级的按模型汇总 → `/api/stats/summary` 的 `models` 数组。
///
/// 与 `build_providers` / `build_accounts` 同形同序（requests 降序 → tokens 降序
/// → 名字升序）。模型维度比那两维更早存在（`model_totals` 一直是
/// `range_totals` 的一部分，`topModel` 就是从它算出来的），但此前只对外暴露了
/// 冠军一个 —— 这里把整张表给出去，供报表的「模型用量」环形图使用。
///
/// ── 与 providers 的两处差异 ─────────────────────────────────
///   1. **空模型名的组照常返回**（label 为「未知模型」）：`build_top_model` 特意
///      把空名字排除在冠军评选之外（避免标题栏显示空白），但这里是要画一张
///      「用量都花在哪」的图，那一组代表的真实用量不该从图上消失 ——
///      它会让各扇区之和对不上 `overview.tokens`。
///   2. 没有 `label` 与 `id` 的区分：模型名既是身份也是展示名，一个字段就够。
///
/// 全零的组仍然滤掉（与另两维一致）：只可能来自手改过的数据，
/// 画出来是一段永远为 0 的扇区。
pub(super) fn build_models(totals: &[ModelAccum]) -> Vec<Value> {
    let mut rows: Vec<&ModelAccum> = totals
        .iter()
        .filter(|item| item.requests > 0 || item.tokens > 0)
        .collect();
    rows.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then(right.tokens.cmp(&left.tokens))
            .then(left.model.cmp(&right.model))
    });
    rows.into_iter()
        .map(|item| {
            json!({
                "model": item.model,
                "label": if item.model.is_empty() {
                    UNKNOWN_MODEL_LABEL.to_string()
                } else {
                    item.model.clone()
                },
                "requests": item.requests,
                "totalTokens": item.tokens,
            })
        })
        .collect()
}

/// 区间级的按账号汇总 → `/api/stats/summary` 的 `accounts` 数组。
///
/// 与 `build_providers` 同形同序（requests 降序 → tokens 降序 → 身份升序）：
/// 两张排行并排显示时，排序口径一致才不会被误读成「一个按请求数、一个按别的」。
///
/// ── 与 providers 的两处差异 ─────────────────────────────────
///   1. 身份字段是 `id` + `label` 两个：provider 的 `label` 由注册表**现算**，
///      而账号的展示名是**聚合时留下的快照**（账号可能已被删除，注册表里查不到）。
///      取名字的优先级是「快照名 → id」，两者都空的那一组用「未知账号」。
///   2. 没有 `providerLabel` 那种「id → 现算 label」的派生：账号名不是派生值，
///      就是数据本身，所以直接原样给出。
pub(super) fn build_accounts(totals: &[AccountAccum]) -> Vec<Value> {
    // 与 build_providers 同一取舍：全零的组不返回
    let mut rows: Vec<&AccountAccum> = totals
        .iter()
        .filter(|item| item.requests > 0 || item.tokens > 0)
        .collect();
    rows.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then(right.tokens.cmp(&left.tokens))
            .then(left.account_id.cmp(&right.account_id))
    });
    rows.into_iter()
        .map(|item| {
            // 展示名取「快照名 → id」；两者都空才是真正的未知（默认登录态转发）
            let label = if !item.account_name.is_empty() {
                item.account_name.clone()
            } else if !item.account_id.is_empty() {
                item.account_id.clone()
            } else {
                UNKNOWN_ACCOUNT_LABEL.to_string()
            };
            json!({
                "id": item.account_id,
                "label": label,
                "requests": item.requests,
                "success": item.successful,
                "failures": (item.requests - item.successful).max(0),
                "totalTokens": item.tokens,
            })
        })
        .collect()
}

/// 明细行 → 响应 JSON：在序列化结果上补两个**派生字段**
/// `providerLabel` 与 `phaseElapsedMs`。
///
/// ── providerLabel ───────────────────────────────────────────
/// 为什么不在 `RequestEntry` 上加一个字段：契约里 `providerLabel` 是给前端直接
/// 显示的（§6「记账展示的 provider 用 label」），它由 id 换算而来、会随注册表
/// 变化，而 `RequestEntry` 是**落盘格式**——把派生值写进文件会让
/// 「以后改了 label，历史行仍是旧名字」。
///
/// ── phaseElapsedMs（本次新增，与上面同一手法）────────────────
/// 「在当前阶段里已经待了多久（毫秒）」，只对**在途**行（有 `phase` 的那几行）
/// 有值；不在途的行给 `null`，前端据此不渲染状态列的第二行。
///
/// 为什么现算而不是把耗时落库：耗时的两个端点里，起点（`phase_started_at`）
/// 是库里的事实、终点（现在）在读取那一刻才知道。落库一个值意味着它从写下去的
/// 那一刻就开始过期 —— 而状态列第二行的全部意义正是「它还在走」。
/// 现算还让读数与服务端时钟同源：浏览器时钟偏了也不会算出离谱的秒数
/// （与 OmniProxy 的 `phase_elapsed_ms` 用 `julianday('now')` 现减同一取向）。
///
/// `saturating_sub` + `max(0)`：手改过的库可能给出一个未来时刻，那种值算出来
/// 的负耗时会渲染成「-5秒」，两条都挡掉。
///
/// 序列化失败理论上不可能（字段全是 i64 / String / Option<String>），但仍然
/// 不做 unwrap（panic=abort 下会带走整个应用）：退化成一个空对象，
/// 前端那一行显示空白，比进程消失好。
pub(super) fn entry_json(entry: &RequestEntry) -> Value {
    let Ok(mut value) = serde_json::to_value(entry) else {
        return Value::Object(serde_json::Map::new());
    };
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "providerLabel".to_string(),
            Value::String(provider_label(&entry.provider)),
        );
        object.insert(
            "phaseElapsedMs".to_string(),
            match entry.phase_started_at {
                Some(started) => json!((now_ms() - started).max(0)),
                None => Value::Null,
            },
        );
    }
    value
}

/// 区间级 topModel：按 `tokens` 降序，并列比 `requests`，再并列比名字
/// （名字兜底是为了结果稳定，不受聚合行内部顺序影响）。
/// 无数据或所有记录都没带模型名时返回 `null`。
pub(super) fn build_top_model(totals: &[ModelAccum], range_tokens: i64) -> Value {
    let best = totals
        .iter()
        // 空模型名不参与评选：否则会冒出个「空名字冠军」，标题栏显示成空白，
        // 比不给结果更像故障
        .filter(|item| !item.model.is_empty() && (item.tokens > 0 || item.requests > 0))
        .max_by(|left, right| {
            left.tokens
                .cmp(&right.tokens)
                .then(left.requests.cmp(&right.requests))
                // 取反：名字小的排前面，保证并列时结果确定
                .then(right.model.cmp(&left.model))
        });
    let Some(best) = best else {
        return Value::Null;
    };
    // percentage = 该模型 tokens / 区间总 tokens；总为 0 时给 0.0，
    // 绝不产生 NaN/Infinity（serde_json 序列化非有限浮点会 panic 或产出 null）
    let percentage = if range_tokens > 0 {
        best.tokens as f64 / range_tokens as f64
    } else {
        0.0
    };
    json!({
        "model": best.model,
        "tokens": best.tokens,
        "requests": best.requests,
        "percentage": percentage,
    })
}

/// 范围内的逐天序列（缺失日期补零，升序）。
///
/// 前端按天画柱状图，缺的那天必须占位（补 0）而不是不返回 ——
/// 少一天会让整条曲线在视觉上被压缩，趋势看起来是错的。
///
/// 每天额外带 `models`（`[{model, label, tokens, requests}]`，按 tokens 降序）：
/// 柱状图的悬停读数要给出「这一天总用量是多少、分别花在哪些模型上」，
/// 而按模型的分解只存在于聚合行（`model_tokens` 列）—— 明细只在报表读明细的
/// 24 小时窗口里才被读，这里拿不到也不会回算（见 [`model_breakdown_json`]）。
pub(super) fn daily_trend(daily: &BTreeMap<String, DailyEntry>, start: &str, end: &str) -> Vec<Value> {
    let (Some(start_date), Some(end_date)) = (parse_key(start), parse_key(end)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = start_date;
    // 上限兜底：手改文件把区间撑到十年也不会把响应撑爆
    while cursor <= end_date && out.len() < MAX_TREND_DAYS as usize {
        let key = date_key(cursor);
        let day = daily.get(&key);
        let (tokens, requests) = day.map(|item| (item.tokens, item.requests)).unwrap_or((0, 0));
        let models = day.map(|item| model_breakdown_json(&item.model_tokens)).unwrap_or_default();
        out.push(json!({
            "date": key,
            "tokens": tokens,
            "requests": requests,
            "models": models,
        }));
        cursor += ChronoDuration::days(1);
    }
    out
}

/// 热力图：**固定 365 天**（含今天），与 range 解耦，无数据的日期补零。
/// 升序返回，前端按「一年 = 若干列周」排布。
pub(super) fn heatmap(daily: &BTreeMap<String, DailyEntry>, today: NaiveDate) -> Vec<Value> {
    let start = today - ChronoDuration::days(HEATMAP_DAYS - 1);
    let mut out = Vec::with_capacity(HEATMAP_DAYS as usize);
    let mut cursor = start;
    while cursor <= today {
        let key = date_key(cursor);
        let (requests, tokens) = daily
            .get(&key)
            .map(|day| (day.requests, day.tokens))
            .unwrap_or((0, 0));
        out.push(json!({ "date": key, "requests": requests, "tokens": tokens }));
        cursor += ChronoDuration::days(1);
    }
    out
}

/// 连续有请求的天数：从**本地今天**往前数。
///
/// 「今天还没有请求不算断」—— 凌晨打开报表时今天通常还是空的，
/// 若把今天算成断，连续天数每天都要先归零一次，与用户直觉相反。
/// 所以今天没记录时从昨天起算。
pub(super) fn streak(daily: &BTreeMap<String, DailyEntry>, today: NaiveDate) -> i64 {
    let has = |day: NaiveDate| -> bool {
        daily
            .get(&date_key(day))
            .is_some_and(|item| item.requests > 0)
    };
    let mut cursor = today;
    if !has(cursor) {
        cursor -= ChronoDuration::days(1);
    }
    let mut count = 0i64;
    // 上限兜底：手改数据造出「万年连续」也不会死循环
    while count < MAX_TREND_DAYS && has(cursor) {
        count += 1;
        cursor -= ChronoDuration::days(1);
    }
    count
}

// ─── 缓存命中率（窗口都很短，必须从明细算）────────────────────

/// 四档缓存命中率
pub(super) fn cache_rates(totals: &[(i64, i64)]) -> Value {
    let mut out = serde_json::Map::new();
    for (index, (key, _window)) in CACHE_WINDOWS.iter().enumerate() {
        // 缺项按「这个窗口没有数据」补零（不该发生：SQL 侧每个窗口都有一组
        // 合计，见 `sql::sum_cache_windows`）—— 与 safe_rate 的零除口径一致
        let (hit, input) = totals.get(index).copied().unwrap_or((0, 0));
        out.insert(
            key.to_string(),
            json!({ "hitTokens": hit, "inputTokens": input, "rate": safe_rate(hit, input) }),
        );
    }
    Value::Object(out)
}

/// 近 24 个**本地整点**的命中率与用量趋势（无数据的整点补零）。
///
/// 按本地时区切整点：先把时间戳还原成 `DateTime<Local>` 再取 `%H`，
/// 于是「14 点」就是用户时钟上的 14 点，不是 UTC 的 14 点。
///
/// 每个整点同时给出 `totalTokens`：前端那一张图要画**两条线**
/// （命中率 + 总 Token），两段数据必须同源同桶 —— 分成两次请求会让两条线
/// 在「这一小时算不算」上出现分歧，看起来像其中一条错位了一格。
///
/// 每个整点还带 `models`（`[{model, label, tokens, requests}]`，按 tokens 降序）：
/// 折线图的悬停读数要给出「这一小时总用量是多少、分别花在哪些模型上」。
/// 模型统计键走 [`model_stat_key`]（与日报折算同一口径），于是同一小时里
/// 「各模型之和」必然等于该小时的 `totalTokens`（当份数据之内）。
///
/// 入参是**报表窄行**（[`SummaryRow`]，只 6 列）而不是完整明细：本函数是改造后
/// 唯一还要逐条读明细的地方，而它需要的字段全在窄行的列集里（见
/// `sql::select_summary_rows`）。窗口外（`ts` 早于 23 个整点前）的行照旧跳过 ——
/// 调用方给的可能是整个 `request_stats::SUMMARY_WINDOW_MS` 窗口，两者在边界上
/// 不完全重合（本地时区的整点切法与「now 往前推 N 小时」差几十分钟）。
pub(super) fn cache_trend_24h(entries: &[SummaryRow], now: i64) -> Vec<Value> {
    /// 单个整点的桶：三项总量 + 按模型的累计（模型数是个位到十位级，
    /// 线性查找即可 —— 与 `push_model_accum` 同一取舍）
    #[derive(Default)]
    struct HourBucket {
        hit: i64,
        input: i64,
        total: i64,
        models: Vec<ModelAccum>,
    }

    let start = hour_floor(ms_to_local(now)) - ChronoDuration::hours(23);
    let start_ms = start.timestamp_millis();

    // 先把窗口内的条目按整点键归桶，再按 24 个整点取值 ——
    // 逐个整点扫一遍明细是 24×N，这样只需一次遍历
    let mut buckets: HashMap<String, HourBucket> = HashMap::new();
    for item in entries {
        if item.ts < start_ms || item.ts > now {
            continue;
        }
        let slot = buckets.entry(hour_key(ms_to_local(item.ts))).or_default();
        slot.hit += item.cache_read_tokens;
        slot.input += item.prompt_tokens;
        slot.total += item.total_tokens;
        push_model_accum(
            &mut slot.models,
            model_stat_key(&item.model, &item.upstream_model),
            item.total_tokens,
            1,
        );
    }

    let mut out = Vec::with_capacity(24);
    for step in 0..24 {
        let key = hour_key(start + ChronoDuration::hours(step));
        let slot = buckets.remove(&key).unwrap_or_default();
        out.push(json!({
            "hour": key,
            "hitTokens": slot.hit,
            "inputTokens": slot.input,
            "totalTokens": slot.total,
            "rate": safe_rate(slot.hit, slot.input),
            "models": model_breakdown_json(&slot.models),
        }));
    }
    out
}

/// 命中率：`hit / input`，分母为 0（或无命中）时返回 0.0。
///
/// **必须挡住零除**：0/0 在浮点下是 NaN，`serde_json` 序列化 NaN 时
/// 要么 panic 要么产出 `null`，前端拿到非数字后图表直接空白。
/// 这里也挡住负数（手改文件可能塞进负值）——「命中」为负没有物理意义，
/// 直接按 0 处理。
///
/// 注意**不把结果夹到 1.0**：上游有些接口把 cacheRead 与 promptTokens
/// 分开报（promptTokens 不含缓存部分），此时命中率合法地会超过 100%。
/// 夹一下确实更好看，但那是拿数据真实性换观感 —— 报表要反映真实比值，
/// 越界与否交给前端展示层决定。
pub(super) fn safe_rate(hit: i64, input: i64) -> f64 {
    if input <= 0 || hit <= 0 {
        return 0.0;
    }
    let rate = hit as f64 / input as f64;
    if rate.is_finite() {
        rate
    } else {
        0.0
    }
}

/// status 过滤归一的**结果**：三种取值各自对应一个 SQL 条件（见
/// `sql::FilterPlan::of` 里逐个的写法）。
///
/// 为什么从「`Option<bool>`」扩成枚举：原来只有 ok / error 两个值，bool 够用；
/// 「进行中」（running，status=0）加入后是第三种互斥状态 —— 硬塞 bool 要么
/// 让调用方先判字符串再判 bool，要么把 running 混进 error 的取反里。
/// 三分支枚举让「漏处理新分支」在编译期就暴露。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusFilter {
    /// 只看成功（2xx 且无错误摘要）
    Ok,
    /// 只看失败（成功条件的取反；**不含**进行中行，见 `sql::FilterPlan::of`）
    Error,
    /// 只看进行中（status = 0，见 `request_stats::record_started`）
    Running,
}

/// status 过滤归一：`"ok"` → 成功，`"error"` → 失败，`"running"` → 进行中，
/// 其余（含 None 与拼错的值）不过滤。大小写不敏感、容忍前后空白。
pub(super) fn normalize_status_filter(value: Option<&str>) -> Option<StatusFilter> {
    match value.map(str::trim).map(str::to_lowercase).as_deref() {
        Some("ok") => Some(StatusFilter::Ok),
        Some("error") => Some(StatusFilter::Error),
        Some("running") => Some(StatusFilter::Running),
        _ => None,
    }
}

/// `YYYY-MM-DD` → NaiveDate（手改文件里的坏值返回 None，调用方各自兜底）
fn parse_key(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}
