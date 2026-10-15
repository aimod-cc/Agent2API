//! 管理面板的访问控制：管理员注册、双令牌会话、失败锁定与 /v1 fail-closed。
//!
//! ── 令牌模型（照 OmniProxy 的短效 + 长效语义）────────────────
//!   · access token：**短效**（2 小时），HttpOnly cookie（path=/），
//!     只存进程内存 —— 泄露窗口小，过期即废；
//!   · refresh token：**长效**（30 天），HttpOnly cookie（path=/api/panel，
//!     缩小暴露面），库里只存 **sha256 哈希**；携带它到
//!     `/api/panel/refresh` 轮换出新的 access + 新 refresh（同一会话链，
//!     旧条目标记 rotated）。access 在进程重启后丢失没关系 —— 浏览器凭
//!     refresh 静默换新，用户无感。
//!   · 撤销：登出按会话链整链撤销；**已轮换的旧 refresh 再次出现即视为
//!     泄露**（重放检测），整条会话链作废，需要重新登录。
//!
//! ── 管理员的两种来路 ─────────────────────────────────────────
//!   · 面板首次注册（推荐）：全新部署时登录页出现「创建管理员账号」，
//!     写入 `kv` 表的 `panelAdmin` 键 —— 不需要预先在部署配置里放密码；
//!   · 环境变量预置（`AGENT2API_ADMIN_USER` + 密码，跳过注册流程，
//!     无人值守 / IaC 部署用）：密码填 `AGENT2API_ADMIN_PASSWORD`（明文，
//!     内存里现场转成 bcrypt 哈希）或 `AGENT2API_ADMIN_PASSWORD_HASH`
//!     （已是 bcrypt 哈希，优先于明文）。启动时把哈希同步进库 ——
//!     **任何落盘形态都只有哈希**，明文只出现在部署配置里。
//!
//! ── 何时启用面板认证 ────────────────────────────────────────
//! 注册过（或环境变量预置了）管理员即启用：`/api/*` 需要会话或 API Key。
//! 桌面壳不注册也不设置 —— 访问控制维持原有免鉴权语义，桌面行为零变化。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;

use crate::server::db::Db;

/// 会话 cookie 名：**一枚装两段**（`agent2api-panel=<access>.<refresh>`）。
///
/// ── 为什么合成一枚 ─────────────────────────────────────────
/// 登录/刷新原先发**两条** `Set-Cookie`（access 在 `Path=/`、refresh 在
/// `Path=/api/panel`）。宿主中转（fnOS docker 管理页那一类）会把多条同名响应头
/// 按逗号拼成一条，于是属性被降级成 cookie 名 —— 现场读数里罐子上行的是
/// `path|domain|max-age|mode|session|version|expires` 这一堆，而新会话的值根本没
/// 按 `/` 落进罐里。后果是每次经中转开面板都要重登：生产一手读数是同一把
/// 对不上任何登录的僵尸指纹两天里出现 63 次。
/// **一条 Set-Cookie 没有可拼的对象**，属性不会被吞，cookie 通道自己就站得住；
/// 也就不必再依赖 localStorage（那台设备的 `凭证头=无` 是常态）。
///
/// ── 代价（有意接受）───────────────────────────────────────
/// refresh 的 Path 从 `/api/panel` 扩到 `/`：这一枚里同时装着轮换凭据。
/// 这个源只服务网关自己的面板与 `/v1/*`，且 access 本来就在 `/`，扩出去的增量
/// 面是「同源下别的可读路径」；而换到的是「经中转打开面板不再每次重登」。
/// 寿命取 refresh（30 天）：access 的生死由服务端会话表判（`AccessRecord` 的墙钟
/// 过期），cookie 只是载体 —— 过期后浏览器照样带着、服务端判无效、走 refresh，
/// 与头令牌模式同一套语义。
pub const ACCESS_COOKIE: &str = "agent2api-panel";
/// 一枚 cookie 里两段的分隔符。两段都是 `random_hex`，十六进制里不含点，
/// 所以 `split_once('.')` 是精确切分，不需要转义。
const TOKEN_SEP: char = '.';
/// 会话传输探测 cookie（90 天寿命）：登录页连打两发 `/api/panel/cookie-probe`
/// 测「这个环境里 cookie 能不能往返」。中转入口下发/回带都活不成，探测失败时
/// 前端才显式要求令牌进响应体（见 api::panel::cookie_probe 与 panel::issue_response）。
/// 它不关联任何会话，服务端只比对值、不认会话。
///
/// **只留一枚**：探针要预测的是登录响应那趟 `Set-Cookie` 的命运，而登录响应现在
/// 也只发一条 —— 判据的形状必须和被预测的对象一致（原来种两枚正是因为登录发两条；
/// 合成一枚之后两枚探针反而会误报「通道好」）。
pub const PROBE_COOKIE: &str = "agent2api-panel-probe";
/// **升级过渡期**的旧 refresh cookie 名（上一版独立签发、`Path=/api/panel`）。
///
/// 只读、不签发：合成一枚之后登录/刷新都不再写它，这里读它是为了**升级那一刻
/// 不把所有在线的人踢掉**。那一版的 access 表在内存里（重启即空），而本次改动
/// 又把 access 挪到了新 kv 键（`panelAccessSessions`），所以升级后罐里那枚裸
/// access 必定判无效 —— 唯一还能把会话救回来的是 `panelTokens` 里那条 refresh，
/// 而它只存在这枚旧 cookie 里（refresh 表格式本次没动，旧记录照旧有效）。
/// 不读它就等于**升级即全员重登一次**。
///
/// 与 `split_session_cookie` 对裸 access 的回退是同一条理由的两半：那边不读会
/// 把还活着的会话无谓踢掉，这边不读会把还活着的 refresh 一起丢掉。
///
/// 排在会话 cookie 之后：两者都在罐里时新的优先（旧值多半是它上一次轮换的残本，
/// 命中「已轮换记录」只会白走一趟重放判定）。**下个大版本删掉**。
const LEGACY_REFRESH_COOKIE: &str = "agent2api-panel-rt";

/// access token 有效期（短效）
const ACCESS_TTL: Duration = Duration::from_secs(2 * 3600);
/// refresh token 有效期（长效）
const REFRESH_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// 登录失败锁定：同一来源连续失败 5 次，锁 5 分钟（按 IP 不是全局 ——
/// 全局锁会把「攻击者锁死管理员」变成一种攻击，Tinyauth 那个 CVE 的教训）
const LOCKOUT_THRESHOLD: u32 = 5;
const LOCKOUT_DURATION: Duration = Duration::from_secs(300);

const KV_ADMIN_KEY: &str = "panelAdmin";
const KV_TOKENS_KEY: &str = "panelTokens";
/// access 会话表（只存 sha256，见 `AccessRecord`）。⚠️ 新增键必须同时登记进
/// `db::schema::RESERVED_KV_KEYS`，否则用户改一次配置就把它静默删掉。
const KV_ACCESS_KEY: &str = "panelAccessSessions";

// ── 数据库句柄（启动时注入；kv 读写都走它）────────────────────

static STORE_DB: OnceLock<Option<Db>> = OnceLock::new();

/// 把库句柄交给本模块（`ServerState::bootstrap` 完成后调用一次）。
/// `None` = 库不可用：注册与刷新令牌的落盘能力随之降级
/// （环境变量预置的管理员仍可登录，但会话只存在于内存里）。
pub fn attach_db(db: Option<Db>) {
    STORE_DB.set(db).ok();
}

fn db() -> Option<&'static Db> {
    STORE_DB.get().and_then(|slot| slot.as_ref())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sha256_hex(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── 管理员（环境变量优先，其次库里的注册记录）────────────────

static ENV_ADMIN: OnceLock<Option<(String, String)>> = OnceLock::new();

fn env_admin() -> Option<&'static (String, String)> {
    // OnceLock<Option<(String, String)>>：首次调用时读入，此后返回静态引用。
    // 密码有两个变量：HASH（已是 bcrypt 哈希）优先；PASSWORD（明文）则
    // 现场 bcrypt::hash 一次 —— 内存与落库从此都只有哈希形态。
    ENV_ADMIN.get_or_init(|| {
        let user = std::env::var("AGENT2API_ADMIN_USER")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let from_hash = std::env::var("AGENT2API_ADMIN_PASSWORD_HASH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            // 兼容 `htpasswd -nBC` 的整行输出（"user:$2y$…"）：冒号后面是
            // bcrypt 哈希就只取哈希 —— 部署者把生成命令的输出整行粘进来
            // 就能用，不必手工剥前缀
            .map(|value| match value.split_once(':') {
                Some((_, hash)) if hash.starts_with("$2") => hash.to_string(),
                _ => value,
            });
        let hash = match from_hash {
            Some(hash) => Some(hash),
            None => std::env::var("AGENT2API_ADMIN_PASSWORD")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .and_then(|plain| match bcrypt::hash(plain, 10) {
                    Ok(hash) => Some(hash),
                    // bcrypt 拒绝的密码（如超过 72 字节）：报出来，别静默
                    // 失效让部署者以为账号已预置成功
                    Err(error) => {
                        eprintln!("❌ AGENT2API_ADMIN_PASSWORD 无法转成哈希: {error}");
                        None
                    }
                }),
        };
        match (user, hash) {
            (Some(user), Some(hash)) => Some((user, hash)),
            _ => None,
        }
    })
    .as_ref()
}

/// 库里的注册记录（`kv` 的 `panelAdmin`：{username, hash}）。
fn store_admin() -> Option<(String, String)> {
    let db = db()?;
    db.with(|conn| {
        conn.query_row(
            "SELECT value FROM kv WHERE key = ?1",
            rusqlite::params![KV_ADMIN_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok()
    })
    .flatten()
    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    .and_then(|object| {
        let username = object.get("username")?.as_str()?.to_string();
        let hash = object.get("hash")?.as_str()?.to_string();
        Some((username, hash))
    })
}

/// 面板认证是否启用（注册过或预置过管理员）。
pub fn panel_auth_enabled() -> bool {
    env_admin().is_some() || store_admin().is_some()
}

/// 管理员是否已经存在（登录页据此显示「注册」还是「登录」）。
pub fn admin_registered() -> bool {
    panel_auth_enabled()
}

/// 密码 → bcrypt 哈希（成本因子 10，与 env 预置、htpasswd -nBC 的输出互通）。
///
/// 哈希参数只在这一处定义：面板注册（`api::panel`）与桌面壳的注册命令
/// （绕过 HTTP 闸门的受信本地路径）都要产出同一格式的哈希，参数分叉会让
/// 两条入口造出互相验证不了的凭证。
pub fn hash_password(password: &str) -> Result<String, String> {
    bcrypt::hash(password, 10).map_err(|error| format!("密码加密失败: {error}"))
}

/// 首次注册管理员（只在无人注册时成功 —— 幂等安全：
/// 竞争下只有第一个写入者生效，后到的会看到「已注册」）。
///
/// 返回 Ok(false) = 已有管理员（环境变量或库），拒绝覆盖。
pub fn setup_admin(username: &str, password_hash: &str) -> Result<bool, String> {
    if panel_auth_enabled() {
        return Ok(false);
    }
    let Some(db) = db() else {
        return Err("数据库不可用，无法保存管理员账号".to_string());
    };
    let payload = serde_json::json!({ "username": username, "hash": password_hash });
    // ON CONFLICT DO **NOTHING**（不是 DO UPDATE）：面板注册是**竞争面** ——
    // 两个请求同时闯过上面的 panel_auth_enabled() 检查时，DO UPDATE 会让
    // 后写者覆盖先写者（先注册的管理员被凭空换人）。DO NOTHING 让竞态下
    // 只有一个写入者生效，后到者拿到 changed=0 → Ok(false) → 409。
    // （env 预置管理员的覆盖式落库走 `sync_env_admin_to_store`，与本函数无关。）
    let changed = db
        .with_mut(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO NOTHING",
                rusqlite::params![KV_ADMIN_KEY, payload.to_string()],
            )
        })
        .and_then(|result| result.ok())
        .ok_or_else(|| "写入管理员账号失败".to_string())?;
    Ok(changed > 0)
}

/// 环境变量预置的管理员同步进库（`kv` 的 `panelAdmin`，与面板注册同一处）：
/// env_admin() 里明文已转成 bcrypt 哈希，落库的自然也是哈希 —— 数据库的
/// 任何角落都不会出现明文密码。env 是显式的部署意图，覆盖式写入（改了
/// env 重启即生效）；没配 env 则什么都不做，走面板注册路径。
/// 启动时（`attach_db` 之后）调用一次。
pub fn sync_env_admin_to_store() {
    let Some((user, hash)) = env_admin() else {
        return;
    };
    let Some(db) = db() else {
        return;
    };
    let payload = serde_json::json!({ "username": user, "hash": hash });
    let _ = db.with_mut(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KV_ADMIN_KEY, payload.to_string()],
        )
    });
}

/// 校验账号密码。bcrypt 哈希兼容 `htpasswd -nBC` 的输出格式。
fn verify_credentials(username: &str, password: &str, expected: &(String, String)) -> bool {    let user_ok = constant_time_eq(username.as_bytes(), expected.0.as_bytes());
    let pass_ok = bcrypt::verify(password, &expected.1).unwrap_or(false);
    // 两个都算完再返回，避免「用户名对不对」的时序差异
    user_ok && pass_ok
}

/// 按面板提交的凭证找管理员并校验（环境变量优先，其次库里的注册记录）。
pub fn verify_login(username: &str, password: &str) -> bool {
    if let Some(expected) = env_admin() {
        return verify_credentials(username, password, expected);
    }
    store_admin().is_some_and(|expected| verify_credentials(username, password, &expected))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ── access token（短效，库里只存 sha256）──────────────────────

/// access token 的一条记录。**库里和内存里都以 sha256(token) 为键** ——
/// 原先键是裸令牌、表只在内存里，于是「重启」等价于「把所有活会话打成无效」：
/// 浏览器罐子里那枚 cookie 还在、还在每次开页时被带回来，服务端却不认了
/// （生产一手读数：`面板会话无效` 里 51 次是同一把早已对不上任何登录的指纹，
/// 时间点与 12:26 那次容器重启之后逐台吻合）。
///
/// 现在照 `panelTokens` 那条已经跑通的做法落库：只存哈希，启动时载回。
/// 安全性不降级 —— 库里那条哈希本来就够冒充（refresh 同形），而拿不到库的
/// 攻击者既拿不到旧版的内存表、也拿不到新版这条。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct AccessRecord {
    /// sha256(token)
    hash: String,
    /// 会话链 id：同一登录签发的 access 共享，撤销按链整条作废
    session: String,
    /// 过期时刻（毫秒，墙钟 —— 内存里的 `Instant` 跨不了进程）
    expires_at: i64,
}

type AccessTable = HashMap<String, (String, i64)>;

static ACCESS_TOKENS: OnceLock<Mutex<AccessTable>> = OnceLock::new();

fn access_tokens() -> &'static Mutex<AccessTable> {
    ACCESS_TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 生成随机 hex 字符串（会话令牌 / altcha 的 salt 等都用它）。
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("系统随机源不可用");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 读一个 kv 固定键（altcha 密钥等零散状态用；不存在返回 None）。
pub fn kv_get(key: &str) -> Option<String> {
    let db = db()?;
    db.with(|conn| {
        conn.query_row(
            "SELECT value FROM kv WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get::<_, String>(0),
        )
        .ok()
    })
    .flatten()
}

/// 写一个 kv 固定键（upsert）。调用方只传「RESERVED_KV_KEYS 里登记过的
/// 固定名」—— 配置写入按那个集合排除，这里写错键会被一次配置改动删掉。
pub fn kv_put(key: &str, value: &str) {
    let Some(db) = db() else { return };
    let _ = db.with_mut(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
    });
}

// ── refresh token（长效，库里只存 sha256）─────────────────────

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RefreshRecord {
    /// sha256(token)
    hash: String,
    /// 会话链 id：同一登录的轮换链共享，撤销按链整条作废
    session: String,
    /// 过期时刻（毫秒）
    expires_at: i64,
    /// 已被轮换（旧令牌不能再换；再次出现 = 泄露，整链作废）
    rotated: bool,
}

static REFRESH_TOKENS: OnceLock<Mutex<Vec<RefreshRecord>>> = OnceLock::new();

fn refresh_tokens() -> &'static Mutex<Vec<RefreshRecord>> {
    REFRESH_TOKENS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 整库恢复后清空短效令牌（`db::backup::import_from_file` 调用）。
///
/// 恢复把 `panelTokens` 换成了备份机器的记录，本机浏览器的 cookie 对不上
/// 新表；内存里的旧 access 会话若保留，用户会顶着「看似已登录」的状态用到
/// 过期才撞 401。清空让所有人立刻走重新登录 —— 行为与新库一致。
pub fn clear_access_tokens() {
    let mut table = match access_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.clear();
    // 表现在落库，只清内存会留一份"下一次启动又活过来"的库内容 —— 一起清空
    persist_access_tokens(&mut table);
}

/// 启动时把库里的 access 会话载入内存（与 `load_refresh_tokens` 同时调用）。
pub fn load_access_tokens() {
    let Some(loaded) = kv_get(KV_ACCESS_KEY) else { return };
    let Ok(records) = serde_json::from_str::<Vec<AccessRecord>>(&loaded) else { return };
    let table = access_table_from(records, now_ms());
    match access_tokens().lock() {
        Ok(mut current) => *current = table,
        Err(poisoned) => *poisoned.into_inner() = table,
    }
}

/// 记录 → 内存表：过期条目一律丢掉（库可能是两天前那份）。
fn access_table_from(records: Vec<AccessRecord>, now: i64) -> AccessTable {
    records
        .into_iter()
        .filter(|record| record.expires_at > now)
        .map(|record| (record.hash, (record.session, record.expires_at)))
        .collect()
}

/// 内存表 → 落库记录（过期条目顺手清掉）。库里**只存哈希**：整库导出与备份
/// 走的是同一个 kv，裸令牌进去就等于把可用凭据写进明文文件。
fn access_records(table: &AccessTable, now: i64) -> Vec<AccessRecord> {
    table
        .iter()
        .filter(|(_, (_, expiry))| *expiry > now)
        .map(|(hash, (session, expires_at))| AccessRecord {
            hash: hash.clone(),
            session: session.clone(),
            expires_at: *expires_at,
        })
        .collect()
}

fn persist_access_tokens(table: &mut AccessTable) {
    let records = access_records(table, now_ms());
    let payload = serde_json::to_string(&records).unwrap_or_else(|_| "[]".to_string());
    kv_put(KV_ACCESS_KEY, &payload);
}

/// 启动时把库里的刷新令牌载入内存（`bootstrap` 完成后调用一次；
/// 整库恢复后由 `db::backup::import_from_file` 再调一次完成重装）。
pub fn load_refresh_tokens() {
    let Some(loaded) = db().and_then(|db| {
        db.with(|conn| {
            conn.query_row(
                "SELECT value FROM kv WHERE key = ?1",
                rusqlite::params![KV_TOKENS_KEY],
                |row| row.get::<_, String>(0),
            )
            .ok()
        })
        .flatten()
    }) else {
        return;
    };
    let Ok(records) = serde_json::from_str::<Vec<RefreshRecord>>(&loaded) else {
        return;
    };
    match refresh_tokens().lock() {
        Ok(mut table) => *table = records,
        Err(poisoned) => *poisoned.into_inner() = records,
    }
}

fn persist_refresh_tokens(table: &mut Vec<RefreshRecord>) {
    // 过期条目顺手清掉（条目量级：每设备一条活链 + 若干轮换遗留）
    let now = now_ms();
    table.retain(|record| record.expires_at > now);
    if let Some(db) = db() {
        let payload = serde_json::to_string(&*table).unwrap_or_else(|_| "[]".to_string());
        let _ = db.with_mut(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![KV_TOKENS_KEY, payload],
            )
        });
    }
}

// ── 会话签发 / 校验 / 轮换 / 撤销 ─────────────────────────────

/// 一次登录的产出：**一条** Set-Cookie 值交给 handler 下发（一枚装两段，
/// 见 `ACCESS_COOKIE` 的说明）；裸令牌同时进响应体，供 cookie 走不通的环境
/// 存 localStorage 后走请求头（见 `session_valid` 的头回退说明）。
pub struct IssuedSession {
    pub session_cookie: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// 一条签发用的 `Set-Cookie` 值：`agent2api-panel=<access>.<refresh>`。
///
/// Path 恒为 `/`、`HttpOnly`、`SameSite=Lax`；`Max-Age` 取 refresh 的寿命
/// （access 的过期由服务端表判，cookie 只是载体 —— 让它跟着 access 两小时消失
/// 会把长效轮换凭据一起带走）。
fn issue_cookie(access_token: &str, refresh_token: &str) -> String {
    format!(
        "{ACCESS_COOKIE}={access_token}{TOKEN_SEP}{refresh_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        REFRESH_TTL.as_secs()
    )
}

/// 一条清除用的 `Set-Cookie`（同名同 Path + `Max-Age=0`）。
///
/// Path 必须与签发时**逐字相同** —— 浏览器删 cookie 要名字与 Path 同时命中。
/// 从前这里两处都写死 `Path=/`，而 refresh cookie 签在 `Path=/api/panel` 下，
/// 于是它从来没被删掉过：每台设备的罐子里都攒着一枚陈旧 refresh，每次
/// `/api/panel/*` 都把它带回来；而活的那把一旦不在候选里，`rotate_session`
/// 就只能按「已轮换的记录」命中这枚陈旧的 ⇒ 判重放、整条会话链作废 ⇒ 用户被
/// 弹回登录页。这是「过期 cookie 为什么反复出现」的直接成因之一。
/// 现在只有一枚会话 cookie、只有一个 Path，这类"另一格清不掉"的形状不存在了。
pub fn clear_cookie() -> String {
    format!("{ACCESS_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// 从一枚会话 cookie 的值里拆出 (access, refresh)。
///
/// 带分隔符的必须**两段都非空**，否则两侧都不算：`a.` / `.b` / `.` 这类形状是
/// 中转把响应拼歪之后留下的碎片，喂进候选只会让判定去比一把根本不存在的凭据
/// （判据要宁缺毋滥 —— 空串在 `cookie_values` 那一步已经被滤掉了，碎片没有）。
///
/// 没有分隔符的值（升级前那版留下的裸 access）只当 access 候选：它要么在服务端
/// 表里还活着（那就正常续用，代价是这一趟没有 cookie 侧的 refresh，头令牌仍在候选里），
/// 要么已经不活（下一次 401 会被 `clear_cookie` 清掉）。不值得为此留一套双格式框架。
fn split_session_cookie(value: &str) -> (Option<&str>, Option<&str>) {
    if let Some((access, refresh)) = value.split_once(TOKEN_SEP) {
        return if access.is_empty() || refresh.is_empty() {
            (None, None)
        } else {
            (Some(access), Some(refresh))
        };
    }
    if value.is_empty() { (None, None) } else { (Some(value), None) }
}

impl IssuedSession {
    fn issue(session: String) -> Self {
        let access_token = random_hex(32);
        let refresh_token = random_hex(48);
        let expiry = now_ms() + ACCESS_TTL.as_millis() as i64;
        match access_tokens().lock() {
            Ok(mut table) => {
                table.insert(sha256_hex(&access_token), (session.clone(), expiry));
                persist_access_tokens(&mut table);
            }
            Err(poisoned) => {
                let mut table = poisoned.into_inner();
                table.insert(sha256_hex(&access_token), (session.clone(), expiry));
                persist_access_tokens(&mut table);
            }
        }
        match refresh_tokens().lock() {
            Ok(mut table) => {
                table.push(RefreshRecord {
                    hash: sha256_hex(&refresh_token),
                    session,
                    expires_at: now_ms() + REFRESH_TTL.as_millis() as i64,
                    rotated: false,
                });
                persist_refresh_tokens(&mut table);
            }
            Err(poisoned) => {
                let mut table = poisoned.into_inner();
                table.push(RefreshRecord {
                    hash: sha256_hex(&refresh_token),
                    session,
                    expires_at: now_ms() + REFRESH_TTL.as_millis() as i64,
                    rotated: false,
                });
                persist_refresh_tokens(&mut table);
            }
        }
        Self {
            session_cookie: issue_cookie(&access_token, &refresh_token),
            access_token,
            refresh_token,
        }
    }

    /// 登录：开一条新会话链。
    pub fn new_session() -> Self {
        Self::issue(random_hex(16))
    }
}

/// 面板 access 令牌的**请求头**取值：`x-panel-token` 头 → `Authorization: Bearer`。
///
/// 存在的理由：面板不总在直连环境里跑 —— 从 fnOS docker 管理页一类宿主进入
/// 时，请求经宿主侧中转，HttpOnly cookie 的两跳（下发 → 回带）都可能被掐掉。
/// 前端（web_shim / login.html）把登录响应体里的裸令牌存 localStorage，之后
/// 每个请求带头；服务端**cookie 在前、头在后**（见 `access_candidates`），
/// 直连环境行为不变。
/// 头里的令牌 JS 可读（localStorage），与 HttpOnly cookie 相比多暴露给面板
/// 自身的 XSS 面 —— 这是反代环境下的可用性换安全，面板是同源可信代码。
fn access_header_tokens(headers: &HeaderMap) -> Vec<String> {
    named_header_tokens(headers, "x-panel-token")
        .into_iter()
        .chain(bearer_tokens_of(headers))
        .collect()
}

/// 取一个（可能重复出现的）请求头的所有非空值。
///
/// 复数是必要的而非防御性的：合并型中转（把多条同名响应头拼成一条的那类）也会
/// 这样处理**请求**头，于是浏览器同一个 `Cookie` 头里可能出现两次 `agent2api-panel=`
/// —— 一次有效一次陈旧。只取第一条就等于"看运气"。
fn named_header_tokens(headers: &HeaderMap, name: &str) -> Vec<String> {
    let name = axum::http::HeaderName::from_bytes(name.as_bytes());
    let Ok(name) = name else { return Vec::new() };
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn bearer_tokens_of(headers: &HeaderMap) -> Vec<String> {
    named_header_tokens(headers, "authorization")
        .into_iter()
        .filter_map(|value| {
            let token = value.strip_prefix("Bearer ")?.trim();
            if token.is_empty() {
                return None;
            }
            Some(token.to_string())
        })
        .collect()
}

/// access 凭据的**全部候选**，按「同名 cookie 的每个值 → `x-panel-token` → Bearer」
/// 排列（cookie 在前，直连环境的判定结果与从前逐字一致）。
///
/// ── 为什么不再是「cookie 优先、命中即返」────────────────────
/// 实测（2026-09-29，fnOS docker 管理页 :5666 中转）：登录成功签发新会话后，
/// 下一个 `/api/session` 仍被判无效，而日志里两行的指纹对不上 —— 浏览器回带的是
/// **上一轮**那把（`7a951233`），刚发的（`84f8d751`）没被带上。成因是中转把登录
/// 响应的两条 `Set-Cookie` 按逗号拼成一条：`agent2api-panel=新值, Path=/, HttpOnly, …`
/// 里 `Path` 从属性降级成了一个独立 cookie（回带名单里那些 `path|domain|max-age|
/// expires|session|version` 就是证据），新会话因此没按 `/` 存下来，罐里盖着的还是
/// 直连那次登录留下的旧值。
///
/// 此时**唯一**还能救回这条会话的是请求头令牌，而头令牌只在「令牌进响应体」模式
/// 下存在（见 `api::panel::issue_response`）。所以这里的改动必须与探针配套：探针把
/// 环境判成中转 → 客户端拿到 body 里的令牌并带头 → 服务端**不许**再被罐里那把陈旧
/// cookie 短路。只改一侧的结果是「探针判通了但 cookie 存歪」的老死循环，或「带了头
/// 却永远轮不到试」。
///
/// 上面那条**拼接**成因后来被从源头消掉了：登录/刷新改为只发**一枚**会话 cookie
/// （access 与 refresh 装进同一枚，见 `ACCESS_COOKIE`），一条 Set-Cookie 没有可拼
/// 的对象；探针也随之回到一枚 —— 判据的形状要和被预测的对象一致，现在要预测的就是
/// 一条。「同名 cookie 可能有几格」这条判定照旧留着：罐里那些升级前存下的陈值不会
/// 自己消失，穷尽候选仍是正确形态。
///
/// cookie 仍然排在头前面：中转哪天修好了，用户回到直连 cookie 登录，localStorage
/// 里那条早已作废的旧链令牌不该抢跑（它验证不过，自然也不再影响判定 —— 判定看的是
/// 「有没有一把有效」，不是「第一把是谁」）。
fn access_candidates(headers: &HeaderMap) -> Vec<String> {
    dedup(
        session_cookie_halves(headers)
            .into_iter()
            .filter_map(|half| half.0)
            .chain(access_header_tokens(headers))
            .collect(),
    )
}

/// refresh 凭据的**全部候选**，顺序与 `access_candidates` 同形
/// （会话 cookie 的每一条的第二段 → 升级期旧 refresh cookie → `x-panel-refresh`
/// → Bearer）。
///
/// 与 access 共用一份「为什么不再短路」的理由：轮换请求同样会被罐里那把旧
/// refresh 短路，结果是把活链旁边的僵尸链拿去轮换 —— 撞上重放检测，整链作废，
/// 用户被迫重登（这条路径此前只在日志里见过"登录已过期"，没人看得出是被短路）。
///
/// 旧名前那一格是**升级过渡**用的（见 `LEGACY_REFRESH_COOKIE`）：它排在会话 cookie
/// 之后，新格式在场时旧值不会抢跑。
pub fn refresh_candidates(headers: &HeaderMap) -> Vec<String> {
    dedup(
        session_cookie_halves(headers)
            .into_iter()
            .filter_map(|half| half.1)
            .chain(cookie_values(headers, LEGACY_REFRESH_COOKIE))
            .chain(named_header_tokens(headers, "x-panel-refresh"))
            .chain(bearer_tokens_of(headers))
            .collect(),
    )
}

/// 罐里**每一枚**会话 cookie 各自拆成 (access, refresh)。
///
/// 逐条拆（不是只取第一条）是必须的：合并型中转会在同名下留下多格，其中一格
/// 是新会话、另一格是陈值；只取第一条等于看运气。
fn session_cookie_halves(headers: &HeaderMap) -> Vec<(Option<String>, Option<String>)> {
    cookie_values(headers, ACCESS_COOKIE)
        .iter()
        .map(|value| {
            let (access, refresh) = split_session_cookie(value);
            (access.map(str::to_string), refresh.map(str::to_string))
        })
        .collect()
}

fn dedup(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

/// 请求是否携带有效 access token —— 候选里**任一把**在服务端会话表里就算有效。
///
/// 从前这里只取一把（cookie 有就只看 cookie），于是"罐里有一把陈年的同名 cookie"
/// 与"没带任何凭证"判定结果相同（都是 401），而现场完全不同 —— 见
/// `access_candidates` 的中转事故记录。
pub fn session_valid(headers: &HeaderMap) -> bool {
    let tokens = access_candidates(headers);
    if tokens.is_empty() {
        return false;
    }
    let now = now_ms();
    let mut table = match access_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.retain(|_, (_, expiry)| *expiry > now);
    tokens
        .iter()
        .any(|token| table.contains_key(&sha256_hex(token)))
}

/// 本次请求里能看到的 access 指纹（诊断日志用，最多两把 —— 排查「同名 cookie
/// 重复上行」时要能看见每一把分别是什么，值本身绝不进日志）。
pub fn access_fingerprints(headers: &HeaderMap) -> Vec<String> {
    access_candidates(headers)
        .into_iter()
        .take(2)
        .map(|token| token_fingerprint(&token))
        .collect()
}

/// 用请求里的 refresh 候选轮换出新一组令牌（同一会话链）。
///
/// 候选按顺序试，**第一个仍活着的**（未轮换、未过期）就是当前会话 —— 陈旧 cookie
/// 与头里的旧令牌都不会把活链挤掉。返回 `None` = 没有一把能用。
///
/// **重放检测**照旧生效，但只在「没有任何一把候选是活的」时才判：此时若某把候选
/// 命中了一条已轮换记录，说明它泄露了（活链上新令牌在浏览器手里）—— 整条会话链
/// 作废，逼着重新登录。之所以要先穷尽活候选：罐里同时躺着新旧两把是**正常现象**
/// （合并型中转就是这么留脏的），把"读到了旧的那把"当成泄露会把好环境踢成反复重登。
pub fn rotate_session(headers: &HeaderMap) -> Option<IssuedSession> {
    let candidates = refresh_candidates(headers);
    if candidates.is_empty() {
        return None;
    }
    let hashes: Vec<String> = candidates.iter().map(|token| sha256_hex(token)).collect();
    let now = now_ms();
    let mut table = match refresh_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.retain(|record| record.expires_at > now);
    let live = hashes.iter().find_map(|hash| {
        table
            .iter()
            .position(|record| &record.hash == hash && !record.rotated)
    });
    if let Some(index) = live {
        let session = table[index].session.clone();
        table[index].rotated = true;
        drop(table);
        return Some(IssuedSession::issue(session));
    }
    // 一把活的都没有：按「候选里最先出现的已轮换记录」判重放，整条会话链作废
    // （refresh 链 + 该链签发过的所有 access）。
    let replay = hashes.iter().find_map(|hash| {
        table
            .iter()
            .position(|record| &record.hash == hash && record.rotated)
    });
    // 连已轮换记录都没命中 → 纯粹的无效令牌（过期 / 别的会话 / 拼歪的碎片）：
    // 按「刷新失败」处理，不动任何链。
    let index = replay?;
    let session = table[index].session.clone();
    table.retain(|record| record.session != session);
    persist_refresh_tokens(&mut table);
    drop(table);
    revoke_access_of_session(&session);
    None
}

/// 请求里是否带着一把**仍活着**的 refresh（未过期、未轮换）。只读：不轮换、
/// 不判重放、不动任何会话链。
///
/// 存在的理由是 401 的处置必须分清两件事，而这两件事在请求上长得一模一样：
///
/// - **access 过期、refresh 还活着**：这是续期前的**正常**中间态。那枚会话 cookie
///   必须留在浏览器手里 —— 客户端拿到 401 的下一步就是拿它去 `/api/panel/refresh`
///   换新令牌。此时清掉等于把 30 天的静默续期砍成每 2 小时一次强制重登
///   （access 寿命就是 `ACCESS_TTL`，而同一枚 cookie 里装着 30 天的 refresh）。
/// - **整条会话链已经没了**：这才是该清 cookie 的情形 —— 不清的话那枚死凭据每次
///   开页都被带回来重新判一次无效（生产读数：同一把对不上任何登录的指纹两天里
///   出现 51 次）。
///
/// 判据与 `rotate_session` 的「活候选」一致：候选里只要有一把命中未轮换且未过期的
/// 记录，就说明轮换走得通，cookie 不能清。
pub fn has_live_refresh(headers: &HeaderMap) -> bool {
    let hashes: Vec<String> = refresh_candidates(headers)
        .iter()
        .map(|token| sha256_hex(token))
        .collect();
    if hashes.is_empty() {
        return false;
    }
    let now = now_ms();
    let mut table = match refresh_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.retain(|record| record.expires_at > now);
    hashes.iter().any(|hash| {
        table
            .iter()
            .any(|record| &record.hash == hash && !record.rotated)
    })
}

/// 登出：按 refresh 候选找到会话链，整链撤销 + 清掉请求里能定位到的 access。
pub fn revoke_session(headers: &HeaderMap) {
    let hashes: Vec<String> = refresh_candidates(headers)
        .iter()
        .map(|token| sha256_hex(token))
        .collect();
    let presented_access = access_candidates(headers);
    let target_session;
    {
        let mut table = match refresh_tokens().lock() {
            Ok(table) => table,
            Err(poisoned) => poisoned.into_inner(),
        };
        target_session = hashes
            .iter()
            .find_map(|hash| {
                table
                    .iter()
                    .find(|record| &record.hash == hash)
                    .map(|record| record.session.clone())
            });
        if let Some(session) = &target_session {
            table.retain(|record| record.session != *session);
            persist_refresh_tokens(&mut table);
        }
    }
    // access 一并作废：带 token 的按值删（候选里的每一把都删，包括陈旧那把 ——
    // 登出时没人希望罐里的旧令牌还活着），能定位会话链的按链删
    let mut table = match access_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    for token in presented_access {
        table.remove(&sha256_hex(&token));
    }
    if let Some(session) = &target_session {
        table.retain(|_, (session_of, _)| session_of != session);
    }
    persist_access_tokens(&mut table);
}

/// 把某条会话链签发过的所有 access token 作废（重放检测用）。
fn revoke_access_of_session(session: &str) {
    let mut table = match access_tokens().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.retain(|_, (session_of, _)| session_of != session);
    persist_access_tokens(&mut table);
}

/// `Cookie` 头里某个名字的**全部**取值（按上行顺序）。
///
/// 这里**不提供**「只取第一条」的那个体态：本次事故（见 `access_candidates`）的
/// 成因正是「同名 cookie 有两条上行时只看第一条」。留一个 `Option<String>` 版本
/// 等于给下一个人留同一条错路 —— 要判存在性用 `.first()`，要判凭据就把全部
/// 候选交给 `session_valid` / `rotate_session` 那一层。
///
/// 同名两条是**实测会出现**的形态，不是理论：中转拼头、或同一主机不同端口/路径
/// 各存了一份（cookie 按主机算不分端口，fnOS :5666 中转与 :3065 直连共用一份罐）。
pub fn cookie_values(headers: &HeaderMap, name: &str) -> Vec<String> {
    let Some(header) = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok()) else {
        return Vec::new();
    };
    cookie_values_from_str(header, name)
}

/// `Cookie` 头里出现过的**名字**（排序去重，**只给名字、不取值**）。
///
/// 为什么要这份名单：面板「登录成功却被弹回」这类现场，只看得到「cookie 有 / 无」
/// 是不够的 —— 中转入口（fnOS docker 管理页这类）常见的毛病是把多条 `Set-Cookie`
/// 合并或只透传一条，于是浏览器带回来的往往是**探针** cookie 或**上一轮**的会话
/// cookie：同样是"cookie=有"，但对不上这一把会话。名字列表能一次把三种情况分开
/// （没送到 / 送到但不是这把 / 两条都在却各自过期），值本身是凭据，进日志等于把
/// 令牌抄到磁盘上，所以这里刻意只出名字。
pub fn cookie_names(headers: &HeaderMap) -> Vec<String> {
    let Some(header) = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = header
        .split(';')
        .filter_map(|part| part.split('=').next().map(str::trim))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// 令牌的**短指纹**（sha256 前 8 位十六进制）—— 只为把两行日志对上，不还原令牌。
///
/// 日志里绝不能出现令牌本身（那等于把凭据抄进 SQLite 与备份文件）。但排查
/// 「登录明明成功、下一个请求却仍被判无效」时必须能回答一个问题：带回的那把
/// access cookie 是**这次发的**还是**上一轮残留的** —— 两者在"cookie=有"里
/// 长得一模一样。8 位十六进制（4×10⁸ 空间）足以区分同机先后两把，又不足以反推。
pub fn token_fingerprint(value: &str) -> String {
    sha256_hex(value).chars().take(8).collect()
}

/// 本次登录下发的 access 令牌指纹（登录日志与后续请求日志对得上用）。
pub fn access_fingerprint(session: &IssuedSession) -> String {
    token_fingerprint(&session.access_token)
}

fn cookie_values_from_str(header: &str, name: &str) -> Vec<String> {
    header
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            let value = part.strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().to_string())
        })
        // 只收**真的有值**的那几条：中转把 Set-Cookie 拼歪时常见 `name=`（空值）
        // 留在罐里，它不是凭据，让它进候选只会多一次注定落空的查表。
        .filter(|value: &String| !value.is_empty())
        .collect()
}

// ── 登录失败锁定（按来源 IP）────────────────────────────────

struct Attempt {
    failures: u32,
    locked_until: Option<Instant>,
}

static ATTEMPTS: OnceLock<Mutex<HashMap<IpAddr, Attempt>>> = OnceLock::new();

fn attempts() -> &'static Mutex<HashMap<IpAddr, Attempt>> {
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 该来源当前是否处于登录锁定中。
pub fn login_locked(source: IpAddr) -> bool {
    let table = match attempts().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table
        .get(&source)
        .and_then(|attempt| attempt.locked_until)
        .is_some_and(|until| Instant::now() < until)
}

/// 登录失败：累计；达到阈值就锁定一段时间。
pub fn record_login_failure(source: IpAddr) {
    let mut table = match attempts().lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    let attempt = table
        .entry(source)
        .or_insert(Attempt { failures: 0, locked_until: None });
    attempt.failures += 1;
    if attempt.failures >= LOCKOUT_THRESHOLD {
        attempt.locked_until = Some(Instant::now() + LOCKOUT_DURATION);
        attempt.failures = 0;
    }
}

/// 登录成功：清空该来源的失败记录。
pub fn clear_login_failures(source: IpAddr) {
    match attempts().lock() {
        Ok(mut table) => {
            table.remove(&source);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(&source);
        }
    };
}

// ── /v1 fail-closed（headless 未配 Key 时拒绝转发，见 bin）────

static V1_FAIL_CLOSED: AtomicBool = AtomicBool::new(false);

/// headless 形态标记：未注册管理员时 `/api/*`（除注册/状态两个认证边界
/// 端点）一律拒绝 —— 否则面板前端拿到 200 会直接进主界面，把「部署完
/// 必须先注册」变成可绕过的一步。桌面壳不设置它（桌面免鉴权语义不变）。
static PANEL_GATE: AtomicBool = AtomicBool::new(false);

/// headless 形态是否处于「未注册 /api/* 全拒」闸门下。
pub fn panel_gate() -> bool {
    PANEL_GATE.load(Ordering::Relaxed)
}

/// headless 启动时打开（面板认证启用后即形同虚设，保留开关是为了
/// 语义清晰：两态各自独立判断，不隐式推导）。
pub fn set_panel_gate(on: bool) {
    PANEL_GATE.store(on, Ordering::Relaxed);
}

/// `/v1/*` 是否处于 fail-closed（未配置任何 Key 的 headless 形态）。
pub fn v1_fail_closed() -> bool {
    V1_FAIL_CLOSED.load(Ordering::Relaxed)
}

/// headless 启动时按「有没有 Key」决定是否进入 fail-closed。
pub fn set_v1_fail_closed(on: bool) {
    V1_FAIL_CLOSED.store(on, Ordering::Relaxed);
}

// 注意：`panelAdmin` / `panelTokens` / `panelAccessSessions` 三个 kv 键已登记进
// `db::schema::RESERVED_KV_KEYS`（配置写侧据它排除）—— 新增键时必须两处
// 同步，否则用户改一次配置就会把管理员与令牌静默删掉。

#[cfg(test)]
mod credential_candidate_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn header_name(value: &str) -> HeaderName {
        HeaderName::from_bytes(value.as_bytes()).expect("测试里的头名必须是合法 token")
    }

    /// 组装上行请求头：`cookie` 原文（空串 = 不带 Cookie 头）+ 任意个
    /// `(头名, 值)`（同名可重复，走 append，因为被测的正是「同一名字两条上行」
    /// 这种中转产物）。
    fn request(cookie: &str, pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if !cookie.is_empty() {
            headers.insert(
                axum::http::header::COOKIE,
                HeaderValue::from_str(cookie).unwrap(),
            );
        }
        for (name, value) in pairs {
            headers.append(header_name(name), HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    /// 会话 cookie 的上行文本（一枚装两段，与 `issue_cookie` 的形状同源）。
    fn session_cookie(session: &IssuedSession) -> String {
        format!(
            "{ACCESS_COOKIE}={}.{}",
            session.access_token, session.refresh_token
        )
    }

    /// 只带 refresh 段的会话 cookie（access 那一段故意是废值）——
    /// 让轮换类用例能只喂 refresh 而不顺带把 access 也判过。
    fn refresh_only_cookie(token: &str) -> String {
        format!("{ACCESS_COOKIE}=not-a-live-access.{token}")
    }

    /// 候选顺序：同名 cookie 的每个值 → `x-panel-refresh` → Bearer，去重。
    ///
    /// cookie 仍在最前（直连环境的判定与从前同序）；改动只发生在「第一把无效时
    /// 不许就此收摊」。cookie 侧取的是**一枚会话 cookie 的第二段**（refresh）。
    #[test]
    fn candidates_are_cookie_values_first_then_headers_and_deduplicated() {
        let headers = request(
            "agent2api-panel=acc-a.rt-a; x=1; agent2api-panel=acc-b.rt-b",
            &[("x-panel-refresh", "from-header"), ("authorization", "Bearer from-bearer")],
        );
        assert_eq!(
            vec![
                "rt-a".to_string(),
                "rt-b".to_string(),
                "from-header".to_string(),
                "from-bearer".to_string(),
            ],
            refresh_candidates(&headers)
        );
        // 同一条 cookie 的第一段进 access 候选、第二段进 refresh 候选，不串门
        let halves = request("agent2api-panel=acc-a.rt-a", &[]);
        assert_eq!(vec!["acc-a".to_string()], access_candidates(&halves));

        // 同一把既在 cookie 又在头里：只试一次
        let once = request("agent2api-panel=acc.same", &[("x-panel-refresh", "same")]);
        assert_eq!(vec!["same".to_string()], refresh_candidates(&once));

        // 空值不进候选（`name=` 是拼歪的 Set-Cookie 留下的碎片，不是凭据）
        let blanks = request("agent2api-panel=;", &[("x-panel-refresh", "   ")]);
        assert!(refresh_candidates(&blanks).is_empty());
        // 半个配对也是碎片（中转把响应拼歪之后就是这种形状）：两侧都不进候选
        for broken in ["agent2api-panel=.", "agent2api-panel=acc.", "agent2api-panel=.rt"] {
            let headers = request(broken, &[]);
            assert!(access_candidates(&headers).is_empty(), "{broken} 不该贡献 access 候选");
            assert!(refresh_candidates(&headers).is_empty(), "{broken} 不该贡献 refresh 候选");
        }
    }

    /// access 侧同一条序（cookie → x-panel-token → Bearer）。
    ///
    /// 前两把写成"只有一段"的形状是**有意的**：升级前那版罐子里留下的裸 access
    /// 值也要能被读出来（`split_session_cookie` 的那条回退），否则升级会把还活着
    /// 的会话无谓地踢掉一次。合成一枚之后的正常形状见下一条用例。
    #[test]
    fn access_candidates_follow_the_same_order() {
        let headers = request(
            "agent2api-panel=cookie-one; agent2api-panel=cookie-two",
            &[("x-panel-token", "header-one"), ("authorization", "Bearer bearer-one")],
        );
        assert_eq!(
            vec![
                "cookie-one".to_string(),
                "cookie-two".to_string(),
                "header-one".to_string(),
                "bearer-one".to_string(),
            ],
            access_candidates(&headers)
        );
    }

    /// 一枚装两段的正常形状：第一段是 access 候选，且这条 cookie 同时也贡献 refresh。
    #[test]
    fn one_cookie_feeds_both_sides() {
        let session = IssuedSession::new_session();
        let text = session_cookie(&session);
        // 与签发侧同源：会话 cookie 的文本必须就是 `issue_cookie` 造出来的那条
        assert_eq!(text, session.session_cookie.split(';').next().unwrap().to_string());
        assert_eq!(vec![session.access_token.clone()], access_candidates(&request(&text, &[])));
        assert_eq!(vec![session.refresh_token.clone()], refresh_candidates(&request(&text, &[])));
        assert!(session_valid(&request(&text, &[])), "一枚 cookie 就该把会话撑起来");
    }

    /// 升级过渡：罐里还留着上一版那枚独立 refresh cookie（`agent2api-panel-rt`）时，
    /// 它必须仍被当 refresh 候选读出来 —— 否则升级那一刻 access 表是新键（空），
    /// 唯一能救回会话的就只有它，不读等于**全员被迫重登一次**。
    ///
    /// 两条断言分别钉住两侧：能被读出来（旧值不丢），以及**不抢跑**（会话 cookie
    /// 在场时新值排在前面）。
    #[test]
    fn a_legacy_refresh_cookie_still_feeds_the_rotation() {
        let session = IssuedSession::new_session();
        // 只有旧名：仍要认得出来
        let legacy_only = request(
            &format!("{LEGACY_REFRESH_COOKIE}={}", session.refresh_token),
            &[],
        );
        assert_eq!(
            vec![session.refresh_token.clone()],
            refresh_candidates(&legacy_only),
            "旧名的 refresh 必须仍进候选（升级期不把人踢下线）"
        );
        assert!(
            rotate_session(&legacy_only).is_some(),
            "带着旧 cookie 的浏览器应当能静默换到新令牌"
        );

        // 旧名 + 新格式同时在罐里：新的排在前面，旧值不抢跑
        let both = request(
            &format!(
                "{}=acc.{rt}; {LEGACY_REFRESH_COOKIE}=legacy-rt",
                ACCESS_COOKIE,
                rt = session.refresh_token
            ),
            &[],
        );
        assert_eq!(
            vec![session.refresh_token.clone(), "legacy-rt".to_string()],
            refresh_candidates(&both),
            "会话 cookie 的 refresh 段要排在旧名之前"
        );
    }

    /// 本次事故的正对照（2026-09-29，fnOS :5666 中转）：罐里盖着一把**陈年**同名
    /// cookie、头里带着刚拿到的令牌 —— 旧实现在 cookie 那一步就短路了，于是
    /// 「登录成功 → 下一个请求 401 → 弹回登录页」死循环。现在每一把候选都要试。
    #[test]
    fn a_stale_cookie_cannot_shadow_the_header_token() {
        let session = IssuedSession::new_session();
        let stale = random_hex(32);

        let with_header = request(
            &format!("{ACCESS_COOKIE}={stale}"),
            &[("x-panel-token", &session.access_token)],
        );
        assert!(
            session_valid(&with_header),
            "陈 cookie 不许把有效的头令牌挡在门外"
        );

        // 同一条会话、同一份 cookie，只是**不带那个头** → 仍然无效。
        // 这一条控制项是上一条断言的全部意义：少了它，「上面变 true」既可能是
        // 「试了头」也可能是「判定被放宽成无条件放行」。
        let cookie_only = request(&format!("{ACCESS_COOKIE}={stale}"), &[]);
        assert!(
            !session_valid(&cookie_only),
            "只带陈 cookie 时必须仍然判无效（否则上一条断言什么都没证明）"
        );
    }

    /// 同名 cookie 重复上行（浏览器把不同 Path 的两份一起带上来）时，每一把
    /// 都会被试 —— 有效那把排在后面也一样。
    #[test]
    fn every_repeated_cookie_value_gets_tried() {
        let session = IssuedSession::new_session();
        let stale = random_hex(32);

        let stale_first = request(
            &format!("{ACCESS_COOKIE}={stale}; {ACCESS_COOKIE}={}", session.access_token),
            &[],
        );
        assert!(session_valid(&stale_first), "第二把同名 cookie 也必须试到");

        let live_first = request(
            &format!("{ACCESS_COOKIE}={}; {ACCESS_COOKIE}={stale}", session.access_token),
            &[],
        );
        assert!(session_valid(&live_first));

        // 两把都是陈的 → 无效（同上，给上一条做对照）
        let both_stale = request(
            &format!("{ACCESS_COOKIE}={stale}; {ACCESS_COOKIE}={}", random_hex(32)),
            &[],
        );
        assert!(!session_valid(&both_stale));
    }

    /// Bearer 通道同样要兜得住：连自定义请求头都被剥的中转只放标准头过去。
    #[test]
    fn the_bearer_channel_is_a_candidate_too() {
        let session = IssuedSession::new_session();
        let headers = request(
            &format!("{ACCESS_COOKIE}={}", random_hex(32)),
            &[("authorization", &format!("Bearer {}", session.access_token))],
        );
        assert!(session_valid(&headers));
    }

    /// 轮换：cookie 里那把**已轮换**的陈 refresh 排在前面时，要跳到头里那把活的，
    /// 不许把活链当成重放作废掉。
    #[test]
    fn rotation_skips_a_rotated_stale_refresh_to_the_live_one() {
        let first = IssuedSession::new_session();
        let second = rotate_session(&request(&refresh_only_cookie(&first.refresh_token), &[]))
            .expect("首次轮换应成功");
        assert!(
            session_valid(&request(&session_cookie(&second), &[])),
            "轮换签发的 access 应进会话表（否则后面的断言都在真空里跑）"
        );

        // 现在 first.refresh 是「已轮换」记录，second.refresh 是活链。
        // 罐里若还留着 first 那把（同名残本）并排在其前，旧实现会命中它、
        // 判重放、整链作废 → 返回 None → 用户被迫重登。
        let mixed = request(
            &refresh_only_cookie(&first.refresh_token),
            &[("x-panel-refresh", &second.refresh_token)],
        );
        let third = rotate_session(&mixed).expect("陈旧那把不该挡住活链的轮换");

        // 链仍然连着：新签发的 access 有效、新 refresh 还能再轮换一次
        assert!(session_valid(&request(&session_cookie(&third), &[])));
        assert!(rotate_session(&request(&session_cookie(&third), &[])).is_some());
    }

    /// 重放检测没被削弱：只拿**已轮换**的旧令牌来换（没有一把活的在场），
    /// 仍是整条会话链作废。
    #[test]
    fn a_replayed_refresh_alone_still_kills_its_chain() {
        let first = IssuedSession::new_session();
        let second = rotate_session(&request(&refresh_only_cookie(&first.refresh_token), &[]))
            .expect("首次轮换应成功");

        // 旧令牌单独上行 → 判重放，None
        let replayed = rotate_session(&request(&refresh_only_cookie(&first.refresh_token), &[]));
        assert!(replayed.is_none(), "已轮换令牌单独来换必须判重放");

        // 同链的**活**令牌也应随之作废（整链撤销）—— 这一条才是「整链」二字的意思
        assert!(
            rotate_session(&request(&session_cookie(&second), &[])).is_none(),
            "重放检测应作废整条会话链"
        );
        assert!(
            !session_valid(&request(&session_cookie(&second), &[])),
            "该链签发过的 access 要一并作废"
        );
    }

    /// 登出：候选里任一把命中就整链撤销，且**每一把**上行的 access 都按值删。
    #[test]
    fn logout_revokes_through_the_stale_cookie_and_the_header_together() {
        let session = IssuedSession::new_session();
        let foreign_access = random_hex(32);
        let headers = request(
            &session_cookie(&session),
            &[("x-panel-token", &session.access_token)],
        );
        assert!(session_valid(&headers));
        revoke_session(&headers);

        // 链没了：refresh 换不出新令牌，access 也不再有效
        assert!(rotate_session(&request(&session_cookie(&session), &[])).is_none());
        assert!(!session_valid(&request(
            &format!("{ACCESS_COOKIE}={}", session.access_token),
            &[]
        )));
        // 对照：没被登出的另一把 access 不受牵连（撤的是链，不是全表）
        let other = IssuedSession::new_session();
        assert!(session_valid(&request(
            &format!("{ACCESS_COOKIE}={}", other.access_token),
            &[]
        )));
        assert!(!session_valid(&request(
            &format!("{ACCESS_COOKIE}={foreign_access}"),
            &[]
        )));
    }
}

#[cfg(test)]
mod session_lifecycle_tests {
    //! 面板会话的两条生命线：**落得回**（重启不把人踢下线）与**清得掉**
    //! （失效的 cookie 不会每次开页都被带回来撞 401）。
    //!
    //! 两条都是生产读数的直接对应物：access 表原先只在内存，一次容器重启就把
    //! 所有已登录浏览器打成无效，而 401 又不清 cookie ⇒ 同一把对不上任何登录的
    //! 指纹两天里出现 51 次。判据都写成「旧实现必红」的形状。

    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    /// 上行 Cookie 头（值是整枚会话 cookie 的内容）。
    fn cookie_header(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!("{ACCESS_COOKIE}={value}")).unwrap(),
        );
        headers
    }

    /// 一枚会话 cookie 的值（与 `issue_cookie` 同形）—— 用例按它拼上行文本。
    fn cookie_value(session: &IssuedSession) -> String {
        format!("{}.{}", session.access_token, session.refresh_token)
    }

    /// 一条 Set-Cookie 里的 Path 属性（没有就 None）—— 签发与清除的比对点。
    fn path_of(cookie: &str) -> Option<&str> {
        cookie
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("Path=").map(str::trim))
    }

    /// 签发的会话载回后仍然有效（= 重启不再把人踢下线）。
    ///
    /// 这条钉的是**键的一致性**：表从「裸令牌为键」改成「sha256 为键」时，只要
    /// 签发侧与查找侧有一边忘了换，这里就红（而 `session_valid` 自己看不出 ——
    /// 它只是查不到，症状与「令牌真的过期」一模一样）。
    #[test]
    fn an_issued_session_survives_a_reload() {
        let issued = IssuedSession::new_session();
        let table = access_tokens().lock().unwrap().clone();
        let dumped = serde_json::to_string(&access_records(&table, now_ms())).unwrap();
        let reloaded =
            access_table_from(serde_json::from_str::<Vec<AccessRecord>>(&dumped).unwrap(), now_ms());
        assert!(
            reloaded.contains_key(&sha256_hex(&issued.access_token)),
            "载回的表必须按 sha256 认得这把令牌"
        );
        // 内存表换载回的那份后，判定照旧为真（这里不写全局表，避免与并发用例互踩）
        let hash = sha256_hex(&issued.access_token);
        assert!(reloaded.get(&hash).is_some_and(|(_, expiry)| *expiry > now_ms()));
        assert!(session_valid(&cookie_header(&cookie_value(&issued))));
    }

    /// 库里那份是两天前的话，过期条目不能复活（墙钟判据，不是 `Instant`）。
    #[test]
    fn expired_records_do_not_come_back() {
        let expired = vec![AccessRecord {
            hash: sha256_hex("a-dead-token"),
            session: "s".to_string(),
            expires_at: now_ms() - 1,
        }];
        assert!(access_table_from(expired, now_ms()).is_empty(), "过期条目不该被载入");
    }

    /// 落库的只有哈希：整库导出/备份走同一张 kv 表，裸令牌进去等于把可用凭据
    /// 写成明文文件。
    #[test]
    fn stored_records_never_carry_a_raw_token() {
        let issued = IssuedSession::new_session();
        let table = access_tokens().lock().unwrap().clone();
        let dumped = serde_json::to_string(&access_records(&table, now_ms())).unwrap();
        assert!(dumped.contains(&sha256_hex(&issued.access_token)));
        assert!(
            !dumped.contains(&issued.access_token),
            "落库内容里不能出现裸令牌（那样备份文件就等于凭据文件）"
        );
    }

    /// **一条** `Set-Cookie` 装下整条会话，清除也落在同一条上。
    ///
    /// 三个判据都是症状面的（合并型中转的现场教训）：
    ///   · 签发只有**一枚** cookie —— 两条 `Set-Cookie` 会被中转按逗号拼成一条，
    ///     属性降级成名为 `path|domain|max-age|expires` 的 cookie，新会话存不进罐子
    ///     （经中转开面板每次都要重登，生产一手读数：同一把僵尸指纹两天出现 63 次）；
    ///   · Path 与清除头逐字一致 —— 浏览器删 cookie 要名字与 Path 同时命中，
    ///     从前 refresh 签在 `/api/panel` 而清除写 `/`，那枚从来没被删掉过；
    ///   · 一枚里两段都在（access 与 refresh），否则轮换链跟不到人。
    /// 旧实现这三条里至少红两条（两条 Set-Cookie、refresh 的 Path 不相等）。
    #[test]
    fn one_cookie_carries_the_whole_session_and_clearing_targets_it() {
        let issued = IssuedSession::new_session();
        // 一条：名字只有一个，属性段不再夹第二个 cookie
        let (pair, attributes) = issued.session_cookie.split_once("; ").expect("至少带 Path 属性");
        assert!(
            pair.contains('.'),
            "会话 cookie 必须是 access.refresh 两段的形状，实得 {pair}"
        );
        assert!(
            pair.starts_with(&format!("{ACCESS_COOKIE}=")),
            "cookie 名要还是那一个，实得 {pair}"
        );
        assert!(
            !issued.session_cookie.contains(','),
            "一条 Set-Cookie 里不该有逗号 —— 逗号正是合并型中转拼出来的形状"
        );
        assert!(attributes.contains("Path=/"));
        assert!(attributes.contains("HttpOnly"));
        assert!(attributes.contains("SameSite=Lax"));
        assert!(
            attributes.contains(&format!("Max-Age={}", REFRESH_TTL.as_secs())),
            "寿命取 refresh：access 的过期由服务端会话表判，cookie 只是载体"
        );
        assert_eq!(
            path_of(&clear_cookie()),
            path_of(&issued.session_cookie),
            "清除 Path 必须与签发一致"
        );
        assert_eq!(path_of(&issued.session_cookie), Some("/"));
        assert!(clear_cookie().contains(&format!("{ACCESS_COOKIE}=;")));
        assert!(clear_cookie().contains("Max-Age=0"));
        // 两段各归各位：第一段撑 access，第二段撑轮换
        assert!(session_valid(&cookie_header(&cookie_value(&issued))));
        assert!(rotate_session(&cookie_header(&cookie_value(&issued))).is_some());
    }
}
