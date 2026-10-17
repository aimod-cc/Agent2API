//! 自定义提供商账号：手动添加、级联删除、公开形态的字段来源
//! （架构见 `core::custom_providers` 的模块头）。
//!
//! ── 与各家添加路径的关系 ────────────────────────────────────
//! 内置八家的凭证链路各不相同（JWT / token / apiKey / 桌面端登录态），因此
//! 各有自己的 `add_*_account`。自定义提供商最简单：**用户直接填一把上游
//! API Key**，或者显式声明「这家上游不需要鉴权」（`noAuth`，见下）—— 网关
//! 不代管任何登录态，所以这里没有刷新、没有导入、没有桌面端实时读取，
//! 只有「写一条记录」。
//!
//! ── 记录形状（与内置家对齐的部分 + 本家特有的键）─────────────
//! ```json
//! {
//!   "id": "custom-acct-8f2c1d4a6b90",  // custom-acct- + 12 位 hex
//!   "provider": "custom-3f2a91b04c7e", // 指向自定义提供商（外键字符串）
//!   "name": "我的网关 A 账号",
//!   "apiKey": "sk-…",                  // 凭证（可空：无需鉴权的上游）
//!   "noAuth": true,                    // 可选：显式声明「该上游无需鉴权」。
//!                                      // 与 apiKey 互斥（写入侧归一，见
//!                                      // `state::no_auth`）；勾了它才不发鉴权头
//!   "baseUrl": "https://…",            // 可选覆盖项；不存在则用提供商的 baseUrl
//!   "tokenTail": "…abcd",              // 界面上展示的尾号（apiKey 派生）
//!   "source": "manual",
//!   "priority": 5,                     // 全局一条队列（与各家共用号段）
//!   "enabled": true,
//!   "addedAt": 1730000000000,
//!   "updatedAt": 1730000000000
//! }
//! ```
//!
//! ── 为什么 id 是派生 + 随机的两段式 ─────────────────────────
//! `apiKey` 非空时用它的 SHA-256 前 12 位：**同一把 key 重复添加会合并更新
//! 同一条记录** —— 与各家「同标识合并」（workbuddy 的 `user-<uid>`、Qoder 的
//! `qoder-<region>-<hash>`）语义一致，用户粘错重试不会堆出重复账号。
//! 没有 key 时（显式声明无需鉴权，或凭证还没配）退化为随机 12 位
//! （`getrandom`）：没有稳定标识可派生，而且**同一家并存多条无需鉴权账号是
//! 刻意的**（各自可配不同的出网代理，转发按优先级轮着走），重复添加就该是
//! 新的一条 —— id 的唯一性靠随机段保证（并发/重装都要安全，理由同
//! `custom_providers` 的 id）。派生规则见 [`account_id_for`]。
//! 前缀 `custom-acct-` 与 provider id 的 `custom-` **刻意不同形**：
//! `is_custom_provider_id` 按 provider 前缀判定，账号 id 若同形会被误认成
//! 提供商 id（两套 id 空间混淆是难查的静默错配）。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::custom_providers;
use crate::server::core::proxies::{resolve_account_proxy_rotating, ResolvedProxy};
use crate::server::logging;

/// 账号 id 前缀。**与 provider id 的 `custom-` 不同形**（理由见模块头）。
pub const CUSTOM_ACCOUNT_ID_PREFIX: &str = "custom-acct-";

/// 备注名长度上限（与另外几家一致）
const MAX_NAME_CHARS: usize = 100;

/// apiKey 长度上限（防御：粘贴进一整段文档时不落盘）。
/// 上游 key 一般 32~256 字符，8192 与 `MAX_TOKEN_LENGTH` 同量级。
const MAX_API_KEY_LENGTH: usize = 8192;

/// `add_custom_account` 的返回：落盘记录的公开形态 + 这次是新建还是更新。
///
/// 幂等口径（见 [`account_id_for`]）会让「同一把 key 的重复添加」合并进既有
/// 记录。这次提交到底落在哪一边只有存储层
/// 知道（日志里有 🔄 / ✅ 之分），把它作为返回值交给 API 层带出去，前端才能
/// 把提示说成「已更新」而不是「已添加」（照搬「已添加」会让人以为列表会多
/// 出一条，实测反馈过的误导）。
pub struct CustomAccountUpsert {
    /// 落盘记录的公开形态（与列表快照同一形状，见 `public_account`）
    pub account: Value,
    /// `true` = 新建了一条记录；`false` = 合并更新了既有记录（id 命中同 key
    /// 的账号）
    pub created: bool,
}

impl AccountStore {
    // ─── 写：手动添加 ────────────────────────────────────────

    /// 添加/更新一个自定义提供商账号（`POST /api/accounts` 的 custom 分支与
    /// `POST /api/custom-providers` 的「首个账号」共用）。
    ///
    /// `provider_id` 必须**以 `custom-` 开头**（调用方已用
    /// `custom_providers::is_custom_provider_id` 校验过它确实存在；这里只做
    /// 前缀防御 —— 存储层不回头查配置，免得账号层与配置层互相依赖）。
    ///
    /// `payload` 认四个键（其余忽略，与各家「未知键不动」口径一致）：
    ///   - `apiKey`：凭证（可空 —— 部分自建上游不需要 key）；
    ///   - `noAuth`：**显式声明「该上游无需鉴权」**（布尔，缺省 false）。与
    ///     `apiKey` **互斥**：为真时 apiKey 强制落空串，非空 apiKey 时该键
    ///     直接不写。判据这样收，是为了让 `has_credentials` 与「出网发不发
    ///     鉴权头」都只有一种读法（见 `state::no_auth`）。**它是「无鉴权」
    ///     与「忘了填 key」的唯一分界**：后者不落这个键，账号在界面上会被
    ///     标成未配置凭证，转发选路照样跳过；
    ///   - `baseUrl`：**可选覆盖项**（缺省不写这条键，转发阶段回落到提供商的
    ///     baseUrl）。校验与提供商侧同一套（`normalize_base_url`）；
    ///   - `name` 由 `name` 参数决定（调用方已从请求体取好，与各家一致）。
    ///
    /// 幂等口径：`apiKey` 非空时同 key 重复添加 = **更新**既有记录（备注名 /
    /// baseUrl 覆盖项随之更新，优先级与启用状态沿用）。没有 key（无需鉴权 /
    /// 凭证未配）则每次都是新建 —— 同一家并存多条无需鉴权账号是刻意的（各自
    /// 可配不同的出网代理）。撞到**其它 provider** 的账号 id（hash 空间巧合）
    /// 时报 409 而不是覆写。落在哪一边由返回值的
    /// [`CustomAccountUpsert::created`] 表达，API 层原样透传给前端。
    pub fn add_custom_account(
        &self,
        provider_id: &str,
        payload: &Value,
        name: Option<&str>,
    ) -> Result<CustomAccountUpsert, AccountStoreError> {
        let provider_id = provider_id.trim();
        if !provider_id.starts_with(custom_providers::ID_PREFIX) {
            return Err(AccountStoreError::bad_request(format!(
                "不是自定义提供商 id: {provider_id}"
            )));
        }
        let Some(object) = payload.as_object() else {
            return Err(AccountStoreError::bad_request("账号内容必须是 JSON 对象"));
        };
        // noAuth：类型不对就当场报错（与名/优先级等字段同一取向：「保存成功但
        // 没生效」比一条 400 难查）。它是「这条账号不需要鉴权」的显式声明，
        // 与 apiKey 互斥的归一在下面做。
        let no_auth = match object.get("noAuth") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => {
                return Err(AccountStoreError::bad_request("noAuth 必须是布尔值"));
            }
        };
        // apiKey：trim 后落盘（粘贴时常见首尾空白/换行 —— 原样落盘会让
        // Authorization 头带上换行，请求在网络层就失败，用户看不出为什么）。
        // 勾了「无需鉴权」就没有 key 可存：两个字段的互斥不变量在这里收口。
        let api_key = if no_auth {
            String::new()
        } else {
            object
                .get("apiKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        if api_key.chars().count() > MAX_API_KEY_LENGTH {
            return Err(AccountStoreError::bad_request("apiKey 过长"));
        }
        // baseUrl 覆盖项：给了就必须合法（与提供商侧同判据）；**没给不写键** ——
        // 「没有覆盖项」与「覆盖项指到空串」在转发阶段是两种语义，形状上要能区分
        let base_url_override = match object
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            Some(raw) => Some(
                custom_providers::normalize_base_url(raw)
                    .map_err(AccountStoreError::bad_request)?,
            ),
            None => None,
        };

        let _guard = self.guard();
        let id = account_id_for(&api_key)
            .map_err(|error| AccountStoreError::new(error, 500))?;
        // 撞 id 保护（与 cline 同一考虑）：hash 空间巧合或手改数据都会撞上
        // 别家的记录，覆写会把那条记录的凭证一起弄丢
        let existing = self.record_by_id(&_guard, &id);
        if let Some(existing) = existing.as_ref() {
            if existing.provider() != provider_id {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{}账号占用，请先处理那条记录",
                        existing.provider()
                    ),
                    409,
                ));
            }
        }

        // 备注名兜底链：显式传入 → 记录里原有的 → 「账号 + apiKey 尾号」。
        // 尾号对人有参考价值（哪把 key）；无鉴权账号没有尾号可展示，退化成
        // 「<提供商名>（无需鉴权）」—— 它比一个光秃秃的「自定义账号」更能
        // 回答「这条是什么」；两者都没有时才是那个通用兜底。兜底名不做查重：
        // 同家并存的多条无需鉴权账号默认同名（区分靠用户自己起的备注名）。
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_CHARS));
        let explicit = explicit_name.is_some();
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                let tail = token_tail_of(&api_key);
                if !tail.is_empty() {
                    format!("账号 {tail}")
                } else if no_auth {
                    let label = custom_providers::label_of(provider_id)
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "自定义账号".to_string());
                    format!("{label}（无需鉴权）")
                } else {
                    "自定义账号".to_string()
                }
            });

        let priority = existing
            .as_ref()
            .map(StoredAccount::priority)
            .unwrap_or_else(|| {
                // 号段取全部账号：优先级全局唯一（所有家共用一条队列，见
                // `account_store` 模块头）。查询失败（库不可用）回落到默认号段，
                // 与 cline 的兜底一致。
                let used = self
                    .with_conn(&_guard, |conn| sql::priorities_all(conn))
                    .unwrap_or_default();
                next_free_priority(&used)
            });

        let now = logging::now_ms();
        // 从既有记录的字段表出发（不变量「未知字段全量保留」：用户手加的备注、
        // 将来版本新增的字段都不能在这一次重建里丢）—— 新建时是空表
        let mut fields: Map<String, Value> = existing
            .as_ref()
            .map(|item| item.fields().clone())
            .unwrap_or_default();
        fields.insert("id".to_string(), Value::String(id));
        fields.insert(
            "provider".to_string(),
            Value::String(provider_id.to_string()),
        );
        fields.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(&mut fields, explicit, existing.as_ref());
        fields.insert("apiKey".to_string(), Value::String(api_key.clone()));
        // 「无需鉴权」标记：真时才落键，假时**摘掉**键 —— 与模型条目的
        // capabilities 同一取向（「空表不落键」），老记录读回再写不会凭空多出
        // 一个 `"noAuth": false`。
        if no_auth {
            fields.insert("noAuth".to_string(), Value::Bool(true));
        } else {
            fields.remove("noAuth");
        }
        match base_url_override {
            Some(base_url) => {
                fields.insert("baseUrl".to_string(), Value::String(base_url));
            }
            // 显式传了空 baseUrl（想清掉覆盖项回落到提供商默认值）也要支持：
            // 请求里**带了这个键**但值为空 → 删掉覆盖项；完全没带 → 保留原值。
            // 这是「清空覆盖」唯一可行的表达方式（空串不是合法 URL，不能落盘）。
            None => {
                if object.contains_key("baseUrl") {
                    fields.remove("baseUrl");
                }
            }
        }
        // 余额查询配置（`balanceQuery`，可选）：预置卡带来的余额方案要的那份
        // 凭证（NewAPI 的访问令牌 / 商汤的控制台账号密码 / 智谱的控制台令牌）。
        // 与 apiKey 同一层是刻意的：它也是「这条账号的凭证」，只是用于查询
        // 而不是转发（见 `custom_balance_query_by_id` 的说明）。
        if let Some(value) = object.get("balanceQuery") {
            if !value.is_null() {
                let Some(patch) = value.as_object() else {
                    return Err(AccountStoreError::bad_request("balanceQuery 必须是 JSON 对象"));
                };
                let mut query = fields
                    .get("balanceQuery")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                normalize_balance_query_patch(&mut query, patch)
                    .map_err(AccountStoreError::bad_request)?;
                if query.is_empty() {
                    fields.remove("balanceQuery");
                } else {
                    fields.insert("balanceQuery".to_string(), Value::Object(query));
                }
            }
        }
        fields.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&api_key)),
        );
        fields.insert(
            "source".to_string(),
            Value::String(
                existing
                    .as_ref()
                    .map(StoredAccount::source)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "manual".to_string()),
            ),
        );
        fields.insert("priority".to_string(), Value::from(priority));
        fields.insert(
            "enabled".to_string(),
            Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)),
        );
        fields.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .filter(|value| *value != 0)
                    .unwrap_or(now),
            ),
        );
        fields.insert("updatedAt".to_string(), Value::from(now));

        let saved = StoredAccount::from_map(fields);
        // 单行落地：`put` = DELETE + INSERT（更新既有记录时它落到列表末尾，
        // 与旧实现 retain + push 的结果一致，见 `sql::put`）
        self.with_conn(&_guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!(
                "{} 自定义账号{}: {}（{}）",
                if existing.is_some() { "🔄" } else { "✅" },
                if existing.is_some() { "已更新" } else { "已添加" },
                record_name,
                custom_providers::label_of(provider_id)
                    .unwrap_or_else(|| provider_id.to_string()),
            ),
        );
        Ok(CustomAccountUpsert {
            account: self.public_account(&saved),
            created: existing.is_none(),
        })
    }

    /// 删除某个自定义提供商名下的**全部账号**，返回删除条数。
    ///
    /// 只服务级联删除（`custom_providers::remove` 在删掉提供商之前调用它）：
    /// 逐个删而不是一条批量 SQL —— 账号数量在个位数，而 `sql::delete` 是
    /// 既有单行删除路径（含「不存在返回 false」的判据），复用它可以不新增
    /// SQL、也不引入新的事务语义（本操作不需要跨行原子性：删到一半失败时
    /// 剩下的账号仍在原组里，重试一次即可）。
    pub fn remove_custom_accounts(&self, provider_id: &str) -> Result<usize, AccountStoreError> {
        let provider_id = provider_id.trim();
        let _guard = self.guard();
        let records = self.with_conn(&_guard, |conn| sql::load_by_provider(conn, provider_id))?;
        let mut removed = 0usize;
        for record in &records {
            let deleted = self.with_conn(&_guard, |conn| sql::delete(conn, record.id()))?;
            if deleted {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 按账号 id 取它的 provider —— **仅当它属于自定义提供商时**返回 Some。
    ///
    /// 消费方是 `api::accounts::refresh_account` 的分支：自定义账号没有可刷新
    /// 的凭证（key 是用户填的），若不拦截会落进 workbuddy 的刷新链路并得到
    /// 一条误导性的「账号不存在」。判据用前缀而不是 `is_custom_provider_id`：
    /// 这里问的是「这条账号记录是不是自定义形态」，与提供商是否还在无关
    /// （提供商被删后残留的账号同样不该走 workbuddy 刷新）。
    pub fn custom_account_provider(&self, id: &str) -> Option<String> {
        let _guard = self.guard();
        let record = self.record_by_id(&_guard, id)?;
        let provider = record.provider();
        provider
            .starts_with(custom_providers::ID_PREFIX)
            .then_some(provider)
    }

    // ─── 读：内部凭证快照（转发 / 拉取模型清单用）────────────────

    /// 读取一条自定义账号的**转发凭证**（`custom_credential_by_id`）。
    ///
    /// 消费方：`providers::custom::forward`（对话转发，出网要 apiKey 与
    /// baseUrl 覆盖项）。**返回值含 apiKey 明文，绝不进任何 HTTP 响应** ——
    /// 公开形态（`to_custom_public_account`）刻意只有 tokenTail，那是给界面的；
    /// 这条是网关自身出网的内部读数，两条管道不能共用一个形状。
    ///
    /// 代理在锁内只做**解析**（读 Clash 快照，本地文件 IO；与
    /// `get_session_by_id` 同一口径），真正的网络动作都在锁外。
    pub fn custom_credential_by_id(&self, account_id: &str) -> Option<CustomCredential> {
        let _guard = self.guard();
        let record = self.record_by_id(&_guard, account_id)?;
        Some(credential_of_record(&record))
    }

    /// 该自定义提供商下第一个**可用**账号的凭证（`custom_providers::
    /// fetch_upstream_models` 的取数口径）。
    ///
    /// 「可用」= 启用 **且**（apiKey 非空 或 显式声明无需鉴权）—— 与
    /// `has_credentials` 同一判据：拉清单本身就是一次出网，能不能拉成功
    /// 取决于「这条账号出网时有没有凭证可用」，而不是「有没有填 key」
    /// （无鉴权上游不填 key 也拉得到，见 `no_auth`）。
    ///
    /// 「第一个」= 优先级最小（`order_key` 与选路同一排序）—— 拉清单是
    /// 管理动作，没有逐账号轮换的语义，用队首账号即可；多个账号各有清单
    /// 可见性的场景现实中不存在（同一家上游的清单对每把 key 一致）。
    /// 一个都没有时报错文案由调用方给（「请先添加账号」），这里以 None 表达。
    pub fn first_custom_credential(&self, provider_id: &str) -> Option<CustomCredential> {
        let _guard = self.guard();
        self.records_for_provider(&_guard, provider_id)
            .into_iter()
            .filter(|record| record.enabled() && (record.has_api_key() || record.no_auth()))
            .min_by_key(StoredAccount::order_key)
            .map(|record| credential_of_record(&record))
    }

    /// 改一条自定义账号的**凭证**：`apiKey`（可清除）与 `noAuth`（无需鉴权
    /// 标记）。返回变更说明（供界面提示与日志），没有任何变化时返回空表。
    ///
    /// ── 为什么单开一个方法（而不是并进 `update_account` 的 apply_patch）──
    /// 与 CatPaw 的 `balanceToken`、ZCode 的 `zcodePlan` 同一处境：这是**自定义
    /// 家独有**的字段。并进通用 patch 意味着内置八家的账号也认识它们，而
    /// 「普通账号被写进一个 apiKey」只会让选路凭空多出一条无凭证记录。
    /// 入口在 `api::accounts::patch_account` 里按 id 分流（那里已经有两处同款
    /// 先例）。
    ///
    /// ── 三条不变量（写入侧的唯一收口处）────────────────────────
    ///   1. `noAuth` 与 `apiKey` 互斥：开无需鉴权 → key 清空；写入非空 key
    ///      → 摘掉标记。于是读侧不必判优先级；
    ///   2. **账号 id 不变**：id 只是身份（限额冷却、请求日志、导出文件都按它
    ///      关联），换一把 key 是「同一条账号换了凭证」而不是「换了一条账号」。
    ///      「同 key 合并」只在**添加**路径上成立（那里 id 还没生成）；
    ///   3. `apiKey` 为 null / 空串 = **清除**（显式动作，沿用界面上的「清除」
    ///      按钮语义 —— 输入框留空是「不修改」，靠这个键表达「清掉」）。
    ///
    /// 改 key 会同步刷新 `tokenTail`（界面展示的尾号必须跟着走，否则行上显示的
    /// 还是上一把 key 的尾号）。
    pub fn update_custom_credentials(
        &self,
        id: &str,
        patch: &Value,
    ) -> Result<Vec<String>, AccountStoreError> {
        let id = id.trim();
        let Some(object) = patch.as_object() else {
            return Err(AccountStoreError::bad_request("请求内容必须是 JSON 对象"));
        };
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return Err(AccountStoreError::not_found("账号不存在"));
        };
        if !record.provider().starts_with(custom_providers::ID_PREFIX) {
            return Err(AccountStoreError::bad_request(
                "只有自定义提供商的账号有 API Key / 无需鉴权这两项",
            ));
        }
        let mut changes: Vec<String> = Vec::new();
        // ── 两个键的取值都在这里验完再落盘：先算「写完之后是什么样」，
        // 再一次性写进记录 —— 中途报错不会留下「标记摘了、key 还在」的中间态
        let mut next_no_auth = record.no_auth();
        let mut next_api_key = record
            .fields()
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(value) = object.get("noAuth") {
            // null = 不改（与全仓「缺省不动」同一取向）：关闭标记要显式传 false，
            // 免得「带了个 null」被当成一次静默的关闭
            if !value.is_null() {
                let flag = value
                    .as_bool()
                    .ok_or_else(|| AccountStoreError::bad_request("noAuth 必须是布尔值"))?;
                next_no_auth = flag;
                if flag {
                    // 不变量 1：勾了无需鉴权就没有 key 可存
                    next_api_key = String::new();
                }
            }
        }
        if let Some(value) = object.get("apiKey") {
            let key = match value {
                Value::Null => String::new(),
                Value::String(text) => text.trim().to_string(),
                _ => {
                    return Err(AccountStoreError::bad_request(
                        "apiKey 必须是字符串（传 null 表示清除）",
                    ))
                }
            };
            if key.chars().count() > MAX_API_KEY_LENGTH {
                return Err(AccountStoreError::bad_request("apiKey 过长"));
            }
            next_api_key = key;
            if !next_api_key.is_empty() {
                // 不变量 1 的另一半：存了 key 就不该再挂「无需鉴权」
                next_no_auth = false;
            }
        }
        // ── 差异落盘（三处：标记、key、尾号）──────────────────────
        let before_no_auth = record.no_auth();
        let before_key = record
            .fields()
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if next_no_auth != before_no_auth {
            if next_no_auth {
                record.set("noAuth", Value::Bool(true));
                changes.push("无需鉴权 → 已开启（转发不发鉴权头）".to_string());
            } else {
                record.remove("noAuth");
                changes.push("无需鉴权 → 已关闭".to_string());
            }
        }
        if next_api_key != before_key {
            let tail = token_tail_of(&next_api_key);
            record.set("apiKey", Value::String(next_api_key.clone()));
            record.set("tokenTail", Value::String(tail.clone()));
            changes.push(if next_api_key.is_empty() {
                "API Key → 已清除".to_string()
            } else {
                format!("API Key → 已更新（尾号 {tail}）")
            });
        }
        if changes.is_empty() {
            return Ok(changes);
        }
        // 改凭证不动顺序、不改启用状态：原地更新（与 token 回写同一条路径）
        record.set_updated_at(logging::now_ms());
        let name = record.name();
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))?;
        logging::log(
            "[Accounts]",
            &format!("🔑 自定义账号凭证已更新: {name}（{}）", changes.join("，")),
        );
        Ok(changes)
    }

    // ─── 余额查询配置（`balanceQuery`）──────────────────────────

    /// 读取一条自定义账号的**余额查询配置**（查询链路用）。
    ///
    /// **返回值可能含明文凭证，绝不进任何 HTTP 响应** —— 公开形态只有
    /// `has*` 标记与两个非密字段（见 `store_view::to_custom_public_account`），
    /// 与 `custom_credential_by_id` 是同一条纪律。
    ///
    /// 没有这条键（这一家的方案不需要凭证，或用户还没配）给 `None`。
    pub fn custom_balance_query_by_id(&self, account_id: &str) -> Option<CustomBalanceQuery> {
        let _guard = self.guard();
        let record = self.record_by_id(&_guard, account_id)?;
        Some(balance_query_of_record(&record))
    }

    /// 改一条自定义账号的余额查询配置。返回变更说明（供界面提示与日志），
    /// 没有任何变化时返回空表。
    ///
    /// ── 语义（与 `update_custom_credentials` 同一套）────────────
    ///   · 字段缺省 = 不修改；`null` 或空串 = **清除**（输入框留空是「不修改」，
    ///     清除必须由显式动作表达，见界面上的「清除」按钮）；
    ///   · 只认方案里登记的那五个字段（[`BALANCE_QUERY_FIELDS`]），其余键
    ///     400 —— 静默忽略会让用户以为存进去了；
    ///   · 非密字段原样落盘（`userId` / `username`），密字段同样落进记录
    ///     ——整行落库已经是 `enc1:` 密文（见 `sql.rs` 模块头），不再做字段级
    ///     信封（那会让读数多一层解包、也失去「整行一个信封」的统一处置）。
    ///
    /// ── 为什么换凭证要连 JWT 缓存一起清 ─────────────────────────
    /// 商汤的查询票（`sensenovaJwt`）是用账号密码换来的：密码一换，旧票代表的
    /// 还是旧账号 —— 留着它会让「改了密码」这件事看起来没生效（查询继续用旧
    /// 身份成功）。清掉之后下一次查询自然重登。
    pub fn update_custom_balance_query(
        &self,
        id: &str,
        patch: &Value,
    ) -> Result<Vec<String>, AccountStoreError> {
        let id = id.trim();
        let Some(object) = patch.as_object() else {
            return Err(AccountStoreError::bad_request("请求内容必须是 JSON 对象"));
        };
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return Err(AccountStoreError::not_found("账号不存在"));
        };
        let provider = record.provider();
        if !provider.starts_with(custom_providers::ID_PREFIX) {
            return Err(AccountStoreError::bad_request(
                "只有自定义提供商的账号有余额查询配置（balanceQuery）",
            ));
        }
        // 方案是「这一家的哪条查询链路」的事实来源：没有方案的家没有可配的
        // 凭证（预置卡的方案在创建时写进提供商记录，见 custom_providers）
        if custom_providers::balance_scheme_of(&provider).is_none() {
            return Err(AccountStoreError::bad_request(
                "该提供商没有登记余额查询方案，无法配置查询凭证",
            ));
        }
        let before = record
            .fields()
            .get("balanceQuery")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut after = before.clone();
        normalize_balance_query_patch(&mut after, object).map_err(AccountStoreError::bad_request)?;
        if after == before {
            return Ok(Vec::new());
        }
        let changes = describe_balance_query_changes(&before, &after);
        if after.is_empty() {
            record.remove("balanceQuery");
        } else {
            record.set("balanceQuery", Value::Object(after));
        }
        record.set_updated_at(logging::now_ms());
        let name = record.name();
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))?;
        logging::log(
            "[Accounts]",
            &format!("🔎 自定义账号余额查询配置已更新: {name}（{}）", changes.join("，")),
        );
        Ok(changes)
    }

    /// 回写商汤登录换来的 JWT 缓存（查询路径用；失败只影响下一次重登）。
    ///
    /// 走与凭证更新同一条落盘路径，但不产生变更日志、不动 `updatedAt` 之外的
    /// 任何字段 —— 它是一次**缓存**写入，不是用户动作。
    pub fn store_custom_balance_jwt(
        &self,
        id: &str,
        jwt: &str,
        expires_at_ms: i64,
    ) -> Result<(), AccountStoreError> {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id.trim()) else {
            return Err(AccountStoreError::not_found("账号不存在"));
        };
        let mut query = record
            .fields()
            .get("balanceQuery")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        query.insert(
            "sensenovaJwt".to_string(),
            Value::String(jwt.trim().to_string()),
        );
        query.insert(
            "sensenovaJwtExpiresAt".to_string(),
            Value::from(expires_at_ms),
        );
        record.set("balanceQuery", Value::Object(query));
        record.set_updated_at(logging::now_ms());
        let _ = self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))?;
        Ok(())
    }
}

/// 自定义账号的**余额查询配置快照**（`custom_balance_query_by_id` 的返回值）。
///
/// 字段与记录里的键一一对应（`userId` → `user_id`…）。`sensenova_jwt` 是
/// **缓存**而不是用户输入：商汤方案登录一次换来的票，过期前复用、过期后重登
/// （见 `providers::custom::balance` 的模块头）。
pub struct CustomBalanceQuery {
    /// NewAPI：New-Api-User 头的取值（新版可空）
    pub user_id: String,
    /// NewAPI：控制台访问令牌（必填）
    pub access_token: String,
    /// 商汤：控制台账号（必填）
    pub username: String,
    /// 商汤：控制台密码（必填）
    pub password: String,
    /// 智谱：bigmodel.cn 控制台令牌（必填）
    pub token: String,
    /// 商汤：缓存的登录票（空 = 还没登过）
    pub sensenova_jwt: String,
    /// 商汤：缓存票的到期时刻（毫秒，0 = 未知）
    pub sensenova_jwt_expires_at: i64,
}

/// 记录 → 余额查询快照（读侧只做容错取值，不校验合法性 —— 写入侧已归一）
fn balance_query_of_record(record: &StoredAccount) -> CustomBalanceQuery {
    let fields = record
        .fields()
        .get("balanceQuery")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let text_of = |key: &str| {
        fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    CustomBalanceQuery {
        user_id: text_of("userId"),
        access_token: text_of("accessToken"),
        username: text_of("username"),
        password: text_of("password"),
        token: text_of("token"),
        sensenova_jwt: text_of("sensenovaJwt"),
        sensenova_jwt_expires_at: fields
            .get("sensenovaJwtExpiresAt")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    }
}

/// 方案可配置的字段（顺序即报错文案与界面顺序）。
///
/// 五个字段横跨三种方案（NewAPI 用前两个、商汤用中间两个、智谱用最后一个），
/// 合成一张表而不是按方案分开：写入侧不认识方案（它只认字段），按方案建表会
/// 让「同一字段属于谁」在两处各写一遍。
pub const BALANCE_QUERY_FIELDS: &[&str] = &["userId", "accessToken", "username", "password", "token"];

/// 字段的界面/日志名（变更说明与错误文案用；键名对用户没有意义）
fn balance_query_field_label(key: &str) -> &'static str {
    match key {
        "userId" => "用户 ID",
        "accessToken" => "访问令牌",
        "username" => "账号",
        "password" => "密码",
        "token" => "控制台令牌",
        _ => "查询凭证",
    }
}

/// 换凭证时要连缓存票一起清的字段（见 `update_custom_balance_query` 的说明）
const BALANCE_QUERY_IDENTITY_FIELDS: &[&str] = &["accessToken", "username", "password", "token"];
/// 单个字段的长度上限（防御：粘贴进一整段文档时不落盘；与 apiKey 同量级）
const MAX_BALANCE_QUERY_FIELD_CHARS: usize = 8192;

/// 把一份补丁套到余额查询配置上（`add_custom_account` 与
/// `update_custom_balance_query` **共用同一套语义**：缺省不动、null / 空串清除）。
fn normalize_balance_query_patch(
    query: &mut Map<String, Value>,
    patch: &Map<String, Value>,
) -> Result<(), String> {
    let mut identity_changed = false;
    for key in BALANCE_QUERY_FIELDS {
        let Some(value) = patch.get(*key) else { continue };
        let next = match value {
            Value::Null => String::new(),
            Value::String(text) => text.trim().to_string(),
            _ => {
                return Err(format!(
                    "balanceQuery.{} 必须是字符串（传 null 表示清除）",
                    balance_query_field_label(key)
                ))
            }
        };
        if next.chars().count() > MAX_BALANCE_QUERY_FIELD_CHARS {
            return Err(format!(
                "balanceQuery.{} 过长（最多 {MAX_BALANCE_QUERY_FIELD_CHARS} 个字符）",
                balance_query_field_label(key)
            ));
        }
        let before = query.get(*key).and_then(Value::as_str).unwrap_or("");
        if before == next {
            continue;
        }
        if BALANCE_QUERY_IDENTITY_FIELDS.contains(key) {
            identity_changed = true;
        }
        if next.is_empty() {
            query.remove(*key);
        } else {
            query.insert((*key).to_string(), Value::String(next));
        }
    }
    // 身份字段变过 → 缓存的登录票作废（见调用点的说明）
    if identity_changed {
        query.remove("sensenovaJwt");
        query.remove("sensenovaJwtExpiresAt");
    }
    Ok(())
}

/// 两次快照的差异 → 变更说明（**只说字段名与动作，不带值**）。
/// 缓存票（sensenovaJwt）不进说明：它不是用户动作。
fn describe_balance_query_changes(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Vec<String> {
    let mut changes: Vec<String> = Vec::new();
    for key in BALANCE_QUERY_FIELDS {
        let old = before.get(*key).and_then(Value::as_str).unwrap_or("");
        let new = after.get(*key).and_then(Value::as_str).unwrap_or("");
        if old == new {
            continue;
        }
        let label = balance_query_field_label(key);
        changes.push(match (old.is_empty(), new.is_empty()) {
            (true, false) => format!("查询凭证 → {label} 已配置"),
            (false, true) => format!("查询凭证 → {label} 已清除"),
            _ => format!("查询凭证 → {label} 已更新"),
        });
    }
    changes
}

/// 自定义账号的**内部凭证快照**（`custom_credential_by_id` /
/// `first_custom_credential` 的返回值）。
///
/// 与公开形态（`to_custom_public_account`）的字段差别只有两个：
/// `apiKey`（明文，出网鉴权用）与代理的**已解析形态**（`ResolvedProxy`，
/// 直接可交给 egress；解析失败按直连兜底 —— 公开形态里那是给用户看的
/// 「解析失败」气泡，这里是转发可执行的出口）。
pub struct CustomCredential {
    /// 账号 id
    pub account_id: String,
    /// 备注名（日志用）
    pub account_name: String,
    /// 上游 API Key（`add_custom_account` 落盘的凭证；**可空** —— 无需鉴权的
    /// 账号没有 key，转发与拉清单都不发鉴权头）
    pub api_key: String,
    /// 是否显式声明「该上游无需鉴权」（见 `state::no_auth`）
    pub no_auth: bool,
    /// 账号上的 baseUrl **覆盖项**（None = 未覆盖，转发回落提供商的 baseUrl）
    pub base_url_override: Option<String>,
    /// 出网代理（None = 未配置 / 解析失败 → 直连）
    pub proxy: Option<ResolvedProxy>,
}

/// 一条记录 → 凭证快照（两个入口共用一份取数口径，不会各漂一份）
fn credential_of_record(record: &StoredAccount) -> CustomCredential {
    let fields = record.fields();
    // 轮询档取下一个出口：凭证快照的消费方（余额查询、拉清单）都是**真实出网**，
    // 与转发同一条口径；展示那一路（公开形态）走 `describe_account_proxy`，
    // 它不动游标（见 `core::proxies` 的两个解析入口）。
    let proxy = match resolve_account_proxy_rotating(Some(&record.proxy()), record.id()) {
        Some(resolution) => resolution.resolved().cloned(),
        // 无代理配置 → 直连（与「解析失败回退直连」在出网行为上等价，
        // 区别只在日志：这里没有请求上下文可挂 notice，静默直连即可）
        None => None,
    };
    CustomCredential {
        account_id: record.id().to_string(),
        account_name: record.name(),
        api_key: fields
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        no_auth: record.no_auth(),
        base_url_override: fields
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|text| !text.is_empty()),
        proxy,
    }
}

/// 账号 id：`custom-acct-` + 12 位 hex。两条派生链，按凭证形态分派：
///
///   - `apiKey` 非空 → 它的 SHA-256 前 12 位：**同一把 key 重复添加合并更新
///     同一条记录**（同 key 就是同一条上游线路，堆两条没有意义；用户粘错
///     重试也不会堆出重复账号，见模块头）；
///   - 没有 key（显式声明无需鉴权，或凭证还没配）→ 随机 12 位（`getrandom`）：
///     **每次添加都是一条新账号**。同一家允许并存多条无需鉴权账号是刻意的
///     （各自可配不同的出网代理、备注名，转发按优先级轮着走），它们没有任何
///     可派生 id 的稳定标识 —— 随机段保 id 唯一，不把「重复添加」误当合并。
///
/// 随机源失败时如实报错（本函数返回 Err）：本模块硬约束「绝不 panic」，
/// 宁可让用户重试一次，也不落一条 id 可靠性没保证的记录。
fn account_id_for(api_key: &str) -> Result<String, String> {
    if !api_key.is_empty() {
        return Ok(format!("{CUSTOM_ACCOUNT_ID_PREFIX}{}", short_hash(api_key)));
    }
    let mut bytes = [0u8; 6];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| "无法生成安全的随机账号 id（系统随机源不可用），请重试".to_string())?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{CUSTOM_ACCOUNT_ID_PREFIX}{hex}"))
}

/// SHA-256 前 12 位小写 hex（账号 id 的稳定派生段）
fn short_hash(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    format!("{digest:x}").chars().take(12).collect()
}
