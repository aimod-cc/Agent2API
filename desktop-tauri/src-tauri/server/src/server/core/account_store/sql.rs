//! 账号表的**行级 SQL 访问层** —— 账号存储里唯一出现 SQL 的地方。
//!
//! ── 为什么单独一层 ──────────────────────────────────────────
//! 改造前账号数据在 `{config_dir}/accounts.json`：每次读写都是「整份 JSON 解析 →
//! 内存里改一条 → 整份重新序列化写回」。单看数据量不大，但「改一个字段要重写整份文件」
//! 意味着：改备注名要碰全部账号的凭证、一次写盘失败丢掉的是全部账号、且每次读都要
//! 把全部记录的 JSON 全部解析一遍（含 rateLimits 这类嵌套结构）。
//!
//! 换到 SQLite 之后，单条增删改只该动**涉及的那一行**。本文件就是这件事的落点：
//! 把「一行 ↔ `StoredAccount`」的编解码、以及各种按需查询集中在这里，
//! 上层（`store_*`、各 provider 的账号模块）不再自己拼 SQL。
//!
//! ── `data` 是权威副本，五列是投影 ───────────────────────────
//! 表结构（见 `server/db/schema.rs`）里 `data` 列存**整条账号记录的 JSON 原文**，
//! 这是「用户升级绝不能丢账号里任何字段」这条不变量的实现方式 —— 含未知字段、
//! 用户手工加的备注、旧版本遗留的键。`id` / `provider` / `priority` / `enabled` /
//! `added_at` 五列是从 JSON 里**复制出来**的查询投影（排序、筛选、唯一性校验用）。
//!
//! 由此推出一条硬规则：**写的时候以 `data` 为准去派生投影列**，绝不让两者漂移。
//! 本文件的写入一律走 `record.provider()` / `record.priority()` / `record.enabled()`
//! / `record.added_at()` 这几个**容错访问器**（而不是直接取 JSON 字段），于是
//! 「手工写成 `enabled: "false"` 的脏值」这类情况在列与访问器之间也是同一个答案：
//! 列里存的是**访问器的取值**，不是字段的原样搬运。读的时候反过来 —— 只认
//! `data`，五列不参与重建（`decode_data` 只看 data 一列）。
//!
//! ── `data` 现在是**密文**（本切片的整列加密）─────────────────
//! 落库的 `data` 不再是那条记录的 JSON 原文，而是 `core::crypto` 的 `enc1:` 信封
//! —— `AES-256-GCM` 加密后的 base64（nonce 随机、同一明文两次得到不同密文）。
//! 编解码收敛到本文件的一对函数：写走 [`encode_data`]（`StoredAccount` → 密文），
//! 读走 [`decode_data`]（密文 → `StoredAccount`）。**只有本文件知道 `data` 是密文**。
//!
//! 三条不变量，改这个文件时不要破：
//!   1. **内存里永远是明文**。`state.rs` 的 `StoredAccount::to_value` / `from_value`
//!      语义一字未动，各 provider 模块照常拿 `record.to_value()` 做业务判断
//!      （`Credentials::from_payload(&record.to_value())`）。加密只在「进出库」
//!      这一层发生，绝不渗透进内存形态。
//!   2. **密钥在库外**（`{config_dir}/account.key`，兜底环境变量
//!      `AGENT2API_ACCOUNT_KEY`，见 `core::crypto` 模块头）。这挡的是「库文件被
//!      单独拷走 / 备份 / 整库导出」这一类泄露 —— 拷走 `agent2api.db` 看不到凭证；
//!      挡不住拿到整个 `config_dir` 或整台机器的人（强度边界见 `core::crypto`）。
//!   3. **`decode_data` 兼容历史明文**。升级前写进库的行还是明文 JSON，迁移（见
//!      [`encrypt_legacy_rows`]）把它们就地加密；但迁移可能失败、也可能被中断，
//!      读侧因此必须「先认 `enc1:` 信封、不是就当明文 JSON 读」——否则一次迁移
//!      没跑完就等于账号全丢。
//! 加密失败（密钥文件建不出来等）**不 panic**（本文件的硬约束），但整次写入
//! **失败并上抛错误**（fail closed）：加密的意义就是「库被单独拿走时看不到凭证」，
//! 在保护最该生效的异常环境里静默落明文等于让它失效，所以 [`encode_data`] 返回
//! `Result`、调用点 `?` 上抛 —— 用户看到的是「保存失败」，而不是「保存成功但凭证
//! 是明文」。读侧仍兼容明文（上面第 3 条），所以这不影响历史行的读出。
//!
//! ── 顺序：`ORDER BY rowid` ──────────────────────────────────
//! `list_accounts` 的契约是「按**文件顺序**返回，界面自己按优先级排序」。
//! 表里没有「数组下标」这一列（也不该为它加一列：下标是文件的实现细节，
//! 不是数据本身），所以用 SQLite 的隐式 `rowid` 作为物理顺序，它与旧的
//! accounts.json 数组顺序在三类场景下逐字对应：
//!   - **迁移**：导入按数组序逐条 INSERT → rowid 升序 == 原数组序；
//!   - **新增/替换**：旧代码是 `retain(≠id)` + `push`（移到数组末尾），
//!     这里的 [`put`] 是 `DELETE` + `INSERT`（拿到新的最大 rowid）→ 同样落到末尾；
//!   - **原地修改**：旧代码在下标上改字段（位置不变），这里的
//!     [`update_in_place`] 是 `UPDATE`（rowid 不变）→ 位置同样不变。
//! 一处**有意的差异**：`promote_to_front` / `migrate_startup` 在旧实现里会把
//! 整份数组按优先级重排后落盘（文件行序随之改变），这里改成原地 UPDATE
//! （rowid 不变）、物理顺序不动。
//! **例外**：`migrate_startup` 的 Cline 拆池改名会改 `id`（主键），那几条在
//! `save_state` 里走「删旧 id + 插新 id」两个分支，于是**会**落到末尾 ——
//! 形状与旧实现「整份数组重排」不同，但同样无消费方可观察（理由见下）。
//! 上面这些差异都不会被调用方观察到 —— 两处消费方（界面、转发选路）都自己
//! 按优先级排（`accounts-view.js:144` 与 `accounts-groups.js:351` 的
//! `byPriorityOrder`、`routing::pick_account_by_priority` 的 `sort_by`），
//! 而账号数组的顺序本身**没有任何消费方**（不按位置取、不按下标索引）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件不做网络请求、不持锁（锁由调用方的 `AccountStore::guard` 保证），
//! 也绝不 panic：所有 SQL 失败都以 `rusqlite::Result` 往上传，由调用方转成
//! `AccountStoreError`（500）。

use rusqlite::{params, Connection, OptionalExtension};

use crate::server::core::account_store::priority::{grouped_queue_order, QueueEntry};
use crate::server::core::account_store::state::{AccountState, StoredAccount};
use crate::server::core::crypto::{self, SecretScope};

/// 一行 `data` 列 → 记录字段的 JSON `Value`（**密文与历史明文都能读**）。
///
/// 这是「`data` 列 ↔ 内存明形态」这条边界的读侧内核，[`decode_data`] 与
/// [`save_state`] 的差异比较都复用它。两段分支：
///   - 是密文信封（`crypto::is_envelope`，判据只有 `core::crypto` 一处）→ 走
///     `core::crypto::open` 解开信封（它已校验明文是合法 UTF-8），再当 JSON 解析；
///   - 其余 → 按**历史明文** JSON 解析（本轮改造的过渡期安全网，见模块头）。
/// 任何一步失败都返回 None：坏行跳过、不拖垮整份数据（与旧文件读取路径的口径
/// 一致，见 `decode_data`）。**注意 `None` 不只是「坏数据」，也包含「密钥不可用
/// 导致解不开」** —— 后者在 `save_state` 的删除段有专门处理，绝不能当「这行不存在」。
fn decode_value(text: &str) -> Option<serde_json::Value> {
    if crypto::is_envelope(text) {
        let plaintext = crypto::open(text, SecretScope::Accounts).ok()?;
        serde_json::from_slice(&plaintext).ok()
    } else {
        serde_json::from_str(text).ok()
    }
}

/// 一行 `data` 列 → `StoredAccount`；解不开或不是对象/缺 id 时返回 None。
///
/// 与旧文件读取路径的口径**逐字一致**：那时也是 `filter_map(from_value)`
/// 逐条过滤（坏条目跳过、不拖垮整份数据）。差异只有一处：文件整体解析失败会
/// 退化成「空列表」，而这里坏的是单行、其余行照常读出 —— 比整体丢弃更好，
/// 也不是行为倒退（同一份数据在文件形态下本来就整体读不出来）。
fn decode_data(text: &str) -> Option<StoredAccount> {
    StoredAccount::from_value(decode_value(text)?)
}

/// `StoredAccount` → 一行 `data` 列文本（**加密成 `enc1:` 信封**）。
///
/// 先 `to_value().to_string()` 得到那条记录的 JSON 原文（与改造前存进库的
/// 是**同一串**，所以「JSON 原文是权威副本、未知字段全量保留」这条不变量
/// 一字未动），再把它的 UTF-8 字节整段加密。内存形态是明文、只有落库这一下
/// 变密文，见模块头第 1 条不变量。
///
/// ── 加密失败为什么**上抛**而不是降级写明文 ────────────────────
/// 加密的全部意义是「库文件被单独拿走时看不到凭证」，而失败恰恰发生在保护最该
/// 生效的异常环境（`config_dir` 不可写、密钥文件建不出来等）。此时若静默落明文，
/// 就等于「越该保护越失效」，而用户从界面上看不出任何区别（之前那条 verbose 默认
/// 不显示）。所以**失败即让这次写入失败**（fail closed）：调用方（`store_*` →
/// API）把错误报出来，用户看到的是「保存失败」而不是「保存成功但凭证是明文」。
/// 错误用 `ToSqlConversionFailure` 承载（本仓既有手法，其 `Display` 就是那句
/// 原话，见 `db::migrate::logs::assign_log_id` 的说明）。
///
/// `save_state` 是「先比较、再写」，只有**真正要写**的行才会走到这里 —— 内容没变
/// 的行不会 encode，也就不会因为密钥问题而失败（这条行为有意保持）。
fn encode_data(record: &StoredAccount) -> rusqlite::Result<String> {
    let json = record.to_value().to_string();
    crypto::seal(json.as_bytes(), SecretScope::Accounts).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(
            format!("账号凭证加密失败（{error}）：为避免把凭证明文落库，本次保存已中止")
                .into(),
        )
    })
}

/// 全部账号，按物理顺序（见模块头「顺序」一节）。
///
/// 这是「确实需要整份数据」的路径才用的读法：列表快照、导出、启动迁移、
/// 批量操作、`pick_current` 一类的全局派生。**单条增删改不该用它** ——
/// 那些路径走 [`load_by_id`] / [`priorities_except`]。
pub(crate) fn load_all(conn: &Connection) -> rusqlite::Result<Vec<StoredAccount>> {
    let mut stmt = conn.prepare("SELECT data FROM accounts ORDER BY rowid")?;
    let mut rows = stmt.query([])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let text: String = row.get(0)?;
        if let Some(record) = decode_data(&text) {
            records.push(record);
        }
    }
    Ok(records)
}

/// 按主键取一条（不存在返回 None）。单条读写路径的第一步。
pub(crate) fn load_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<StoredAccount>> {
    let mut stmt = conn.prepare("SELECT data FROM accounts WHERE id = ?1")?;
    let mut rows = stmt.query(params![id])?;
    while let Some(row) = rows.next()? {
        let text: String = row.get(0)?;
        return Ok(decode_data(&text));
    }
    Ok(None)
}

/// 某个 provider 的全部账号（按物理顺序）。
///
/// 用投影列 `provider` 过滤：它是写入时从 `record.provider()` 派生的，
/// 而 `provider()` 对缺失/空/非字符串的字段一律兜底成默认 provider ——
/// 所以「历史账号没写 provider 字段」这种情况在列里已经是 `workbuddy`，
/// 按 `workbuddy` 查得到它，与旧代码在内存里过滤的结果一致。
pub(crate) fn load_by_provider(
    conn: &Connection,
    provider: &str,
) -> rusqlite::Result<Vec<StoredAccount>> {
    let mut stmt =
        conn.prepare("SELECT data FROM accounts WHERE provider = ?1 ORDER BY rowid")?;
    let mut rows = stmt.query(params![provider])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let text: String = row.get(0)?;
        if let Some(record) = decode_data(&text) {
            records.push(record);
        }
    }
    Ok(records)
}

/// 账号总数 —— 统计与管理接口用（如清空前的确认提示）。
pub(crate) fn count_all(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
}

/// 某个 provider 的账号数 —— 「这一家有没有账号」的判定
/// （三家旧数据导入的触发条件就是「本家账号列表为空」）。
///
/// 用投影列 COUNT 而不是读记录：这里只问「有没有 / 有几条」，
/// 不需要知道它们长什么样。
pub(crate) fn count_by_provider(conn: &Connection, provider: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM accounts WHERE provider = ?1",
        params![provider],
        |row| row.get(0),
    )
}

/// 全部账号的 id —— 旧数据导入的「同 id 跳过」判定用。
///
/// 与 `priorities_all` 同理：只取主键一列，不解析任何记录的 JSON。
/// 导入过程中新加的 id 由调用方自己往集合里补（同批内也不能重号）。
pub(crate) fn all_ids(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM accounts")?;
    let mut rows = stmt.query([])?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        values.push(row.get(0)?);
    }
    Ok(values)
}

/// 全部账号的优先级数值 —— `next_free_priority` 的入参。
///
/// 与 [`priorities_except`] 的区别只在于「要不要排除某个 id」：新增账号时
/// 要排除的那个 id 还不存在（新账号的 id 刚算出来），所以用这个；更新既有账号
/// 时要把它自己排除在外，用另一个。
pub(crate) fn priorities_all(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT priority FROM accounts")?;
    let mut rows = stmt.query([])?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        values.push(row.get(0)?);
    }
    Ok(values)
}

/// 「除某 id 之外」的全部优先级数值 —— `next_free_priority` 的入参。
///
/// 只取一列：号段计算是纯数值运算，没必要把 20 条记录的完整 JSON（含
/// `rateLimits` 这类嵌套结构）都解析出来。冲突判定另走 [`priority_holder`]。
pub(crate) fn priorities_except(conn: &Connection, id: &str) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT priority FROM accounts WHERE id <> ?1")?;
    let mut rows = stmt.query(params![id])?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        values.push(row.get(0)?);
    }
    Ok(values)
}

/// 占用某优先级的账号名（排除 `exclude_id` 自己），没有则 None。
///
/// 对应旧实现的 `find_priority_holder(&peers, priority, None)`：`peers` 是
/// 「除自己之外的全部账号」，判据是**归一后的优先级相等**。投影列 `priority`
/// 存的正是 `normalize_priority_value(record.priority())`，所以 SQL 里的
/// `priority = ?` 与内存里的判据等价，不需要把记录读出来再比一遍。
///
/// `ORDER BY rowid LIMIT 1`：多个账号占同一个号（手工编辑过的脏数据）时，
/// 取物理顺序里最靠前的那个 —— 也就是旧实现在数组里找到的第一个，
/// 于是 409 文案里出现的「占位者名字」与改造前一致。
pub(crate) fn priority_holder(
    conn: &Connection,
    priority: i64,
    exclude_id: &str,
) -> rusqlite::Result<Option<String>> {
    let text: Option<String> = conn
        .query_row(
            "SELECT data FROM accounts WHERE priority = ?1 AND id <> ?2 ORDER BY rowid LIMIT 1",
            params![priority, exclude_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(text
        .as_deref()
        .and_then(decode_data)
        .map(|record| record.name()))
}

/* ─── 队列顺序（分组模式的整队读改写）────────── */

/// 一行「队列排序投影」：分组排序需要的最小输入（见 `priority::QueueEntry`）。
pub(crate) struct QueueRow {
    pub id: String,
    pub provider: String,
    pub priority: i64,
    pub added_at: i64,
}

/// 全部账号的排序投影（id / provider / priority / added_at 四列）。
///
/// 只取四列，不解析 `data`：整队排序是纯数值 + 字符串运算，没必要把每条记录的
/// 完整 JSON（含 rateLimits 这类嵌套结构）都解析一遍 —— 与 `priorities_all` 同一取向。
/// `added_at` 用投影列 + `COALESCE(…, 0)`：它与 JSON 里的 `addedAt` 是同一份值
/// （写入时由 `record.added_at()` 派生），列缺值时只影响并列时的先后，不影响正确性。
pub(crate) fn queue_rows(conn: &Connection) -> rusqlite::Result<Vec<QueueRow>> {
    let mut stmt = conn
        .prepare("SELECT id, provider, priority, COALESCE(added_at, 0) FROM accounts")?;
    let mut rows = stmt.query([])?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        values.push(QueueRow {
            id: row.get(0)?,
            provider: row.get(1)?,
            priority: row.get(2)?,
            added_at: row.get(3)?,
        });
    }
    Ok(values)
}

/// 按 `(id, 新优先级)` 改号：**投影列与 JSON 里的 priority 一起刷**。
///
/// 两条口径：
///   · 只写真正变化的行（与 `save_state` 的差异写入同一取向）—— 一次整队里
///     常常只有一半的号码真的变了；
///   · **不动 `updatedAt`**：改号不是「账号数据变了」，界面上「更新于」回答的是
///     凭证/备注最后一次被碰的时间（`move_account` 那种一次只换两行的操作为了
///     日志留痕会动它，整队重排若也动，拖一次就会把十几条都标成「刚更新过」）。
pub(crate) fn apply_priorities(
    conn: &Connection,
    updates: &[(String, i64)],
) -> rusqlite::Result<usize> {
    let mut changed = 0usize;
    for (id, priority) in updates {
        let data: Option<String> = conn
            .query_row("SELECT data FROM accounts WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(record) = data.as_deref().and_then(decode_data) else {
            continue;
        };
        if record.priority() == *priority {
            continue;
        }
        let mut record = record;
        record.set_priority(*priority);
        update_in_place(conn, &record)?;
        changed += 1;
    }
    Ok(changed)
}

/// 新账号的号是否插进了别家的区段里（需要整理队列）。
///
/// 调用方给的号是「全局最大值 + 1」，所以判据只有一条：**除它以外队尾那一条是不是
/// 同一家**。是同一家 → 它本来就在本组末尾，无需整理；不是 → 得整理。
pub(crate) fn queue_needs_normalize(
    conn: &Connection,
    provider: &str,
    id: &str,
) -> rusqlite::Result<bool> {
    let last_provider: Option<String> = conn
        .query_row(
            "SELECT provider FROM accounts WHERE id <> ?1 ORDER BY priority DESC, rowid DESC LIMIT 1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(last_provider.as_deref(), Some(last) if last != provider))
}

/// 把队列整理成「分组连续 + 连续编号 0、1、2…」（规则见 `priority::grouped_queue_order`）。
///
/// 用于两处：新增账号之后（[`put`] 里的那一跳），以及 `AccountStore::reorder_accounts`
/// 之外的整队场景。读全量是语义要求（组序由各组队首决定），但**写**只碰真的变了号的行。
pub(crate) fn normalize_grouped_queue(conn: &Connection) -> rusqlite::Result<usize> {
    let rows = queue_rows(conn)?;
    let entries: Vec<QueueEntry<'_>> = rows
        .iter()
        .map(|row| QueueEntry {
            id: &row.id,
            provider: &row.provider,
            priority: row.priority,
            added_at: row.added_at,
        })
        .collect();
    let ordered = grouped_queue_order(&entries);
    let updates: Vec<(String, i64)> = ordered
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, index as i64))
        .collect();
    apply_priorities(conn, &updates)
}

/// 写入（upsert）一条记录，**并把它移到物理顺序的末尾**。
///
/// 语义与旧实现的 `state.accounts.retain(≠id); push(record)` 逐字对应：
/// 先删同 id 的旧行、再插新行 → 新行拿到当前最大的 rowid，落到末尾。
/// 所有「添加/更新账号」的路径都用它（workbuddy / 小浣熊 / CatPaw / AutoClaw /
/// Cline / Qoder 六家的添加与桌面端导入），"更新既有记录让它在列表里往后挪"
/// 这个既有行为因此原样保留。
///
/// 为什么不用 `INSERT OR REPLACE`：它其实也会「删旧行 + 插新行」（同样落到末尾），
/// 但那条保证埋在 SQLite 文档的 REPLACE 算法里，读代码时看不出来；显式的
/// `DELETE` + `INSERT` 把这条语义写在脸上，也省得日后有人改成 `INSERT OR IGNORE`
/// 之类的变体却以为行为不变。
///
/// ── 新增的那一条要落在**本组的末尾**（分组模式）────────────────
/// 调用方给新账号的号是「全局最大值 + 1」，那是「排在整条队列最后」。分组模式
/// 下同一家的账号在队列里必须连续（见 `priority::grouped_queue_order`），于是：
/// 新账号若不属于**当前排在最后的那一家**，它就被插进了别家的区段里 —— 界面上
/// 它显示在本组末尾，真实转发却排在末尾那一组之后，两处对不上。这里补一次整理
/// （[`normalize_grouped_queue`]）：重编号之后它正好落在本组末尾（它是本组最大
/// 号），组序不变。
///
/// 只在**真的需要**时整理（判据见 [`queue_needs_normalize`]）：给末尾那一组加
/// 账号、以及一次只写一家的导入，都命中不到这条，省掉整表重写。
///
/// 一个已知的口径变化：调用方若显式传了 `priority`（`POST /api/accounts` 的
/// `payload.priority`，历史客户端才有），整理会把那个号**改写成本组末尾**的号 ——
/// 分组模式下「插在别家区段中间」不是一个有意义的请求（界面上也没有这个入口），
/// 落在本组末尾是唯一自洽的结果。
pub(crate) fn put(conn: &Connection, record: &StoredAccount) -> rusqlite::Result<()> {
    let existed = delete(conn, record.id())?;
    insert_row(conn, record)?;
    if !existed && queue_needs_normalize(conn, &record.provider(), record.id())? {
        normalize_grouped_queue(conn)?;
    }
    Ok(())
}

/// 原地更新一条记录（**物理位置不变**）。
///
/// 与 [`put`] 的区别只在顺序：`update_account`（改备注名/优先级/启用/代理）、
/// `move_account`（交换两个账号的优先级）、限额标记、token 回写这些「就地改
/// 字段」的操作走这里 —— 旧代码在下标上改字段，数组位置本来就不动，
/// 界面上的行序（优先级序）也不该因为一次「改备注名」而跳动。
///
/// 行不存在时退化成插入（拿到末尾位置）：那是「读出来之后被并发删掉了」的
/// 竞态窗口 —— 旧实现在那个窗口里是「把整份内存状态写回文件」，效果同样是
/// 让这条记录重新出现。这里保持同一结果（而不是静默丢弃这次写入），
/// 因为调用方大多已经把「写入成功」当作前提继续往下算了。
pub(crate) fn update_in_place(conn: &Connection, record: &StoredAccount) -> rusqlite::Result<()> {
    let changed = conn.execute(
        "UPDATE accounts SET provider = ?2, priority = ?3, enabled = ?4, added_at = ?5, data = ?6
         WHERE id = ?1",
        params![
            record.id(),
            record.provider(),
            record.priority(),
            enabled_flag(record),
            record.added_at(),
            encode_data(record)?,
        ],
    )?;
    if changed == 0 {
        insert_row(conn, record)?;
    }
    Ok(())
}

/// 删除一条记录，返回是否真的删掉了（`false` = 本来就没有这一行）。
pub(crate) fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])? > 0)
}

/// 一行 INSERT（[`put`] / [`update_in_place`] 的公共落点）
fn insert_row(conn: &Connection, record: &StoredAccount) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO accounts (id, provider, priority, enabled, added_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            record.id(),
            record.provider(),
            record.priority(),
            enabled_flag(record),
            record.added_at(),
            encode_data(record)?,
        ],
    )?;
    Ok(())
}

/// `enabled` 列取值：SQLite 没有布尔类型，按惯例存 0/1。
///
/// 取的是 `record.enabled()`（`enabled !== false` 的宽松语义）而不是 JSON 字段
/// 的原样值 —— 于是 `enabled: "false"` 这种手工脏值在列里是 1（启用），
/// 与访问器给上层看到的答案一致。
fn enabled_flag(record: &StoredAccount) -> i64 {
    if record.enabled() { 1 } else { 0 }
}

/// 把一整份 `AccountState` **按差异**写入（同一个事务内）。
///
/// 用于「本来就是整份集合的操作」：`batch_update` / `batch_remove`、
/// `migrate_startup`、三家旧数据导入、以及 `account_transfer` 的导入合并
/// （它经 `save_locked` 走到这里）。这些路径的算法本身要遍历全部账号
/// （去重编号、批量匹配、快照回显），读全量是**语义要求**而不是偷懒；
/// 但**写**不该因此重写整表 —— 因此这里先取现有行的 JSON，与目标状态逐条比对：
///   - 目标里有、库里没有 → INSERT
///   - 两边都有但 JSON 不同 → UPDATE（投影列一并刷新）
///   - 库里有的、目标里没有 → DELETE
///   - JSON 相同 → **一条语句都不发**（这就是「改了一个账号不重写其余 19 个」
///     的落地方式）
///
/// 顺序只在「新增」时体现（INSERT 按 `state.accounts` 的次序，于是新行的 rowid
/// 相对顺序与旧实现的 push 顺序一致）；既有行一律 UPDATE，物理位置不动。
///
/// 为什么拿 `&mut Connection`：调用方要么已经持有可变借用（`Db::with_mut`），
/// 要么在事务里。事务由本函数自己开（`conn.transaction()`），批量的原子性
/// 因此在最内层就成立 —— 调用方不需要记得包一层。
///
/// `priorityScope` 单独写在 `kv` 表（键名约定见 `server/db/schema.rs` 模块头）：
/// 它不属于任何一条账号记录，只在 `Some` 时写入（旧文件里也是「没有这个键」
/// 表示尚未迁移），读取见 [`load_priority_scope`]。
pub(crate) fn save_state(conn: &mut Connection, state: &AccountState) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    // 现有行：id → data 的**解密后**值。比字符串更可靠：两边序列化格式若有一丝
    // 差异（空格、键序）不会误判成「变了」而白写一遍。
    //
    // 这里**必须**解密（`decode_value`）而不是直接 `serde_json::from_str`：
    //   · 直接解析密文 → 解析失败 → 得到 None → 下面走 INSERT 分支 → 主键冲突；
    //   · 改成比较密文串 → GCM 每次 nonce 随机、同一明文两次的密文不同 →
    //     永远不相等 → 每次 `save_state` 都把全表重写一遍。
    // 存解密后的 `Value` 两个坑都不踩：内容没变的行解密后与 `record.to_value()`
    // 相等（`serde_json::Value` 的比较按结构，与键序无关），于是**一条语句都不发**；
    // 内容变了的行照常 UPDATE。**已加密且未变的行因此不会被降级回明文。**
    let mut existing: Vec<(String, Option<serde_json::Value>)> = Vec::new();
    {
        let mut stmt = tx.prepare("SELECT id, data FROM accounts")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let text: String = row.get(1)?;
            existing.push((id, decode_value(&text)));
        }
    }

    // 删除：库里在、目标状态里没有的 —— 但**只删「读得出来」的行**。
    //
    // `existing` 的 id 来自主键列（总能读到），而 `state.accounts` 来自 `load_all`，
    // 那里解不开的行会被 `decode_data` 跳过。于是「密钥丢失 / 密钥不匹配 / 拷库没拷
    // `account.key`」这种场景下，全部密文行都解不开 → `state.accounts` 为**空** ——
    // 若仍按「库里在、内存里没有」删，任何一次 bulk 操作（`batch_update` /
    // `batch_remove` / 导入合并 / `migrate_startup` 有改动时）都会把**全部账号
    // DELETE 掉**。这正是我们要替旧程序避免的灾难，新版在密钥出问题时自己也会犯。
    //
    // 判据因此收紧成：**只有 `decode_value` 成功（`value.is_some()`）的行才参与
    // 删除**；`None`（解不开 / 坏数据）的行一律**不删** —— 读不出的行不删，
    // 密钥丢失时保住数据，宁可留着让人恢复也不能删。
    for (id, value) in &existing {
        if value.is_none() {
            continue;
        }
        if !state.accounts.iter().any(|record| record.id() == *id) {
            tx.execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        }
    }
    // 新增 / 更新
    for record in &state.accounts {
        let current = existing
            .iter()
            .find(|(id, _)| id == record.id())
            .map(|(_, value)| value.as_ref())
            .unwrap_or(None);
        match current {
            Some(value) if *value == record.to_value() => {}
            Some(_) => {
                tx.execute(
                    "UPDATE accounts SET provider = ?2, priority = ?3, enabled = ?4,
                     added_at = ?5, data = ?6 WHERE id = ?1",
                    params![
                        record.id(),
                        record.provider(),
                        record.priority(),
                        enabled_flag(record),
                        record.added_at(),
                        encode_data(record)?,
                    ],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO accounts (id, provider, priority, enabled, added_at, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        record.id(),
                        record.provider(),
                        record.priority(),
                        enabled_flag(record),
                        record.added_at(),
                        encode_data(record)?,
                    ],
                )?;
            }
        }
    }

    if let Some(scope) = &state.priority_scope {
        // 幂等写：值没变时 SQLite 也会重写这一行，但它只有一行、且这条路径
        // 本来就是「整份状态落盘」，不必为它再查一次
        tx.execute(
            "INSERT INTO kv (key, value) VALUES ('priorityScope', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![scope],
        )?;
    }
    tx.commit()
}

/// 读 `priorityScope`（`kv` 表；没有这个键时 None）。
///
/// 值是**裸字符串**而不是 JSON 编码的字符串：键名约定见 `server/db/schema.rs`
/// 模块头（「其它零散状态用固定名」一栏），`priorityScope` 是其中的固定名之一，
/// 存的就是 `"global"` 原文 —— 于是用 `sqlite3` 命令行直接看库时一眼能读懂。
pub(crate) fn load_priority_scope(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM kv WHERE key = 'priorityScope'",
        [],
        |row| row.get(0),
    )
    .optional()
}

/* ─── 历史明文行的整列加密（启动期一次性迁移）────────── */

/// 把 `accounts` 表里**还不是密文**的 `data` 行就地加密，返回成功加密的条数。
///
/// ── 它补的是什么 ────────────────────────────────────────────
/// 本轮改造起 `data` 写的是密文，但**升级前已经在库里的行**仍是明文 JSON
/// （见模块头第 3 条不变量）。`decode_data` 兼容明文，所以那些行照常能读 ——
/// 那是过渡期的安全网，不是最终形态：只要还有明文行，拷走库文件就带走凭证。
/// 本函数在启动时把过渡态收敛掉。
///
/// ── 幂等判据：数据自证 ──────────────────────────────────────
/// 判据是「还有没有 `data` 不以 `enc1:` 开头的行」，**不用任何 kv 标记**：
/// 加密是可重复的纯变换，迁完就没有明文行，再跑一遍在第一段就早退、连事务都不开。
/// 加标记键反而多一份「这条跑过没有」的事实来源，还会与「用户手工把某行改回
/// 明文」这类边角情形打架。
///
/// ── 为什么整批一个事务（且先在内存里把信封全算好）────────────
/// 加密到一半中断会留下「部分密文、部分明文」的中间态。这里先只读收集明文行、
/// 再把全部信封算进内存（任何一行 `seal` 失败就整批不写、直接返回 Err），最后
/// 才开事务逐行 UPDATE 一次提交 —— 于是「要么全加密、要么一行都不动」，中断后
/// 下次启动从头再来一遍（幂等）。
///
/// 失败用 `ToSqlConversionFailure` 承载一句人能读的话（本仓既有手法，见
/// `db::migrate::logs::assign_log_id` 的说明）：这个变体的 `Display` 就是那句
/// 原文，`Db::open` / `with_conn_mut` 会把它原样带进日志。**失败不阻断启动**
/// 由调用方决定（见 `store_admin::encrypt_legacy_accounts`）。
pub(crate) fn encrypt_legacy_rows(conn: &mut Connection) -> rusqlite::Result<usize> {
    // ① 只读收集「还不是密文」的行，不动库
    let mut pending: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, data FROM accounts")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let text: String = row.get(1)?;
            if !crypto::is_envelope(&text) {
                pending.push((id, text));
            }
        }
    }
    if pending.is_empty() {
        return Ok(0);
    }
    // ② 在内存里把全部信封算好：任何一行失败就整批放弃（此时库还一字未改）
    let mut updates: Vec<(String, String)> = Vec::with_capacity(pending.len());
    for (id, plaintext) in pending {
        let envelope = crypto::seal(plaintext.as_bytes(), SecretScope::Accounts)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))?;
        updates.push((id, envelope));
    }
    // ③ 一个事务写回（要么全成、要么全不成）
    let tx = conn.transaction()?;
    for (id, envelope) in &updates {
        tx.execute(
            "UPDATE accounts SET data = ?2 WHERE id = ?1",
            params![id, envelope],
        )?;
    }
    tx.commit()?;
    Ok(updates.len())
}
