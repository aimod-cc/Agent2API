//! 账号的**公开形态**与按 provider 的查询（从 store.rs 拆出，单文件行数约定）。
//!
//!   to_public_account      workbuddy 形状（对照 Node 版 toPublicAccount）
//!   public_account         按 provider **分派**到各家的形状，并注入跨家事实
//!   snapshot               /api/accounts 的列表快照（含 providers 摘要）
//!   provider_summary       provider 摘要 `{id,label,count}`
//!   accounts_for_provider  指定 provider 的启用账号（聚合目录判可用性）
//!   list_accounts          快照入口（取锁 + 读全部账号）
//!
//! ── 为什么拆出来 ──────────────────────────────────────────
//! store.rs 在账号存储改造后要承载的东西变多了（句柄、锁、连接、装载/保存的
//! 两个端点），而「怎么把一条记录渲染成给前端/给选路层的形状」是另一件事：
//! 前者的读者关心并发与存储语义，后者关心字段口径。按项目的单文件行数约定
//! 与既有的 `store_*` 拆分风格（`store_crud` / `store_batch` / `store_admin`）
//! 单独成文件，改字段时不必在存储语义里翻找。
//!
//! 这些方法仍是 `impl AccountStore` 的分块（同一个类型，跨文件不改变可见性）。

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::server::core::account_store::state::{AccountState, StoredAccount};
use crate::server::core::account_store::store::{forwards_requests, AccountStore};
use crate::server::core::account_store::store_util::{js_truthy, max_concurrent_public_for, value_or, value_or_nullish};
use crate::server::core::custom_providers;
use crate::server::core::endpoints::{resolve_edition, EditionInfo};
use crate::server::core::proxies::describe_account_proxy;

/// 这条账号所属的家，此刻**该不该出现在管理界面上**。
///
/// ── 判据（只有这一条，别加别的）─────────────────────────────
/// 内置注册表里没有这个 id（自定义家 / 手改的未知 id）→ 显示；
/// 在注册表里 → 看插件是否接管（`serves_provider`），没接管就隐藏。
///
/// ── 为什么隐藏而不是保留可见（2026-10 用户决定）───────────────
/// 各家反代的转发 / 账号 / 签到实现已整体搬进插件。没装插件时那些功能
/// **一个都不可用**，把它们留在列表里就是「看得见、点什么都没反应」——
/// 用户明确要的是「插件不在，这家就不存在」。数据不动：记录仍在库里，
/// 导出直接读库（`account_transfer::export_accounts` 不走本函数），
/// 装上插件后原样回来。
///
/// ── 为什么不能统一按「有没有执行体」判 ───────────────────────
/// 自定义提供商的转发**仍在网关里**（`core::upstream::provider_loop` 按
/// `custom-` 前缀直连，不经插件），它们没有插件可装 —— 按执行体判会把
/// 用户自己建的 API 全藏起来。所以先看「是不是内置家的 id」。
fn account_visible(provider: &str) -> bool {
    if crate::server::core::providers::kind_from_id(provider).is_none() {
        return true;
    }
    crate::server::core::plugin_host::serves_provider(provider)
}

/// 快照的两种用途：内部逻辑要全部账号，界面只要此刻能用的。
///
/// ── 为什么必须是两个入口而不是一个布尔参数 ────────────────────
/// 内部逻辑（冷却记账、凭证维护、目录刷新、余额查询）读的是**库里的数据**，
/// 插件在不在都得看得见 —— 插件卸载期间把这些读写掐掉，用户的冷却记录、
/// 刷新进度会跟着一起坏，而那是没有任何提示的数据损坏。界面相反：插件不在，
/// 这家就不该出现（用户明确要求）。
///
/// 两者用同一个 `snapshot` 保证形状一致（`providers` 摘要与 `accounts` 永远
/// 同步过滤），区别只在传进来的这个值。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Visibility {
    /// 全部账号（内部逻辑用）。
    All,
    /// 只含此刻能用的家（界面用，判据见 [`account_visible`]）。
    UserFacing,
}

impl AccountStore {
    // ─── 公开形态 ────────────────────────────────────────────

    /// 记录 → 公开形态（对照 Node 版 `toPublicAccount`，字段逐个对齐）。
    ///
    /// 注意几处 `||` / `??` 的差别：`prefixPath` 是 `??`（空串有效），
    /// `endpoint` 是 `||`（空串回落版本默认值）；`name` 在 Node 里没有兜底，
    /// 因此这里也是「缺失则不出键」而不是补空串。
    pub(crate) fn to_public_account(&self, record: &StoredAccount) -> Value {
        let edition: &'static EditionInfo = resolve_edition(record.edition().as_deref());
        let fields = record.fields();
        let mut public = Map::new();
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        // 所属提供商（Agent2API 改造新增字段）：缺失时按 workbuddy 兜底，
        // 保证界面拿到的每条账号都能直接分组，不必自己判断「没有 provider = 老数据」
        public.insert(
            "provider".to_string(),
            Value::String(record.provider()),
        );
        // name 无兜底：原样透出（含非字符串的脏值），缺失时不出现该键
        if let Some(value) = fields.get("name") {
            public.insert("name".to_string(), value.clone());
        }
        public.insert(
            "uid".to_string(),
            value_or(fields.get("uid"), Value::String(String::new())),
        );
        public.insert(
            "nickname".to_string(),
            value_or(fields.get("nickname"), Value::String(String::new())),
        );
        public.insert(
            "type".to_string(),
            value_or(fields.get("type"), Value::String("personal".to_string())),
        );
        public.insert(
            "enterpriseId".to_string(),
            value_or(fields.get("enterpriseId"), Value::String(String::new())),
        );
        public.insert(
            "enterpriseName".to_string(),
            value_or(fields.get("enterpriseName"), Value::String(String::new())),
        );
        public.insert(
            "tokenTail".to_string(),
            value_or(fields.get("tokenTail"), Value::String(String::new())),
        );
        public.insert(
            "expiresAt".to_string(),
            value_or(fields.get("expiresAt"), Value::Null),
        );
        public.insert(
            "hasRefreshToken".to_string(),
            Value::Bool(
                fields
                    .get("refreshToken")
                    .map(js_truthy)
                    .unwrap_or(false),
            ),
        );
        // 「需要重新登录」标记（issue #215）：上游注销了会话、refresh token 一并
        // 作废时由刷新链路落。界面据此把「刷新失败」显示成「需要重新登录」——
        // 两者对用户的下一步动作完全不同（前者再点一次，后者必须去登录）。
        // 键恒在（false 表示正常），前端不必判「键缺失」。
        public.insert(
            "needsRelogin".to_string(),
            Value::Bool(
                fields
                    .get("needsRelogin")
                    .map(js_truthy)
                    .unwrap_or(false),
            ),
        );
        public.insert(
            "needsReloginReason".to_string(),
            value_or(fields.get("needsReloginReason"), Value::String(String::new())),
        );
        public.insert(
            "prefixPath".to_string(),
            value_or_nullish(fields.get("prefixPath"), Value::String(String::new())),
        );
        public.insert(
            "endpoint".to_string(),
            value_or(fields.get("endpoint"), Value::String(String::new())),
        );
        public.insert("edition".to_string(), Value::String(edition.id.to_string()));
        public.insert(
            "editionLabel".to_string(),
            Value::String(edition.label.to_string()),
        );
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert(
            "proxy".to_string(),
            describe_account_proxy(Some(&value_or(fields.get("proxy"), Value::Null))),
        );
        // Node 是 `record.rateLimits || {}`：任何真值都原样透出
        // （手工写成数组/字符串时也照透，前端按对象读会得到 undefined，
        // 与 Node 的行为保持一致比「顺手修正」重要）
        public.insert(
            "rateLimits".to_string(),
            value_or(fields.get("rateLimits"), Value::Object(Map::new())),
        );
        // 本切片所有账号都视为可用（限额/可用性判定属切片 3 的转发层）
        public.insert("available".to_string(), Value::Bool(true));
        Value::Object(public)
    }

    /// 全部账号的快照（**不做展示过滤**）。
    ///
    /// accounts 按**存储顺序**返回（与 Node 版按文件顺序返回一致）—— 界面自己按
    /// 优先级排序，不要在这里改成优先级序，否则与 Node 版的行为就分叉了。
    /// 「存储顺序」在 SQLite 下是 `rowid` 升序，它与旧的 accounts.json 数组顺序
    /// 在三类场景下逐字对应（迁移按序导入、新增落到末尾、原地修改不改位置），
    /// 推演见 `sql.rs` 模块头「顺序」一节。
    ///
    /// ── 这个入口是**内部**用的，别拿去渲染界面 ──────────────────
    /// 冷却记账、目录刷新、余额查询这些逻辑必须看得见全部账号 —— 插件没装时
    /// 它们照样要能读写已有数据，否则「插件不在」会连带把冷却记录、凭证维护
    /// 一起弄坏。**界面要走 [`Self::list_visible_accounts`]**（按插件在位过滤）。
    ///
    /// ── 「当前账号」的两个字段 ──────────────────────────────
    ///   · `currentAccountId`：**全局队首**（`pick_current`：启用 + 有可用凭证 +
    ///     优先级序），与 `/api/session` 的 `session.currentAccountId` 同源。
    ///     账号页的 ★ / 「首选」读这个。
    ///   · `currentAccountIds`：provider → 该家队首账号 id（逐家派生，
    ///     `pick_current_for_provider`）。全局队列之后它只剩「这一家有没有可用账号」
    ///     的参考意义，保留是为了不破坏读它的旧客户端。
    ///
    /// `providers` 是 provider 摘要（`{id,label,count}`）：
    /// 界面用它渲染账号页的分组标题与「作用提供商」多选，因此**必须在列表接口
    /// 上就给出**，前端不必再发第二个请求。计数是各 provider 的账号**总数**
    /// （含禁用账号）—— 与分组标题显示的「N 个账号」口径一致。
    pub fn list_accounts(&self) -> Value {
        let _guard = self.guard();
        let state = self.load(&_guard);
        self.snapshot(&state, Visibility::All)
    }

    /// 界面用的账号快照：**只含此刻真的能用的那些家**（判据见 [`account_visible`]）。
    ///
    /// 所有面向用户的账号入口都应走这一条（`/api/session`、`/api/accounts`、
    /// 签到中心、成长页…）。没装插件时插件家的账号不出现 —— 用户明确要的是
    /// 「插件不在，这家就不存在」；数据仍在库里，装上插件原样回来。
    pub fn list_visible_accounts(&self) -> Value {
        let _guard = self.guard();
        let state = self.load(&_guard);
        self.snapshot(&state, Visibility::UserFacing)
    }

    /// 全部账号 proxy 指向的池条目 id 集合（去重）。
    ///
    /// 判据是 `proxy.source == "pool"`（**落库的原始形态**，`normalize_account_proxy`
    /// 的产出 —— `{source:'pool', proxyId}`；`config` 包装是 `describe_account_proxy`
    /// 之后的展示形态，落库前不存在 —— 但老版本写入的记录可能整份带着展示形态，
    /// 两种都认）。解析失败的引用（条目已被删等）同样统计得到 —— 那正是最需要
    /// 提醒「有账号还挂着它」的情况。消费方是内核的 lazy listeners
    /// （`sub_kernel::runtime` 只给被引用的订阅节点开监听器，见
    /// `proxy_pool::subscription_node_keys`），这里不做启用状态过滤：
    /// 禁用账号的引用也算「在用」—— 重新启用不该再付一次内核重启的等待。
    ///
    /// ── 这里读错形态的代价（实测踩过）──────────────────────────
    /// 曾经按 `proxy.config.source` 判 —— 那是**展示形态**，落库的原始记录上
    /// 没有这个键，引用集合恒为空：内核侧「账号引用」那一半从未生效，lazy
    /// listeners 只剩「测试钉住」在撑，统一测试口上线、pinned 退役后暴露
    /// （所有被引用的节点都拿不到监听端口，地址列全红）。
    ///
    /// ── 轮询档（rotate）怎么算 ──────────────────────────────────
    /// 它引用的条目**随时可能被取用**，所以整个候选集合都算「在用」：
    /// `all: true` 时把池里全部启用条目的 id 都返回（轮询的语义就是「池里每个
    /// 启用的出口都可能轮到」，漏掉的那部分会在取用时因「未分配端口」被跳过，
    /// 等于轮询范围被悄悄缩小）；指定档返回它列出的 id（不存在的条目查不出
    /// 节点，天然跳过）。这一条判据实现复用 `proxy_rotate::candidate_ids` ——
    /// 「哪些条目可能被用到」在取用与预建监听器两处必须是同一个答案。
    pub fn referenced_pool_ids(&self) -> std::collections::HashSet<String> {
        let _guard = self.guard();
        let state = self.load(&_guard);
        let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for record in &state.accounts {
            let proxy = record.proxy();
            // 原始形态优先；带 `config` 包装的（老版本落库的展示形态）取里层
            let config = proxy
                .get("config")
                .filter(|value| value.is_object())
                .unwrap_or(&proxy);
            if config.get("source").and_then(Value::as_str) == Some("pool") {
                if let Some(id) = config
                    .get("proxyId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    ids.insert(id.to_string());
                }
            }
            if crate::server::core::proxies::is_rotate_config(config) {
                ids.extend(crate::server::core::proxy_rotate::candidate_ids(config));
            }
        }
        ids
    }

    /// 账号 id → 该记录最近一次改动的时刻（`max(addedAt, updatedAt)`，毫秒）。
    ///
    /// 给「这条结论现在还算不算数」的时效判定用（见 `core::usage_query` 的快照
    /// 出口过滤）。判据刻意只看记录自己的时间戳：重新登录、重新导入、token 被
    /// 刷新、改设置都会把它往前推，而**凭证内容本身不必为了这个判定被读出来**
    /// —— 比指纹更省事，也不给「把 token 拼进一个新结构」多开一条口子。
    ///
    /// 代价是改动类型区分不出来（换凭证与改备注名同样是「记录变了」）：判定方
    /// 按「记录变了就作废旧结论」处理，多作废一条失败提示，比留着一条过期结论
    /// 更轻 —— 用户手点一次查询就能得到新的（见 `usage_query` 的说明）。
    pub fn account_change_times(&self) -> HashMap<String, i64> {
        let _guard = self.guard();
        let state = self.load(&_guard);
        state
            .accounts
            .iter()
            .map(|record| {
                (
                    record.id().to_string(),
                    record.added_at().max(record.updated_at()),
                )
            })
            .collect()
    }

    /// 已持锁时的列表快照（CRUD 内部要在同一次锁里连做「写入 + 取快照」）
    ///
    /// ── 插件家的账号只在插件在位时出现（2026-10 用户决定）────────────
    /// 各家反代的转发 / 账号 / 签到实现已整体搬进提供商插件。没装插件时，
    /// 那些账号**留在库里但不出现在列表里** —— 用户要的是「插件不在就等于
    /// 这家不存在」，而不是「看得见、点什么都没反应」。数据一个字节都不动：
    /// 导出（`account_transfer::export_accounts`）直接读库、不经这里；
    /// 装上插件后这些账号原样回来。
    ///
    /// ⚠️ **不能**用「有没有执行体」当统一判据：自定义提供商（`custom-` 前缀）
    /// 的转发仍在网关里（`core::upstream::provider_loop` 按前缀直连），它们
    /// 没有插件可装、也不该被隐藏。所以判据只作用于**内置注册表里的 id**：
    /// 在注册表里且插件没接管 ⇒ 隐藏；注册表外（自定义家 / 手改的未知 id）
    /// ⇒ 原样显示。
    pub(crate) fn snapshot(&self, state: &AccountState, visibility: Visibility) -> Value {
        // 过滤一次、克隆一次（两个队首派生函数都要 `&[StoredAccount]`）
        let mut visible: Vec<StoredAccount> = Vec::with_capacity(state.accounts.len());
        // 被隐藏的账号按 provider 聚合（界面用它说「哪几家等插件」）。
        // 只在 `UserFacing` 时收集 —— `All` 的语义是「全都要」，那儿的
        // 「隐藏」恒为空，算它只是白跑一遍。
        let mut hidden: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for record in &state.accounts {
            if visibility == Visibility::All || account_visible(record.provider().as_str()) {
                visible.push(record.clone());
            } else {
                *hidden.entry(record.provider()).or_insert(0) += 1;
            }
        }
        // 逐家派生队首：provider 取值的来源就是账号数据里出现过的家
        // （含手工塞进来的未知 id —— 它们也有自己的队首，不该被静默忽略）
        let mut current_ids = Map::new();
        for record in &visible {
            let provider = record.provider();
            if current_ids.contains_key(&provider) {
                continue;
            }
            let head = Self::pick_current_for_provider(&visible, &provider)
                .map(|record| Value::String(record.id().to_string()))
                .unwrap_or(Value::Null);
            current_ids.insert(provider, head);
        }
        let global_current = Self::pick_current(&visible)
            .map(|record| Value::String(record.id().to_string()))
            .unwrap_or(Value::Null);
        // 隐藏摘要独立于可见账号数量，混有自定义账号时也必须能解释缺失的部分。
        let hidden = super::hidden_accounts::summary(hidden);
        json!({
            "currentAccountId": global_current,
            "currentAccountIds": Value::Object(current_ids),
            "accounts": visible
                .iter()
                .map(|record| self.public_account(record))
                .collect::<Vec<_>>(),
            "providers": self.provider_summary(&visible),
            "hiddenAccounts": hidden,
        })
    }

    /// 记录 → 公开形态（**按 provider 分派**）。
    ///
    /// 各家的公开字段集不同：workbuddy 有 uid/nickname/edition/enterprise…，
    /// 小浣熊有 userId/tokenExpiresAt/desktop 且**没有**积分字段（架构文档 §5/§6），
    /// CatPaw 有 uid/loginName/tokenTail/desktop 且**没有**积分/签到（W5-T-d4）。
    /// 分派点放在这里，调用方（列表快照、批量目标解析、限额事件日志）
    /// 一行都不用改 —— 它们拿到的就是各自 provider 该有的形状。
    ///
    /// 兜底分支给的是 **workbuddy 形状**，这对**未知** provider id 是刻意的
    /// （与 `StoredAccount::provider()` 的容错口径一致：手改文件塞进来的陌生 id
    /// 至少还能在界面上显示出来）。AutoClaw 现在**加不了账号**
    /// （`api::accounts::add_account` 显式 400），将来它的公开形态在适配器接线
    /// 波次里补一个分支即可 —— 那条分支落地前，万一有手改的账号记录落进这里，
    /// 走 workbuddy 形状总比整条列表报错好。
    ///
    /// ── `hasCredentials` / `chatSupported`：统一注入的**跨家事实** ──────
    /// 在这里**统一注入**而不是改各家的公开形态函数：判据只有一条
    /// （`has_credentials` / 适配器的 `supports_chat`），而「谁有凭证」「谁能转发」
    /// 正是转发选路与「当前账号」派生共用的那两道闸门。前端要按模型
    /// 自行推算「这一家此刻会走谁」时（后端只给不限模型的队首），必须拿得到同一
    /// 事实，否则会出现「界面标 ★ 的账号其实转发时会因无凭证被跳过」的分歧。
    /// 放在分派点让各家形状**同字段名、同语义**，前端不必按 provider 查表。
    ///
    /// `nameCustom` / `checkinAt` / `usageQuery` / `lowBalance` / `limiters` /
    /// `maxConcurrent` 走同一条路子。`maxConcurrent` 尤其不能下放到各家：
    /// 它是**恒有的数字键**，而各家公开形态只要漏写一次，选路读到的缺键就等于
    /// 0 = 不限（上限静默失效），现场看不出任何异常。
    ///
    /// 两条出口（本函数与各家的「添加 / 更新响应」）必须形状一致，所以那些响应
    /// 也一律回 [`Self::public_account`] 的结果，而不是各自调 `to_*_public_account`
    /// —— 后者会绕过这里注入的全部字段。
    ///
    /// 纯新增字段：各家的既有字段一个不动，旧客户端忽略它即可。
    pub(crate) fn public_account(&self, record: &StoredAccount) -> Value {
        // ── 插件记录优先（必须排在所有内置分支之前）─────────────────
        // 内置家装上插件后，新账号由插件落库（凭证在 `pluginSecret` 里，顶层
        // 没有任何内置字段）。若继续按 provider id 走下面的内置投影，那条
        // 记录会被渲染成「uid 空、tokenTail 空、hasRefreshToken false」的空行
        // —— 账号明明能用，界面上却像坏了。
        //
        // 判据是**记录上的 `pluginSecret`** 而不是「这个 id 由插件服务」：
        // 环境里还没装插件时，存量内置记录（没有 `pluginSecret`）照旧走内置
        // 投影，升级路径因此不受影响。
        let shaped = if record.has_plugin_secret() {
            self.to_plugin_public_account(record)
        } else if record.provider() == super::RACCOON_PROVIDER_ID {
            self.to_raccoon_public_account(record)
        } else if record.provider() == super::CATPAW_PROVIDER_ID {
            self.to_catpaw_public_account(record)
        } else if super::is_autoclaw_family(&record.provider()) {
            // 两个地区（`autoclaw` / `autoclaw-intl`）共用这一份公开形态：
            // 账号字段、桌面端判定、deviceId 语义两地完全一致，
            // 差别只在域名（那是转发与凭证层的事，公开形态不体现）
            self.to_autoclaw_public_account(record)
        } else if super::is_qoder_family(&record.provider()) {
            // 两个地区（`qoder` / `qoder-intl`）共用这一份公开形态：地区由
            // provider id（兜底 `mode` 字段）派生进 `edition` / `editionLabel`
            self.to_qoder_public_account(record)
        } else if record.provider() == super::codearts_accounts::CODEARTS_PROVIDER_ID {
            self.to_codearts_public_account(record)
        } else if super::is_cline_family(&record.provider()) {
            // 两个池（`cline-free` / `cline-pass`）共用这一份公开形态
            self.to_cline_public_account(record)
        } else if super::is_accio_family(&record.provider()) {
            // 两个地区（`accio` / `accio-cn`）共用这一份公开形态：账号字段、
            // 续期语义两地完全一致，差别只在登录站点与区域头（那是转发与
            // 凭证层的事，公开形态不体现）
            self.to_accio_public_account(record)
        } else if super::is_zcode_family(&record.provider()) {
            // 两个地区（`zcode` / `zcode-intl`）共用这一份公开形态：账号字段两地
            // 完全一致，差别只在推理域名与领取时的上游 provider 取值（那是转发
            // 与领取层的事，公开形态只用 `edition` 把地区标出来供界面显示）
            self.to_zcode_public_account(record)
        } else if record.provider() == super::TRAE_PROVIDER_ID {
            // Trae 只有一家（国内 SOLO）：国际版是**另一套协议**而不是一个地区，
            // 所以这里没有 `is_trae_family` —— 将来接国际版时另立 kind、
            // 另开一个分支，不要往本家的记录上挂 `region` 字段。
            // 这一支只服务**存量内置记录**：插件新落的账号有 `pluginSecret`，
            // 上面第一条分支已经带走（`account_view` 由插件给）。
            self.to_trae_public_account(record)
        } else if record.provider() == super::LOOMY_PROVIDER_ID {
            // Loomy（讯飞）：单一入口（手机验证码登录），公开形态带 userId /
            // phone / session 尾四位的展示字段（见 `loomy_accounts.rs`）
            self.to_loomy_public_account(record)
        } else if super::is_monkeycode_family(&record.provider()) {
            // MonkeyCode 的两个站点（`monkeycode` / `monkeycode-intl`）共用这一份
            // 公开形态：字段两地完全一致，差别只在站点域名（那是转发与凭证层的
            // 事，公开形态只用 `edition` 把地区标出来供界面显示）
            self.to_monkeycode_public_account(record)
        } else if record.provider() == super::kuku_accounts::KUKU_PROVIDER_ID {
            // KukuAI（百度文库库库 AI）：单一入口（粘贴 Cookie / 导入本机登录态），
            // 公开形态带 uid(=uk) / loginName(=昵称) / tokenTail（见
            // `kuku_accounts.rs`）
            self.to_kuku_public_account(record)
        } else if record.provider() == super::COMMANDCODE_PROVIDER_ID {
            // Command Code：单一入口（粘贴 `user_` API Key），公开形态去掉 key
            // 本体、只留尾号与「有没有 key」（见 `commandcode_accounts.rs`）
            self.to_commandcode_public_account(record)
        } else if record.provider() == super::ANTIGRAVITY_PROVIDER_ID {
            // Antigravity（Google）：单一入口（粘贴 Google refresh token），
            // 公开形态去掉 token 本体、只留 refresh token 尾号与
            // 「有没有 refreshToken / accessToken」（见 `antigravity_accounts.rs`）
            self.to_antigravity_public_account(record)
        } else if record.provider() == super::OFFICEACE_PROVIDER_ID {
            // OfficeAce（华为云果办）：只透出标识与到期时刻，网关 Basic 那一对
            // 与控制面临时凭据都不出这个函数（见 `to_officeace_public_account`）。
            // 漏这一支时它会掉进下面的 workbuddy 兜底形状：界面多出一批 uid/edition
            // 之类与它无关的空字段，而「刷新 Token」按钮靠的 hasRefreshToken
            // 只是**恰好**在兜底形状里也算得出来 —— 那不是设计，是巧合。
            self.to_officeace_public_account(record)
        } else if record.provider() == super::orcaterm_accounts::ORCATERM_PROVIDER_ID {
            // OrcaTerm（腾讯云遨驰终端桌面客户端）：单一入口（粘贴 accessToken /
            // 导入桌面端登录态），公开形态带 uid(=userId) / loginName(=昵称) /
            // tokenTail / tokenExpireAt（续期是否在工作可被肉眼校验，见
            // `orcaterm_accounts.rs`）
            self.to_orcaterm_public_account(record)
        } else if super::is_modelscope_family(&record.provider()) {
            // ModelScope（魔搭）的两个站点（`modelscope` / `modelscope-intl`）共用
            // 这一份公开形态：账号字段两地完全一致，差别只在域名（那是转发与凭证
            // 层的事，公开形态不体现）。去掉令牌本体，只留尾 4 位与「有没有令牌」
            // （见 `modelscope_accounts.rs`）。
            self.to_modelscope_public_account(record)
        } else if record.provider() == super::ATOMCODE_PROVIDER_ID {
            // AtomCode（AtomGit）：公开形态要摘掉**两个**令牌本体
            // （accessToken 与 refreshToken）—— 只摘前者的那套在这里是漏的，
            // refreshToken 是长期有效的续期凭据，不能随账号列表下发。
            self.to_atomcode_public_account(record)
        } else if record.provider() == super::LOBSTERAI_PROVIDER_ID {
            // LobsterAI：公开形态要摘掉**令牌本体 + 载荷件三件套**
            // （refreshToken / uuid / 两个 keyfrom）。它们都是「能续期」的条件，
            // 一起下发等于把账号的续期能力送出去；界面要的只是 canRefresh 布尔。
            self.to_lobsterai_public_account(record)
        } else if record
            .provider()
            .starts_with(crate::server::core::custom_providers::ID_PREFIX)
        {
            // 自定义提供商的账号。判据用**前缀**而不是 `custom_providers::
            // is_custom_provider_id`（后者还要求提供商仍在配置里）：形状分派
            // 是纯展示决定，提供商被删后残留的账号（迁移/手改残留）也应如实
            // 显示出来，而不是退化成 workbuddy 形状让界面出现 uid/edition 等
            // 与它无关的字段
            self.to_custom_public_account(record)
        } else {
            self.to_public_account(record)
        };
        match shaped {
            Value::Object(mut fields) => {
                fields.insert(
                    "hasCredentials".to_string(),
                    Value::Bool(record.has_credentials()),
                );
                // 没有转发能力的家：界面据此说明「启用了也不会被转发」，
                // 而不是把一个失效的启用开关当成正常账号展示。
                // 各家现在都能转发，所以正常配置下这里恒为 true ——
                // 保留这个字段是因为「能用账号管理、但转发还没接上」这种过渡期
                // 状态将来还会出现，而界面需要有办法如实说出来。
                fields.insert(
                    "chatSupported".to_string(),
                    Value::Bool(forwards_requests(record)),
                );
                // 最近一次签到成功的时刻（0 = 从未签过）。跨家统一注入的理由与上面
                // 两条相同：它是「这条账号的签到状态」这一个事实，各家的存放位置
                // 一致（`checkinAt`），界面不必按 provider 查表。
                //
                // 给出的是**时间戳**而不是「今天签过没」的布尔：自然日的边界要按
                // 用户本地时区算，而那个判定在界面上已有同款实现（限流恢复时间的
                // 「今天 / 明天」也是本地自然日，见 accounts-model.js 的 startOfDay）。
                // 传原始时间戳还让界面能显示「今天 08:30 已签到」这类信息。
                //
                // 恒为数字（无记录时 0）而不是缺键：界面按 `Number(...) || 0` 读，
                // 两种形态都能吃，但恒定的形状让「字段缺失」与「值为 0」不再需要分开判。
                fields.insert("checkinAt".to_string(), Value::from(record.checkin_at()));
                // 成长中心连登天数（WorkBuddy 国内版；签到后回读落库，见
                // `AccountStore::mark_streak_days`）。恒为数字键（0 = 还没读到过），
                // 与 checkinAt 同一理由：界面按数字读，不必判「字段缺失」。
                // 只有国内版会写这个字段，别家恒 0 —— 不按 provider 分支是刻意的：
                // 形状恒定，读侧不必查表。
                fields.insert(
                    "streakDays".to_string(),
                    Value::from(
                        record
                            .fields()
                            .get("streakDays")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                    ),
                );
                // 单账号并发上限（所有家通用的账号属性）：跨家统一注入的理由与上面
                // 三条相同 —— 选路（`routing::max_concurrent_of`）与前端账号菜单读的
                // 是**同一份事实**。恒为数字键（0 = 不限），且**只在这里写一次**：
                // 从前它是各家公开形态里各自的 `insert`，于是逐字段白名单那几家
                // （monkeycode / antigravity / officeace）漏了它 —— 缺键被选路当成
                // 0 = 不限，上限**静默失效**。CodeArts 的「0 = 按本家默认并发」口径
                // 由 `max_concurrent_public_for` 一处承载，别家 0 就是不限。
                fields.insert(
                    "maxConcurrent".to_string(),
                    Value::from(max_concurrent_public_for(
                        &record.provider(),
                        record.get("maxConcurrent"),
                    )),
                );
                // 用户显式设置过备注名（update_account 真正改到 name 时打的标，
                // 见 apply_patch）。恒为布尔而不是缺键：界面据此决定「备注名赢过
                // 邮箱 / 昵称等默认口径」还是「维持原展示行为」，两种形态都不必
                // 再判「字段缺失」。
                fields.insert(
                    "nameCustom".to_string(),
                    Value::Bool(
                        record
                            .fields()
                            .get("nameCustom")
                            .map(js_truthy)
                            .unwrap_or(false),
                    ),
                );
                // 每账号的余额查询设置（自动查询 + 余额不足处理）：跨家统一注入
                // 的理由与 hasCredentials 相同 —— 心跳调度、选路跳过 / 禁用与前端
                // 弹窗、徽章读的是**同一份事实**，不能各家形状一个口径。
                // 恒为对象（未配置 = 各家缺省档的规范化形状，与写入侧
                // `apply_patch` 的归一化一致），读侧不必判「键缺失」。
                fields.insert(
                    "usageQuery".to_string(),
                    usage_query_public(record.fields().get("usageQuery")),
                );
                fields.insert(
                    "lowBalance".to_string(),
                    low_balance_public(&record.provider(), record.fields().get("lowBalance")),
                );
                // 每账号的「限制器」有效规则（余额 / Token 规则列表）：恒为数组 ——
                // 记录上没有 `limiters` 键（旧版写的记录）时由 lowBalance / provider
                // 缺省**推导**（见 `limiter::effective_rules`），前端因此不必自带
                // 推导逻辑，弹窗、徽章与后端选路读到的永远是同一份规则。
                fields.insert(
                    "limiters".to_string(),
                    crate::server::core::limiter::effective_rules_json_in(record.fields()),
                );
                // ── 出网代理：跨家统一注入（插件家否则**永远显示「直连」**）──
                // 与 `maxConcurrent` 完全同一个坑，而且更隐蔽：它是「值对了但
                // 读不到」而不是「值丢了」。`apply_patch` 把 `proxy` 正确写进
                // 记录（`record.set_proxy`），但公开形态原先只有各家的
                // `to_*_public_account` 里各自投影一份 —— 插件家走
                // `to_plugin_public_account`，那个骨架里没有 `proxy`，于是：
                //   · 库里明明存着 `{source:"pool",proxyId:"…"}`；
                //   · 界面读到 `proxy: undefined` → 整列显示「直连」；
                //   · 再选一次同一条时 `apply_patch` 判「值没变」→ `changes: []`
                //     → 前端仍弹「✅ 代理已更新」（假成功），用户点了无数次
                //     也不会变 —— 实测在 Cline Free 上复现。
                //
                // 恒为「对象或 null」而不是缺键：`null` 就是直连（与写入侧
                // `normalize_account_proxy` 的 `None → Value::Null` 同一口径），
                // 前端 `ProxyCell` 按 `proxy?.config?.source` 读，两种形态都能吃。
                fields.insert(
                    "proxy".to_string(),
                    describe_account_proxy(Some(&record.proxy())),
                );
                // 跨家统一注入的**账号展示名**：与前端 `displayNameOf` 同口径的
                // 唯一后端实现（规则见本文件底部的 `display_name_of`）。
                // 恒为非空字符串（`displayName` 全兜底失效时退 id），请求日志 /
                // 运行日志 / 选路面板 / 签到台账等后端文案一律读它 —— 账号页
                // 显示什么，日志里就是什么，不再出现「账号页邮箱、日志昵称」
                // 这种两套口径的错位（issue #249）。
                fields.insert(
                    "displayName".to_string(),
                    Value::String(display_name_of(&fields, &record.id().to_string())),
                );
                // 智谱清言的**访客标记**：签到范围判定
                // （`billing::checkin::supports_checkin`）与签到中心的「范围外」
                // 原因都读它，两处必须看到同一份事实 —— 而访客形态只存在记录
                // 里（公开形态是逐个字段组装的，不会自动带上 `isGuest`）。
                //
                // 只对本家注入：别的家没有访客形态，给所有账号加一个恒 false 的
                // 键只会让读侧多一个「要不要判它」的疑问。判据见
                // `chatglm_accounts::guest_of_record`（只认确凿的访客证据，
                // 与新手任务列表那边的兜底方向刻意相反）。
                if record.provider() == super::CHATGLM_PROVIDER_ID {
                    fields.insert(
                        "isGuest".to_string(),
                        Value::Bool(super::chatglm_accounts::guest_of_record(record)),
                    );
                }
                Value::Object(fields)
            }
            // 各家形状恒为对象；真出现异常形态时原样透出，不在这里改语义
            other => other,
        }
    }

    /// provider 摘要：注册表顺序 + 各 provider 的账号总数。
    ///
    /// `records` 传**已经按用途过滤过**的那份（`snapshot` 里就是 `visible`）：
    /// 界面视图与内部视图的口径因此天然一致 —— 不会出现「分组标题写 3 个
    /// 账号、列表里一个都没有」。
    fn provider_summary(&self, records: &[StoredAccount]) -> Value {
        let counts: Vec<(String, usize)> = records.iter().fold(Vec::new(), |mut acc, record| {
            let provider = record.provider();
            match acc.iter_mut().find(|(id, _)| *id == provider) {
                Some((_, count)) => *count += 1,
                None => acc.push((provider, 1)),
            }
            acc
        });
        let ids: Vec<String> = counts.iter().map(|(id, _)| id.clone()).collect();
        let value = crate::server::core::providers::summary_json(
            |id| {
                counts
                    .iter()
                    .find(|(known, _)| known == id)
                    .map(|(_, count)| *count)
                    .unwrap_or(0)
            },
            &ids,
        );
        Value::Array(value)
    }

    // ─── 按 provider 查询 ────────────────────────────────────

    /// 指定 provider 的启用账号（公开形态，按 priority、addedAt 升序）。
    ///
    /// 消费方：聚合目录判断「这一家现在有没有可用登录态」
    /// （`catalog::provider_available`）—— 直接调适配器会让「有清单但没账号」
    /// 的家被广告给客户端，所以聚合层必须能按 provider 查可用性。
    ///
    /// 转发选路**不用**本函数：`rotate::provider_accounts` 走的是公开快照
    /// 过滤（那份要**看得见禁用账号**，第三级「全禁用 → 503」的文案依赖它）。
    ///
    /// 「启用」口径与选路一致：`enabled !== false`**且**有凭证
    /// （`has_credentials`：小浣熊的桌面端实时账号记录里没有 token，但凭证在
    /// auth.json，同样算「有凭证」，见 `state.rs` 的说明）。返回值是**排序后**的：
    /// 调用方按序尝试即可，不必自己再排一遍 —— 优先级的相对顺序就是主备顺序。
    pub fn accounts_for_provider(&self, provider: &str) -> Vec<Value> {
        let _guard = self.guard();
        let records = self.records_for_provider(&_guard, provider);
        let mut records: Vec<StoredAccount> = records
            .into_iter()
            .filter(|record| record.enabled() && record.has_credentials())
            .collect();
        records.sort_by_key(StoredAccount::order_key);
        records
            .iter()
            .map(|record| self.public_account(record))
            .collect()
    }

    /// 自定义提供商账号的公开形态（`custom_accounts` 写入的记录 → 界面形状）。
    ///
    /// ── 为什么不是 workbuddy 兜底形状 ────────────────────────────
    /// 兜底形状（`to_public_account`）会补 uid / nickname / edition /
    /// enterprise 这些**只对 WorkBuddy 有意义**的字段 —— 自定义账号拿到它们
    /// 只会在界面上渲染出一排空值，还会诱导前端去读一个不存在的 edition。
    /// 独立形状让「这条账号有什么」如实反映「这条记录存了什么」。
    ///
    /// ── `baseUrl` 的缺省语义 ────────────────────────────────────
    /// 记录里的 `baseUrl` 是**覆盖项**（缺省时转发回落到提供商的 baseUrl），
    /// 因此记录里没有这个键时公开形态也不带它 —— 前端据「键是否存在」区分
    /// 「未覆盖（跟随提供商）」与「覆盖成了某个值」，不能把空串/null 塞进来
    /// 让两种语义混在一起。
    ///
    /// `hasCredentials` / `chatSupported` / `checkinAt` 由 [`Self::public_account`]
    /// 统一注入（跨家事实，见那里的说明）；自定义账号的 `hasCredentials`
    /// 判据是「`apiKey` 非空 **或** 声明了无需鉴权」（见 `state::no_auth`），
    /// 两者都没有 = 未配置凭证，界面据此标出「转发会被跳过」。
    pub(crate) fn to_custom_public_account(&self, record: &StoredAccount) -> Value {
        let fields = record.fields();
        let mut public = Map::new();
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        public.insert("provider".to_string(), Value::String(record.provider()));
        public.insert("name".to_string(), Value::String(record.name()));
        // apiKey 的尾号（与各家的 tokenTail 同一展示语义；空 key 时是空串）
        public.insert(
            "tokenTail".to_string(),
            value_or(fields.get("tokenTail"), Value::String(String::new())),
        );
        // ── 凭证形态的两个读数（界面「设置」里改凭证要用）──────────
        // `hasApiKey` 恒给一个布尔：输入框的提示文案要区分「已配置，留空则
        // 不修改」与「还没配」；`noAuth` 恒给布尔（缺键即 false）—— 开关的
        // 初值由它决定。**apiKey 本身绝不透出**（值与尾号两条管道分开，
        // 见 `custom_credential_by_id` 的说明）。
        public.insert("hasApiKey".to_string(), Value::Bool(record.has_api_key()));
        public.insert("noAuth".to_string(), Value::Bool(record.no_auth()));
        // 覆盖项：记录里**写了键**才透出（缺键 = 未覆盖，语义见函数头）
        if let Some(base_url) = fields.get("baseUrl") {
            public.insert("baseUrl".to_string(), base_url.clone());
        }
        // ── 余额查询配置（`balanceQuery`）────────────────────────────
        // **只在提供商登记了余额方案时出现**（预置卡写入的 `balanceScheme`，
        // 见 `providers::custom::balance`）。前端据「有没有这个键」决定渲染
        // 「查询设置」段、方案说明与余额按钮 —— 与后端 `supports_usage` 同一
        // 判据（两处都问 `balance_scheme_of`，不会各漂一份）。
        //
        // 三个密字段（`accessToken` / `password` / `token`）与商汤的缓存登录票
        // **绝不透出**，只给 `has*` 标记（输入框的占位文案与「清除」按钮要用）；
        // 两个非密字段（`userId` / `username`）原样回显 —— 与 apiKey 的
        // 「值与尾号两条管道分开」同一条纪律（见 `custom_credential_by_id`）。
        if let Some(scheme) = custom_providers::balance_scheme_of(&record.provider()) {
            public.insert(
                "balanceQuery".to_string(),
                balance_query_public(fields, &scheme),
            );
        }
        public.insert("source".to_string(), Value::String(record.source()));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert("proxy".to_string(), describe_account_proxy(Some(&record.proxy())));
        // 「可用」= 凭证完整（与各家同一语义，见 `accounts-shared` 的 available 说明）。
        // 自定义账号这一项以前恒 true，于是「没填 Key 也没勾无需鉴权」的账号照样出现在
        // 「模型来源」下拉里（界面按 `available !== false` 过滤，见 models-fetch-modal）
        // ——它去拉清单必然报「请先添加账号」，那条选项本身就是误导。
        // 现在按 `has_credentials` 如实给：这类账号在界面上显示为「未配置凭证」。
        public.insert(
            "available".to_string(),
            Value::Bool(record.enabled() && record.has_credentials()),
        );
        Value::Object(public)
    }
}

/// 记录上的 `balanceQuery` → 公开形态（方案 id + 两个非密字段 + 三个密字段的
/// `has*` 标记；**值本身绝不透出**）。
///
/// 形状恒为对象（没有 `balanceQuery` 键时两个非密字段给空串、`has*` 给 false），
/// 前端不必判「键缺失」——「这一家有没有余额查询」由**外层那个键在不在**回答
/// （只有登记了方案的家才有），与「凭证配没配」是两件事。
fn balance_query_public(fields: &Map<String, Value>, scheme: &str) -> Value {
    let query = fields.get("balanceQuery").and_then(Value::as_object);
    let text_of = |key: &str| {
        query
            .and_then(|map| map.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let configured = |key: &str| !text_of(key).is_empty();
    json!({
        "scheme": scheme,
        "userId": text_of("userId"),
        "username": text_of("username"),
        "hasAccessToken": configured("accessToken"),
        "hasPassword": configured("password"),
        "hasToken": configured("token"),
    })
}

/// 记录上的 `usageQuery` → 公开形态的规范化形状（恒为对象）。
///
/// 读侧只做**容错展开**、不校验：合法性由写入侧（`apply_patch` 的
/// `normalize_usage_query`）保证，这里负责把缺失 / 脏值收敛成缺省形状，让
/// 调度与前端都按「恒定形状」读。缺省与 `usage_records::query_interval_of`
/// 同一口径 —— **未配置 = 开启、1 分钟**（对齐全局任务时代「默认就在查」的
/// 行为，升级后余额列不停更），只有显式 `{enabled:false}` 才是关。
fn usage_query_public(value: Option<&Value>) -> Value {
    use crate::server::core::usage_records::{
        DEFAULT_QUERY_INTERVAL_SECONDS, MAX_QUERY_INTERVAL_SECONDS, MIN_QUERY_INTERVAL_SECONDS,
    };
    let Some(fields) = value.and_then(Value::as_object) else {
        return json!({ "enabled": true, "interval": DEFAULT_QUERY_INTERVAL_SECONDS });
    };
    let enabled = fields.get("enabled").map(js_truthy).unwrap_or(true);
    let interval = fields.get("interval").and_then(Value::as_i64).unwrap_or(0);
    // 开着但间隔缺失 / 越界（脏值）→ 缺省间隔；关着则原样透出（0 = 未设过）
    let interval = if enabled
        && !(MIN_QUERY_INTERVAL_SECONDS..=MAX_QUERY_INTERVAL_SECONDS).contains(&interval)
    {
        DEFAULT_QUERY_INTERVAL_SECONDS
    } else {
        interval
    };
    json!({ "enabled": enabled, "interval": interval })
}

/// 记录上的 `lowBalance` → 公开形态的规范化形状。
///
/// 缺省（无配置）按 provider 区分（与 `usage_records::balance_blocked` 同一口径，
/// 见 `default_low_balance_mode`）：Cline 免费池 = `off`（不处理），其余 = `skip`、
/// 阈值 1；显式配置一律原样尊重 —— 那是用户选过的，缺省不能覆盖它。skip / disable
/// 档下阈值缺失或非法时回落缺省 1。
fn low_balance_public(provider: &str, value: Option<&Value>) -> Value {
    use crate::server::core::usage_records::{default_low_balance, default_low_balance_mode, DEFAULT_LOW_BALANCE_THRESHOLD};
    let default_mode = default_low_balance_mode(provider);
    let Some(fields) = value.and_then(Value::as_object) else {
        return default_low_balance(provider);
    };
    let mode = fields.get("mode").and_then(Value::as_str).unwrap_or(default_mode);
    let threshold = fields
        .get("threshold")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(DEFAULT_LOW_BALANCE_THRESHOLD);
    match mode {
        "skip" => json!({ "mode": "skip", "threshold": threshold }),
        "disable" => json!({ "mode": "disable", "threshold": threshold }),
        _ => json!({ "mode": "off", "threshold": 0.0 }),
    }
}

/// 公开形态字段 → 账号展示名（`displayName`，恒为非空字符串）。
///
/// 这是前端 `displayNameOf`（ui-islands accounts-domain.ts）的**唯一后端口径**，
/// 两条线与它逐条对齐：
///   1. 用户显式设置过备注名（`nameCustom`）→ 备注名恒为主名；
///   2. 未打标 → 「以邮箱报名字」的三家（Qoder 两地 / AutoClaw 国际版 / Accio
///      两地）**邮箱优先** —— 这是历史约定：这四家登录拿到的是邮箱、昵称往往
///      缺失或不可读（issue #249 的 Qoder 国版正是「账号页邮箱、日志昵称」的
///      错位来自两套口径）；其余家昵称优先，再退 name / 标识 / id。
/// 前端 displayNameOf 的兜底链是 `nickname || name || identifierOf || id`，
/// 这里 `identifier` 按各家的能力表逐家写死（与前端 `identifier` 列同一份口径，
/// 后端没有那张表，家数有限不值得再抄一份）。
///
/// **消费方只读公开形态**（选路快照 / 签到台账 / telemetry），所以入参就是
/// 公开形态字段本身（`name` / `nickname` / `email` / `nameCustom` / `provider`），
/// 不再回读记录。
fn display_name_of(public: &Map<String, Value>, id: &str) -> String {
    let text = |key: &str| -> String {
        public
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let name = text("name");
    // 用户显式设置过备注名（update_account 真正改到 name 时打的标）→ 备注名恒为主名
    if public.get("nameCustom").map(js_truthy).unwrap_or(false) && !name.is_empty() {
        return name;
    }
    let email = text("email");
    let provider = text("provider");
    // 「以邮箱报名字」的三家：与前端 emailAsName 列一致 ——
    // Qoder 两个地区 / AutoClaw 国际版 / Accio 两个地区
    let email_as_name = super::is_qoder_family(&provider)
        || provider == super::AUTOCLAW_INTL_PROVIDER_ID
        || super::is_accio_family(&provider);
    if email_as_name && !email.is_empty() {
        return email;
    }
    let nickname = text("nickname");
    let identifier = identifier_key_of(&provider).map(text).unwrap_or_default();
    let fallback = [nickname, name, identifier]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| id.to_string());
    fallback
}

/// 各家公开形态上的「账号标识」字段键（与前端能力表 `identifier` 列同一份口径；
/// 未登记的家按 userId 兜底 —— 与前端 GENERIC_FEATURES 一致）。
fn identifier_key_of(provider: &str) -> Option<&'static str> {
    match provider {
        "workbuddy" | "workbuddy-intl" | "catpaw" | "trae" | "kuku" => Some("uid"),
        "cline-free" | "cline-pass" => Some("account"),
        "officeace" => Some("id"),
        _ => Some("userId"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::display_name_of;

    fn fields(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn qoder_shows_email_like_the_accounts_page() {
        // issue #249：Qoder 国版账号页显示邮箱、请求日志显示昵称 —— 统一后
        // 「以邮箱报名字」的家在后端同样邮箱优先
        let value = fields(json!({
            "provider": "qoder", "name": "昵称", "email": "a@b.com", "userId": "u1",
        }));
        assert_eq!(display_name_of(&value, "id-1"), "a@b.com");
        let intl = fields(json!({
            "provider": "qoder-intl", "email": "c@d.com", "userId": "u2",
        }));
        assert_eq!(display_name_of(&intl, "id-2"), "c@d.com");
    }

    #[test]
    fn custom_note_wins_over_email() {
        let value = fields(json!({
            "provider": "qoder", "name": "备注名", "nameCustom": true,
            "email": "a@b.com", "userId": "u1",
        }));
        assert_eq!(display_name_of(&value, "id-1"), "备注名");
    }

    #[test]
    fn other_providers_prefer_nickname() {
        let value = fields(json!({
            "provider": "workbuddy", "nickname": "昵称", "name": "种子名", "uid": "uid-9",
        }));
        assert_eq!(display_name_of(&value, "id-1"), "昵称");
    }

    #[test]
    fn falls_back_to_identifier_then_id() {
        let value = fields(json!({ "provider": "raccoon", "userId": "u-77" }));
        assert_eq!(display_name_of(&value, "id-1"), "u-77");
        let empty = fields(json!({ "provider": "raccoon" }));
        assert_eq!(display_name_of(&empty, "id-9"), "id-9");
    }
}
