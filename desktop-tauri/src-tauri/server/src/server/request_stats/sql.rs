//! 请求统计两张表的**行级 SQL 访问层** —— 本模块里唯一出现 SQL 的地方。
//!
//! ── 为什么单独一层 ──────────────────────────────────────────
//! 改造前明细是内存里一个按 ts 升序的 `Vec`（`insert_sorted` 维护），聚合是
//! 一个 `BTreeMap<String, DailyEntry>`，两个文件各有自己的落盘策略（明细
//! 「追加为主 + 攒够 `COMPACT_STEP` 次才整文件重写」，聚合「变更满 50 次或
//! 距上次落盘超 60 秒才重写」）。那套「内存快照 + 落盘镜像」的双份状态整体
//! 消失之后：明细是一行 `INSERT`、聚合是一行 UPSERT，报表口径仍由
//! `report.rs` 的纯函数算 —— 本文件承接这些语句，上层（`request_stats.rs`）
//! 只做归一、降级与 API 形状。
//!
//! ── `row_id` 与 `id` 请务必分清（`db/schema.rs` 的同名注释是权威）──
//!   - `row_id`：`INTEGER PRIMARY KEY AUTOINCREMENT`，**物理行号**，排序用它。
//!     显式 AUTOINCREMENT 保证**不复用**（删掉最大行后新行不会拿到同一个号）——
//!     翻页游标一旦复用就会漏行 / 重行。
//!   - `id`：请求的关联键（与调试报文同值），**可以重复、可以是空串**，
//!     所以它既不是主键也不参与排序（同 ts 的次序靠 `row_id`）。
//! 旧实现的「明细按 ts 升序、同毫秒先写入的在前」对应 `ORDER BY ts, row_id`；
//! 「新在前」对应 `ORDER BY ts DESC, row_id DESC`（同 ts 时 row_id 大的就是后
//! 写入的那条，与原实现「升序数组反转」的次序逐位一致）。
//!
//! ── request_raw 表与「进行中」行（本次新增的两件事）────────────
//!   - `request_raw`：下游原始正文（预览对话的地基），与 requests 同 id 关联。
//!     它的写入（`upsert_raw`）由上层在明细记账后单独调；**删除必须挂在
//!     requests 明细的每一个删除点之后/之前同步做**（见各删除函数的注释），
//!     否则明细没了正文还在，成为永远读不出来的孤儿行。
//!   - status=0 的行：转发开始前由 `insert_started_request` 先插的「进行中」
//!     行，收尾时由 `update_running_request` 补全终态字段（先 UPDATE 后
//!     INSERT 的理由见 `update_running_request`）。这类行**不进日报**
//!     （`fold_into_daily` 按 status=0 跳过），且会被 `delete_stale_running`
//!     按时间清理（网关崩溃留下的僵尸行）。
//!
//! ── 过滤条件为什么能全部下推给 SQL ──────────────────────────
//! 这里没有「只能在 Rust 侧算」的条件：`model` 是精确匹配、`status` 是两列的
//! 复合判定、`start`/`end` 是数值区间，都能表达成 WHERE。`query_requests` 与
//! 清理与预览计数共用同一份
//! [`FilterPlan`] ——「页面上筛出来的 N 条」与「清空删掉的那批」因此必然是
//! 同一个集合（事件日志那边的 keyword 不能下推是因为 SQLite 的 LIKE 只折叠
//! ASCII 大小写；本模块没有关键词过滤，也就没有那个约束）。
//!
//! ── 聚合行的读-改-写 ────────────────────────────────────────
//! `request_daily` 一天一行，三个 `*Stats` 列是 JSON 数组文本。累计它们必须
//! **先读出整行、在 Rust 里累加、再整行写回**：总量列与三个维度列是同一批
//! 字段，用 `UPDATE ... SET requests = requests + 1` 只推进总量、维度另算，
//! 会让「各维之和 = 当天总量」这条对账前提变成两处维护（见 `fold_into_daily`）。
//! 一天只有一行、三个数组通常几十个条目，读-改-写在**一个事务**里完成，代价可接受
//! （这是连接池 / 增量 UPDATE 都换不来的东西：口径只有一处）。
//! 这一支的语句（含列常量与编解码）住在 `daily.rs` —— 两张表分文件后各自
//! 演化，本文件专注 `requests` 明细表与跨表共用的 [`FilterPlan`]。
//!
//! ── 并发：本层不加锁 ────────────────────────────────────────
//! 所有函数取裸 `&Connection`，串行化由 `Db` 那把 Mutex 负责（上层每个公开方法
//! 都在**一次** `Db::with` / `with_mut` 调用里跑完，多语句操作用事务包住）。
//! **硬约束**：持这把锁期间绝不能再调 `logging::log` —— 日志要写同一个库，
//! `std::sync::Mutex` 不可重入，会当场死锁（`request_stats.rs` 模块头也记了这条）。

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::Value;

use super::record::{AttemptDetail, RequestEntry, RequestQuery, RunningProgress, SensitiveHit};
use super::report::{normalize_status_filter, SummaryRow};

mod raw;
pub(super) use raw::{
    count_raw, count_raw_matching, count_upstream_raw, delete_raw_matching, select_raw, trim_raw_rows,
    upsert_raw,
};

/// `requests` 的列顺序（所有 SELECT 都按这个顺序取，`decode_request` 依赖它；
/// 不写 `SELECT *` 是为了「加列时读侧要显式跟上」这件事在 diff 里可见）
///
/// **新列一律追加在末尾**：`decode_request` 按序号取值，只有追加才不会牵动
/// 前面字段的序号（中间插一列会让所有后续字段整体挪一位，那种改动在整个文件里
/// 看不出错，只会静默取错值）。因此后加的列都在最后：
///   - `attempt_details` / `sensitive_hits`（schema v2 的 JSON 文本列）；
///   - `credit`（schema v23 的积分消耗，可空 REAL）。
pub(super) const REQUEST_COLUMNS: &str = "id, ts, model, account_id, account_name, status, duration_ms, \
     first_response_ms, attempts, error, prompt_tokens, completion_tokens, total_tokens, \
     cache_read_tokens, provider, client_model, upstream_model, attempt_details, sensitive_hits, \
     client_reasoning, upstream_reasoning, phase, phase_started_at, is_test, route, client, \
     api_key_id, api_key_name, api_key_source, credit, client_evidence";

/// 报表窄列查询的列顺序 —— **与 `db::schema` 的 `idx_requests_summary` 列序一致**
/// （索引是 `ts, is_test` 前缀 + 这里的后 5 列），改任一侧都要回来对一眼。
///
/// 报表侧的明细只服务两件事：近 24 小时逐小时趋势（本文件的
/// [`select_summary_rows`]）与四档命中率（已由 [`sum_cache_windows`] 在 SQL 侧
/// 算完，不再搬行）。两件事合起来只碰这 6 列，而 `requests` 一行里另外那些列
/// （`route` 在实测库里平均 1.5 KB、`attempt_details` / `sensitive_hits` 各是
/// JSON 数组文本…）报表一个都用不上 —— 按 [`REQUEST_COLUMNS`] 全列读，等于把
/// 「读 24 小时的统计口径」变成「读 24 小时里躺在行内的十几 MB 无关字节」，
/// 这正是本次改造要消掉的主要开销（同一份数据实测：全列读 273 ms、窄列读
/// 98 ms，Rust 侧比例相同）。
///
/// `is_test` 只在 WHERE 里出现（报表恒排除测试流量），不进结构体，所以不在这
/// 份列清单里 —— 它在索引中排在 `ts` 之后、这 5 列之前。
const SUMMARY_COLUMNS: &str =
    "ts, model, upstream_model, total_tokens, cache_read_tokens, prompt_tokens";

// `request_daily`（按天聚合）那一支的列常量、编解码与读-改-写语句在
// `daily.rs` —— 两张表的语句分文件后各自独立演化。

// ─── 行解码 ─────────────────────────────────────────────────

/// 行 → `RequestEntry`。
///
/// 列与字段一一对应，**不做任何归一**（不重新夹数值、不重算 `is_success`）：
/// 库里的值都是写入侧 `NewRequestEntry::normalize` 归一过的，读侧再归一既多余，
/// 也会掩盖「有人绕过写入侧直接改库」这件事。可空的两列（`error` /
/// `first_response_ms`）原样读成 `Option` ——「没有错误」与「空串错误」在写入侧
/// 已经收敛成同一个 `None`，读侧不必再分。
///
/// 末尾两个 JSON 列走 [`decode_json_list`]：坏值退化成空表而不是让整条明细读不出来
/// （理由见那个函数）。
pub(super) fn decode_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestEntry> {
    let attempt_details: String = row.get(17)?;
    let sensitive_hits: String = row.get(18)?;
    Ok(RequestEntry {
        id: row.get(0)?,
        ts: row.get(1)?,
        model: row.get(2)?,
        account_id: row.get(3)?,
        account_name: row.get(4)?,
        status: row.get(5)?,
        duration_ms: row.get(6)?,
        first_response_ms: row.get(7)?,
        attempts: row.get(8)?,
        error: row.get(9)?,
        prompt_tokens: row.get(10)?,
        completion_tokens: row.get(11)?,
        total_tokens: row.get(12)?,
        cache_read_tokens: row.get(13)?,
        provider: row.get(14)?,
        client_model: row.get(15)?,
        upstream_model: row.get(16)?,
        attempt_details: decode_json_list::<AttemptDetail>(&attempt_details),
        sensitive_hits: decode_json_list::<SensitiveHit>(&sensitive_hits),
        // 末尾两列是 schema v5 加的（顺序与 REQUEST_COLUMNS 一致）；
        // 旧行的 DEFAULT '' 原样读出，前端按空串处理
        client_reasoning: row.get(19)?,
        upstream_reasoning: row.get(20)?,
        // 末尾两列是 schema v6 加的（在途阶段与阶段起点）；不在途的行/旧行
        // 读出来是 '' 与 NULL —— 与「不在途」在写入侧收敛成同一组值
        phase: row.get(21)?,
        phase_started_at: row.get(22)?,
        // 末列是 schema v7 加的（测试来源）；旧行 DEFAULT 0 → false（真实流量）。
        // SQLite 没有布尔类型，按 INTEGER 读再判非零 —— 不依赖驱动对 bool 的
        // 隐式转换（手改过的库里写成 2 也照样算「是测试」）
        is_test: row.get::<_, i64>(23)? != 0,
        // 末列是 schema v10 加的（选路决策快照）；旧行 / 没走到选路的行是
        // NULL —— 读成 Null，展示层不渲染那一段。rusqlite 未开 serde_json
        // feature，Value 不能直接 FromSql：读 TEXT 再解析，坏值退 Null
        // （与 decode_json_list 的「坏值退化」同一取向）
        route: row
            .get::<_, Option<String>>(24)?
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null),
        // 末列是 schema v12 加的（请求来源客户端）；旧行的 DEFAULT '' 原样
        // 读出 —— 空串 = 没有记录（前端显示占位符），与无 UA 的请求同一形态
        source: super::RequestSource {
            client: row.get(25)?,
            api_key: super::ApiKeyIdentity {
                id: row.get(26)?,
                name: row.get(27)?,
                source: row.get(28)?,
            },
            evidence: serde_json::from_str::<crate::server::core::client::ClientEvidence>(&row.get::<_, String>(30)?).ok()
                .filter(|evidence| !evidence.method.is_empty()),
            behavior_client: String::new(),
        },
        // 末列是 schema v23 加的（积分消耗）；旧行 / 没上报的行是 NULL ——
        // 读成 None，与「上报了 0」分开（前端前者显示「—」）。
        credit: row.get(29)?,
    })
}

/// JSON 数组文本 → `Vec<T>`（明细行的两个附属列）。解析失败或内容不是数组时
/// 退化成空表。
///
/// 与 `decode_accum`（聚合行那三个列，已拆到 `daily.rs`）同一取向：这两个列是
/// **补充信息**，一个坏值不该让整条明细读不出来 —— 退化成空表只是少了重试链 /
/// 命中表，而状态、模型、用量、错误全都还在。反过来，若在这里报错，一条手改坏的
/// JSON 就能让请求日志整页拉不出来（`select_page_desc` 会因为一个 `?` 直接失败）。
fn decode_json_list<T: serde::de::DeserializeOwned>(text: &str) -> Vec<T> {
    serde_json::from_str(text).unwrap_or_default()
}

/// JSON 数组文本 ← `Vec<T>`。序列化失败给 `'[]'`（与 DDL 的默认值同一形态，
/// 于是「这一列没有数据」在库里只有一种表示）。
fn encode_json_list<T: serde::Serialize>(items: &[T]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
}

/// `Value` → JSON 文本列（`Null` 写成 SQL NULL —— 「没有决策」与旧行同形态，
/// 与 `encode_json_list` 的 `'[]'` 不同：那里空表是有意的信息，这里 NULL 才是）。
fn encode_optional_json(value: &serde_json::Value) -> SqlValue {
    if value.is_null() {
        SqlValue::Null
    } else {
        SqlValue::Text(value.to_string())
    }
}

// ─── 过滤条件 → SQL ─────────────────────────────────────────

/// 一次查询的**筛选计划**：WHERE 片段 + 绑定值。
///
/// 查询、清理和预览共用一份筛选计划。
/// 旧实现是两处共用同一个 `matches_filter` 函数，现在共用的层次更靠下：连 SQL 的
/// WHERE 片段都是同一份，两处不可能漂移。
/// `offset` / `limit` 是分页参数、不是筛选条件，所以不在这里。
pub(super) struct FilterPlan {
    /// `WHERE ...` 片段；无条件时为空串（直接拼在 FROM 后面）
    where_sql: String,
    /// 与 `where_sql` 里的 `?` 一一对应的绑定值
    binds: Vec<SqlValue>,
}

impl FilterPlan {
    /// 按旧 `matches_filter` 的条件逐条编译（顺序无关，SQL 只做 AND 连接）。
    pub(super) fn of(filter: &RequestQuery) -> Self {
        let mut fragments: Vec<String> = Vec::new();
        let mut binds: Vec<SqlValue> = Vec::new();

        // ① 模型名精确匹配；空串（输入框清空）当没筛。
        //    匹配**两个**名字列：`model`（请求侧解析名）与 `upstream_model`
        //    （实际发给上游的名字）。报表的模型维度按上游真名聚合（见
        //    `fold_into_daily` 的 `model_stat_key`），而明细的 `model` 列存的是
        //    请求侧解析名 —— 只匹配一列时，映射请求在报表里显示的真名
        //    （只落在 upstream_model 列）会筛出 0 条。两个名字都该能筛到，
        //    条件因此取并集；与下拉候选（`select_model_options`）同一口径。
        if let Some(want) = filter.model.as_deref().filter(|text| !text.is_empty()) {
            fragments.push("(model = ? OR upstream_model = ?)".to_string());
            binds.push(SqlValue::Text(want.to_string()));
            binds.push(SqlValue::Text(want.to_string()));
        }

        // ①′ provider id 精确匹配（同一口径：空串当没筛）。
        //
        // 按 `provider` 列而不是「账号属于哪一家」判：那一列记的是**实际承载本次
        // 请求的那一家**（备援换号后是最后扛下来的那家），与列表里显示的提供商
        // 是同一个值 —— 筛选结果与肉眼看到的行必然一致。
        // 空 id 的行（一次都没发出去就失败）不会被任何非空 id 命中，这是有意的：
        // 它们不属于任何一家，用 status=error 看更直接（见 RequestQuery::provider）。
        if let Some(want) = filter.provider.as_deref().filter(|text| !text.is_empty()) {
            fragments.push("provider = ?".to_string());
            binds.push(SqlValue::Text(want.to_string()));
        }

        if let Some(want) = filter.client.as_deref() {
            fragments.push("client = ?".to_string());
            binds.push(SqlValue::Text(want.to_string()));
        }
        if let Some(identity) = filter.api_key.as_deref().and_then(super::ApiKeyIdentity::from_filter) {
            fragments.push("api_key_source = ? AND api_key_id = ?".to_string());
            binds.push(SqlValue::Text(identity.source));
            binds.push(SqlValue::Text(identity.id));
        }
        if filter.production_only {
            fragments.push("is_test = 0".to_string());
        }
        if filter.completed_only {
            fragments.push("status <> 0".to_string());
        }

        // ② 成功 / 失败 / 进行中：与 `RequestEntry::is_success` **逐字等价**的
        // 复合条件（成功与失败），加上进行中行的专属条件。
        //
        // 成功的判据是「2xx **且**没有错误摘要」（两列合起来才算一个条件，见
        // record.rs 里那条注释：流式请求的 200 是响应头阶段就发出去的，之后
        // 上游断流只能靠 error 表达）。所以：
        //   成功   → `status >= 200 AND status < 300 AND error IS NULL`
        //   失败   → 上面整条的取反（**不是** `status NOT BETWEEN`，那会漏掉
        //            「2xx 但带错误摘要」这一类 —— 它们必须是失败）
        //   进行中 → `status = 0`（转发开始时插的行，还没收尾；见
        //            `insert_started_request`）
        //
        // 失败条件**额外排除** status=0：取反会把「还没收尾」的进行中行也判成
        // 失败 —— 那是种草效应最差的一类误报（用户看到一排失败，其实只是
        // 还在跑）。status=0 在旧数据里从未被用过（schema 注释），这个排除
        // 只影响新语义的行。
        //
        // `error IS NULL` 与 `is_none()` 的等价性对**两种写入路径**都成立：
        //   - 运行期写入的行走 `normalize`，它把空串摘要收敛成 `None`
        //     （`error.filter(|text| !text.is_empty())`）→ 库里是 NULL；
        //   - 从旧文件导入的行**原样搬**（不过 `normalize`），旧文件里写了
        //     `"error": ""` 的行会落成空串。此时 SQL 判它是「有错误」（`IS NULL`
        //     为假）、Rust 的 `is_success()` 也判它有错误（`Some("")` 不是 `None`）
        //     —— 两边仍然一致。所以这个条件不需要额外的 `error <> ''` 分支。
        if let Some(want) = normalize_status_filter(filter.status.as_deref()) {
            let success = "status >= 200 AND status < 300 AND error IS NULL";
            let fragment = match want {
                super::report::StatusFilter::Ok => format!("({success})"),
                super::report::StatusFilter::Error => format!("(NOT ({success}) AND status <> 0)"),
                super::report::StatusFilter::Running => "status = 0".to_string(),
            };
            fragments.push(fragment);
        }

        // ③ / ④ 时间区间：闭开 [start, end)。`end` 是开区间，与分页口径一致
        //（上一页最后一条的 ts 可以直接当下一页的 end，不会重复取到同一条）。
        if let Some(from) = filter.start {
            fragments.push("ts >= ?".to_string());
            binds.push(SqlValue::Integer(from));
        }
        if let Some(to) = filter.end {
            fragments.push("ts < ?".to_string());
            binds.push(SqlValue::Integer(to));
        }

        let where_sql = if fragments.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", fragments.join(" AND "))
        };
        Self { where_sql, binds }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.where_sql.is_empty()
    }
}

// ─── 明细：计数与读取 ───────────────────────────────────────

/// 明细总条数（列表页的 `total` 读数用它）
pub(super) fn count_all(conn: &Connection) -> rusqlite::Result<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |row| row.get(0))?;
    Ok(count.max(0) as usize)
}

/// 命中筛选的条数（`QueryResult.matched`）—— 与 `limit` 无关，
/// 前端据此算总页数（见 `ui/requests-panel.js` 的 `renderPager`）
pub(super) fn count_matching(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM requests{}", plan.where_sql);
    let count: i64 = conn.query_row(&sql, params_from_iter(plan.binds.iter()), |row| row.get(0))?;
    Ok(count.max(0) as usize)
}

/// 列表页一次要的三个计数（总条数 / 命中条数 / 其中进行中）—— **一次扫描**。
///
/// 原先三条独立 `COUNT(*)`：每条都在自己的 WHERE 上扫一遍表，而列表页每秒
/// 轮询一次，明细上百万行时等于每秒三遍全表。这里把三个计数写成同一个
/// `SELECT` 里的 `COUNT(*)` 与两个 `SUM(CASE ...)`，扫描次数从三降到一。
///
/// 每个 `CASE` 都要各自一组绑定参数（SQLite 不保证跨表达式复用），所以绑定
/// 值按「命中」与「命中且 status=0」各复制一份。
pub(super) fn counts(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<(usize, usize, usize)> {
    let filter = plan
        .where_sql
        .strip_prefix(" WHERE ")
        .unwrap_or("1");
    let sql = format!(
        "SELECT COUNT(*), \
         COALESCE(SUM(CASE WHEN ({filter}) THEN 1 ELSE 0 END), 0), \
         COALESCE(SUM(CASE WHEN ({filter}) AND status = 0 THEN 1 ELSE 0 END), 0) \
         FROM requests"
    );
    let mut binds = Vec::with_capacity(plan.binds.len() * 2);
    binds.extend(plan.binds.iter().cloned());
    binds.extend(plan.binds.iter().cloned());
    let (total, matched, running): (i64, i64, i64) = conn.query_row(
        &sql,
        params_from_iter(binds.iter()),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok((
        total.max(0) as usize,
        matched.max(0) as usize,
        running.max(0) as usize,
    ))
}

/// 筛选下拉的候选清单：明细里出现过的模型名与 provider id（各按出现次数降序）。
///
/// ── 为什么不按当前时间档位过滤 ──────────────────────────────
/// 清单回答的是「这份日志里出现过什么」，与界面上的时间档位是两个独立维度：
/// 跟着档位过滤的话，切一次档位就要重拉一次清单，而下拉的内容还会在用户正要选
/// 的时候整体换掉（选中项可能凭空消失）。全量清单稳定得多，代价是切到「今天」
/// 档位后可能列出一个很久以前出现过的模型 —— 选中它得到空结果，
/// 而原因一眼可见（时间档位那一栏还写着「今天」）。
///
/// ── 上限只截断展示 ──────────────────────────────────────────
/// 下拉是给人扫读的，几百项已超出可读范围；而且这份清单按出现次数排序，
/// 尾部那些只出现一两次的值价值极低。截断的是**候选**，不是筛选能力：
/// 前端仍会把当前选中的值保留在列表里（见 ui/requests-panel.js 的
/// `fillFilterSelect`），所以「筛了某值 → 清单里没有它」不会发生。
pub(super) fn select_filter_options(
    conn: &Connection,
    max: usize,
) -> rusqlite::Result<(Vec<String>, Vec<String>)> {
    Ok((
        select_model_options(conn, max)?,
        select_distinct_ordered(conn, "provider", max)?,
    ))
}

/// 模型筛选候选：`model` 与 `upstream_model` 两列非空值的并集。
///
/// 与 [`FilterPlan`] 的模型条件同一口径（两个名字都该能筛到，理由见那边的
/// 注释）：候选清单若只取 `model` 列，报表按上游真名聚合后，用户拿着真名
/// 来明细里筛时下拉里没有那个选项 —— 正是「列表里有这一行、下拉里却没有
/// 这个选项」的不一致（见 `stats_request_filters` 的说明）。非映射请求两个
/// 名字相同，在并集里自然合成一项，计数按两列出现次数合计。
///
/// 不复用 [`select_distinct_ordered`]：那个函数按单列分组，这里是两列
/// `UNION ALL` 后再分组，硬套会让列名参数变成两段 SQL 拼接，可读性反而差。
fn select_model_options(conn: &Connection, max: usize) -> rusqlite::Result<Vec<String>> {
    let sql = "SELECT name FROM (
         SELECT model AS name FROM requests WHERE model <> ''
         UNION ALL
         SELECT upstream_model AS name FROM requests WHERE upstream_model <> ''
       ) GROUP BY name ORDER BY COUNT(*) DESC, name ASC LIMIT ?1";
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params![max as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// 取某一列的非空值，按出现次数降序、同次数按值升序（最多 `max` 个）。
///
/// 同次数时**必须**有第二个排序键：只按 `COUNT(*)` 排序时同次数的值顺序由
/// SQLite 的扫描顺序决定，两次调用可能给出不同的顺序 —— 下拉里的项会无理由地
/// 换位置（用户刚要点的那一项可能正好跳走）。
///
/// 列名由调用方以字面量给出（不接收任何用户输入），所以直接拼进 SQL 是安全的；
/// 上限值仍走绑定参数，与其它查询保持同一种写法。
fn select_distinct_ordered(
    conn: &Connection,
    column: &str,
    max: usize,
) -> rusqlite::Result<Vec<String>> {
    let sql = format!(
        "SELECT {column} FROM requests WHERE {column} <> '' \
         GROUP BY {column} ORDER BY COUNT(*) DESC, {column} ASC LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![max as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

pub(super) fn source_filter_options(conn: &Connection, max: usize) -> rusqlite::Result<Value> {
    let mut client_stmt = conn.prepare(
        "SELECT grouped.client, CASE WHEN json_valid(r.client_evidence) \
         THEN COALESCE(json_extract(r.client_evidence, '$.clientName'), '') ELSE '' END \
         FROM (SELECT client, MAX(row_id) AS latest, COUNT(*) AS n FROM requests \
         GROUP BY client ORDER BY n DESC, client LIMIT ?1) grouped \
         JOIN requests r ON r.row_id = grouped.latest ORDER BY grouped.n DESC, grouped.client",
    )?;
    let clients = client_stmt.query_map(params![max as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare(
        "SELECT api_key_id, api_key_source, \
         (SELECT r.api_key_name FROM requests r WHERE r.api_key_id = requests.api_key_id \
          AND r.api_key_source = requests.api_key_source AND r.api_key_name <> '' ORDER BY r.ts DESC, r.row_id DESC LIMIT 1) \
         FROM requests GROUP BY api_key_source, api_key_id \
         ORDER BY COUNT(*) DESC, api_key_source, api_key_id LIMIT ?1",
    )?;
    let keys = stmt.query_map(params![max as i64], |row| {
        let identity = super::ApiKeyIdentity {
            id: row.get(0)?, source: row.get(1)?, name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        };
        Ok(serde_json::json!({ "value": identity.filter_value(), "label": super::source_report::key_label(&identity) }))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut clients: Vec<Value> = clients.into_iter().filter(|(client, _)| !client.is_empty()).map(|(client, label)| serde_json::json!({
        "value": client, "label": if label.is_empty() { super::source_report::client_label(&client).to_string() } else { label },
    })).collect();
    clients.push(serde_json::json!({ "value": "", "label": "未记录客户端" }));
    Ok(serde_json::json!({ "clients": clients, "apiKeys": keys }))
}

/// 最早 / 最晚的明细时间戳（空表为 `None`）—— `stats()` 的 `firstTs` / `lastTs`。
///
/// 旧实现取的是内存数组（恒按 ts 升序）的首尾元素的 ts，与 `MIN` / `MAX` 等价。
pub(super) fn min_ts(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MIN(ts) FROM requests", [], |row| row.get(0))
}

pub(super) fn max_ts(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MAX(ts) FROM requests", [], |row| row.get(0))
}

/// 分页取明细，**新在前**（`ORDER BY ts DESC, row_id DESC`）。
///
/// 旧实现是「升序数组反转后 `skip(offset).take(limit)`」：反转后同 ts 的次序
/// 是后写入的在前，正是 `row_id DESC` 的效果。`offset` 从 0 起（路由层的
/// 解析保证非负）。
pub(super) fn select_page_desc(
    conn: &Connection,
    plan: &FilterPlan,
    offset: usize,
    limit: usize,
) -> rusqlite::Result<Vec<RequestEntry>> {
    let sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM requests{} ORDER BY ts DESC, row_id DESC LIMIT ? OFFSET ?",
        plan.where_sql
    );
    let mut binds = plan.binds.clone();
    binds.push(SqlValue::Integer(limit as i64));
    binds.push(SqlValue::Integer(offset as i64));
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(decode_request(row)?);
    }
    Ok(out)
}

/// 取区间 `[from_ms, to_ms]` 内的**完整明细**，按写入顺序（ts 升序、同 ts 按 row_id）。
///
/// 现在只剩一个用途：`aggregate::rebuild_day` 重算某天聚合时取那一天的明细
/// （报表读明细的两条路都已改窄 —— 命中率走 [`sum_cache_windows`] 的 SQL 聚合、
/// 趋势走 [`select_summary_rows`] 的 6 列，见各自的说明；它们正是拿掉了这里
/// 那 25 列里「报表一个字段都用不到」的大字段）。
/// 顺序取升序是为了让重算与 `record` 的累加次序一致 —— 三个维度数组里条目的
/// 先后只影响视觉，但没必要制造差异。
///
/// `exclude_tests` 由调用方按「这份明细要不要算真实流量」给（`rebuild_day` 传
/// true，见 `RequestEntry::is_test`）：模型测试是人工反复发起的样本，折进聚合会
/// 把真实流量读歪。请求日志那一侧走的是 `select_page_desc`，不受这个开关影响
/// —— 日志要照常显示测试行。
pub(super) fn select_between(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    exclude_tests: bool,
) -> rusqlite::Result<Vec<RequestEntry>> {
    // 条件拼在这里而不是拆成两条 SQL：两条 SQL 迟早会漂（一处改了排序、
    // 另一处忘了），而它们的差异只有一个布尔条件
    let filter = if exclude_tests { " AND is_test = 0" } else { "" };
    let sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM requests WHERE ts >= ?1 AND ts <= ?2{filter} \
         ORDER BY ts ASC, row_id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![from_ms, to_ms])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(decode_request(row)?);
    }
    Ok(out)
}

/// 取区间内的**报表窄列**（[`SUMMARY_COLUMNS`] 那 6 列），给近 24 小时趋势用。
///
/// 与 [`select_between`] 的分工：那个返回完整 `RequestEntry`（按天重算与别处
/// 需要逐条看的场景都要），本函数只服务报表 —— 列集就是索引
/// `idx_requests_summary` 覆盖的那几列，于是这条查询是**纯索引扫描**，一行明细
/// 里的大字段（`route` 等）根本不会被读出来（列集与索引列序的对应关系见
/// [`SUMMARY_COLUMNS`]）。
///
/// 恒排除测试流量（`is_test = 0`）：报表回答的是「真实流量长什么样」，模型测试
/// 是人工反复发起的样本（与 `select_between` 的 `exclude_tests` 同一口径）。
/// 因此调用方不必再传那个开关 —— 报表是这里唯一的用途。
///
/// 不写 `ORDER BY`：调用方按整点归桶，跨桶与桶内都不依赖行序
/// （`report::cache_trend_24h` 的桶序由 24 个整点键决定，模型明细由
/// `model_breakdown_json` 显式排序）。省掉排序也让 SQLite 能自由选扫描方向。
pub(super) fn select_summary_rows(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<SummaryRow>> {
    let sql = format!(
        "SELECT {SUMMARY_COLUMNS} FROM requests \
         WHERE ts >= ?1 AND ts <= ?2 AND is_test = 0"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![from_ms, to_ms])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SummaryRow {
            ts: row.get(0)?,
            model: row.get(1)?,
            upstream_model: row.get(2)?,
            total_tokens: row.get(3)?,
            cache_read_tokens: row.get(4)?,
            prompt_tokens: row.get(5)?,
        });
    }
    Ok(out)
}

/// 各命中率窗口的 `(hitTokens, inputTokens)` 合计，**一次索引扫描算完**。
///
/// ── 为什么下推给 SQL ────────────────────────────────────────
/// 改造前四档窗口是「把 7 天明细全读回 Rust，再对同一个数组遍历四遍」——
/// 搬那次（含 `route` 等无关列）比算这次贵两个数量级。而报表页默认每秒轮询
/// 一次（`config::DEFAULT_REPORT_AUTO_REFRESH_SECONDS`），这笔开销是**持续**的。
/// 一次范围扫描就能同时算出四组 `CASE WHEN` 合计。
///
/// ── 扫描下界必须是**最长**的那个窗口（踩过一次的坑）──────────
/// 初版写成「取最短窗口当下界」，看着像省了扫描量，实则**算错**：`WHERE`
/// 已经把结果集限制在最近 10 分钟内了，后面 `CASE WHEN ts >= (now-24h)` 的
/// 条件对这批行**恒为真**，于是四档算出同一个数（全等于 10 分钟窗口）。
/// 这个错不会报错、不会抛异常，只会让报表上四档命中率长得一模一样 ——
/// 实测数据里前三个窗口刚好同值时尤其难发现。正确形态是「扫描范围覆盖最长
/// 窗口，各档在 `CASE WHEN` 里自己收窄」。
///
/// 代价上这也划算：`WHERE ts >= ?1`（最长窗口）配 `idx_requests_summary`
/// 是**纯索引扫描**（命中的列全在索引里，不回表），实测同一份数据上
/// 3 万行库、7 天窗口 68 ms → 3.4 ms。
///
/// `windows_ms` 由调用方给出（`report::CACHE_WINDOWS`，窗口值与 JSON 键名在
/// 那里一处定义），返回顺序与入参一一对应 —— 本函数只做「按窗口求和」，不含
/// 任何口径判断（`safe_rate` 的零除与越界处理仍在 `report` 层）。
///
/// 每个窗口给 `SUM(CASE WHEN ... END)` 而不是 `SUM(...) FILTER`：SQLite 到
/// 3.30 才支持 `FILTER` 子句，而 `CASE WHEN` 在所有版本上语义相同。`COALESCE`
/// 兜住「一个窗口都没有行」时的 NULL —— 返回 0 与「没有数据」在展示层是同一
/// 种读数（命中率 0%），不必让调用方区分。
pub(super) fn sum_cache_windows(
    conn: &Connection,
    now_ms: i64,
    windows_ms: &[i64],
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let Some(longest) = windows_ms.iter().copied().max() else {
        return Ok(Vec::new());
    };
    // 参数位：?1 = 扫描下界（**最长**窗口，见上面那段）、?2..?n = 各窗口下界、
    // 最后一位 = now（闭区间上界，与原来的 `ts <= now` 判定一致）
    let mut sql = String::from("SELECT ");
    for index in 0..windows_ms.len() {
        if index > 0 {
            sql.push_str(", ");
        }
        let slot = index + 2;
        sql.push_str(&format!(
            "COALESCE(SUM(CASE WHEN ts >= ?{slot} THEN cache_read_tokens END), 0), \
             COALESCE(SUM(CASE WHEN ts >= ?{slot} THEN prompt_tokens END), 0)"
        ));
    }
    let last = windows_ms.len() + 2;
    sql.push_str(&format!(
        " FROM requests WHERE ts >= ?1 AND ts <= ?{last} AND is_test = 0"
    ));
    let mut binds: Vec<i64> = Vec::with_capacity(windows_ms.len() + 2);
    binds.push(now_ms - longest);
    binds.extend(windows_ms.iter().map(|window| now_ms - window));
    binds.push(now_ms);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    // 无 GROUP BY 的聚合查询恒返回一行；真出现空结果（不该发生）时按「各窗口
    // 都没有数据」降级 —— 与 `safe_rate` 的零除口径一致，比 panic 合适
    let Some(row) = rows.next()? else {
        return Ok(vec![(0, 0); windows_ms.len()]);
    };
    let mut out = Vec::with_capacity(windows_ms.len());
    for index in 0..windows_ms.len() {
        out.push((row.get::<_, i64>(index * 2)?, row.get::<_, i64>(index * 2 + 1)?));
    }
    Ok(out)
}

/// 插一条**进行中**的明细（status=0，转发开始前调用；见 `RequestStats::record_started`）。
///
/// 幂等：同 id 已有进行中行时跳过（重复调用不产生第二行 —— 收尾的
/// `update_running_request` 只按「id + status=0」匹配，多一行会让终态字段
/// 补到错误的行上）。「已有终态行」不算重复：同 id 多行记账是存量语义
/// （重试、去重排队），新一次转发仍该有自己的进行中行。
///
/// 只写四列，其余列走 DDL 的 DEFAULT：与「验收路径不预填字段」的取向一致 ——
/// 进行中行只承诺「这条请求开始了、它叫什么名字」，终态字段一律等收尾时补。
///
/// ── 例外：阶段两列在这里就写 ────────────────────────────────
/// `phase='connecting'` + `phase_started_at=ts`（请求开始时刻）是**在途行**的
/// 固有属性，不是「将来才有的终态字段」：请求一进网关就处在「连接中」这一段
/// （选路 / 取凭证 / 建连都在里面），起点就是这一行的 `ts`。在这里写而不是等
/// 第一次在途回写，是为了让「刚插入、还没轮到任何采集点上报」的那一小段
/// （毫秒级，但列表一拍就可能读到）也有阶段可显示 —— 少了它，状态列会先闪一下
/// 空徽章再变成「连接中」。与 OmniProxy 的 `insertPendingLog`（落库即
/// `phase='connecting'`）逐字同法。
///
/// 返回是否真的插入了新行（调用方目前不需要区分，但测试与日志可能想看）。
pub(super) fn insert_started_request(
    conn: &Connection,
    id: &str,
    ts: i64,
    model: &str,
    client_model: &str,
    client_reasoning: &str,
    source: &super::RequestSource,
    is_test: bool,
) -> rusqlite::Result<bool> {
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM requests WHERE id = ?1 AND status = 0",
        params![id],
        |row| row.get(0),
    )?;
    if existing > 0 {
        return Ok(false);
    }
    // client_reasoning 随首发写入：下游等级在请求开始时就定稿了（与 client_model
    // 同一时刻、同一来源），进行中行就能显示「请求的什么(等级)」
    // is_test 同样随首发写入：来源是入口就知道的事实，等收尾再补会让「进行中」
    // 那一段的测试行看起来与真实流量一样
    // client（来源客户端）同一理由：它在入口解析请求头那一刻就定稿了，
    // 进行中行也应带上 —— 等收尾再补会让列表里「客户端」列在转发期间空一格
    conn.execute(
        "INSERT INTO requests (id, ts, model, client_model, client_reasoning, status, \
         phase, phase_started_at, is_test, client, api_key_id, api_key_name, api_key_source, client_evidence) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 'connecting', ?2, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![id, ts, model, client_model, client_reasoning, is_test as i64,
            source.client, source.api_key.id, source.api_key.name, source.api_key.source, source.evidence_json()],
    )?;
    Ok(true)
}

/// 收尾**陈旧的进行中行**：status=0 且开始时刻早于 cutoff 的行没有机会再收到
/// 收尾 UPDATE（进程崩溃 / 流任务泄漏），把它就地补一个明确的失败终态。
///
/// 为什么是「补终态」而不是删除：界面上的进行中 → 结束是用户会一直盯着看的状态
/// 流转。删掉一行等于让请求凭空消失，用户会以为平台漏记；补 408 则如实回答
/// 「这次没跑完」。OmniProxy 的 `applyInterruptedLogs`（requestLogLifecycle.ts）
/// 走的是同一条语义。`duration_ms` 与 `ts` 一并补齐，避免列出 0 耗时的假象。
pub(super) fn finish_stale_running(
    conn: &Connection,
    cutoff: i64,
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE requests SET status = 408, error = '网关重启或流中断，请求未能完成', \
         duration_ms = MAX(?2 - ts, 0), attempts = 1, \
         attempt_details = '[]', sensitive_hits = '[]', phase = '', phase_started_at = NULL \
         WHERE status = 0 AND ts < ?1",
        params![cutoff, now],
    )
}

/// 按 id 收尾一条**在途**行（`RequestStats::finalize_interrupted` 的唯一语句）。
///
/// 与 [`finish_stale_running`] 的分工：那个按「开始时刻早于阈值」批量扫，
/// 是崩溃恢复；这个按 id 精确收尾，由断线兜底守卫（`api::pipeline::DisconnectGuard`）
/// 在 handler 被 axum 取消（客户端放弃连接）时调用。两者都补 408 —— 对报表
/// 而言都是「这一轮没跑完」，具体原因在 error 文案里。
///
/// ── 只改终态三列，**不动**两个明细列 ────────────────────────
/// 在途回写可能已经攒了真实的尝试链与敏感词命中，那是排障材料（对比
/// `finish_stale_running`：崩溃行没有任何在途数据，整体重置是合理的）。
/// `attempts` 取 `MAX(attempts, 1)`：保留在途已记录的轮数，但不让它落到 0
/// （存储契约是「含首次、恒 ≥1」）。
///
/// 两个阶段列**清掉**（与另两条收尾同一纪律）：阶段只在途有意义，这一行已经
/// 落定成 408 了，留着「上次看到的阶段」会让状态列的第二行凭空多出一个
/// 不再前进的计时。
///
/// 匹配条件与收尾 UPDATE 一致（`id + status = 0`）：已收尾的行打空，
/// 不覆盖终态 —— 与「在途回写晚到一步」同一纪律。
pub(super) fn finish_running_request(
    conn: &Connection,
    id: &str,
    error: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE requests SET status = 408, error = ?2, duration_ms = MAX(?3 - ts, 0), \
         attempts = MAX(attempts, 1), phase = '', phase_started_at = NULL \
         WHERE id = ?1 AND status = 0",
        params![id, error, now],
    )
}

/// 回写一条**进行中**明细的**在途**字段（转发期间每次状态真的变化时调一次；
/// 见 `RequestStats::update_running`）。
///
/// ── 匹配条件与收尾**完全一致**（`id + status = 0`）────────────
/// 收尾之后这一条必然打空 —— 那正是想要的：终态字段由收尾统一写，在途回写
/// 晚到一步不该把终态覆盖回去（把一个已经收尾的行改回「还在飞」的样子）。
/// 同一条请求写多少次都只是覆盖同一行的同几列，所以不需要幂等之外的判据。
///
/// ── 为什么不动这几列 ────────────────────────────────────────
///   - `status` / `error`：它们决定前端认不认这行是「进行中」（有 error 就不再是，
///     见 `ui/requests-panel.js` 的 isRunning）—— 转发中途的失败可能只是换号前的
///     一次尝试失败，不该让列表里的行提前变成失败；
///   - `ts` / `model` / `client_model`：请求发起时就定稿了，在途没有新值；
///   - `duration_ms` / 四个 token 列：只有收尾才有值（用量在途中不显示）。
///
/// 阶段两列**在这一份里的含义与别处不同**：它们是要写的（见 `phase` 字段的
/// 说明），只是与 status/error 一样属于「在途」而不是终态。
pub(super) fn update_running_progress(
    conn: &Connection,
    id: &str,
    progress: &RunningProgress,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE requests SET provider = ?2, account_id = ?3, account_name = ?4, \
         upstream_model = ?5, upstream_reasoning = ?6, attempts = ?7, first_response_ms = ?8, \
         attempt_details = ?9, sensitive_hits = ?10, phase = ?11, phase_started_at = ?12, \
         route = COALESCE(?13, route) WHERE id = ?1 AND status = 0",
        params![
            id,
            progress.provider,
            progress.account_id,
            progress.account_name,
            progress.upstream_model,
            progress.upstream_reasoning,
            progress.attempts,
            progress.first_response_ms,
            encode_json_list(&progress.attempt_details),
            encode_json_list(&progress.sensitive_hits),
            progress.phase,
            progress.phase_started_at,
            progress.route.as_ref().map(Value::to_string),
        ],
    )
}

/// 收尾一条进行中的明细：按 `id + status=0` 匹配，**补齐全部终态字段**。
///
/// ── 为什么是「先 UPDATE 后 INSERT」而不是唯一索引 + UPSERT ──────
/// `requests.id` 上**不能**建唯一索引：存量数据里同 id 可能有多行
/// （重试 / 去重排队等路径产生的重复记账，见 schema 的 id 列注释），
/// 建索引会直接撞唯一约束、丢数据。而没有唯一约束就没有 UPSERT 的冲突目标
/// —— `ON CONFLICT(id) DO UPDATE` 在没有对应唯一索引时会报语法错误。
/// 所以收尾走两步：先 UPDATE 进行中行（有就补全），影响 0 行再 INSERT
/// （转发前就失败等没有进行中行的路径维持原样）。两步在同一个事务里，
/// 不存在「UPDATE 完、INSERT 前」被读到的中间态。
///
/// 更新**不动 row_id**：进行中行已经占住的物理行号不变，列表顺序稳定
/// （行在「进行中」期间就出现在页面上，收尾后不该跳到别处）。`id` 为空的行
/// 直接返回 0（不 UPDATE 任何行）：空 id 的 started 行不存在，早期失败路径
/// 也没有 id 可匹配 —— 交给 INSERT。
pub(super) fn update_running_request(
    conn: &Connection,
    entry: &RequestEntry,
) -> rusqlite::Result<usize> {
    if entry.id.is_empty() {
        return Ok(0);
    }
    conn.execute(
        "UPDATE requests SET ts = ?2, model = ?3, account_id = ?4, account_name = ?5, status = ?6, \
         duration_ms = ?7, first_response_ms = ?8, attempts = ?9, error = ?10, prompt_tokens = ?11, \
         completion_tokens = ?12, total_tokens = ?13, cache_read_tokens = ?14, provider = ?15, \
         client_model = ?16, upstream_model = ?17, attempt_details = ?18, sensitive_hits = ?19, \
         client_reasoning = ?20, upstream_reasoning = ?21, phase = '', phase_started_at = NULL, \
         is_test = ?22, route = ?23, client = ?24, \
         api_key_id = ?25, api_key_name = ?26, api_key_source = ?27, credit = ?28, client_evidence = ?29 \
         WHERE id = ?1 AND status = 0",
        params![
            entry.id,
            entry.ts,
            entry.model,
            entry.account_id,
            entry.account_name,
            entry.status,
            entry.duration_ms,
            entry.first_response_ms,
            entry.attempts,
            entry.error,
            entry.prompt_tokens,
            entry.completion_tokens,
            entry.total_tokens,
            entry.cache_read_tokens,
            entry.provider,
            entry.client_model,
            entry.upstream_model,
            encode_json_list(&entry.attempt_details),
            encode_json_list(&entry.sensitive_hits),
            entry.client_reasoning,
            entry.upstream_reasoning,
            // 来源标记收尾时**再写一次**（而不是只在首发时写）：这条 UPDATE 是
            // 无进行中行时的兜底之外的主路径，写一次保证两条插入路径口径一致
            entry.is_test as i64,
            encode_optional_json(&entry.route),
            // 客户端同 is_test：首发已写过一次，收尾再写一次保证
            // 「有进行中行」与「直接插入」两条路径落库的值同源
            entry.source.client,
            entry.source.api_key.id,
            entry.source.api_key.name,
            entry.source.api_key.source,
            entry.credit,
            entry.source.evidence_json(),
        ],
    )
}

pub(super) fn terminal_record_exists(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM requests WHERE id = ?1 AND status <> 0) \
         AND NOT EXISTS(SELECT 1 FROM requests WHERE id = ?1 AND status = 0)",
        params![id], |row| row.get(0),
    )
}

/// 插入一条明细（`row_id` 由库自增，调用方不必也不该给）
///
/// 末尾两个 JSON 列由 [`encode_json_list`] 编码；空表写成 `'[]'`，与 DDL 的
/// 默认值同形（于是「没有数据」在库里只有一种表示，读侧不必分「NULL 还是空数组」）。
///
/// **两个阶段列不在这里写**：终态行的阶段恒为空（见 `RequestEntry::phase`），
/// 而 DDL 的默认值（`''` / `NULL`）就是那个值 —— 走 DEFAULT 比显式写一遍更不容易
/// 漂移（写入侧将来加字段时不会漏掉这两列）。
pub(super) fn insert_request(conn: &Connection, entry: &RequestEntry) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO requests (id, ts, model, account_id, account_name, status, duration_ms, \
         first_response_ms, attempts, error, prompt_tokens, completion_tokens, total_tokens, \
         cache_read_tokens, provider, client_model, upstream_model, attempt_details, \
         sensitive_hits, client_reasoning, upstream_reasoning, is_test, route, client, \
         api_key_id, api_key_name, api_key_source, credit, client_evidence) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
         ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
        params![
            entry.id,
            entry.ts,
            entry.model,
            entry.account_id,
            entry.account_name,
            entry.status,
            entry.duration_ms,
            entry.first_response_ms,
            entry.attempts,
            entry.error,
            entry.prompt_tokens,
            entry.completion_tokens,
            entry.total_tokens,
            entry.cache_read_tokens,
            entry.provider,
            entry.client_model,
            entry.upstream_model,
            encode_json_list(&entry.attempt_details),
            encode_json_list(&entry.sensitive_hits),
            entry.client_reasoning,
            entry.upstream_reasoning,
            entry.is_test as i64,
            encode_optional_json(&entry.route),
            entry.source.client,
            entry.source.api_key.id,
            entry.source.api_key.name,
            entry.source.api_key.source,
            entry.credit,
            entry.source.evidence_json(),
        ],
    )?;
    Ok(())
}

// ─── 明细：删除与裁剪 ───────────────────────────────────────

/// 删除命中筛选的明细，返回删除条数。
///
/// 调用方必须先在同一事务中清理两侧报文（`delete_raw_matching`）。
pub(super) fn delete_matching(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<usize> {
    let sql = format!("DELETE FROM requests{}", plan.where_sql);
    conn.execute(&sql, params_from_iter(plan.binds.iter()))
}

/// 清空全部明细。调用方必须先在同一事务中清理两侧报文。
pub(super) fn delete_all_requests(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM requests", [])
}

/// 删掉 `ts < cutoff` 的过期明细（保留期裁剪：`record` 每次记账与 `prune` 都走它）。
///
/// **先删**这批明细对应的 request_raw 行再删明细本身（顺序理由同 `delete_raw_matching`）：
/// 过期行通常为 0～少量，`ts` 索引让子查询的代价可以忽略 —— 记账热路径上
/// 多一条带子查询的 DELETE，代价可忽略（明细默认按 99999 天近似全量保留，
/// 真正频繁触发删除的是用户把天数改小的那段时间窗）。
pub(super) fn delete_expired_requests(conn: &Connection, cutoff: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM request_raw WHERE id IN \
         (SELECT id FROM requests WHERE ts < ?1 AND id <> '')",
        params![cutoff],
    )?;
    conn.execute("DELETE FROM requests WHERE ts < ?1", params![cutoff])
}

// ─── 聚合：读-改-写 ─────────────────────────────────────────
//
// `request_daily` 的全部语句已拆到 `daily.rs`（列常量、编解码与
// select / upsert / delete / trim），拆分理由见该文件的模块头。
