//! 报表 API 与数据保留策略（设置页「数据保留」）。
//!
//! ```text
//! GET    /api/stats/summary            报表聚合（概览 / 热力图 / 缓存命中率 / 趋势）
//! GET    /api/stats/requests           请求日志（分页 + 模型 / 提供商 / 状态 / 时间区间过滤）
//! GET    /api/stats/requests/filters   请求日志筛选下拉的候选清单（出现过的模型 / 提供商）
//! GET    /api/stats/requests/raw       单条请求的原始正文（预览对话详情弹窗的数据源）
//! GET    /api/stats/requests/clear-preview  清理弹窗的预览统计（将删明细数 / 带正文数 / 库占用 / 压缩状态）
//! DELETE /api/stats/requests           清理（mode=raw 清两侧原文 / mode=all 同时删明细，默认 all）
//! POST   /api/stats/requests/compact   压缩数据库（checkpoint + VACUUM，后台执行）
//! GET    /api/retention                三档保留天数与日志 / 请求日志数量上限
//! PUT    /api/retention                更新保留设置并**立即**触发清理
//! ```
//!
//! ── 为什么两条前缀放在一个文件里 ─────────────────────────────
//! `/api/retention` 是三档数据保留期的**统一入口**（事件日志在 `logs_store`、
//! 请求日志与聚合在 `request_stats`），其中两条属于请求统计；而改完设置必须
//! 立刻裁剪，裁剪又要拿 `request_stats` 句柄 —— 两者共享同一份校验与清理逻辑，
//! 拆成两个文件只会让「校验 → 写配置 → 立即清理」这条链断成两段。
//! 文件名沿用 `logs_api.rs` / `config_api.rs` 的 `*_api.rs` 约定。
//!
//! ── 参数解析的取舍（与 logs_api 同一口径）────────────────────
//!   - `range` 非法 → **400**（与存储层「非法按 7 处理」不同）：报表区间是用户
//!     在界面上显式选的值，静默换一个区间会让页面显示的数据与选项对不上，
//!     报错比默默改口径更容易排查。此处的合法值清单与 `report::normalize_range`
//!     保持一致（那边是存储层的归一实现，这里是路由层的校验白名单）。
//!   - `start` / `end` / `limit` / `offset` 非法 → **忽略**（存储层各自兜底）：
//!     这些是筛选器，前端可能传空串（「不限时间」）；为一次筛选参数不合法
//!     让整页报错，不如把该维度当作没筛。
//!   - `status` 只接受 `ok` / `error`，其余忽略 —— 归一逻辑在存储层
//!     （`report::normalize_status_filter`），这里只做透传，不重复实现一套。
//!
//! ── 与既有 API 的关系 ───────────────────────────────────────
//! 全部是**新增**端点与新增可选查询参数：`/api/logs` 只多认 `start` / `end`
//! 两个可选参数，不传时行为与以前完全一致（见 `logs_api::query_logs`）。
//!
//! ── provider / account 维度（Agent2API W4：T-e1）────────────
//! 两条报表路由的响应各多一处 **provider 维度**，都由存储层组装完再透传
//! （路由层不加工，理由同上）：
//!   - `/api/stats/requests`：每行含 `provider`（id，旧行/未承载为空串）
//!     与派生的 `providerLabel`（id → 中文展示名；未知 id 原样回显 id，
//!     空 id 回显空串，由展示层用「—」占位），以及在途行的 `phase` +
//!     派生 `phaseElapsedMs`（状态列的阶段徽章与阶段计时，见 `ui/request-phase.js`）。
//!   - `/api/stats/summary`：新增顶层 `providers` 数组
//!     `[{id,label,requests,success,failures,totalTokens}]`，按 requests 降序，
//!     统计区间与 `overview` / `topModel` 完全一致；空 id 的组 label 为「未知」。
//!
//! **account 维度**与 providers 同形并列（`accounts` 数组，同样的字段与排序），
//! 是「具体哪个登录态在出力」的那一维 —— 一家提供商可以挂多个账号，
//! 所以它比 providers 更细。两点差异：`label` 是**聚合时留下的名字快照**
//! 而不是现算（账号可能已被删除，注册表里查不到），取「快照名 → id」，
//! 两者都空的那一组 label 为「未知账号」。
//!
//! ── 按 provider 筛选（本次新增）────────────────────────────
//! `GET` / `DELETE` 两条都多了 `provider` 查询参数（精确匹配 provider id），
//! 与 `model` 同一份过滤链（存储层的 `sql::FilterPlan`）—— 所以「列表里筛出
//! 的 N 条」与「清空删掉的那批」是同一个集合。候选清单走
//! `GET /api/stats/requests/filters`，理由见那个处理函数。
//!
//! 以上都是**新增键**，既有字段的键名、类型、语义一个都没动
//! （前端按「providers / accounts 存在则展示、缺失则隐藏」消费）。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use serde_json::{json, Value};

use super::date_range::{date_bounds, parse_day_param};
use crate::server::config::{
    self, RetentionPatch, RetentionSettings, KEY_DAILY_RETENTION_DAYS, KEY_LOG_RETENTION_COUNT,
    KEY_LOG_RETENTION_DAYS, KEY_REQUEST_RETENTION_COUNT, KEY_REQUEST_RETENTION_DAYS,
    RETENTION_MAX_COUNT, RETENTION_MAX_DAYS, RETENTION_MIN_COUNT, RETENTION_MIN_DAYS,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body, parse_query_ms};
use crate::server::logging;
use crate::server::request_stats::RequestQuery;
use crate::server::ServerState;

/// `/api/stats/summary` 的合法 range 取值（与 `request_stats::report` 的
/// `normalize_range` 白名单逐字一致；那边是存储层的归一，这里是路由层的校验）。
/// `custom` 另需 `startDate` / `endDate`（见 `stats_summary`）。
const RANGES: [&str; 7] = ["today", "yesterday", "7", "30", "month", "all", "custom"];
/// range 缺省值：与存储层「非法/缺省按 7 处理」的默认口径相同
const DEFAULT_RANGE: &str = "7";

/// `/api/stats*` 上未匹配子路径的 404。
///
/// 照 `logs_api::not_found` 的做法：管理 API 的 404 是信封形状
/// （`{success:false,error:"Not found: <METHOD> <path>"}`），
/// 与全局兜底的 OpenAI 形状（`{error:{message}}`）不同。
pub async fn not_found(request: axum::extract::Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    errors::management_error(404, format!("Not found: {method} {path}"))
}

/// 查询串（与 logs_api 同一写法：HashMap 比给每个参数定义结构体更贴合
/// 「有则用、无则忽略」的语义）
type Params = HashMap<String, String>;

/// 取「有效」的文本参数：空串与全空白一律当缺省，返回值已 trim。
///
/// 前端把输入框清空时会发出 `?model=&start=`，这类值若交给下游过滤
/// 会变成「模型名等于空串」这种永不命中的条件（页面看起来是空的）。
/// 顺带 trim：模型名不会有前后空白，容忍用户从别处复制粘贴带进来的空白
/// 比严格相等更符合「筛选」的语义（存储层做的是精确匹配）。
fn text_of(params: &Params, key: &str) -> Option<String> {
    params
        .get(key)
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 分页偏移：空串 / 非数字 / 负数 → 0（存储层的 `skip` 以 0 为起点）
fn parse_offset(value: Option<&String>) -> usize {
    let Some(text) = value.map(|text| text.trim()).filter(|text| !text.is_empty()) else {
        return 0;
    };
    text.parse::<usize>().unwrap_or(0)
}

/// 每页条数：空串 / 非数字 / 0 → None（用存储层默认值 50）。
///
/// 0 当「没传」而不是「要 0 条」（与 logs_api 的 `0 || 200` 同一口径）；
/// 超上限的夹紧在存储层（`query_requests` 里 `clamp(1, MAX_LIMIT)`，即 500），
/// 这里只负责把字符串解析成数字 —— 夹紧逻辑只有一处，两层各写一份会漂。
fn parse_limit(value: Option<&String>) -> Option<usize> {
    let text = value.map(|text| text.trim()).filter(|text| !text.is_empty())?;
    text.parse::<usize>().ok().filter(|size| *size > 0)
}

/// GET /api/stats/summary?range=7&startDate=&endDate=
///
/// `range=custom` 时必须带 `startDate` / `endDate`（`YYYY-MM-DD`，本地自然日，
/// 闭区间）：两个日期都由用户在选择器上显式选定，缺失 / 格式错 / 起止倒置一律
/// 400 —— 与 `range` 同一取舍（隐式换一个区间会让页面显示的与选的对不上，
/// 报错比默默改口径好排查）。
pub async fn stats_summary(State(state): State<ServerState>, Query(params): Query<Params>) -> Response {
    // 先 trim 再比对：存储层的 `normalize_range` 也是先 trim 后匹配，
    // 若这里比对未 trim 的值，`?range=%207%20` 会被判 400 而存储层本可接受它 ——
    // 两层的容错度不一致时，用户看到的 400 会显得莫名其妙
    let requested = text_of(&params, "range").unwrap_or_else(|| DEFAULT_RANGE.to_string());
    // 校验放在路由层：非法值给 400 并列出合法值（见文件头的取舍说明）
    if !RANGES.contains(&requested.as_str()) {
        return errors::management_error(
            400,
            format!("range 取值非法: {requested}（合法值: {}）", RANGES.join("、")),
        );
    }
    let custom = if requested == "custom" {
        let start = match parse_day_param(&params, "startDate") {
            Ok(value) => value,
            Err(message) => return errors::management_error(400, message),
        };
        let end = match parse_day_param(&params, "endDate") {
            Ok(value) => value,
            Err(message) => return errors::management_error(400, message),
        };
        if start > end {
            return errors::management_error(
                400,
                format!(
                    "开始日期不能晚于结束日期（startDate={}, endDate={}）",
                    start, end
                ),
            );
        }
        Some((start, end))
    } else {
        None
    };
    // 原样透传存储层的报表结果（`{range, startDate, endDate, overview, providers,
    // heatmap, cacheRates, cacheTrend24h, dailyTrend}`），路由层不加工，避免两处口径。
    // `providers` 是 W4 新增的 provider 维度汇总（见文件头的说明）
    //
    // ── 为什么在 `spawn_blocking` 里算 ──────────────────────────
    // `usage_summary` 整条路是**同步**的（SQLite 读 + 纯函数），而且持着统计库
    // 那把全局连接锁。直接在 async handler 里调用会占住一个 tokio worker 线程
    // 直到它跑完 —— 报表是前端轮询的接口（默认每秒一次、可在定时任务里调），
    // 这种「同步读库」不该出现在 async 运行时线程上。与 `backup_api` /
    // `upgrade_api` 的慢操作同一处理。
    //
    // 锁竞争（与转发记账抢同一把 `Db` 锁）不因这一层改变 —— 那件事由「报表
    // 只读 24 小时窄列 + 命中率下推 SQL」在**工作量**上解决（见
    // `RequestStats::usage_summary` 的读取窗口说明）；这里解决的是「占着
    // runtime 线程」这一半。
    let api_key = text_of(&params, "apiKey");
    if api_key.as_deref().is_some_and(|value| crate::server::request_stats::ApiKeyIdentity::from_filter(value).is_none()) {
        return errors::management_error(400, "apiKey 取值非法（需要 key:<id>、env、none 或 unknown）");
    }
    let stats = state.request_stats();
    let result = tokio::task::spawn_blocking(move || stats.usage_summary(&requested, custom, api_key.as_deref())).await;
    match result {
        Ok(value) => ok_json(value),
        // 任务被取消（runtime 关闭中）或 panic：给一个可读的 500 而不是让
        // axum 把一个无 body 的错误连接丢给前端
        Err(error) => errors::management_error(500, format!("报表计算任务失败: {error}")),
    }
}

/// GET /api/stats/requests?offset=&limit=&model=&provider=&status=&start=&end=
///
/// 每行含 `provider`（id）与 `providerLabel`（展示名），由存储层的 `entry_json`
/// 在序列化后派生填入；同时派生一个 `phaseElapsedMs`（在途行「当前阶段已持续
/// 多久」，不在途为 null）—— 与 `phase` 列合起来就是请求日志状态列的
/// 「阶段徽章 + 阶段计时」两行（见 `ui/request-phase.js`）。
///
/// `provider` 与 `model` 都是**精确匹配**（下拉里选的是明细里出现过的原值，
/// 不做模糊匹配 —— 模糊匹配会让「筛了 A 却看到 B」变得无法解释）。
///
/// 响应额外带 `running`：同一筛选条件下仍在进行中（status=0）的条数 ——
/// 转发开始时就会插一条这样的行（见 `RequestStats::record_started`），
/// 收尾后由终态记账覆盖。status 过滤也认 `running` 值（只看进行中）。
pub async fn stats_requests(State(state): State<ServerState>, Query(params): Query<Params>) -> Response {
    let filter = match checked_filter(&params) { Ok(value) => value, Err(response) => return response };
    ok_json(state.request_stats().query_requests(&filter))
}

/// 从查询串解析筛选条件 —— GET / DELETE / clear-preview 三条路由共用同一份
/// 解析：筛选口径一旦漂了，三个端点就各说各话（「筛出的 N 条」与「删掉的
/// 那批」「预览的 N 条」必须是同一个集合）。
fn filter_from_params(params: &Params) -> RequestQuery {
    RequestQuery {
        offset: parse_offset(params.get("offset")),
        limit: parse_limit(params.get("limit")),
        // 模型名精确匹配；空串（输入框清空）当没筛
        model: text_of(&params, "model"),
        // provider id 精确匹配；空串当没筛（见 RequestQuery::provider）
        provider: text_of(&params, "provider"),
        client: params.get("client").map(|value| value.trim().to_string()),
        api_key: text_of(&params, "apiKey"),
        production_only: params.get("productionOnly").is_some_and(|value| value == "1"),
        completed_only: params.get("completedOnly").is_some_and(|value| value == "1"),
        // 只认 ok / error / running，其余值在存储层被忽略（不在这里重复实现归一）
        status: text_of(&params, "status"),
        // 闭开区间 [start, end)：翻页时上一页末尾的 ts 可直接当下页的 end
        start: parse_query_ms(params.get("start")),
        end: parse_query_ms(params.get("end")),
    }
}

fn checked_filter(params: &Params) -> Result<RequestQuery, Response> {
    let mut filter = filter_from_params(params);
    if filter.api_key.as_deref().is_some_and(|value| crate::server::request_stats::ApiKeyIdentity::from_filter(value).is_none()) {
        return Err(errors::management_error(400, "apiKey 取值非法（需要 key:<id>、env、none 或 unknown）"));
    }
    if let Some((start, end)) = date_bounds(params).map_err(|message| errors::management_error(400, message))? {
        filter.start = Some(start);
        filter.end = Some(end);
    }
    Ok(filter)
}

/// GET /api/stats/requests/filters —— 筛选下拉的候选清单。
///
/// 形状 `{models: [名字…], providers: [{id, label}…]}`，两者都按出现次数降序
/// （常用的排前面）。清单来自**明细里实际出现过的值**，不是当前的配置清单 ——
/// 历史请求用过的模型名 / 已删除账号所属的 provider 同样要能筛到，否则会出现
/// 「列表里有这一行、下拉里却没有这个选项」这种最难解释的不一致。
///
/// 不带任何查询参数：它与时间档位无关（理由见存储层 `select_filter_options`）。
pub async fn stats_request_filters(State(state): State<ServerState>) -> Response {
    ok_json(state.request_stats().filter_options())
}

/// DELETE /api/stats/requests?mode=raw|all&model=&provider=&status=&start=&end=&all=
///
/// `raw` 清理命中请求关联的上游调试报文与下游正文，保留明细与日报；
/// `all` 同时删除命中的明细，无筛选时重置日报。有筛选时保留日报，因为
/// 按剩余明细重算会抹掉此前已清理请求的历史贡献。
///
/// 无筛选时两种模式都必须显式带 `all=1`，并清空两张报文表的全部记录。
/// `deleted` 在 raw 模式为按 ID 去重的请求数，在 all 模式为明细行数；
/// `raw` 始终为按 ID 去重的报文请求数，`upstreamRaw` / `downstreamRaw` 为两侧行数。
pub async fn clear_stats_requests(
    State(state): State<ServerState>,
    Query(params): Query<Params>,
) -> Response {
    let mode = match params.get("mode").map(|value| value.trim()) {
        Some("raw") => "raw",
        // 缺省按 all：与「不带 mode 的旧调用方」行为兼容
        Some("all") | None => "all",
        Some(other) => {
            return errors::management_error(
                400,
                format!("mode 取值非法: {other}（合法值: raw、all）"),
            )
        }
    };
    let filter = match checked_filter(&params) { Ok(value) => value, Err(response) => return response };
    let has_filters = filter.has_filters();
    if !has_filters {
        let explicit_all = params.get("all").map_or(false, |value| {
            let text = value.trim();
            text == "1" || text.eq_ignore_ascii_case("true")
        });
        if !explicit_all {
            return errors::management_error(
                400,
                "未指定筛选条件：要清空全部请求数据请显式带 all=1",
            );
        }
    }
    let stats = state.request_stats();
    let result = tokio::task::spawn_blocking(move || stats.clear_requests(&filter, mode == "raw")).await;
    let cleared = match result {
        Ok(Some(value)) => value,
        Ok(None) => return errors::management_error(500, "请求数据清理失败：数据库不可用或事务执行失败"),
        Err(error) => return errors::management_error(500, format!("请求数据清理任务失败: {error}")),
    };
    let deleted = cleared.deleted;
    match (mode, has_filters) {
        ("raw", _) => logging::log(
            "[Stats]",
            &format!(
                "已清空 {deleted} 个请求的原始报文（上游 {} 条、下游 {} 条；明细与按天聚合不受影响）",
                cleared.upstream_raw, cleared.downstream_raw,
            ),
        ),
        ("all", true) => logging::log(
            "[Stats]",
            &format!("已按筛选条件删除 {deleted} 条请求明细与两侧原始报文（按天聚合未重算）"),
        ),
        _ => logging::log("[Stats]", "请求统计已清空（明细 + 按天聚合 + 两侧原始报文）"),
    }
    let mut body = json!({
        "success": true,
        "mode": mode,
        "deleted": deleted,
        "raw": cleared.raw,
        "downstreamRaw": cleared.downstream_raw,
        "upstreamRaw": cleared.upstream_raw,
    });
    if mode == "all" && has_filters {
        body["note"] = Value::String(
            "已删除命中的请求明细与对应的上游、下游原始报文；按天聚合是全量数据，未按筛选重算，报表数字仍包含已删除的请求".to_string(),
        );
    }
    ok_json(body)
}

/// GET /api/stats/requests/raw?id=... —— 单条请求的原始正文。
///
/// 详情弹窗（预览对话）的数据源：`{id, requestBody, responseBody, truncated}`，
/// 两个文本字段完整返回，`truncated` 仅提示旧版本保存的正文可能不完整。
/// 不存在给 404 —— 正文只在**调试模式开启期间**的
/// 请求上采集（关着时不写行，见 `api::pipeline::capture_raw_body`），
/// 此外也可能已被清理过；两种情况都是正常状态，不是错误。
pub async fn stats_request_raw(
    State(state): State<ServerState>,
    Query(params): Query<Params>,
) -> Response {
    let Some(id) = text_of(&params, "id") else {
        return errors::management_error(400, "缺少 id 参数");
    };
    match state.request_stats().raw_body(&id) {
        Some(value) => ok_json(value),
        None => errors::management_error(404, format!("未找到该请求的原始报文（id: {id}）")),
    }
}

/// GET /api/stats/requests/clear-preview —— 清理弹窗的预览统计。
///
/// 预览与 DELETE 共用同一份筛选计划。`all` 是命中明细行数，`raw` 是两侧
/// 报文按 ID 去重的请求数，`upstreamRaw` / `downstreamRaw` 是各侧行数；
/// `dbBytes` 是主文件 + WAL 的全库占用，不按筛选计算。
pub async fn stats_clear_preview(
    State(state): State<ServerState>,
    Query(params): Query<Params>,
) -> Response {
    let filter = match checked_filter(&params) { Ok(value) => value, Err(response) => return response };
    let stats = state.request_stats();
    match tokio::task::spawn_blocking(move || stats.clear_preview(&filter)).await {
        Ok(Some(value)) => ok_json(value),
        Ok(None) => errors::management_error(500, "清理预览读取失败：数据库不可用或查询失败"),
        Err(error) => errors::management_error(500, format!("清理预览任务失败: {error}")),
    }
}

/// POST /api/stats/requests/compact —— 压缩数据库（checkpoint + VACUUM）。
///
/// 后台线程执行（理由见 `RequestStats::compact`），响应只表示「是否受理」：
/// 已受理给 `{success, started:true}`，前端靠 clear-preview 的 `vacuumRunning`
/// / `lastVacuum` 轮询进度；重复触发给 409（同一时间只允许一个压缩任务）；
/// 库不可用如实报错，不谎报「已启动」。
pub async fn compact_stats_db(State(state): State<ServerState>) -> Response {
    match state.request_stats().compact() {
        crate::server::request_stats::CompactStart::Started => {
            ok_json(json!({ "success": true, "started": true }))
        }
        crate::server::request_stats::CompactStart::AlreadyRunning => {
            errors::management_error(409, "数据库压缩正在进行中，请稍后再试")
        }
        crate::server::request_stats::CompactStart::Unavailable => {
            errors::management_error(500, "数据库不可用，无法压缩")
        }
    }
}

/// POST /api/stats/requests/terminate?id=... —— 手动终止一条**在途**请求。
///
/// 参考 OmniProxy 的 `POST /api/request-logs/:id/terminate`，但更薄：受理只做
/// 一件事 —— 置位该请求的取消令牌（`core::upstream::cancellation`）。真正的
/// 收尾（断开上游、给客户端错误帧、把明细写成 408 + 「请求已被手动终止」）
/// 全部由转发链自己的收尾路径完成 —— **明细的写者仍然只有那一条路径**
/// （与「在途回写晚到一步不该覆盖终态」同一纪律：接口自己不碰库）。
///
/// 不在在途表里给 404（已结束 / 来自上次启动的遗留行 / id 打错）：
/// 前端据此提示「已结束或已刷新」，比谎报「已受理」诚实。
/// 重复点按幂等（令牌置位是单向的）：第二次调用仍返回受理 —— 收尾可能
/// 正在进行中，重复点击不该被当成错误。
pub async fn stats_request_terminate(
    State(_state): State<ServerState>,
    Query(params): Query<Params>,
) -> Response {
    let Some(id) = text_of(&params, "id") else {
        return errors::management_error(400, "缺少 id 参数");
    };
    if crate::server::core::upstream::cancellation::cancel(&id) {
        ok_json(json!({ "success": true, "terminated": true }))
    } else {
        errors::management_error(
            404,
            "这条请求已不在进行中（可能已结束，或来自上次启动遗留的进行中行）",
        )
    }
}

/// GET /api/retention
pub async fn get_retention(State(_state): State<ServerState>) -> Response {
    ok_json(retention_json(config::retention_settings()))
}

/// PUT /api/retention —— 支持部分更新三档天数及 logRetentionCount / requestLogRetentionCount。
///
/// 允许部分字段（未出现的项保持原值）。校验通过后：写 config.json → 立即裁剪
/// → 返回**生效后**的值（前端直接用响应刷新界面，不必再 GET 一次）。
///
/// 立即裁剪的必要性：保留期只在「记账 / 写日志」时顺带生效，
/// 用户把 30 天改成 7 天之后如果不动，旧数据要等到下一条请求或下次重启才消失 ——
/// 那时的界面会显示「设置是 7 天，日志里还有 30 天前的条目」，看起来像没生效。
pub async fn put_retention(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };

    // 全部字段先校验，任何一项非法都不写入。
    let mut patch = RetentionPatch::default();
    let targets = [
        (
            KEY_LOG_RETENTION_DAYS,
            &mut patch.log_days,
            RETENTION_MIN_DAYS,
            RETENTION_MAX_DAYS,
            "天",
        ),
        (
            KEY_REQUEST_RETENTION_DAYS,
            &mut patch.request_days,
            RETENTION_MIN_DAYS,
            RETENTION_MAX_DAYS,
            "天",
        ),
        (
            KEY_DAILY_RETENTION_DAYS,
            &mut patch.daily_days,
            RETENTION_MIN_DAYS,
            RETENTION_MAX_DAYS,
            "天",
        ),
        (
            KEY_LOG_RETENTION_COUNT,
            &mut patch.log_count,
            RETENTION_MIN_COUNT,
            RETENTION_MAX_COUNT,
            "条",
        ),
        (
            KEY_REQUEST_RETENTION_COUNT,
            &mut patch.request_count,
            RETENTION_MIN_COUNT,
            RETENTION_MAX_COUNT,
            "条",
        ),
    ];
    for (key, slot, min, max, unit) in targets {
        let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        match parse_retention_value(key, value, min, max, unit) {
            Ok(value) => *slot = Some(value),
            Err(message) => return errors::management_error(400, message),
        }
    }

    // 空 PUT 只返回当前值，不写盘、不清理、不记录伪更新。
    let updated = patch.log_days.is_some()
        || patch.log_count.is_some()
        || patch.request_days.is_some()
        || patch.request_count.is_some()
        || patch.daily_days.is_some();
    if updated {
        if !config::set_retention(patch) {
            // 写盘失败：内存快照已更新（本次运行仍生效），但重启后会回到旧值 ——
            // 必须让用户知道，否则「改了设置重启又变回去」会被当成玄学问题
            logging::log("[Config]", "⚠️  数据保留设置写入失败，本次运行内仍生效");
        }
        // ── 立即清理（两种数据各自的时机不同，这里一次触发）──
        // 请求统计：prune() 内部**动态取**保留期回调，所以取到的就是刚写入的值
        state.request_stats().prune();
        // 事件日志：LogStore 的保留天数同样走回调（见 `LogStore::with_db` 的注释），
        // prune 返回裁掉的条数，打一行控制台便于确认「改小天数确实删了东西」
        if let Some(store) = logging::store_ref() {
            let removed = store.prune();
            if removed > 0 {
                logging::console_line("[Logs]", &format!("按保留设置清理了 {removed} 条日志"));
            }
        }
        // 调试报文最多 100 条，并跟随请求日志条数；用户改小请求日志上限时
        // 立即把上游调试报文一并收敛，不必等下一次请求写入
        let removed = crate::server::core::debug_traffic::prune();
        if removed > 0 {
            logging::console_line("[Debug]", &format!("按保留设置清理了 {removed} 条调试报文"));
        }
    }

    let settings = config::retention_settings();
    if updated {
        logging::log(
            "[Config]",
            &format!(
                "数据保留设置已更新: 日志 {} 天 / {} 条，请求日志 {} 天 / {} 条，按天聚合 {} 天",
                settings.log_days,
                settings.log_count,
                settings.request_days,
                settings.request_count,
                settings.daily_days
            ),
        );
    }
    ok_json(retention_json(settings))
}

/// 数据保留设置的响应体（GET 与 PUT 共用，保证两边键名完全一致）。
///
/// 键用**常量标识符**而不是手写字符串：`json!` 会把标识符当表达式求值再
/// `.into()` 成键名，所以这里落到 JSON 里的就是常量的值（`"logRetentionDays"` 等）。
/// 好处是键名只在 `config.rs` 定义一次，读侧（`days_field`）、写侧
/// （`set_retention`）与响应体三处必然一致 —— 手写三遍字符串，拼错一处
/// 不会报错，只会静默地少一个字段。
fn retention_json(settings: RetentionSettings) -> Value {
    json!({
        KEY_LOG_RETENTION_DAYS: settings.log_days,
        KEY_LOG_RETENTION_COUNT: settings.log_count,
        KEY_REQUEST_RETENTION_DAYS: settings.request_days,
        KEY_REQUEST_RETENTION_COUNT: settings.request_count,
        KEY_DAILY_RETENTION_DAYS: settings.daily_days,
    })
}

fn parse_retention_value(
    key: &str,
    value: &Value,
    min: i64,
    max: i64,
    unit: &str,
) -> Result<i64, String> {
    config::parse::int_in_range(value, min, max)
        .ok_or_else(|| format!("{key} 必须是 {min}-{max} {unit}范围内的整数（收到: {value}）"))
}
