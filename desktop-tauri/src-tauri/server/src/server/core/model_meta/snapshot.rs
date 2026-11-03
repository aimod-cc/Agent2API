//! models.dev 精简索引的装载与查询。
//!
//! ── 三份形态同一套解析 ──────────────────────────────────────
//! 索引的**文件形态**只有一种（脚本与运行期刷新产出同一形状，见
//! `scripts/sync-modelsdev.mjs` 的文件头），三个来源共用这里的 [`parse`]：
//!   · 内嵌快照（[`EMBEDDED`]，`include_str!` 进二进制，`source = "builtin"`）
//!   · 统一库 `kv` 里的上一份刷新结果（`source = "cache"`）
//!   · 本次联网刷新刚拉到的（`source = "network"`，见 `refresh`）
//! 装载优先级是 **cache > builtin**：内嵌快照是「一份都没有时的地板」，只要
//! 成功刷新过一次，就一直用那份到下一次成功刷新覆盖它（与
//! `providers::catalog_cache` 的「缓存不带有效期」同一取向：宁可给一份旧的，
//! 也不要退回发版时那份更旧的）。
//!
//! ── 归一化口径必须与脚本逐字一致 ────────────────────────────
//! [`key_of`] 是 `scripts/sync-modelsdev.mjs` 里 `keyOf` 的孪生实现：取 id 的
//! **末段**（我们各家的模型名基本等同于模型本名，而 models.dev 的 id 一律带
//! lab 前缀），小写、只留字母数字。两侧任何一处改了，索引就会「刷新后突然
//! 全不命中」—— 这个症状不会报错，只会让档位悄悄变回通用表，所以两处都写了
//! 指向对方的注释。
//!
//! ── 一次装载、之后只读 ──────────────────────────────────────
//! 索引是「读多写极少」的共享态：读路径（每次清单成型都要按模型查一次）拿读
//! 锁，刷新落地时拿写锁整体替换。硬约束与各家目录相同：**持锁期间不做网络
//! 请求**（刷新先把两份 JSON 拉完、压完，最后才在锁内替换）。
//!
//! ── 硬约束：持锁期间不打日志 ────────────────────────────────
//! 与 `config::sql` / `providers::catalog_cache` 同一条：`Db::with` 拿的是全局
//! 唯一的连接锁，而 `logging::log` / `verbose` 要往同一个库写 `logs` 表 ——
//! `std::sync::Mutex` 不可重入，在闭包里打日志会当场死锁。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::server::db::Db;
use crate::server::logging;

/// 内嵌快照：仓库里检入的构建产物（生成方式见 `scripts/sync-modelsdev.mjs`）。
const EMBEDDED: &str = include_str!("modelsdev.json");

/// `kv` 表里的键名（保留键，登记在 `db::schema::RESERVED_KV_KEYS`）。
pub const KV_KEY: &str = "modelsDevCatalog";

/// 一条模型元数据（**只留网关真正会用的字段**，其余在同步阶段就丢掉）。
///
/// 布尔位与数值都是 `Option`：同步阶段理论上六项齐全（models.json 的 schema
/// 把 `reasoning` / `tool_call` 标成必填），但这份数据来自外部、也可能被手工
/// 改过的缓存喂进来，读侧一律宽容 —— `None` 的含义是「这份数据没说」，
/// 调用方据此**不填**，而不是填一个 `false`（那会凭空断言「不支持」）。
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// canonical id（`zhipuai/glm-5.3`）—— 日志与排障用，网关不据此路由
    pub canonical: String,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub images: Option<bool>,
    pub video: Option<bool>,
    pub context: Option<u64>,
    pub output: Option<u64>,
    /// 该模型**认识的命名档位**（由弱到强，models.dev 原始顺序）。
    /// 空 = 这份数据没给出命名档位（只声明了开关 / 没声明）—— 网关侧回落通用表。
    pub levels: Vec<String>,
}

/// 一份索引 + 它的来历。
pub struct Snapshot {
    models: HashMap<String, Record>,
    /// 这份数据的生成时刻（models.dev 侧没有版本号，用抓取时刻代替）
    generated_at: i64,
    /// `builtin` / `cache` / `network`（管理页与日志据此说明数据新旧）
    source: &'static str,
}

impl Snapshot {
    /// 组装一份索引（`refresh` 压缩完两份官方 JSON 后调用；`source` 传 `network`）。
    pub fn new(models: HashMap<String, Record>, generated_at: i64, source: &'static str) -> Self {
        Self { models, generated_at, source }
    }

    /// 收录的模型条数（刷新结果里的 `count`）。
    pub fn len(&self) -> usize {
        self.models.len()
    }
}

/// 进程级快照（懒装载）。`None` 只在「还没读过」时出现；一旦读过就不会退回。
static SNAPSHOT: OnceLock<RwLock<Option<Arc<Snapshot>>>> = OnceLock::new();

/// 进程级库句柄（与 `config` / `providers::catalog_cache` 同一模式）。
static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 装入库句柄。`bootstrap` 里 `Db` 就绪后调用一次；重复调用忽略。
///
/// 收 `Option<Db>` 与 `catalog_cache::install` 同一形态：库打不开时也照常调用，
/// 那时只读内嵌快照（功能仍在，只是拿不到刷新结果）。
pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn db() -> Option<Db> {
    DB.get().and_then(|slot| slot.as_ref()).cloned()
}

/// 索引键：id 末段 + 小写 + 只留字母数字（脚本 `keyOf` 的孪生，见模块头）。
pub fn key_of(id: &str) -> String {
    id.rsplit('/')
        .next()
        .unwrap_or(id)
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// 按模型 id 查一条元数据（查不到给 `None` = 「这个模型 models.dev 没收录」）。
///
/// 热路径：每次清单成型对每个模型调用一次，索引在内存里，这里是一次哈希查找
/// 加一次小记录克隆。
pub fn lookup(model: &str) -> Option<Record> {
    let key = key_of(model.trim());
    if key.is_empty() {
        return None;
    }
    snapshot().models.get(&key).cloned()
}

/// 当前索引的状态（管理页与刷新结果展示；**不含**任何体积大的内容）。
pub fn status() -> Value {
    let snapshot = snapshot();
    serde_json::json!({
        "source": snapshot.source,
        "updatedAt": snapshot.generated_at,
        "count": snapshot.models.len(),
    })
}

/// 当前索引的来历与生成时刻（日志用；与 [`status`] 同源，避免两处各取一次）。
pub fn origin() -> (&'static str, i64) {
    let snapshot = snapshot();
    (snapshot.source, snapshot.generated_at)
}

/// 取当前快照，必要时装载（**cache > builtin**，理由见模块头）。
fn snapshot() -> Arc<Snapshot> {
    let lock = SNAPSHOT.get_or_init(|| RwLock::new(None));
    if let Some(loaded) = lock.read().ok().and_then(|slot| slot.clone()) {
        return loaded;
    }
    let loaded = Arc::new(load());
    let mut slot = match lock.write() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(loaded) = slot.clone() {
        return loaded;
    }
    *slot = Some(loaded.clone());
    loaded
}

/// 首次装载：库里的刷新结果优先，没有（或坏了）才用内嵌快照。
fn load() -> Snapshot {
    if let Some(cached) = read_cache() {
        return cached;
    }
    // 内嵌快照是构建产物，解析不了属于「发版坏了」；这时给一份空索引而不是
    // panic —— 模型清单本身照常工作，只是所有键都回到「未声明」。
    match parse(EMBEDDED, "builtin") {
        Some(snapshot) => snapshot,
        None => {
            logging::log("[Models]", "内嵌的 models.dev 快照解析失败（模型元数据将不可用，请重新构建）");
            Snapshot { models: HashMap::new(), generated_at: 0, source: "builtin" }
        }
    }
}

/// 用新索引整体替换内存快照（刷新成功后调用）。
pub fn replace(snapshot: Snapshot) {
    crate::server::core::providers::catalog::invalidate();
    let lock = SNAPSHOT.get_or_init(|| RwLock::new(None));
    match lock.write() {
        Ok(mut slot) => *slot = Some(Arc::new(snapshot)),
        Err(poisoned) => *poisoned.into_inner() = Some(Arc::new(snapshot)),
    }
    crate::server::core::providers::catalog::invalidate();
}

/// 解析一份快照文本（三份形态共用，见模块头）。
pub fn parse(text: &str, source: &'static str) -> Option<Snapshot> {
    let root: Value = serde_json::from_str(text).ok()?;
    let object = root.get("models")?.as_object()?;
    let mut models = HashMap::with_capacity(object.len());
    for (key, value) in object {
        if let Some(record) = parse_record(value) {
            models.insert(key.clone(), record);
        }
    }
    if models.is_empty() {
        return None;
    }
    Some(Snapshot {
        models,
        generated_at: root.get("generatedAt").and_then(Value::as_i64).unwrap_or(0),
        source,
    })
}

/// 序列化成文件形态（刷新落库用；与 [`parse`] 严格互逆）。
///
/// 键**排序后**写出：内嵌快照是脚本按同一顺序（字典序）生成的，两份格式一致
/// 才谈得上「互相替换」；也让库里那份 JSON 成为可 diff 的文本（HashMap 的随机
/// 遍历序会让每次刷新的键序都不一样，肉眼比对时全是噪音）。
pub fn serialize(snapshot: &Snapshot) -> String {
    let mut models = serde_json::Map::new();
    let mut keys: Vec<&String> = snapshot.models.keys().collect();
    keys.sort();
    for key in keys {
        models.insert(key.clone(), record_value(&snapshot.models[key]));
    }
    Value::Object(serde_json::Map::from_iter([
        ("generatedAt".to_string(), Value::from(snapshot.generated_at)),
        ("source".to_string(), Value::String(snapshot.source.to_string())),
        ("models".to_string(), Value::Object(models)),
    ]))
    .to_string()
}

fn record_value(record: &Record) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("canonical".to_string(), Value::String(record.canonical.clone()));
    for (key, value) in [
        ("reasoning", record.reasoning),
        ("toolCall", record.tool_call),
        ("images", record.images),
        ("video", record.video),
    ] {
        if let Some(flag) = value {
            object.insert(key.to_string(), Value::Bool(flag));
        }
    }
    for (key, value) in [("context", record.context), ("output", record.output)] {
        if let Some(number) = value {
            object.insert(key.to_string(), Value::from(number));
        }
    }
    if !record.levels.is_empty() {
        object.insert(
            "levels".to_string(),
            Value::Array(record.levels.iter().cloned().map(Value::String).collect()),
        );
    }
    Value::Object(object)
}

fn parse_record(value: &Value) -> Option<Record> {
    let object = value.as_object()?;
    let canonical = object.get("canonical").and_then(Value::as_str).unwrap_or_default().to_string();
    if canonical.is_empty() {
        return None;
    }
    Some(Record {
        canonical,
        reasoning: object.get("reasoning").and_then(Value::as_bool),
        tool_call: object.get("toolCall").and_then(Value::as_bool),
        images: object.get("images").and_then(Value::as_bool),
        video: object.get("video").and_then(Value::as_bool),
        context: object.get("context").and_then(Value::as_u64),
        output: object.get("output").and_then(Value::as_u64),
        levels: object
            .get("levels")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|level| !level.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// 读库里的刷新结果（库不可用 / 没写过 / 值坏了 → `None`，由调用方回落地板）。
fn read_cache() -> Option<Snapshot> {
    let db = db()?;
    // 只读一次库（整份就是一个键），解析放在闭包外做 —— 闭包里不能打日志，
    // 而解析失败的 verbose 要在闭包外补
    let text = db.with(read_text).flatten();
    let snapshot = text.as_deref().and_then(|text| parse(text, "cache"));
    if snapshot.is_none() && text.is_some() {
        logging::verbose("[Models]", "models.dev 缓存解析失败，改用内嵌快照");
    }
    snapshot
}

/// 把刷新结果写回库（失败只记 verbose：写不进去的后果是「下次启动用内嵌
/// 快照」，不该让一次成功的刷新因此报错 —— 与 `catalog_cache::save` 同一取舍）。
pub fn write_cache(text: &str) {
    let Some(db) = db() else { return };
    let written = db.with(|conn| write_text(conn, text)).unwrap_or(false);
    if !written {
        logging::verbose("[Models]", "models.dev 缓存写入失败：下次启动将回落到内嵌快照");
    }
}

fn read_text(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT value FROM kv WHERE key = ?1", params![KV_KEY], |row| row.get(0)).ok()
}

fn write_text(conn: &Connection, text: &str) -> bool {
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![KV_KEY, text],
    )
    .is_ok()
}
