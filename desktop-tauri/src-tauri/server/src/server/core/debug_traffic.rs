//! 调试模式的**上游原始报文**存储：`debug_traffic` 表。
//!
//! ── 存什么（只有上游侧，不含下游）───────────────────────────
//! 开启调试模式（config.json 的 `debugMode`）后，每次转发在**即将发送前**
//! 抓一份 envelope：
//!   - 请求侧：上游 URL、**脱敏后的**请求头、实际发出去的请求体（已按家改写
//!     过 model 的最终形态）；
//!   - 响应侧：上游状态码、**脱敏后的**响应头、响应体（流式为上游原始 SSE
//!     文本，非流式为聚合前的原始文本）。
//! 下游侧（客户端发来的请求体 + 网关下发的响应体）由 `request_raw` 表采
//! （`api::pipeline::capture_raw_body` / `capture_raw_json`），与本表**同一个
//! 开关**：调试模式开启时两处都采、关闭时两处都不采（预览对话因此默认没有
//! 正文可看 —— 对齐 OmniProxy 的口径）。另有一份与本开关无关的覆盖式落盘
//! （`write_debug_files` → `{config_dir}/debug/last-request.json`），那是
//! 「最近一次入站请求体」的快照，三者互不重叠。
//!
//! ── 为什么单独一张表，不塞进 requests 的行里 ──────────────────
//! 一次 SSE 响应动辄数百 KB，塞进请求明细会让那张表迅速膨胀、拖慢每一页的
//! 读取。分开之后请求明细的读取成本与调试模式无关；详情按 id 到这张表取。
//! 两侧用**同一个 id 关联**（`record::RequestEntry::id`）。这条判断在
//! 「文件 → 表」的改造里一字未变，只是「单独一个文件」变成了「单独一张表」。
//!
//! ── 脱敏（**硬要求**，不可关闭）─────────────────────────────
//! 请求头里的 `authorization` / `cookie` / `x-api-key` 等凭据类字段一律替换成
//! `[redacted]`（见 [`redact_headers`]）。这不是可选项：报文是明文落盘的，
//! 不脱敏等于把各家账号的 token 抄在磁盘上。响应头里的 `set-cookie` 同理。
//!
//! ── 留存控制 ──────────────────────────────────────────────
//! 请求体与响应体完整保存，不按单条大小截断；仅超过 [`MAX_ENTRIES`] 时淘汰最旧记录。
//! 未完成的流按实际收到的字节保存；人为中止不会伪造未收到的内容。
//!
//! ── `Db` 是进程级状态（本模块的两处特殊之处之一）────────────
//! 与 `AccountStore` / `LogStore` / `RequestStats` 不同，本模块没有「注入式句柄」
//! 的形态：框架用 `static DB: OnceLock<Option<Db>>` 装一次，其余函数直接取。
//! 原因是采集器的生命周期 —— [`TrafficCapture`] 随响应流活到请求结束才 drop，
//! 那一刻触发 [`record`]，而**那个位置拿不到 `ServerState`**（响应流活得比
//! handler 栈帧久）。所以采集侧只能是进程级可取的，与 `logging::store_ref`
//! 同一模式。
//! 改造前还有两个静态量，它们的去留：
//!   - `STORE: OnceLock<Mutex<TrafficStore>>` **删除**：内存快照（`VecDeque` +
//!     `bytes`）整体不再需要 —— 库就是全集，读取是一次查询；
//!   - `DIRECTORY: RwLock<Option<PathBuf>>` **删除**：报文不再有自己的目录，
//!     也不再有「换目录」这回事（数据进了统一库，见 `file()` 的说明）。
//!
//! ── 并发模型 ────────────────────────────────────────────────
//! 改造前是「一把 `Mutex` 包住内存快照 + 落盘」；现在串行化由 `Db` 那把 Mutex
//! 负责（每次操作在一次 `Db::with` / `with_mut` 里跑完）。**所有失败都不影响
//! 请求**：写库失败只打一行日志。
//!
//! ── 硬约束：持 `Db` 锁期间绝不能调 `logging::log` / `logging::verbose` ──
//! 那两个函数会写**同一个库**（`logging::store` → `LogStore::append`），而
//! `std::sync::Mutex` 不可重入 —— 在 `Db::with` 的闭包里调它们会当场死锁。
//! 所以本模块的错误路径**一律用 `eprintln!`**（改造前 `write_all` / `append_line`
//! 里用的就是它，这条纪律现在更要紧：库是共享的，而死锁会带走整个进程）。

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::db::Db;
use crate::server::logging;

mod sql;

/// 表里保留的最大条数。超过后**精确删到上限**（每次只删超出那几条）。
///
/// 取 500：与事件日志的上限同量级，够看「最近这批请求发生了什么」；
/// 调试模式是**临时排障**用的，不是长期归档 —— 真要长期留，用户该用导出，
/// 而不是指望这里无限增长。
///
/// ── 为什么不再「丢最旧的一半」───────────────────────────────
/// 旧实现的注释写明「丢一半」是为了省下每次超限都整文件重写的代价
/// （一次重写换掉一半的量，比逐条删便宜）。进数据库后这条理由**消失了**：
/// 淘汰是一条 DELETE，它的成本与删多少行无关（旧实现里正是「删多少」决定了
/// 要重写多少字节）。
/// 精确删到上限的好处是可预期的容量：用户看到 `count()` 就到 500 为止，
/// 不会出现「刚过 500 就掉到 250」那种一半的跳变。
/// 代价是超限后每次 `record` 都会发一条 DELETE，但那次删除通常只影响
/// **一两行**（`record` 每次只加一条，除非有人手工往库里塞了一大批）——
/// 而且只在超限时才发（见 [`record`] 的先判后删）。
pub const MAX_ENTRIES: usize = 500;

/// 凭据类请求头：命中即替换成 `[redacted]`（大小写不敏感）。
///
/// 名单在 OmniProxy 的基础上补齐了**我们五家用到的自定义头**：各家适配器
/// 会在头里带 token / cookie / 签名（见 `providers/*/adapter.rs` 的
/// `build_chat_request`）。宁可多脱几个（多脱只损失排障信息，少脱是事故）。
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "cookie2",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-access-token",
    "x-refresh-token",
    "x-ide-token",
    "x-ide-auth",
    "x-workbuddy-token",
    "x-raccoon-token",
    "x-cline-token",
    "x-qoder-token",
    "x-catpaw-cookie",
    "x-autoclaw-token",
];

/// 脱敏占位符（与 OmniProxy 同字面量，便于对照两边的日志）
pub const REDACTED: &str = "[redacted]";

/// 进程级数据库句柄（与 `logging::store_ref` 同一模式：`None` = 库不可用）。
///
/// `OnceLock<Option<Db>>` 而不是 `OnceLock<Db>`：`Db::open` 可能失败，而
/// `init` 的签名要接 `Option<Db>`（调用点手里就是它）—— 包在 `Option` 里
/// 让「未初始化」与「初始化了但库不可用」共用同一条取值路径（都是 `None`），
/// 各函数只需判一次。
static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 进程级库句柄（未初始化或库不可用时为 `None`）
fn db() -> Option<&'static Db> {
    DB.get().and_then(|slot| slot.as_ref())
}

/// 初始化存储（启动时调用一次；`OnceLock` 天然幂等 —— 重复调用只有第一次生效）。
///
/// ── 它现在还需要做什么（相比改造前少了一大半）───────────────
/// 改造前它做三件事：设目录、清空内存、把旧文件载入内存并裁一遍（超限还要
/// 回写文件）。进数据库后后两件都不需要了：
///   - **没有「载入」**：库就是全集，读取是一次查询（改造前必须先把文件读进
///     内存才能 `get` / `count`，那是文件时代的补偿机制）；
///   - **没有「启动裁剪」**：超限的行由 [`record`] 的留存条数守着，而启动时
///     库里最多就是上次退出时的状态（那时已经合规）；即便有人手工往库里塞了
///     超限数据，下一次 `record` 也会收敛它。
/// 所以它只剩「把 `Db` 装上」这一件事 —— 但**仍然必要且必须被调用**：没装
/// 就相当于本模块没启用，[`record`] 会静默丢弃、[`get`] 一律 `None`。
/// 无返回值、不报错：调试数据不值得让网关起不来（模块头的取向）。
///
/// 无论调试开关是否打开都要初始化（`ServerState::bootstrap` 的注释）：开关是
/// **逐请求**判定的（改完设置下一个请求就生效），存储没就绪会让开启后的第一批
/// 请求无处可落。
pub fn init(db: Option<Db>) {
    // 重复调用只让第一次生效（`set` 失败即已初始化）。与改造前一致：
    // 那时是覆盖目录 + 重新载入，现在库自己就是真相，覆盖没有意义。
    let _ = DB.set(db);
}

/// 报文数据所在的文件（**就是库文件**；设置页概况用）。
///
/// 语义从「报文文件路径」变成「装着报文数据的库文件」—— 与 `AccountStore::file()`
/// / `LogStore::file()` / `RequestStats::file()` 的处理一致。
///
/// ── 库不可用时为什么给 `None`（而不是约定路径）────────────────
/// 调用方 `api::storage_api` 现在读的是 `Db::file()`（库不可用时用约定路径兜底，
/// 由那边决定），本函数因此**没有调用方**了。保留它是为了与另外三个 store 的
/// 「file()」形态一致（它们都保留了这个访问器），且它是排障时「报文落在哪」
/// 这个问题的直接答案 —— 按本项目「有意保留的设施逐个标注并写明理由」的做法
/// 标注，而不是删掉（删了以后排障要用 `sqlite3` 才知道库在哪）。
///
/// 给 `None` 而不是约定路径的取舍仍然成立：调用方（若有）能表达「没有」，
/// 比给一个「大小 0 字节、可能根本不存在」的路径更诚实。
#[allow(dead_code)]
pub fn file() -> Option<PathBuf> {
    db().map(|db| db.file().to_path_buf())
}

/// 条数（设置页概况用；库不可用时 0）
pub fn count() -> usize {
    match db() {
        Some(db) => db
            .with(sql::count_and_bytes)
            .and_then(Result::ok)
            .map(|(count, _)| count)
            .unwrap_or(0),
        None => 0,
    }
}

/// 落一条完整报文；同 id 覆盖，失败只打日志，不影响转发。
pub fn record(entry: TrafficEntry) {
    super::blocking::in_place(|| record_sync(entry));
}

fn record_sync(entry: TrafficEntry) {
    let Some(db) = db() else {
        // 库不可用：静默丢弃（与改造前「STORE 未初始化」同一处理）。
        // 这里连日志都不打 —— 每个请求都会走到这一步，刷屏反而掩盖真问题。
        return;
    };
    // 失败只打控制台：**不能**用 logging::log / verbose —— 它们要写同一个库，
    // 而此刻我们正握着那把锁（见模块头的死锁约束）。
    match db.with_mut(|conn| sql::upsert_entry(conn, &entry)) {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            eprintln!("[Debug] 调试报文写入数据库失败: {error}");
            return;
        }
        // 连接中毒（`Db` 的取向：不拿半截数据当真相）
        None => return,
    }
    // 写入后只清理超过留存条数的旧记录。
    match db.with(|conn| sql::enforce_limits(conn)) {
        Some(Ok(())) | None => {}
        Some(Err(error)) => eprintln!("[Debug] 调试报文裁剪失败: {error}"),
    }
}

/// 按 id 取一条（请求日志页的「详情」列用；库不可用时 `None`）
///
/// 同 id 多行时取**最后写入的那一份**（`ORDER BY ts DESC, rowid DESC LIMIT 1`），
/// 与改造前「内存里从尾部往前找第一个匹配」逐字一致。
pub fn get(id: &str) -> Option<TrafficEntry> {
    db()?.with(|conn| sql::select_one(conn, id)).and_then(Result::ok).flatten()
}

/// 一条原始报文记录。
///
/// 字段用 `Option` 表达「这一侧没采到」：请求发不出去时没有响应侧，
/// 关闭调试模式时整条不写。`id` 是关联键 —— 与请求日志的 `id` 同值。
/// 这三个 `Option` 在库里的落点是**可空列**（`status` / `response_headers` /
/// `response_body`），**不用空串代替 NULL**：前端的详情弹窗依赖这个区分
/// （`ui/requests-panel.js` 的 `renderDetail`：`data.status == null` 显示 `-`，
/// `data.responseBody ? String(...)` 为空则不渲染那一块）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficEntry {
    /// 关联键：请求日志条目 id（同一请求在两边同 id）
    pub id: String,
    /// 记录时刻（毫秒 Unix 时间戳）
    pub ts: i64,
    /// 上游 URL（含路径；查询串原样保留）
    #[serde(default)]
    pub url: String,
    /// 实际承载的 provider id
    #[serde(default)]
    pub provider: String,
    /// 发给上游的请求头（**已脱敏**）
    #[serde(default)]
    pub request_headers: Value,
    /// 发给上游的请求体（已按家改写过 model 的最终形态）
    #[serde(default)]
    pub request_body: Value,
    /// 上游响应状态码（未发出 / 未收到响应时缺失）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// 上游响应头（**已脱敏**）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Value>,
    /// 上游响应体：流式为原始 SSE 文本，非流式为聚合前的原始文本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    /// 历史记录的截断标记；新采集的正文不截断。
    #[serde(default)]
    pub truncated: bool,
}

/// 头集合脱敏：凭据类字段换成 [`REDACTED`]，其余原样。
///
/// 收 `Vec<(String, String)>`（适配器给出的形态）与 `HeaderMap`（reqwest 响应）
/// 两种来源，统一输出 JSON 对象 —— 重名头按**后到覆盖**（与 HTTP 语义一致，
/// 且这两种来源里重名极少见）。
pub fn redact_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    let mut object = serde_json::Map::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        let shown = if SENSITIVE_HEADERS.contains(&lower.as_str()) {
            REDACTED
        } else {
            value
        };
        object.insert(lower, Value::String(shown.to_string()));
    }
    Value::Object(object)
}

/// 请求日志「详情」的响应形态：原始报文 + 请求日志的关联字段。
///
/// 由 `api::debug_api` 组装（本模块只给报文那半）。
pub fn detail_payload(entry: &TrafficEntry) -> Value {
    json!({
        "id": entry.id,
        "ts": entry.ts,
        "url": entry.url,
        "provider": entry.provider,
        "requestHeaders": entry.request_headers,
        "requestBody": entry.request_body,
        "status": entry.status,
        "responseHeaders": entry.response_headers,
        "responseBody": entry.response_body,
        "truncated": entry.truncated,
    })
}

// ─── 采集器（转发层用）────────────────────────────────────────

/// 一次转发的原始报文采集器。
///
/// ── 生命周期 ────────────────────────────────────────────────
/// 请求侧在**即将发送前**创建（此时 URL / 头 / 体都已定稿），响应侧在收到
/// 响应头时补上状态码与头，响应体随字节流累积，最后在**流结束或 drop 时**
/// 落库（[`Drop`]）。用 Drop 而不是「结束时显式调用」的理由与
/// `RecordingStream` 相同：客户端断开、上游断流、请求被取消三条路径都不会
/// 走到「正常结束」，只靠显式调用会丢掉那些最需要看的失败现场。
///
/// ── 为什么共享 `Arc` ────────────────────────────────────────
/// 流式路径里采集器要跟着 `ForwardStream` 走（它可能在 handler 返回后才被
/// axum 拉取完），非流式路径里跟着聚合函数走。两条路径都持有同一个
/// `Arc<TrafficCapture>`，谁最后 drop 谁落库。
pub struct TrafficCapture {
    state: std::sync::Mutex<CaptureState>,
}

struct CaptureState {
    entry: TrafficEntry,
    /// 响应体累积（落库时移到 entry.response_body）
    body: Vec<u8>,
    /// 是否真的发过上游请求（`reset_request` 置位）。
    ///
    /// 未置位 = 请求在到达转发层之前就失败了（没有可用账号、模型不存在…），
    /// 那种情况**不落库**：报文里除了 id 与时刻什么都没有，留着只会让
    /// 「详情」列表混进一堆空条目。
    sent: bool,
    /// 已落库（保证只写一次）
    done: bool,
}

impl TrafficCapture {
    /// 请求侧：转发开始时创建，只带 id（URL / 头 / 体等真正发送时由
    /// [`Self::reset_request`] 填上 —— 那时它们才定稿）。
    pub fn begin(id: &str) -> Self {
        Self {
            state: std::sync::Mutex::new(CaptureState {
                entry: blank_entry(id, "", "", &[], &Value::Null),
                body: Vec::new(),
                sent: false,
                done: false,
            }),
        }
    }

    /// 重置为一次新的尝试（同一条请求内的退避重试 / 401 刷新重试）。
    ///
    /// **最后一次为准**（与 OmniProxy 的「重试覆盖为最后一次尝试」同口径）：
    /// 重试时把上一轮的响应现场清掉，用户看到的是最终真正生效的那次往返，
    /// 而不是「第一次失败 + 第二次成功」混在一起的两段 body。
    pub fn reset_request(
        &self,
        url: &str,
        provider: &str,
        headers: &[(String, String)],
        body: &Value,
    ) {
        let mut guard = self.lock();
        guard.entry = blank_entry(&guard.entry.id.clone(), url, provider, headers, body);
        guard.body.clear();
        // 走到这里说明请求体已构造、即将发出 —— 这条报文有内容可落
        guard.sent = true;
        // done 不重置：本条请求只写一行，重试不产生新条目
    }

    /// 响应头到达时补上状态码与响应头（**必须在 consume response 之前调**）。
    ///
    /// 顺带清空已累积的响应体：重试场景下上一次尝试的 body 不该混进来
    /// （见 [`Self::reset_request`] 的「最后一次为准」）。
    pub fn attach_response(&self, status: u16, headers: &reqwest::header::HeaderMap) {
        let mut guard = self.lock();
        guard.entry.status = Some(status);
        guard.body.clear();
        let pairs = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap_or("[binary]")));
        guard.entry.response_headers = Some(redact_headers(pairs));
    }

    pub fn push(&self, chunk: &[u8]) {
        self.lock().body.extend_from_slice(chunk);
    }

    #[cfg(test)]
    pub(crate) fn captured_body(&self) -> String {
        String::from_utf8_lossy(&self.lock().body).into_owned()
    }

    /// 落库（幂等：第二次调用什么都不做）。
    ///
    /// 没真发过请求（`sent` 为假）时直接丢弃：那种报文除了 id 什么都没有
    /// （见 `CaptureState::sent` 的说明）。
    pub fn finish(&self) {
        let mut guard = self.lock();
        if guard.done {
            return;
        }
        guard.done = true;
        if !guard.sent {
            return;
        }
        let body = std::mem::take(&mut guard.body);
        if !body.is_empty() {
            // 字符可能横跨网络分片，完整累积后再解码。
            guard.entry.response_body = Some(match String::from_utf8(body) {
                Ok(text) => text,
                Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
            });
        }
        let entry = std::mem::replace(&mut guard.entry, blank_entry("", "", "", &[], &Value::Null));
        drop(guard);
        record(entry);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for TrafficCapture {
    fn drop(&mut self) {
        self.finish();
    }
}

/// 一条空白记录的构造（`begin` / `reset_request` 共用）
fn blank_entry(
    id: &str,
    url: &str,
    provider: &str,
    headers: &[(String, String)],
    body: &Value,
) -> TrafficEntry {
    let pairs = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()));
    TrafficEntry {
        id: id.to_string(),
        ts: logging::now_ms(),
        url: url.to_string(),
        provider: provider.to_string(),
        request_headers: redact_headers(pairs),
        request_body: body.clone(),
        status: None,
        response_headers: None,
        response_body: None,
        truncated: false,
    }
}

/// 调试模式是否开启（**唯一**判定入口）：上游报文（本模块各采集点）与
/// 下游正文（`api::pipeline::capture_raw_body` / `capture_raw_json`）都问它；
/// 请求日志的「预览对话」与「原始报文」两块数据因此同进同退。
///
/// 每次调用都读配置快照（`config::current()` 是 RwLock 读 + 结构体克隆）：
/// 开关改完下一个请求就生效，不必重启。转发热路径上多一次读锁是可接受的
/// —— 与「每次重试判定都读重试设置」同一取向（见 `config::RetrySettings`）。
pub fn enabled() -> bool {
    crate::server::config::current().debug_mode()
}

/// 迁移项用：把一批旧文件条目写进 `debug_traffic` 表（**一个事务**，含导入后的裁剪）。
///
/// 与 `logs` / `requests` 的迁移项同构：整批原子提交，导入后只按留存条数裁剪。
///
/// 用 `unchecked_transaction` 而不是 `transaction`：迁移框架给的签名是
/// `&Connection`（`transaction()` 要 `&mut`），调用点 `Db::with` 全程只有一把锁、
/// 没有第二个访问者 —— 那条「同连接不得嵌套事务」的约束由调用点保证。
///
/// ── `INSERT OR IGNORE` 而不是纯 INSERT（T11 之后必须如此）──────
/// `id` 是主键，而运行期也在往这张表写（用户在点「升级」之前可能已经开着调试
/// 模式发过请求）。重复点击「升级」时旧文件的 id 已在库里，纯 INSERT 会撞主键
/// 让整批回滚 —— 于是用户看到「调试报文迁移失败」而原因只是「导过了」。
/// 用 `OR IGNORE`：已存在的跳过、新的照常进，重复点击成为无害的空操作。
/// 返回 `(新导入条数, 已存在跳过的条数)` 供调用方记日志。
///
/// 为什么这里**不**用 `upsert_entry`（同 id 覆盖，运行期那条路用它）：
/// 覆盖会把库里那份换掉，而两份内容可能不同（运行期采的报文可能更完整 ——
/// 重试路径会重采、调试开关中途打开也会再采一次）。旧文件是**历史快照**，
/// 不该反过来盖掉更新的那份。
pub(crate) fn import_legacy(
    conn: &rusqlite::Connection,
    entries: &[TrafficEntry],
) -> rusqlite::Result<(usize, usize)> {
    let tx = conn.unchecked_transaction()?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for entry in entries {
        if sql::insert_entry_ignore(&tx, entry)? {
            imported += 1;
        } else {
            skipped += 1;
        }
    }
    sql::enforce_limits(&tx)?;
    tx.commit()?;
    Ok((imported, skipped))
}

// 这里原本有一个 `legacy_count`（「表非空即跳过」的幂等闸门）。**已删除**：
// T11 把迁移改成用户点「升级」触发之后，那个判据不成立了 —— 用户可能在那之前
// 开着调试模式发过请求（表里已有报文），于是本项被永远判成「已迁过」，
// 旧文件里的历史报文再也进不来（`logs` 项已线上复现同类死循环）。
// 现在靠 `INSERT OR IGNORE` 按主键幂等，不需要「表空不空」这个查询 ——
// 完整论证见 `db::migrate::debug::import_debug` 的幂等段。

/// 迁移项用：旧文件的解析口径（**与改造前 `init` 的载入逐字一致**）。
///
/// 逐行解析、空行跳过、坏行跳过（不报错、不 panic）—— 调试数据不值得让网关
/// 起不来。它留在本模块而不是搬进迁移项，理由与 T4 的 `parse_*` 相同：
/// 「一行 JSON 长什么样」是数据契约（`TrafficEntry` 的 serde 注解就是契约），
/// 与运行期必须共用同一份实现。
pub(crate) fn parse_legacy_jsonl(text: &str) -> Vec<TrafficEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TrafficEntry>(line) {
            out.push(entry);
        }
    }
    out
}
