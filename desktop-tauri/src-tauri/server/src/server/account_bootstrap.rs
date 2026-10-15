//! 账号侧的**启动期一次性迁移**：唯一入口，两处调用。
//!
//! 为什么单独一个文件（而不是留在 `server/mod.rs` 里）
//! ────────────────────────────────────────────────────
//! 两件事凑在一起：`server/mod.rs` 已经超过本项目的单文件行数约定（800 行），
//! 而这一组迁移的**调用时机**（启动时跳过、升级后补做）恰恰是本次改动最容易
//! 读错的一处，值得一个能被直接找到的位置。搬出来之后 `mod.rs` 里只剩一句
//! `if !upgrade_pending { account_bootstrap::run(&store); }`，读它的人会顺着
//! 那个名字找到这里，而不是在几百行启动编排里错过那段论证。
//!
//! ── 这一组迁移为什么不能无条件跑（本文件最重要的论证）─────────
//! 它们全都建立在「库里那份账号列表就是当前真相」这个前提上，判据也全是
//! 「列表空不空 / 有没有某家的账号」。而**旧 JSON/JSONL 数据还没导入时这个
//! 前提不成立**：库里的 `accounts` 表是空的（真账号还在 `accounts.json` 里
//! 没搬进来），于是：
//!   1. [`AccountStore::import_legacy_session`]（旧版单账号 `auth.json`）会因为
//!      「表空」而**抢先导入**一条账号，把 `db::migrate::import_accounts` 的
//!      幂等闸门（**表非空即跳过**）**永久关上** —— 用户之后点「升级」，真正的
//!      `accounts.json` 再也进不来，界面上只剩那一条旧登录态。
//!      这是本次改动最危险的一条路径，所以跳过必须发生在**这些导入器之前**，
//!      而不是只跳过迁移框架本身。
//!   2. 三家旧项目导入（raccoon / catpaw / autoclaw）的触发条件同样是「本家
//!      账号列表为空」，会先占住表、把标记键写掉。
//!   3. [`AccountStore::migrate_startup`] 会在空列表上写下
//!      `priorityScope: "global"`（它判「作用域不是 global」就落这个键，
//!      空列表也不例外）—— 而 `import_accounts` 要靠**这个键不存在**来决定
//!      「按旧版转发顺序整一次队」。键被提前写死，那轮整队就再也不会发生，
//!      用户的转发顺序会在升级后突变（正是那个迁移要防的事）。
//!
//! ── 于是它由两处调用，各对应「前提重新成立」的一个时刻 ──────────
//!   - `ServerState::bootstrap`：**没有**待迁移旧文件时调一次（正常启动路径，
//!     与改造前的行为逐字一致）；
//!   - `POST /api/upgrade/run`：用户点完升级、旧数据已经进库之后**补调一次**
//!     （见 `api::upgrade_api`）。不补这一下，用户本次运行看到的就是一个
//!     没整过队、没补过 provider、也没导入过三家旧数据的账号列表 ——
//!     要等到下次重启才恢复，而那时他早已把界面上的空列表当成「升级把账号
//!     弄丢了」。
//! 各步骤本身幂等（`migrate_startup` 无改动时不写库、三家导入各看自己的标记
//! 键与自家账号列表），所以两处都调不会做两遍活。
//!
//! ── 顺序（有一处真实依赖，别随手调）───────────────────────────
//! 与改造前的启动顺序逐字一致，其中最后几条有依赖：
//!   `auth.json` → `migrate_startup`（整队/拆池/拆家）→ 历史账号凭证加密
//!   → 历史网关 Key 加密 → 顶层 `apiKey` 原地加密 → `migrate_cline_split`
//!   → 网关 Key 白名单补齐 → raccoon → catpaw → autoclaw
//!   - 三条「形态加密」（账号凭证 / 网关 Key 数组 / 网关 Key 旧字段）紧随
//!     `migrate_startup` 之后、且排在彼此相邻：第一条收尾它没碰到的历史明文行，
//!     后两条只碰 kv 里的 `apiKeys` 与顶层 `apiKey`，互不依赖、也都不依赖后面的
//!     语义迁移（写入侧从本轮改造起本来就是密文）。
//!   - `migrate_cline_split` 必须排在 `migrate_startup` **之后**：后者会把
//!     Cline 的账号 id 拆成 `cline-free` / `cline-pass`，前者改的是规则键
//!     （`modelRules` 里按 provider id 记账的 disabled / hidden / mappings /
//!     seeded）—— 顺序反了，规则会在「账号还是旧 id」的状态下被判一遍池，
//!     两边对不上。
//!   - 网关 Key 白名单补齐与它同一条依赖（读 `kv` 的 `apiKeys`），且**只依赖
//!     provider id 的改名**（`workbuddy` → `workbuddy` + `workbuddy-intl`），
//!     与账号数据的先后无关；排在这里是为了让「拆家带来的三处存量修补」
//!     （账号归属、模型规则、Key 白名单）在同一段里一眼看得全。
//!   - 它们还必须读**已经导入过配置**的那份快照：升级路径里的调用点已经先做过
//!     `config::reload()`（见 `api::upgrade_api`），启动路径里 `config::init`
//!     早就装好了快照（`read_raw` 会回落读旧文件），两处都成立。
//!   - 三家旧项目导入排在 `migrate_startup` 之后：那时历史账号已经归好组，
//!     导入的新账号不会影响这一轮的去重判定（导入本身按 provider 内号段追加）。

use crate::server::core::account_store::AccountStore;
use crate::server::core::model_rules;
use crate::server::logging;

/// 账号侧的启动期一次性迁移（幂等，可重复调用）。
pub(crate) fn run(store: &AccountStore) {
    import_legacy_auth_json(store);
    // 启动期数据迁移（一次读、一次写，见 store_admin::migrate_startup）：
    //   ① 历史账号补 `provider: "workbuddy"`（Agent2API 惰性迁移，§3.2）
    //   ② 优先级去重 —— 逐 provider 分组重编号，相对顺序保持不变
    //      （所以升级后实际转发顺序不变）
    //   ③ Cline 拆池改名（`provider: "cline"` + `pool` → cline-free / cline-pass）
    //   ④ WorkBuddy 拆家（`provider: "workbuddy"` + `edition: "intl"` → workbuddy-intl）
    store.migrate_startup();
    // 历史明文账号行的整列加密（见 store_admin::encrypt_legacy_accounts）：
    // `accounts.data` 从本轮改造起写的是 `enc1:` 密文，而升级前已在库里的行仍是
    // 明文 JSON（读侧 `decode_data` 兼容明文，所以那期间账号照常可用）。这一步把
    // 过渡态收敛掉 —— 之后「拷走库文件」就看不到任何凭证。
    //
    // 位置在 `migrate_startup` **之后**：那一轮会按差异重写改了字段的行（写出的
    // 已经加密），剩下没被它碰到的历史行由这一步收尾。本身幂等（判据是数据自证，
    // 没有 kv 标记）、失败不阻断启动（密钥不可用时记日志返回，下次启动重试）。
    store.encrypt_legacy_accounts();
    // 网关 Key 的历史明文加密（见 `core::api_keys::encrypt_legacy_keys`）：
    // `apiKeys` 里每把 Key 的 `key` 从本轮改造起写的是 `enc1:` 密文，而升级前
    // 已在配置里的仍是明文（读侧 `from_value` 兼容明文，所以那期间 Key 照常可用）。
    // 这一步把过渡态收敛掉 —— 之后「拷走库文件」就看不到网关口令。
    //
    // 位置与上一条**并列**（账号那一轮加密的后面）：两者互不依赖，只是同为
    // 「同一批本机凭证的形态迁移」，排在一起让这一组一眼看得全。它读
    // `config::current().raw()`，因此**必须等配置快照就位**：启动路径里
    // `config::init` 在 bootstrap 开头就已经装好（早于本函数被调用），升级路径里
    // 调用方先做过 `config::reload()`（见 `api::upgrade_api`），两条路都成立 ——
    // 与同在这段里的 `add_intl_to_workbuddy_allowlists` 是同一条依赖。
    // 本身幂等（判据是数据自证：还有没有明文 key）、失败不阻断启动（密钥不可用时
    // 记日志返回，本次 Key 照常可用，下次启动重试），与上一条同一形态。
    crate::server::core::api_keys::encrypt_legacy_keys();
    // 顶层旧字段 `apiKey` 的**原地**加密（见 `core::legacy_api_key`）：与上一条同族的
    // 第三条「形态加密」—— `apiKeys` 数组与顶层 `apiKey` 是同一把「网关口令」的两种
    // 形态，排在一起让这一组一眼看得全。依赖与失败语义跟上一条逐字相同：
    // 读 `config::current().raw()`（所以同样必须等配置快照就位，两条路都成立，
    // 见上一条的说明）、幂等（判据是数据自证：值还不是 `enc1:` 信封）、
    // 失败只记日志不阻断启动（本次 Key 照常可用，下次启动重试）。
    // 它**只换值**：字段名留在 `apiKey`、不搬进数组 —— 搬了会破坏「写侧一旦动过
    // 列表才落成 `apiKeys` 并删掉 `apiKey`」这条既有语义（完整论证在新模块的模块头）。
    crate::server::core::legacy_api_key::encrypt_legacy_api_key();
    // 模型规则的同一轮迁移（见模块头的顺序说明）：它改的是 `kv` 里的
    // `modelRules`，所以必须读**已经导入过配置**的那份快照。
    if let Some(summary) = model_rules::migrate_cline_split() {
        logging::log("[Models]", &summary);
    }
    // WorkBuddy 拆家（2026-10）的三条存量迁移：统一挂在一次性标记下。
    // 模型规则 / 网关 Key 白名单 / 按家提示词覆盖拆家前都由 `workbuddy` 一个 id
    // 覆盖两个地区，因此都要把那份配置**复制**给国际版。
    // **必须成组只跑一次**（见 `config::workbuddy_split_migrated` 的论证）：
    // 「一边有、另一边没有就补一份」的判据在拆家之后一直成立，不设标记会在每次
    // 启动把用户之后新做的国内版配置反复镜像过去。
    // 顺序：规则在前（它决定模型能不能被点名），另两条互不依赖。
    //
    // 与种子的先后无关（有意）：国际版的种子可能已经在
    // `restore_cached_catalogs` 里跑过并把大部分模型默认禁用，因此规则迁移的
    // 启停同步是「以国内版为准」而不是「只补缺失」—— 见那个函数的说明。
    if !crate::server::config::workbuddy_split_migrated() {
        let (outcome, rules_summary) = model_rules::migrate_workbuddy_split();
        let patched_keys = crate::server::core::api_keys::add_intl_to_workbuddy_allowlists();
        let prompt_summary = crate::server::config::migrate_workbuddy_split_prompt_providers();
        if let Some(summary) = rules_summary {
            logging::log("[Models]", &summary);
        }
        if !patched_keys.is_empty() {
            logging::log(
                "[Keys]",
                &format!(
                    "🔑 已为 {} 把网关 Key 补上「WorkBuddy 国际版」的可用提供商（拆家前 workbuddy 覆盖两地，限制范围保持不变）：{}",
                    patched_keys.len(),
                    patched_keys
                        .iter()
                        .map(|entry| entry.name.as_str())
                        .collect::<Vec<_>>()
                        .join("、"),
                ),
            );
        }
        if let Some(summary) = prompt_summary {
            logging::log("[Prompt]", &summary);
        }
        // 规则迁移**没落盘**时不写标记（下次启动重来一次，好过让用户的启停状态
        // 停在半迁移位置）。另两条各自独立落盘，失败也只影响自己。
        if outcome.allows_marking_done() {
            if !crate::server::config::mark_workbuddy_split_migrated() {
                logging::log(
                    "[Models]",
                    "⚠️  WorkBuddy 拆家迁移标记未能落盘：下次启动会再核对一遍（三条迁移都幂等，不会重复改动）",
                );
            }
        }
    }
    // 限额冷却键的存量修复：把按**请求名**记下的键（映射别名）清掉。
    //
    // 位置必须在这里 —— 它读 `model_rules::current()` 的映射表（要已导入配置，
    // 与 `migrate_cline_split` 同一条依赖），且要早于任何转发（否则那些孤儿键
    // 会继续在账号页上显示成「限流中」，用户以为账号还被限着）。
    // 清的是**已存在的错位记录**，读写两侧的新口径在 `routing::CooldownKeys`
    // 与 `payload::SendBody` 里，不需要额外迁移。
    let stale_keys = store.migrate_rate_limit_keys();
    if !stale_keys.is_empty() {
        logging::log(
            "[Accounts]",
            &format!(
                "🧹 已清理 {} 条错位的限流记录（旧版把映射别名当成冷却键，实际额度按上游真名记）：{}",
                stale_keys.len(),
                stale_keys.join("；"),
            ),
        );
    }
    // 小浣熊旧数据一次性导入（架构文档 §3.3，W3-T4）：
    //   `~/.raccoon-proxy/accounts.json`（旧网关多账号）+
    //   `~/.box-agent/config/auth.json`（桌面端实时登录态）→ raccoon 账号。
    // 触发条件是「raccoon 账号列表为空 **且** config 里没有 raccoonImported」，
    // 幂等且失败不阻断（两个来源各自缺失就跳过，见该方法的说明）。
    store.import_legacy_raccoon_data();
    // CatPaw 旧数据一次性导入（架构文档 §9，W5-T-d4）：
    //   `~/.meituan-catpaw/catpaw-proxy-accounts.json`（原项目多账号）+
    //   `~/.meituan-catpaw/auth.json`（桌面端实时登录态）→ catpaw 账号。
    // 触发条件是「catpaw 账号列表为空 **且** config 里没有 catpawImported」，
    // 与上面那条同构（标记键、幂等性、失败不阻断都一致）。
    store.import_legacy_catpaw_data();
    // AutoClaw 旧数据一次性导入（架构文档 §10.2 末条，W4b-T-c2）：
    //   `~/.autoclaw-proxy/accounts.json`（原项目多账号）+
    //   `%APPDATA%/AutoClaw/auth.json`（桌面端实时登录态；备来源
    //   `~/.openclaw-autoclaw/openclaw.json`）→ autoclaw 账号。
    // 触发条件是「autoclaw 账号列表为空 **且** config 里没有 autoclawImported」，
    // 与上面两条同构（标记键、幂等性、失败不阻断都一致）。
    // 三条导入**追加式共存**：各看自己那一家的账号列表与自己那个标记键，
    // 互不影响，先后顺序不改变任何结果（后一条只增不改前面的账号）。
    store.import_legacy_autoclaw_data();
}

/// 旧版单账号 `auth.json` → 一个 workbuddy 账号（仅当账号列表为空时导入一次）。
///
/// 读文件走 [`read_legacy_session`]（缺失/损坏都当没有），写入走
/// `import_legacy_session`（它自己判「表空不空」并返回导入后的公开形态）。
/// 这条的**判据与迁移框架的闸门直接冲突**（都看 `accounts` 表空不空），
/// 所以它是「为什么不能无条件跑」那一节里最危险的一条 —— 完整论证见模块头。
fn import_legacy_auth_json(store: &AccountStore) {
    let Some(legacy) = read_legacy_session() else {
        return;
    };
    let Some(account) = store.import_legacy_session(&legacy) else {
        return;
    };
    let name = account
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("旧登录态");
    logging::log("[Accounts]", &format!("✅ 旧版登录态已迁移为账号: {name}"));
}

/// 读旧版单账号 auth.json（缺失/损坏都当没有，对应 Node 版 `loadStoredSession`）。
///
/// 只认「有非空 `auth.accessToken`」这一种形态：其余（文件不在、JSON 坏了、
/// 结构不对、token 为空）一律当没有 —— 与 Node 版的容错取向一致，
/// 不因为一份读不懂的旧文件让启动路径出错。
fn read_legacy_session() -> Option<serde_json::Value> {
    let path = crate::server::core::endpoints::legacy_auth_file();
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let token = value
        .get("auth")
        .and_then(|auth| auth.get("accessToken"))
        .and_then(serde_json::Value::as_str)?;
    if token.is_empty() {
        return None;
    }
    Some(value)
}
