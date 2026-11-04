//! 模型管理规则（config.json 的 `modelRules` 字段）：禁用 / 映射 / 自定义模型。
//!
//! 形状：
//! ```json
//! "modelRules": {
//!   "disabled": [{ "provider": "catpaw", "id": "kimi-k3" }],
//!   "mappings": [{ "alias": "gpt-4o", "target": "deepseek-v4-pro", "provider": "raccoon",
//!                  "reasoning": "high" }],
//!   "custom":   [{ "provider": "catpaw", "id": "kimi-k3-preview" }],
//!   "seeded":   ["workbuddy:hy3"]
//! }
//! ```
//! - **禁用**：模型在管理页里仍可见（开关关着），但不出现在 `/v1/models`，
//!   请求它的**原始 id** 返回 404 model_not_found，这一家的**原生路由**也不再
//!   进候选链（`catalog::builtin_target` 的第一个分支按 `default_enabled` 判定）。
//!   **别名不受牵连**：映射条目只吃自己的 `enabled` 开关，目标模型被关不会挡住
//!   别名 —— `manifest_for` 刻意保留被关掉的上游记录（理由见该函数的注释），
//!   别名照旧解析得到它。所以「关掉主模型、用别名照样请求得到」是**预期行为**，
//!   不是漏判；要让别名也失效，得关那条映射自己的开关（或删掉它）。
//!   唯一例外是 `alias == target` 的自映射：它与模型的默认绑定共用一个开关
//!   （`set_state` 同步两者），关模型即关它。
//! - **映射**：把「上游模型（提供商 × 真名）」以对外名 `alias` 暴露给下游。
//!   同一提供商内，同名默认绑定与别名绑定最多启用一条；显式启用会关闭其余同名绑定。
//!   不同提供商可各启用一条同名绑定，用于账号优先级主备切换。
//!   每条绑定独立保存思考等级，关闭默认 ID 不妨碍其他名字调用该上游模型。
//!
//! ── 历史的 `hidden` 机制已移除（一次性清理语义）──────────────
//! 旧版有一个 `hidden` 列表（管理页「删除」按钮背后的东西）：从清单里拿掉、
//! 可在「已删除」筛选里恢复。它已被**每行一个启用开关 + 每条映射一个开关**
//! 取代（参考 OmniProxy 的模型管理）。现在的处理是：
//! `from_raw` **不再读取** `hidden` 键，`to_value` 也不再写出它 ——
//! 于是残留的隐藏名单既不会拦请求、也不会进广告，用户下次做任意一次写操作
//! （启停 / 加删映射 / 种子落盘）时，整份替换会把它从 config.json 里自然抹掉。
//! 这是「功能已移除」该有的行为：数据随下一次写自然蒸发，不做专门的迁移。
//!
//! ── 映射条目的 provider 字段 ────────────────────────────────
//! 新条目都带 `provider`（target 所属的家，UI 从该家的模型行上创建）。
//! 旧版条目（升级前）没有 provider：语义是「alias 改写成 target，由所有承载
//! target 的家接收」——路由扩池时对 provider 缺失的条目取 `providers_for_model(target)`，
//! 发送名同样改成 target，行为与升级前逐字等价（旧校验保证 alias 不与上游 id
//! 同名，所以「原生承载 alias 的家」为空，不存在语义分叉）。
//!
//! ── 启停粒度是「提供商 × 模型 id」，不再是全局 id ─────────────
//! 同一个模型 id（或对外名）常被多家同时提供（如 `kimi-k3`：CatPaw 的上游 id
//! 与小浣熊经去前缀映射暴露的对外名同名）。规则按 `(provider, id)` 存放，
//! 关掉某一家只是这一家不再接收该模型的请求，别家照常。
//!
//! ── 历史条目的兼容（不要「顺手迁移」）─────────────────────────
//! 旧版把 disabled / hidden 存成**纯 id 字符串数组**（全局生效）。读取时兼容：
//! 字符串条目解析成 `provider: None`，对**任何提供商**都命中 —— 升级后用户
//! 之前做的全局启停保持原样，直到他在管理页里对某一家重新启停（那时写侧会
//! 按「展开为其余各家」的语义把它替换掉，见 `set_state`）。
//!
//! - **seeded**：已经做过「默认规则种子」的 (provider, id)，写成
//!   `"provider:id"` 字符串（旧版是纯 id，读取时对任何 provider 都算已种 ——
//!   只影响「要不要再种一次默认值」，保守方向是正确的）。
//!   种子只在模型**首次出现**时生效一次：之后用户在管理页的手动调整
//!   （重新启用、删除映射）不会被下一次清单刷新悄悄改回去。
//!
//! ── 自定义模型（`custom`）────────────────────────────────────
//! 用户在管理页手动登记「这家还有这个上游模型」。它解决的是一个具体的死角：
//! 上游目录接口没广告、但实际能路由的模型（灰度中的新模型、按账号下发但没进
//! 目录的模型），此前网关既列不出、也调不通。
//!
//! 这些条目**不是**禁用那种「规则」——它们是**清单的补充来源**：
//! `providers::catalog::manifest_for` 把该家的自定义条目拼在自己的清单后面，
//! 于是能力判定、路由候选链、`/v1/models`、管理页、入口校验**一次全通**。
//! 因此「删除自定义模型」是从本数组里**移除**：用户要的是
//! 「这个模型我登记错了，删掉」，语义上不存在「恢复」这一步。
//!
//! 登记条目**不编造能力位**：用户登记时并不知道真实值，给一个默认值等于让
//! `/v1/models` 对下游**撒谎**（例如声明支持图片实际不支持）。缺字段的后果
//! 只是 `list_item` 里少几个元数据键（与图像模型同款行为），下游据此保守
//! 处理 —— 比给错值好。
//!
//! ── 能力位覆盖（`capabilities`）──────────────────────────────
//! 但**上游给的值本身也可能是错的**：远程目录撒谎、静态表跟不上上游调整时，
//! 下游拿到错的能力就会按错的形状构造请求（给不支持的模型发图片、按虚高的
//! 窗口堆历史），而以前纠正它的唯一手段是改代码。`capabilities` 就是那层
//! 纠正 —— 按 `(provider, id)` 存一组对下游出口的覆盖值，
//! `providers::catalog::manifest_for` 在清单成型处统一应用，于是 `/v1/models`
//! （含 Anthropic 列表视图）与管理页一次全通。键名与归一规则在
//! `core::capability`：内置家的覆盖存本模块，自定义家存在提供商记录的
//! `models[].capabilities` 里，**两份存储共用同一套判定**。
//!
//! 与其它列表的语义差别：`capabilities` 是**纯出口元数据**，网关内部不读这
//! 几个键做任何决策（路由 / 启停 / 转发都与它无关），所以它不影响
//! `disabled` / `mappings` / `custom` 的任何行为。
//!
//! 所有比对忽略大小写（与 `catalog::providers_for_model` 同口径）。
//!
//! ── 思考等级绑定（`mappings[].reasoning`）─────────────────────
//! 每条映射可以额外带一个思考等级（None / 空 = 不覆盖）。**这段机制、各家的
//! 翻译规则、以及「哪些情况故意不注入」都在 `reasoning.rs`** —— 那里有候选表、
//! 归一规则，以及「等级怎么进到转发链路」的完整说明（动手改这个字段前务必先读
//! 那一段）。这里只留结论：字段挂在映射条目上（与本模块的映射同生共死），
//! `ModelRules::from_raw` / `Mapping::to_value` 负责它的读写容错；
//! **转发侧**在 `upstream::payload::send_body` 按家改写模型名的那一步一并解析、
//! 交给该家适配器翻译（见 `providers::catalog::wire_target_for_provider`）。

use serde_json::{json, Map, Value};

use crate::server::config;

mod bindings;
mod mutations;
mod seeds;

use seeds::seed_extra_aliases;
pub use seeds::{migrate_qoder_default_enabled, migrate_workbuddy_default_enabled, seed_hidden_disabled, seed_qoder_defaults, QODER_DEFAULT_ENABLED};

pub use mutations::{add_custom, add_mapping, remove_custom, remove_mapping, set_capabilities, set_state};

pub const KEY_MODEL_RULES: &str = "modelRules";

/// Cline 专用的规则件：默认映射种子 + 拆分迁移 + 池前缀工具。
///
/// 拆出去的理由见那个文件的模块头（一次性迁移与长期机制不该混在一起，
/// 以及单文件体量约定）。`pub use` 让调用方仍写 `model_rules::seed_cline_defaults`
/// 这类路径，不必知道它住在子模块里。
mod cline;

pub use cline::{migrate_cline_split, seed_cline_defaults};

/// WorkBuddy 专用的规则件：默认启用白名单种子 + **拆家迁移**。
///
/// 与 `cline` 同一模式（那家是拆池、本家是拆地区，两次迁移的形态也接近）：
/// 拆出去的理由见那个文件的模块头（拆家迁移与长期机制不该混在一起，
/// 以及单文件体量约定）。`pub use` 让调用方仍写
/// `model_rules::seed_workbuddy_defaults` / `model_rules::migrate_workbuddy_split`。
mod workbuddy;

pub use workbuddy::{
    migrate_workbuddy_split, seed_workbuddy_defaults, SplitMigrationOutcome,
    WORKBUDDY_DEFAULT_ENABLED,
};

/// 思考等级组件（候选表 + 归一规则 + 设计取舍说明）。
///
/// 与 `cline` 同一模式：拆出去的理由见那个文件的模块头（长期机制与
/// 「这一段为什么这么做」的大段说明不该与规则机制本身挤在一个文件里，
/// 以及单文件体量约定）。`pub use` 让调用方仍写
/// `model_rules::REASONING_LEVELS` / `model_rules::normalize_reasoning`
/// 这类路径，不必知道它住在子模块里。
mod reasoning;

pub use reasoning::{
    effort_rank as reasoning_rank, is_thinking_off as reasoning_is_off,
    normalize as normalize_reasoning, read_client_level, REASONING_LEVELS,
};

/// 一条映射：把上游模型（`provider` × `target`）以对外名 `alias` 暴露给下游。
///
/// `provider` 指明 target 所属的家（新条目总是带；旧版条目为 `None`，
/// 语义是「所有承载 target 的家」，读取兼容见模块头）。同一 `alias` 允许
/// 多条（不同提供商各一条），路由时一起进入候选链主备切换。
///
/// `reasoning` 是这条映射上的**思考等级绑定**（`None` = 不覆盖）。
/// 转发侧在按家改写模型名的同一步解析它（`catalog::wire_target_for_provider`），
/// 交给该家适配器翻译成本家上游认识的档位字段 —— 各家的规则与「故意不注入」
/// 的几种情形见 `reasoning.rs` 的模块头。
///
/// `enabled` 是这条映射自己的开关（参考 OmniProxy 的模型管理）：**false = 这条
/// 别名暂时不存在** —— 不进 `/v1/models` 广告、不参与路由候选链展开、不做发送名
/// 改写，但管理页里仍看得到、可再打开。与模型行的启停开关同一哲学：
/// 「关掉」是可逆的暂态，「删除」（`remove_mapping`）才是不可逆的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub alias: String,
    pub target: String,
    pub provider: Option<String>,
    pub reasoning: Option<String>,
    pub enabled: bool,
}

/// 一条启停规则的键：`(provider, id)`。
///
/// `provider` 为 `None` 的条目来自旧版配置（纯 id 字符串），语义是
/// 「对所有提供商生效」；新版写侧永远带 provider。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEntry {
    pub provider: Option<String>,
    pub id: String,
}

impl RuleEntry {
    fn new(provider: Option<&str>, id: &str) -> Self {
        Self { provider: provider.map(str::to_string), id: id.to_string() }
    }

    fn to_value(&self) -> Value {
        match &self.provider {
            Some(provider) => json!({ "provider": provider, "id": self.id }),
            None => Value::String(self.id.clone()),
        }
    }

    /// 条目是否命中 `(provider, id)`：id 同（忽略大小写），且 provider 为
    /// None（全局条目）或与目标一致。
    fn matches(&self, provider: &str, id: &str) -> bool {
        self.id.eq_ignore_ascii_case(id)
            && self.provider.as_deref().map_or(true, |p| p.eq_ignore_ascii_case(provider))
    }
}

/// 一条**自定义模型**：用户手动登记「这家还有这个上游模型」。
///
/// 与 [`RuleEntry`] 的区别值得强调：`RuleEntry` 是**规则**（对清单里已有的
/// 条目做启停），本结构是**清单的补充**（往清单里加一个原本没有的条目）。
/// 两者的 `provider` 都是必填 —— 规则那边的 None 是旧版全局语义的兼容形态，
/// 这里没有历史包袱，缺 provider 的条目在读取时直接丢弃（见 `from_raw`）。
///
/// 字段只有两个：模型 id 与它所属的家。**展示名不单独存** —— 用户填的就是
/// 上游 id，再让他编一个显示名只会多一处要维护的事实；清单里的 `name` 由
/// [`custom_models_for`] 用 id 顶替（下游看到的名称与请求名一致，不会混淆）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomModel {
    pub provider: String,
    pub id: String,
}

impl CustomModel {
    fn to_value(&self) -> Value {
        json!({ "provider": self.provider, "id": self.id })
    }

    /// 是否命中 `(provider, id)`（都忽略大小写）。
    ///
    /// `pub`：管理页判「来源 = 手动」时要在 `catalog` 侧按同一口径比对
    /// （见 `catalog::manage_entries`）—— 在那里另写一遍忽略大小写的比较，
    /// 迟早会与这里的口径分叉。
    pub fn matches(&self, provider: &str, id: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider) && self.id.eq_ignore_ascii_case(id)
    }
}

/// 一条能力位覆盖：`(provider, id)` 的模型对下游声明的能力位（见模块头的
/// 「能力位覆盖」）。
///
/// `values` 是**稀疏表**：只存被覆盖的键（键名与归一规则在
/// `core::capability`），没被覆盖的键沿用清单原值。空表不存在 —— 写侧
/// （[`set_capabilities`]）会把清空的条目整条移除，读取侧也丢弃空条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityOverride {
    pub provider: String,
    pub id: String,
    pub values: Map<String, Value>,
}

impl CapabilityOverride {
    /// 是否命中 `(provider, id)`（都忽略大小写，与 `CustomModel::matches` 同口径）。
    ///
    /// `pub`：管理视图（`catalog::manage_view`）要按同一口径在快照里查
    /// 「这条模型被覆盖了哪几项」—— 在那里另写一遍比较，迟早会与这里分叉。
    pub fn matches(&self, provider: &str, id: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider) && self.id.eq_ignore_ascii_case(id)
    }

    /// 落盘的 JSON 形态：`{provider, id, …各能力键}` —— 能力值**平铺**在条目
    /// 上（不再套一层 `values` 对象）：手改 config.json 时一眼能看出这条覆盖
    /// 了哪几项，而多一层嵌套只会让排查多一次展开。
    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("provider".to_string(), Value::String(self.provider.clone()));
        object.insert("id".to_string(), Value::String(self.id.clone()));
        for (key, value) in &self.values {
            object.insert(key.clone(), value.clone());
        }
        Value::Object(object)
    }
}

impl Mapping {
    /// 落盘的 JSON 形态。
    ///
    /// `reasoning` 为 `None` 时**仍然写出 `null`**，与 `provider` 的写法同一
    /// 口径（`json!` 对 `Option::None` 给 `null`）：形状稳定比省几个字节重要，
    /// 且读取侧的容错本来就认 `null`（`as_str` 拿到 None）。反过来**不**在
    /// `None` 时省略这个键 —— 那样配置里同一个字段时有时无，排查时更难读。
    ///
    /// `enabled` 与此**刻意相反**：只在 false 时写出 `enabled: false`，
    /// true 一律省略 —— true 是缺省语义（读取侧缺键视为 true），历史条目
    /// （没有这个键的）因此在写回时一个字节都不多，配置文件不被无谓扰动。
    fn to_value(&self) -> Value {
        let mut object = json!({
            "alias": self.alias,
            "target": self.target,
            "provider": self.provider,
            "reasoning": self.reasoning,
        });
        if !self.enabled {
            if let Some(map) = object.as_object_mut() {
                map.insert("enabled".to_string(), Value::Bool(false));
            }
        }
        object
    }
}

/// 解析后的规则快照
#[derive(Clone, Debug, Default)]
pub struct ModelRules {
    pub disabled: Vec<RuleEntry>,
    pub mappings: Vec<Mapping>,
    /// 用户手动登记的上游模型（清单的补充来源，见模块头）
    pub custom: Vec<CustomModel>,
    /// 能力位覆盖（对下游出口的纠正，见模块头的「能力位覆盖」）
    pub capabilities: Vec<CapabilityOverride>,
    pub seeded: Vec<String>,
}

/// 从规则数组的 JSON 形态还原条目列表（兼容旧版纯 id 字符串）
fn entries_from(value: Option<&Value>) -> Vec<RuleEntry> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item {
                    // 旧形态：纯 id 字符串 = 全局规则
                    Value::String(id) => {
                        let id = id.trim();
                        if id.is_empty() {
                            None
                        } else {
                            Some(RuleEntry::new(None, id))
                        }
                    }
                    // 新形态：{provider, id}
                    Value::Object(object) => {
                        let id = object.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
                        if id.is_empty() {
                            return None;
                        }
                        let provider = object
                            .get("provider")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|provider| !provider.is_empty());
                        Some(RuleEntry::new(provider, id))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 从 JSON 数组还原自定义模型列表。
///
/// 容错口径比 `entries_from` **更严**：那里字符串条目有「旧版全局规则」的
/// 合法含义，这里的 `provider` 是**必填**（没有 provider 就不知道该把条目
/// 拼进哪一家的清单，等于一条永远不生效的死数据）。因此缺 provider / 缺 id /
/// 非对象形态的条目一律丢弃 —— 静默留一条不生效的记录，比丢掉它更难排查。
fn custom_from(value: Option<&Value>) -> Vec<CustomModel> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let object = item.as_object()?;
                    let id = object
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or("");
                    let provider = object
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or("");
                    if id.is_empty() || provider.is_empty() {
                        return None;
                    }
                    Some(CustomModel {
                        provider: provider.to_string(),
                        id: id.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 从 JSON 数组还原能力覆盖列表（值走 `core::capability` 的归一）。
///
/// 容错口径与 `custom_from` 同一条：`provider` / `id` 必填 —— 少了就不知道
/// 该覆盖哪一家的哪条模型，等于一条永不生效的死数据。非法能力键 / 非法值
/// 在归一里被丢弃；**归一后为空的条目也丢弃**（它等价于「没有覆盖」，留着
/// 只会让管理页的「已改」标记与 `capOverrides` 多列几个并不生效的键）。
fn capabilities_from(value: Option<&Value>) -> Vec<CapabilityOverride> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let object = item.as_object()?;
                    let id = object.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
                    let provider = object
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or("");
                    if id.is_empty() || provider.is_empty() {
                        return None;
                    }
                    let values = crate::server::core::capability::normalize_object(Some(item));
                    if values.is_empty() {
                        return None;
                    }
                    Some(CapabilityOverride {
                        provider: provider.to_string(),
                        id: id.to_string(),
                        values,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl ModelRules {
    pub fn from_raw(raw: &Map<String, Value>) -> Self {
        let Some(object) = raw.get(KEY_MODEL_RULES).and_then(Value::as_object) else {
            return Self::default();
        };
        let mappings = object
            .get("mappings")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let alias = item.get("alias")?.as_str()?.trim();
                        let target = item.get("target")?.as_str()?.trim();
                        if alias.is_empty() || target.is_empty() {
                            return None;
                        }
                        let provider = item
                            .get("provider")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|provider| !provider.is_empty());
                        // 思考等级：**缺失 / 非字符串 / 空串一律 None（不覆盖）**。
                        // 旧数据（升级前写下的映射）没有这个键，这条容错就是
                        // 「升级不能让已有映射变样」的实现方式：读出来与写回去
                        // 都不带 `reasoning` 的条目，行为与升级前逐字相同。
                        let reasoning = item
                            .get("reasoning")
                            .and_then(Value::as_str)
                            .and_then(normalize_reasoning);
                        // 映射开关：**缺失 / 非 bool 一律 true**。历史条目（开关
                        // 功能上线前写下的映射）没有这个键，这条容错就是
                        // 「升级不能让已有映射变样」的实现方式 —— 读出来是
                        // enabled，而写回时 true 不落键（见 `Mapping::to_value`），
                        // 历史配置在升级前后逐字相同。
                        let enabled = item.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                        Some(Mapping {
                            alias: alias.to_string(),
                            target: target.to_string(),
                            provider: provider.map(str::to_string),
                            reasoning,
                            enabled,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            disabled: entries_from(object.get("disabled")),
            // `hidden` 键**不再读取**（机制已移除，见模块头的「一次性清理语义」）：
            // 残留的隐藏名单在这里被丢弃，既不拦请求也不进广告；下次任意写操作
            // 落盘时整份替换会把它从 config.json 里自然抹掉。
            mappings,
            custom: custom_from(object.get("custom")),
            capabilities: capabilities_from(object.get("capabilities")),
            seeded: object
                .get("seeded")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "disabled": self.disabled.iter().map(RuleEntry::to_value).collect::<Vec<_>>(),
            // `hidden` 不再写出（机制已移除，见模块头）：整份替换落盘时，
            // 配置里残留的 hidden 名单就此消失 —— 一次性清理的实现方式。
            "mappings": self.mappings.iter().map(Mapping::to_value).collect::<Vec<_>>(),
            // `custom` 必须在这里写出：本函数是**整份替换**（`save` → 
            // `config::update_raw_field`），漏掉哪个键，任何一次写规则
            // （启停 / 加映射 / 删映射 / 四个种子）都会把那个键的数据整份抹掉。
            "custom": self.custom.iter().map(CustomModel::to_value).collect::<Vec<_>>(),
            // 与 `custom` 同一条硬约束：本函数是整份替换，漏写出哪个键，
            // 任何一次写规则都会把那个键的数据整份抹掉。
            "capabilities": self.capabilities.iter().map(CapabilityOverride::to_value).collect::<Vec<_>>(),
            "seeded": self.seeded,
        })
    }

    /// 单条列表（disabled）上「按 id 取条目」的匹配判定
    fn hit(list: &[RuleEntry], provider: &str, id: &str) -> bool {
        list.iter().any(|entry| entry.matches(provider, id))
    }

    pub fn is_disabled(&self, provider: &str, id: &str) -> bool {
        Self::hit(&self.disabled, provider, id)
    }

    /// 精确提供商规则覆盖旧版全局规则，包括显式关闭的条目。
    pub fn binding(&self, provider: &str, alias: &str, target: &str) -> Option<&Mapping> {
        let matches = |m: &&Mapping| {
            m.alias.eq_ignore_ascii_case(alias) && m.target.eq_ignore_ascii_case(target)
        };
        self.mappings.iter().filter(matches)
            .find(|m| m.provider.as_deref().is_some_and(|owner| owner.eq_ignore_ascii_case(provider)))
            .or_else(|| self.mappings.iter().filter(matches).find(|m| m.provider.is_none()))
    }

    /// disabled 只控制原始 ID 的默认绑定，不能连带关闭其他别名。
    pub fn default_enabled(&self, provider: &str, id: &str) -> bool {
        !self.is_disabled(provider, id)
            && self.binding(provider, id, id).map_or(true, |mapping| mapping.enabled)
    }

    pub fn is_blocked(&self, provider: &str, id: &str) -> bool {
        !self.default_enabled(provider, id)
    }

    /// 某对外名的**生效**映射条目（同名多条 = 多提供商主备；顺序 = 配置顺序）。
    ///
    /// **只返回 `enabled` 的条目**：关闭的映射在语义上就是「这条别名不存在」，
    /// 不该出现在任何转发侧视图里。转发侧各消费点的过滤口径同源 ——
    /// `catalog::builtin_target` 与 `wire_target_for_provider` 经 `binding()`
    /// 判每条绑定的启停、`catalog::model_blocked_everywhere` 用本函数
    /// 区分「未知模型 vs 全被关闭」、`provider_loop` 的「含映射」日志提示
    /// 用它决定要不要缀那句话。管理页要看见**全量**（含关闭的，才能再打开），
    /// 它直接遍历 `rules.mappings`（见 `catalog::manage_view`），不走本函数。
    pub fn mappings_of(&self, model: &str) -> Vec<&Mapping> {
        self.mappings
            .iter()
            .filter(|m| m.enabled && m.alias.eq_ignore_ascii_case(model))
            .collect()
    }

    /// 该对外名是否已存在任何映射条目（种子防抢占判断用）。
    pub fn has_alias(&self, alias: &str) -> bool {
        self.mappings.iter().any(|m| m.alias.eq_ignore_ascii_case(alias))
    }

    /// 某条映射的对外名：target 归属 `provider`（None = 旧版全局条目）。
    ///
    /// 管理页按「提供商 × 模型 id」分行渲染：全局条目在所有承载 target 的行
    /// 上都显示（旧行为），带 provider 的条目只显示在自己那家的行上。
    ///
    /// **刻意不过滤 disabled**：本函数唯一的消费点是 `catalog::manage_view` 的
    /// 行内 `aliases` 数组，那是管理页的展示视图 —— 关闭的映射必须继续显示
    /// （chip 还在行上，用户要能看见它、把开关再打开）。过滤后 chip 会凭空
    /// 消失，「我关了个开关，映射怎么没了」是个查不出的问题。转发生效视图
    /// 一律走 `mappings_of`（那里过滤），两个口径的分工见它的注释。
    pub fn aliases_of(&self, provider: &str, target: &str) -> Vec<&str> {
        self.mappings
            .iter()
            .filter(|m| m.target.eq_ignore_ascii_case(target))
            .filter(|m| m.provider.as_deref().map_or(true, |p| p.eq_ignore_ascii_case(provider)))
            .map(|m| m.alias.as_str())
            .collect()
    }

    /// 指向某上游模型名的全部对外名（**跨提供商**、去重；忽略大小写比对去重键）。
    ///
    /// `/v1/models` 是一个平面对外目录：同一个对外名无论有几家通过映射提供，
    /// 都只广告一条。
    ///
    /// **只统计 enabled 的条目**：本函数的全部消费点都是对外生效视图 ——
    /// `models_response` 的别名广告段、`advertised_manifest_contains`（入口
    /// 「广告里有才放行」校验）、`advertised_model_ids`（相近模型提示）、
    /// `models_by_provider`（Key 页的可勾模型候选）—— 关闭的映射不广告、
    /// 也调不通，四处自动同口径，不会出现「列表里看不到却能调通」的缝。
    /// 管理页的全量视图走 `aliases_of`（那里不过滤，见它的注释）。
    pub fn aliases_of_any(&self, target: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for m in &self.mappings {
            if m.enabled
                && m.target.eq_ignore_ascii_case(target)
                && !out.iter().any(|a| m.alias.eq_ignore_ascii_case(*a))
            {
                out.push(m.alias.as_str());
            }
        }
        out
    }

    /// 该 (provider, id) 是否已做过默认规则种子。
    ///
    /// ── 旧版纯 id 条目的兼容**只对 workbuddy / raccoon 成立**（别放宽）──
    /// 升级前的 `seeded` 存的是纯 id（旧版种子是全局动作），读取时对这两家
    /// 保留「纯 id 也算已种」的兼容：重种一遍只是把同样的默认值再写一次，
    /// 没必要。
    ///
    /// ── WorkBuddy 国际版**不继承**国内版的种子标记（2026-10 拆家）────
    /// `migrate_workbuddy_split` 会把 `workbuddy:<id>` 的标记**复制**一份给
    /// 国际版（那条迁移在拆家那一刻跑一次）。但读取层**不能**在这里把
    /// 「`workbuddy:<id>` 存在」当成「`workbuddy-intl:<id>` 已种」：
    /// 拆家之后新出现在国际版清单里的模型必须按正常种子路径判一次
    /// （不在白名单里就默认禁用），而继承国内版的标记会让它们直接跳过种子、
    /// 默认全开。两件事各归各处：迁移负责存量，读取层不猜。
    ///
    /// 但这个兼容**不能给所有 provider 开**：Qoder 的目录里有 `Auto` /
    /// `GLM-5.3` / `DeepSeek-V4-Pro` 这类与 workbuddy / raccoon 清单**同名**的
    /// 模型，它们的纯 id 早已躺在旧版 `seeded` 里 —— 一律算已种的话，Qoder
    /// 这几个模型会**跳过白名单种子**而在全新安装上默认启用，与「只默认开
    /// Qwen3.8-Flash」的预期正好相反。旧版种子从未处理过 Qoder，
    /// 那批标记对 Qoder 不构成「已种」的证据。
    fn is_seeded(&self, provider: &str, id: &str) -> bool {
        let key = format!("{provider}:{id}");
        if self.seeded.iter().any(|item| item.eq_ignore_ascii_case(&key)) {
            return true;
        }
        if !matches!(provider, "workbuddy" | "raccoon") {
            return false;
        }
        self.seeded.iter().any(|item| item.eq_ignore_ascii_case(id))
    }
}

/// 当前生效的规则
pub fn current() -> ModelRules {
    (*shared()).clone()
}

pub(crate) fn shared() -> std::sync::Arc<ModelRules> {
    config::model_rules_snapshot()
}

pub(crate) fn save(rules: &ModelRules) -> bool {
    config::update_raw_field(KEY_MODEL_RULES, rules.to_value())
}

use bindings::set_membership;

/// **上游模型 id / 自生成别名**的字符集口径：字母 / 数字 / `- _ . / :`。
///
/// 只用在两处 —— 手动登记的上游模型 id（`api::model_manage` 的 `/api/models/custom`）
/// 与 Cline 的默认映射种子（`cline.rs`，种子名由模型 id 拼出来）。
/// **对外映射名不走它**：那是用户给下游点名用的名字，上游不校验，见
/// [`alias_name_valid`]。
pub fn alias_valid(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 128
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
}

/// **新建对外映射名**能不能落盘。
///
/// ── 为什么不再限制字符集 ──────────────────────────────────────
/// 对外名是给下游点名用的名字，上游根本不看它；而各家官方的模型名并不保证是
/// ASCII —— 千问办公的 `高级` / `标准` 就是中文，且那正是它的**上游模型 id**
/// （见 Agent2Providers 的 `qwenwork` 兜底清单 / 远程目录解析）。既然这种名字
/// 本来就会作为自映射绑定躺在配置里，新建别名时再按「只准字母数字」拦一道，
/// 就成了同一份数据两套标准：中文名能用、却不许你再起一个中文名。
///
/// 保留的是**「没法当模型名用」**那一档：空、超长、带空白或控制字符。空值会让
/// 下游点名点不到；空白 / 控制字符会做出「看上去一模一样、请求却对不上」的名字
/// （千问办公的远程目录也刻意把展示名里的空白抹掉再当 id）。长度按**字符**数
/// 计（128 个中文与 128 个字母一视同仁），不是字节数。
///
/// 调用方负责先 `trim`（API 层的 `text_field` 已经做过）。
pub fn alias_name_valid(alias: &str) -> bool {
    !alias.is_empty()
        && alias.chars().count() <= 128
        && !alias.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 把该家的能力覆盖写进整份清单（内置家清单出口的统一调用点，见
/// `providers::catalog::manifest_for`）。
///
/// 收在「整份清单一次」而不是「一条条目一次」，是性能上的硬约束：
/// `current()` 每次调用都要重新解析整份 modelRules，逐条调用会让一次
/// `/v1/models` 把同一份配置解析几百遍。
pub fn apply_capability_overrides(provider: &str, items: &mut [Value]) {
    let rules = shared();
    if rules.capabilities.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if let Some(entry) = rules
            .capabilities
            .iter()
            .find(|entry| entry.matches(provider, id))
        {
            crate::server::core::capability::apply_overrides(item, &entry.values);
            // 来源标记：被覆盖的键从此是**用户指定**的值，既不是上游给的也不是
            // models.dev 补的（管理页要把这三档分开显示，见 `core::model_meta`
            // 的模块头）。覆盖在 models.dev 补齐**之后**执行，所以这里改的是
            // 已经标好的那几项。
            crate::server::core::model_meta::mark_user(
                item,
                &crate::server::core::capability::overridden_keys(&entry.values),
            );
        }
    }
}

/// 该 `(provider, id)` 是否是用户手动登记的自定义模型。
///
/// 消费方：管理页判「来源 = 手动」（`catalog::manage_entries`）、以及 AutoClaw
/// 的模型路由解析（未知名字不静默回落，见 `autoclaw::models::resolve_model_route`）。
pub fn is_custom(provider: &str, id: &str) -> bool {
    shared().custom.iter().any(|item| item.matches(provider, id))
}

/// 某一家当前登记的自定义模型，转成**聚合层认的条目形态**。
///
/// ── 形状（为什么只有这几个键）───────────────────────────────
/// `id` / `name` 都用用户填的那个 id：下游看到的名称与请求名一致，不会出现
/// 「列表里叫 A、请求要用 B」的困惑。**不编造能力位**（`maxOutputTokens` 等）——
/// 用户无从知道真实值，给一个默认值等于让 `/v1/models` 对下游撒谎。
/// `list_item` 对缺失键的行为是「整键省略」（与图像模型同款），下游据此保守
/// 处理，比给错值好。
///
/// `isDefault` 恒 false：默认模型是各家目录自己的概念（当前只有 workbuddy 认），
/// 手动登记的条目不该去争这个位。
///
/// ── 为什么是「调用时现算」而不是缓存 ─────────────────────────
/// 数据源是 `model_rules::current()`（内存快照），条目通常只有几条，
/// 现算一次的成本远低于维护一份会与配置失同步的缓存。
pub fn custom_models_for(provider: &str) -> Vec<Value> {
    shared()
        .custom
        .iter()
        .filter(|item| item.provider.eq_ignore_ascii_case(provider))
        .map(|item| {
            json!({
                "id": item.id,
                "name": item.id,
                "isDefault": false,
                "kind": "chat",
            })
        })
        .collect()
}
