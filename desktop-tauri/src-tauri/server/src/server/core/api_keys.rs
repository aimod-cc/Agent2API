//! 网关 API Key 列表（config.json 的 `apiKeys` 字段）。
//!
//! 形状：`apiKeys: [{id, name, key, enabled, createdAt, allowedProviders, allowedModels}]`。
//! 多把 Key 任一命中即通过；一把启用的都没有 → 不鉴权（网关只监听 127.0.0.1，
//! 与旧的「未配置 apiKey」语义一致）。
//!
//! ── 与旧字段 `apiKey` 的关系 ────────────────────────────────
//! 1.x 只有一把 Key（`apiKey` 字符串）。读侧兼容：`apiKeys` 缺失而 `apiKey` 存在时，
//! 把它当成一条 id 为 `legacy` 的记录展示 / 校验；写侧一旦动过列表就落成 `apiKeys`
//! 并删掉 `apiKey`，从此只有一份真相。环境变量 `WORKBUDDY_PROXY_API_KEY` 仍然
//! 算一把额外的启用 Key（启动注入语义不变）。
//! 该字段的值从本轮改造起也是 `enc1:` 信封（启动期原地加密迁移见
//! `core::legacy_api_key`，它只换值、字段名与形态都不动），所以读它必须走
//! `decode_key` —— 与数组里的 `key` 同一条口径。
//!
//! ── 每把 Key 的**可用提供商 / 可用模型**白名单（R9，参考 OmniProxy）──
//! 两个字段，语义与 OmniProxy 的 `allowed_models` / `allowed_provider_ids`
//! **逐条对齐**（那一份见 `server/dist/utils/apiKeyRestrictions.js`）：
//!   - `allowedProviders: string[]`：允许路由到的 **provider id**
//!     （`workbuddy` / `raccoon` / `catpaw` / `autoclaw` / `qoder` /
//!     `cline-free` / `cline-pass`）。OmniProxy 存的是**数字** provider id
//!     （它的上游表主键），我们这边 provider 的身份本来就是字符串
//!     （见 `providers::PROVIDERS` 的说明：「字符串本身就是契约」），
//!     所以这里用字符串 —— 不要为了「对齐」把 id 数字化。
//!   - `allowedModels: string[]`：允许请求的**对外模型名**（下游请求体里的
//!     那个名字，含映射的 alias）。
//!
//! **空数组 / 字段缺失 = 不限制**：这与 OmniProxy 的「`NULL` = 不限制」等价
//! （我们这边 `apiKeys` 是 JSON 数组，天然能表达空列表，不需要额外造一个 null
//! 语义）。两个白名单**同时生效时按交集**（模型要在白名单里，且实际承载的家也要
//! 在白名单里），与 OmniProxy 的注释一致。
//!
//! ── 一条硬不变量：升级绝不能让已有 Key 失效 ────────────────────
//! 旧记录没有这两个键，`from_value` 把它们读成空 `Vec`（= 不限制）；
//! **读取路径不许因为字段缺失而丢掉整条记录**（那等于把用户的 Key 静默作废，
//! 所有客户端立刻 401）。所以这两个字段的解析只做「取不到就用空」，
//! 不做任何校验性拒绝 —— 连 `allowedProviders` 里出现未知 provider id 也照收
//! （可能来自新版本写入、被旧版本读到；真正的校验点在管理 API 的写入侧）。
//! 写回时**总是**写出这两个键（空也得是 `[]`）：配置形状稳定，
//! 用户手改配置时也看得出这两个位置存在。
//!
//! ── `key` 是密文（本切片的整字段加密）───────────────────────────
//! 落盘的 `key`（kv 表的 `apiKeys` 键里那份 JSON 数组的 `key` 字段）不再是
//! `sk-a2a-…` 明文，而是 `core::crypto` 的 `enc1:` 信封（AES-256-GCM）。
//! 密钥域是 [`SecretScope::Accounts`] —— 与账号表**共用** `account.key` /
//! `AGENT2API_ACCOUNT_KEY`，**刻意不新增域**：网关 Key 与账号同属「本机凭证」，
//! 没有互相保密的必要，而多一个域就多一份要跟着备份/迁移的密钥文件、多一种
//! 「拷了库没拷密钥」的故障形态。数组结构与其余字段（id / name / enabled /
//! createdAt / 两个白名单）**保持明文 JSON**：加密只覆盖「值本身就是凭证」的
//! 那一个字段 —— 其余字段本来就要给人看（配置形状、界面展示）。
//!
//! 三条边界，改这个文件时不要破：
//!   1. **内存与对外形态是明文**。`ApiKeyEntry::key` 在内存里永远是明文，
//!      `public_json()` 也照常返回明文（桌面端本地管理界面要能复制给客户端）。
//!      加密只发生在 `to_value()` 这**一个**落盘点 —— 与账号表的 `encode_data`
//!      同一取向（见 `core::account_store::sql` 的模块头第 1 条不变量）。
//!   2. **落盘失败绝不静默降级成明文**（fail closed）。`to_value` 返回 `Result`，
//!      加密失败 → 整次保存失败并上抛（见 `save`）：加密的全部意义是「库被单独
//!      拿走时看不到凭证」，而失败恰恰发生在保护最该生效的异常环境（密钥文件建
//!      不出来等）——静默落明文等于「越该保护越失效」，且用户从界面上完全看不出。
//!   3. **解不开的 `key` 不丢记录、不改写、不放行**（下面那条硬不变量的补充，
//!      这是它新增的一层）。密钥文件被删/被换时 `crypto::open` 必然失败，此时
//!      处理方式见 `from_value` 的详细论证：记录保留、原始密文**原样**留在 `key`
//!      里（不置空、不再加密）、进程内只记一条日志。

use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::crypto::{self, SecretScope};
use crate::server::logging;

/// 现役形态：`apiKeys` 数组（本模块的读写主体）
pub const KEY_API_KEYS: &str = "apiKeys";
/// 旧形态：顶层单 Key 字段。只读兼容在本模块（`entries_from` 的兼容分支），
/// **写入侧两处** —— 面板保存 `config::set_api_key`（收明文、落 `enc1:` 信封）
/// 与启动期原地加密迁移（`core::legacy_api_key`）；清除它的两条路径也都在
/// config 层（`set_api_key(None)`、`replace_api_keys` 切到 `apiKeys` 形态时）。
/// 掩码取值的口径见 `RuntimeConfig::masked_api_key`（快照存磁盘原值，解密只在
/// 那个唯一读者里做）。
///
/// 常量与 `KEY_API_KEYS` 并排放着，是为了让这个字段名只有一个来源：上面这些
/// 读写点（含 `config::update_raw_field` 的快照同步判定）用的都是本常量，
/// 各写一遍字面量迟早漏改一处；仅剩 `config::build` 取快照原值那一处还是
/// 旧字面量。
pub const KEY_API_KEY: &str = "apiKey";
const LEGACY_ID: &str = "legacy";
const LEGACY_NAME: &str = "默认 Key";
/// 与旧版一致的最小长度
pub const MIN_KEY_LENGTH: usize = 8;

/// 一条 Key 记录
#[derive(Clone, Debug)]
pub struct ApiKeyEntry {
    pub id: String,
    pub name: String,
    pub key: String,
    pub enabled: bool,
    pub created_at: i64,
    /// 可用提供商白名单（provider id 字符串）；**空 = 不限制**（见模块头）
    pub allowed_providers: Vec<String>,
    /// 可用模型白名单（**对外模型名**，含映射 alias）；**空 = 不限制**
    pub allowed_models: Vec<String>,
    /// 这条记录的 `key` **解不开**（密钥文件被删/被换），处于降级态。
    ///
    /// 为 true 时 `key` 里放的是**原样的密文信封**（不是明文）：
    ///   - 不能拿去和客户端带来的明文比对（密文永远不相等 → 这把 Key 降级期
    ///     只是暂时不可用，**不会误放行**）；
    ///   - 不能再加密一次（会把信封当成明文套第二层 → 即便用户找回密钥文件，
    ///     真 Key 也永远解不回来，见 `to_value`）。
    ///
    /// 私有字段：外部只该通过 `key` 读明文，这个标记是模块内部的「这条记录
    /// 处于降级态」事实（唯一的消费者是 `to_value`）。
    key_unreadable: bool,
}

/// 从 JSON 里读一个「字符串数组」白名单（缺失 / 类型不对 / 空 → 空数组）。
///
/// ── 为什么一律容错、绝不拒绝整条记录 ────────────────────────
/// 这两个字段是**限制**：读不出来时正确的兜底是「不限制」（放行），
/// 而不是「拒绝」或「丢掉这条 Key」。丢掉一条旧记录 = 用户的 Key 凭空失效、
/// 所有客户端立刻 401；把它读成「限制全部」= 所有请求 403。两种都不能接受。
/// 所以这里只做「能读多少读多少」：非数组给空、元素非字符串跳过、
/// 去空白、大小写去重（模型名比对忽略大小写，见 `allows_model`）。
///
/// 去重保留首次出现的拼写：界面上展示的是用户当初填的那个形态。
fn string_list(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if out.iter().any(|known| known.eq_ignore_ascii_case(text)) {
            continue;
        }
        out.push(text.to_string());
    }
    out
}

/// 白名单列表 → 落盘形态（**空也写成 `[]`**，见模块头）
fn list_value(list: &[String]) -> Value {
    Value::Array(list.iter().map(|item| Value::String(item.clone())).collect())
}

/// 「有一把网关 Key 解不开」的**进程级一次性**告警。
///
/// ── 为什么必须限流成一条 ────────────────────────────────────
/// `from_value` 在**鉴权热路径**上（每个请求经 `active_keys_from` 走一遍，
/// 再由 `entry_for_key_from` 走第二遍），而 `logging::log` 是要写库的 ——
/// 逐次打日志会在降级期把日志表刷爆，把真正有用的请求日志全挤掉。
/// 这个事件本身只有一件（密钥文件丢了/换了），看到一条就够定位，所以用
/// `AtomicBool::swap` 做闸门：并发下是谁抢到那条日志不重要，重要的是**只有一条**。
fn warn_unreadable_once(id: &str, reason: &str) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    logging::log(
        "[Keys]",
        &format!(
            "⚠️  网关 Key「{id}」解密失败（{reason}）：记录与原始密文都已保留，这把 Key 暂时不可用（不会误放行）。恢复密钥文件后重启即可，或直接在管理页重存这把 Key"
        ),
    );
}

/// 落盘的 `key` 字段 → 内存里的明文，外加「这条记录处于降级态吗」。
///
/// 三种输入、两种输出：
///   - **历史明文**（没有 `enc1:` 前缀）：原样返回，`false` —— 升级兼容的读侧
///     安全网，迁移（见 `encrypt_legacy_keys`）会去收敛它，但读侧永远认它；
///   - **信封且解得开**：返回明文，`false`；
///   - **信封但解不开**（密钥文件被删/被换/格式坏）：**原样返回信封** + `true`，
///     并（进程内第一次）记一条日志 —— 为什么不能丢记录、不能置空、不能改写，
///     完整论证见 `from_value`。
///
/// 判「是不是信封」只用 `crypto::is_envelope`（口径的唯一来源在 `core::crypto`），
/// 不在这里自己 `starts_with("enc1:")` —— 将来换格式（`enc2:`）才不会漏改一处。
fn decode_key(raw: &str, id: &str) -> (String, bool) {
    if !crypto::is_envelope(raw) {
        return (raw.to_string(), false);
    }
    match crypto::open(raw, SecretScope::Accounts) {
        // `open` 已经校验过 UTF-8（见 `core::crypto` 的模块头「明文闸门」），
        // 这里只是把字节收成 String；真出现非法字节也按「解不开」处理
        // （保留原样），绝不把一段乱码当成明文 Key 用
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => (text.to_string(), false),
            Err(_) => {
                warn_unreadable_once(id, "解密结果不是合法 UTF-8 文本");
                (raw.to_string(), true)
            }
        },
        Err(error) => {
            warn_unreadable_once(id, &error);
            (raw.to_string(), true)
        }
    }
}

impl ApiKeyEntry {
    /// 从一条 JSON 记录解析出条目（**密文与历史明文都能读**）。
    ///
    /// ── 解密失败为什么必须保留记录（本文件最关键的一条补充）────────
    /// 密钥文件（`account.key`）被删/被换时 `crypto::open` 必然失败。若像
    /// 「字段缺失 / 类型不对」那样返回 None，这条记录会被 `entries_from` 的
    /// `filter_map` 丢掉 —— 用户的 Key 从配置里凭空消失：界面上看不到、管理接口
    /// 也删不掉，而这**不是用户的任何操作造成的**（正是模块头那条硬不变量要挡的
    /// 灾难，只是触发原因从「字段缺失」换成了「钥匙丢了」）。所以：
    ///   - 记录照常构造，只把 `key` 留在**原样的密文**上并置 `key_unreadable`；
    ///   - **不置空**：空串会在鉴权比对里命中「带空 token 的请求」
    ///     （`Bearer ` / 空的 `x-api-key`），等于把网关放开 —— 比丢掉记录更糟；
    ///   - 密文也**不会误放行**（客户端不可能带得出密文），实际效果是
    ///     「这把 Key 在降级期暂时不可用」，这是能接受、也最容易被用户理解的结果
    ///     （恢复密钥文件即自愈）；
    ///   - 落盘时原样回写（`to_value`），磁盘上的密文一个字节都不动：
    ///     用户找回密钥文件后，这条记录**照旧能解出真 Key**。
    ///
    /// 兼容历史明文：没有 `enc1:` 前缀的 `key` 就是明文，原样使用（见 `decode_key`）。
    fn from_value(value: &Value) -> Option<Self> {
        let raw = value.get("key")?.as_str()?.trim();
        if raw.is_empty() {
            return None;
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)?;
        let (key, key_unreadable) = decode_key(raw, &id);
        Some(Self {
            id,
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            key,
            enabled: !matches!(value.get("enabled"), Some(Value::Bool(false))),
            created_at: value.get("createdAt").and_then(Value::as_i64).unwrap_or(0),
            allowed_providers: string_list(value.get("allowedProviders")),
            allowed_models: string_list(value.get("allowedModels")),
            key_unreadable,
        })
    }

    /// 落盘形态：`key` 加密成 `enc1:` 信封，其余字段保持明文 JSON（见模块头）。
    ///
    /// ── 为什么返回 `Result` 而不是降级写明文（fail closed）────────
    /// 与账号表 `encode_data` 同一论证（见 `core::account_store::sql` 的模块头）：
    /// 加密的全部意义是「库文件被单独拿走时看不到凭证」，而失败恰恰发生在保护最该
    /// 生效的异常环境（`config_dir` 不可写、密钥文件建不出来等）。此时静默落明文
    /// 就等于「越该保护越失效」，用户还从界面上看不出任何区别。所以失败即让
    /// **整次保存**失败并上抛，调用方（`save` → 管理 API）把错误报出来。
    ///
    /// ── 降级态（`key_unreadable`）为什么原样回写 ────────────────
    /// 那条记录的 `key` 里是解不开的**原始信封**。若照常再加密一次，落盘的就成了
    /// 「信封的信封」：即便用户之后找回密钥文件，解出来的也只是中间那层密文，
    /// 真 Key 永远回不来 —— 一次静默的、不可逆的数据损坏。原样回写让磁盘上的密文
    /// 保持不变，恢复密钥后下一次读取就能解出明文。
    fn to_value(&self) -> Result<Value, String> {
        let key = if self.key_unreadable {
            self.key.clone()
        } else {
            crypto::seal(self.key.as_bytes(), SecretScope::Accounts)?
        };
        Ok(json!({
            "id": self.id,
            "name": self.name,
            "key": key,
            "enabled": self.enabled,
            "createdAt": self.created_at,
            "allowedProviders": list_value(&self.allowed_providers),
            "allowedModels": list_value(&self.allowed_models),
        }))
    }

    /// 管理接口的输出形态（含明文 key：桌面端本地管理界面要能复制给客户端）。
    ///
    /// 唯一的例外是降级态：那时 `key` 里是解不开的密文（见 `from_value`），
    /// 这里照样原样输出 —— 磁盘上是什么、界面上看到什么，用户在密钥丢失时
    /// 看到的就不是一把「看起来正常却复制出去也没用」的假 Key。
    pub fn public_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "key": self.key,
            "masked": mask(&self.key),
            "enabled": self.enabled,
            "createdAt": self.created_at,
            "allowedProviders": list_value(&self.allowed_providers),
            "allowedModels": list_value(&self.allowed_models),
        })
    }
}

// ── 白名单判定住在 `core::key_scope`，这里**不再**各写一份 ──────────
// 本模块只负责「存取」：把两个列表读出来、写回去。判定（忽略大小写、空 =
// 不限制、两个维度各自独立）全在 `key_scope::KeyScope` 上，那里是唯一的
// 实现点 —— 曾经在这里也写过一对 `allows_model` / `allows_provider`，
// 编译器的 dead_code 警告指出了问题：两处实现同一套规则，改一处忘一处
// 就会让「管理页显示的限制」与「转发时实际执行的限制」不一致，
// 而那种分叉在界面上完全看不出来（都是 200，只是拒绝/放行的边界差了几个字）。

/// 掩码：前 6 后 4（沿用旧版口径）
pub fn mask(key: &str) -> String {
    let head: String = key.chars().take(6).collect();
    let total = key.chars().count();
    let tail: String = key.chars().skip(total.saturating_sub(4)).collect();
    format!("{head}...{tail}")
}

/// 从配置底稿解析全部记录（含旧字段兼容）
pub fn entries_from(raw: &Map<String, Value>) -> Vec<ApiKeyEntry> {
    if let Some(Value::Array(items)) = raw.get(KEY_API_KEYS) {
        return items.iter().filter_map(ApiKeyEntry::from_value).collect();
    }
    legacy_entry(raw).into_iter().collect()
}

/// 顶层旧字段 `apiKey` → 一条 `legacy` 记录（`None` = 没配 / 空值）。
///
/// ── 为什么要走 `decode_key` ──────────────────────────────────
/// `apiKey` 从本轮改造起也与数组里的 `key` 一样是 `enc1:` 信封（启动期的原地
/// 加密迁移见 `core::legacy_api_key`），所以取值口径必须和 `from_value` 合并成
/// 一条：历史明文原样用、信封解密、解不开按降级态处置。判「是不是信封」的
/// 唯一来源仍是 `crypto::is_envelope`（在 `decode_key` 里），不在这里另写一份。
///
/// 解密失败时**保留记录、不置空、不丢**的三条理由与 `from_value` 逐字相同
/// （见那里的论证），这里只把「旧字段只有一条记录」这个差别点出：丢掉它意味着
/// 一把启用的 Key 都没有 → 中间件判免鉴权（全放行），比留着一条暂时用不了的
/// 记录糟得多。
///
/// 空值校验发生在解密**之前**（与 `from_value` 同一口径）：存的是非空信封、
/// 解密结果却为空这种极端情形（只可能来自手改数据）也照常产出记录 ——
/// 判断依据与上面同一条（宁可留一条不可用的记录，也不让列表空掉）。
fn legacy_entry(raw: &Map<String, Value>) -> Option<ApiKeyEntry> {
    let stored = raw.get(KEY_API_KEY)?.as_str()?.trim();
    if stored.is_empty() {
        return None;
    }
    let (key, key_unreadable) = decode_key(stored, LEGACY_ID);
    Some(ApiKeyEntry {
        id: LEGACY_ID.to_string(),
        name: LEGACY_NAME.to_string(),
        key,
        enabled: true,
        created_at: 0,
        // 旧字段形态的 Key 从来没有白名单概念 —— 空 = 不限制，
        // 与它升级前的行为（放行一切）逐字一致
        allowed_providers: Vec::new(),
        allowed_models: Vec::new(),
        key_unreadable,
    })
}

/// 某个位置上**存着**的旧字段值 → 明文（界面掩码用）。
///
/// 调用点只有 `config::masked_api_key`：它手里是内存快照里的原始值（磁盘上是什么
/// 就是什么，可能是 `enc1:` 信封），掩码前必须先解出来 —— 否则界面上会显示一封
/// `enc1:b...xxxx`，用户以为 Key 被换掉了。
/// 与 `legacy_entry` 共用 `decode_key`，于是三种输入的口径只有一份：历史明文原样、
/// 信封解开、**解不开则返回密文本身**（降级期的显示口径：磁盘上是什么、界面就显示
/// 什么，与 `public_json` 的既有取向一致）。
///
/// 为什么不在快照构建期（`config::build`）做这件事：那里跑在 `config::init` 里，
/// 日志库还没装，「密钥文件丢了」那条一次性降级告警会落在控制台上（桌面形态下
/// 用户看不到）—— 细节见 `RuntimeConfig::masked_api_key` 的说明。
pub fn stored_plaintext(stored: &str) -> String {
    decode_key(stored.trim(), LEGACY_ID).0
}

/// 当前全部记录
pub fn list() -> Vec<ApiKeyEntry> {
    entries_from(config::current().raw())
}

/// 当前**启用**的明文 Key（鉴权中间件用）；空 = 免鉴权
///
/// ── 降级态（`key_unreadable`）的记录**不要**在这里筛掉 ───────────
/// 那些记录带着**密文**留在列表里（见 `from_value`）。筛掉它们的后果是：
/// 密钥文件一丢、全表都解不开 → 列表变空 → 中间件判「免鉴权模式」直接放行
/// （见模块头），等于一次密钥文件丢失就把网关对全网放开。留在列表里则相反：
/// 密文永远不可能与客户端带来的明文相等，实际效果是「这些 Key 暂时不可用」，
/// 正是降级期该有的行为。
pub fn active_keys_from(raw: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = entries_from(raw)
        .into_iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.key)
        .collect();
    if let Some(env) = crate::server::config::parse::env_api_key_value() {
        if !keys.contains(&env) {
            keys.push(env);
        }
    }
    keys
}

/// 按明文 Key 找出**当前启用**的那条记录（R9：把「命中的是哪把 Key」交给 handler）。
///
/// ── 为什么要有这个函数 ──────────────────────────────────────
/// 鉴权中间件（`http::require_api_key`）原先只需要回答「这个请求能不能进」，
/// 于是它只比对一串明文 Key（`active_keys_from`）。可用提供商 / 可用模型
/// 是**逐把 Key** 的限制，handler 因此必须知道「这次用的是哪一把」——
/// 本函数就是那个反向查找，中间件命中后调它一次，把记录塞进请求扩展
/// （见 `http::require_api_key` 的实现与说明）。
///
/// ── 什么情况下返回 None（且必须放行，不是拒绝）────────────────
///   - **免鉴权模式**：一把启用的 Key 都没有 —— 请求根本没有「命中的 Key」，
///     限制自然不存在（不限制）。中间件在那个分支根本不会调本函数。
///   - **环境变量 Key**（`WORKBUDDY_PROXY_API_KEY`）：它不是一个列表记录，
///     没有地方挂白名单。**按不限制处理**（`None`），这是启动注入旁路的既有
///     语义 —— 它是给脚本/CI 的通用口令，加一层「无法配置的限制」只会让人
///     误以为它被限制了。
///   - Key 在两次读盘之间被改/删：同样按不限制（宁可放行，不可因为在管理页
///     改了一下名称就把正在跑的客户端全挡掉）。
///
/// ── 为什么要传 `raw` 而不是自己调 `list()` ───────────────────
/// `config::current()` 会**克隆整份配置**（含全部 raw 字段）。鉴权是每个请求
/// 都要走的热路径，而调用方手里已经有一份刚取到的快照 —— 让它再调一次
/// `list()` 就是为同一件事多克隆一次配置。这里收快照、调用方复用，
/// 顺带让「判定用的是哪一份快照」在调用点一眼可见（不会出现「查 Key 用一份、
/// 查白名单用另一份」的窗口期）。
pub fn entry_for_key_from(raw: &Map<String, Value>, key: &str) -> Option<ApiKeyEntry> {
    let env = crate::server::config::parse::env_api_key_value();
    if env.as_deref() == Some(key) {
        return None;
    }
    entries_from(raw)
        .into_iter()
        .find(|entry| entry.enabled && entry.key == key)
}

/// 随机生成一把 Key：`sk-a2a-` + 32 位十六进制。
///
/// 项目刻意不引入随机数 crate（见 Cargo.toml 对 getrandom 的说明），这里用
/// sha256(纳秒时间戳 + 进程 id + 单调计数 + 栈地址) 取前 16 字节：本地工具的
/// 访问口令，防的是「猜到」而不是密码学攻击，这个熵源足够。
pub fn generate() -> String {
    use sha2::{Digest, Sha256};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = 0u8;
    let address = &probe as *const u8 as usize;
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    hasher.update(address.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("sk-a2a-{hex}")
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把整份列表写回配置（同时删掉旧字段 `apiKey`）。
///
/// ── 为什么逐条 `?` 上抛（fail closed）─────────────────────────
/// 任一条的 `to_value` 加密失败 → **整个列表一条都不写**：`replace_api_keys`
/// 根本不会被调到，磁盘与内存快照都保持原样，调用方把「保存失败」报给用户。
/// 绝不会出现「保存成功但某把 Key 是明文落盘」这种界面看不出来的降级。
///
/// ── 落库失败（库不可用）为什么不上报 ──────────────────────────
/// `config::replace_api_keys` 返回 false 时内部已经记过日志
/// （见 `config::save_raw`），且配置层刻意的语义是「写失败只影响重启后还在不在，
/// 不该让正在跑的请求或被改的配置报错」（见 `config::update` 的说明）——
/// 这里沿用，不把它升级成一次 API 报错。加密失败与落库失败的区别在于：
/// 前者会留下**形态错误**的数据（明文凭证），后者只是没落到盘上。
fn save(entries: &[ApiKeyEntry]) -> Result<(), String> {
    let mut list: Vec<Value> = Vec::with_capacity(entries.len());
    for entry in entries {
        list.push(entry.to_value()?);
    }
    let _ = config::replace_api_keys(Value::Array(list));
    Ok(())
}

/// `apiKeys` 数组里**仍是明文**的 `key` 条数（0 = 这份配置不需要迁移）。
///
/// ── 为什么要在原始 JSON 上数，而不是在解析结果上数 ─────────────
/// `from_value` 出来的 `key` **两种情况都是明文**（本来就是明文 / 密文被解开），
/// 从解析结果里分不出谁需要迁移 —— 只有还没被 `decode_key` 碰过的原始值上才看得出
/// 「是不是信封」。顺便给日志一个准确条数（而不是「整张表都被回写了一遍」）。
///
/// 判据与 `from_value` 接受的记录口径一致（`key` 与 `id` 都是非空字符串）：
/// 会被 `filter_map` 丢掉的坏条目本来就不会被回写，不该算进迁移条数。
/// 旧字段 `apiKey`（单 Key 兼容形态）**不算** —— 它不在 `apiKeys` 数组里，
/// 本函数只数数组里的明文档数（旧字段的原地加密单独做，见 `core::legacy_api_key`）。
fn plaintext_key_count(raw: &Map<String, Value>) -> usize {
    let Some(Value::Array(items)) = raw.get(KEY_API_KEYS) else {
        return 0;
    };
    items
        .iter()
        .filter(|item| {
            let key = item.get("key").and_then(Value::as_str).map(str::trim);
            let id = item.get("id").and_then(Value::as_str).map(str::trim);
            matches!(
                (key, id),
                (Some(key), Some(id))
                    if !key.is_empty() && !id.is_empty() && !crypto::is_envelope(key)
            )
        })
        .count()
}

/// 把**仍是明文**的 Key 就地加密回写（启动期一次性迁移，幂等，可重复调用）。
///
/// ── 它补的是什么 ────────────────────────────────────────────
/// 本轮改造起落盘的 `key` 是 `enc1:` 密文，但**升级前已经写进配置的记录**仍是
/// 明文（`from_value` 兼容明文，所以那些 Key 照常可用 —— 那是过渡期的安全网、
/// 不是最终形态：只要还有明文，拷走库文件就带走网关口令）。本函数在启动时把过渡态
/// 收敛掉，与账号表的 `encrypt_legacy_rows` 同构（判据、幂等、失败语义都对齐）。
///
/// ── 幂等判据：数据自证 ──────────────────────────────────────
/// 判据是「`apiKeys` 里还有没有不以 `enc1:` 开头的 `key`」，**不用任何 kv 标记**：
/// 加密是可重复的纯变换，迁完自然就没有明文了，再跑一遍在第一段就早退、
/// 连配置的快照都不必回写。加标记键反而多一份「这条跑过没有」的事实来源。
///
/// ── 为什么走 `save`（整体回写）而不是只改那几条 ────────────────
/// `apiKeys` 是**一个** kv 键里的一整份 JSON 数组，本来就没有「按条更新」的余地 ——
/// 回写必然重写整个值，其余条目只是被重新 seal 一遍（nonce 随机、密文不同但明文
/// 相同），语义上没有任何变化（与 `config::replace_api_keys` 的既有做法一致）。
///
/// ── 失败不阻断启动 ──────────────────────────────────────────
/// 密钥不可用 / 库不可用时 `save` 会失败：这里只记一条日志并返回 0 ——
/// 读侧兼容明文，本次运行 Key 照常可用，下次启动重试。
///
/// ── 旧字段 `apiKey` 为什么不在这里 ──────────────────────────
/// 它不在 `apiKeys` 数组里，也就不会出现在本函数「整份数组回写」的范围里。
/// 顺手把它搬进 `apiKeys` 是**不能做**的：那会提前触发「写侧一旦动过列表才落成
/// `apiKeys` 并删掉 `apiKey`」这条既有语义（用户没动过配置，形态却变了）。
/// 它的收敛方式是同族的另一条：**原地加密**（字段名与形态不变，只把值换成信封），
/// 见 `core::legacy_api_key::encrypt_legacy_api_key` —— 与本函数并列，由
/// `account_bootstrap::run` 紧邻调用。
pub fn encrypt_legacy_keys() -> usize {
    // 一次取快照、两处共用（数明文条数 + 解析记录）：配置快照本来就是克隆，
    // 不必为同一份 raw 克隆两遍（与鉴权热路径同一条纪律，这里只是顺带对齐）
    let raw = config::current().raw().clone();
    let pending = plaintext_key_count(&raw);
    if pending == 0 {
        // 全新库 / 已迁过的库走这条早退路径：一个字节都不写
        return 0;
    }
    let entries = entries_from(&raw);
    if let Err(error) = save(&entries) {
        logging::log(
            "[Keys]",
            &format!(
                "⚠️  网关 Key 加密未完成（下次启动会自动重试，本次 Key 照常可用）: {error}"
            ),
        );
        return 0;
    }
    logging::log("[Keys]", &format!("🔒 已加密 {pending} 把网关 Key"));
    pending
}

/// 新增：`key` 为空则自动生成。返回新记录；名称重复不限制，key 重复拒绝。
///
/// 两个白名单随记录一起建（空 = 不限制）。**不在这一层校验 provider id**
/// 是否在注册表里：那是管理 API（写入侧）的职责 —— 本模块是「存取 + 判定」，
/// 校验集中在入口可以让「哪些值算合法」只有一处定义，而读侧永远宽容
/// （见模块头那条硬不变量）。
pub fn add(
    name: &str,
    key: Option<&str>,
    allowed_providers: Vec<String>,
    allowed_models: Vec<String>,
) -> Result<ApiKeyEntry, String> {
    let mut entries = list();
    let key = match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(given) => {
            if given.chars().count() < MIN_KEY_LENGTH {
                return Err(format!("API Key 至少需要 {MIN_KEY_LENGTH} 个字符"));
            }
            given.to_string()
        }
        None => generate(),
    };
    if entries.iter().any(|entry| entry.key == key) {
        return Err("这把 Key 已经存在".to_string());
    }
    let created_at = now_millis();
    let entry = ApiKeyEntry {
        id: format!("k{created_at:x}{}", entries.len()),
        name: name.trim().to_string(),
        key,
        enabled: true,
        created_at,
        allowed_providers,
        allowed_models,
        // 刚生成的 Key 当然是明文可读的，与降级态无关
        key_unreadable: false,
    };
    entries.push(entry.clone());
    // 加密失败（fail closed）→ 这条 Key 一点都没落下（列表没写、磁盘没动），
    // 把错误如实交给调用方，让用户看到「保存失败」而不是一把明文躺在库里
    save(&entries)?;
    Ok(entry)
}

/// 改名 / 启停 / 改白名单（`None` = 该项不动，与原来的两字段语义一致）。
///
/// ── 白名单为什么用 `Option<Vec<String>>` 而不是 `Vec<String>` ──────
/// `PATCH /api/keys/{id}` 是部分更新：旧版前端只发 `{name}` / `{enabled}`，
/// 用 `Vec` 表达「不改」只能靠「空列表」，而那正是「清除限制」的语义 ——
/// 于是「在管理页改个名字」会把已有的限制静默清掉。`Option` 把这两件事分开：
/// `None` = 不动，`Some(vec![])` = 清成不限制，`Some(list)` = 设成 list。
pub fn update(
    id: &str,
    name: Option<&str>,
    enabled: Option<bool>,
    allowed_providers: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
) -> Result<ApiKeyEntry, String> {
    let mut entries = list();
    let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
        return Err("Key 不存在".to_string());
    };
    if let Some(name) = name {
        entry.name = name.trim().to_string();
    }
    if let Some(enabled) = enabled {
        entry.enabled = enabled;
    }
    if let Some(list) = allowed_providers {
        entry.allowed_providers = list;
    }
    if let Some(list) = allowed_models {
        entry.allowed_models = list;
    }
    let updated = entry.clone();
    save(&entries)?;
    Ok(updated)
}

/// 删除
pub fn remove(id: &str) -> Result<(), String> {
    let mut entries = list();
    let before = entries.len();
    entries.retain(|entry| entry.id != id);
    if entries.len() == before {
        return Err("Key 不存在".to_string());
    }
    save(&entries)?;
    Ok(())
}

/// 把某个 provider id 从**所有** Key 的「可用提供商」白名单里摘掉（级联清理）。
///
/// 调用点在「删除一个家」之后（`api::custom_providers::remove_custom_provider`）：
/// 家没了，白名单里那条 id 就成了一条**永远命中不了**的限制 —— 只限制了这一家的
/// Key 会变成「谁都进不来」（每个请求都被 `key_scope` 拒掉），而 Key 行上显示的
/// 还是一串看不出所以然的 `custom-xxxx`。清理动作让这件事回到用户能理解的状态。
///
/// ── 为什么在这里、而不在 `custom_providers::remove` 里 ──────────
/// `custom_providers` 是配置层的一个数据模块，它认识 `AccountStore` 已经是为了
/// 级联账号（同一条取舍，见那边的说明）；再让它认识 apiKeys 这张表的写入，就是
/// 把两张表的写入耦合进一个数据模块。多表协调住在管理 API 层更合适 ——
/// 那里也正是「新建家 + 首个账号」做两步回滚的地方。
///
/// ── 返回值与「可能放宽成不限制」这件事 ───────────────────────
/// 返回**被改动的**记录（摘完后的形态；没提过这家的 Key 不在其中），调用方按
/// `allowed_providers.is_empty()` 就能数出「其中几把已恢复为不限制」。这一点必须
/// 让调用方看见并说出去：白名单**空 = 不限制**（见模块头），一把只限制了这一家的
/// Key 被摘完就是空 —— 它从「谁都进不来」变成「哪家都能进」。这是删掉该家必然带来
/// 的语义变化，不是本函数能消灭的东西；两种坏结局里，留一个死 id 让这把 Key 永远
/// 404（且界面上看不出原因）更难查，所以如实放宽、由调用方在日志与界面上点名。
///
/// 比对忽略大小写（与 `key_scope` 的判定、写入侧的去重同一口径）：手改配置塞进来
/// 的 `CUSTOM-xxx` 在转发时照样命中这一家（判定会归一成小写），清理时不认它就会漏
/// 掉一条。只改**真的提到过这家**的记录 —— 其余记录一个字节都不动（一次删除不该
/// 顺手重写别的 Key）。
pub fn strip_provider_from_allowlists(provider_id: &str) -> Vec<ApiKeyEntry> {
    let id = provider_id.trim();
    if id.is_empty() {
        return Vec::new();
    }
    let mut entries = list();
    let mut touched: Vec<ApiKeyEntry> = Vec::new();
    for entry in entries.iter_mut() {
        let before = entry.allowed_providers.len();
        entry
            .allowed_providers
            .retain(|known| !known.eq_ignore_ascii_case(id));
        if entry.allowed_providers.len() == before {
            continue;
        }
        touched.push(entry.clone());
    }
    if !touched.is_empty() {
        if let Err(error) = save(&entries) {
            // 这条路径返回 `Vec`（不给调用方 Result），失败只能在这里记日志。
            // 只记日志、**不改变返回值**：调用方（删除自定义家）不该因为一次
            // 白名单清理没落盘而回滚整个删家动作 —— 那个动作本身是另一回事，
            // 而白名单的清理下次删除/编辑时会再来一遍（幂等）。
            logging::log(
                "[Keys]",
                &format!("⚠️  网关 Key 白名单清理未落盘（改动仅本次运行生效）: {error}"),
            );
        }
    }
    touched
}

/// WorkBuddy 拆家（2026-10）的白名单补齐：把提到过 `workbuddy` 的 Key 补上
/// `workbuddy-intl`。
///
/// ── 为什么必须做（不做的后果是静默 400）──────────────────────
/// 拆家前 `workbuddy` 这一个 id 覆盖国内版与国际版两个站点，因此「限了
/// WorkBuddy」的 Key 本来就是**两地都能用**。拆家后国际版是另一个 id，
/// 不补的话那把 Key 对国际版账号会报「该 Key 不允许使用该提供商」——
/// 用户什么都没改，功能却少了一半，而报错文案指向 Key 的限制配置，
/// 与真实原因（我们拆了家）完全无关，极难自查。
///
/// ── 为什么是「补齐」而不是「把 workbuddy 换成两个」────────────
/// 原白名单里的 `workbuddy` 保持不动（国内版仍是那一个 id），只**追加**
/// 国际版 —— 拆家前后的可用集合因此逐字相同，没有放宽也没有收紧。
///
/// 幂等：已经同时含两家的记录不再改动。只处理**真的提到过国内版**的记录：
/// 没提过的（不限制，或只限了别家）一个字节都不动 —— 一条 Key 的限制集合是
/// 用户写明的东西（与 `strip_provider_from_allowlists` 同一纪律）。
/// 返回被改动的记录供日志点名。
pub fn add_intl_to_workbuddy_allowlists() -> Vec<ApiKeyEntry> {
    use crate::server::core::providers::{kind_id, ProviderKind};
    let cn = kind_id(ProviderKind::WorkBuddy);
    let intl = kind_id(ProviderKind::WorkBuddyIntl);
    let mut entries = list();
    let mut touched: Vec<ApiKeyEntry> = Vec::new();
    for entry in entries.iter_mut() {
        if !entry
            .allowed_providers
            .iter()
            .any(|known| known.eq_ignore_ascii_case(cn))
        {
            continue;
        }
        if entry
            .allowed_providers
            .iter()
            .any(|known| known.eq_ignore_ascii_case(intl))
        {
            continue;
        }
        entry.allowed_providers.push(intl.to_string());
        touched.push(entry.clone());
    }
    if !touched.is_empty() {
        if let Err(error) = save(&entries) {
            // 与 `strip_provider_from_allowlists` 同一处置：返回 `Vec`、不阻断
            // 调用方（`account_bootstrap` 的启动序列）；失败只影响「重启后还在
            // 不在」，而这条补齐本身幂等，下次启动会再来一遍。
            logging::log(
                "[Keys]",
                &format!("⚠️  网关 Key 白名单补齐未落盘（改动仅本次运行生效）: {error}"),
            );
        }
    }
    touched
}
