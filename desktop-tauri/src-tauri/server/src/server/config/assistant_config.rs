//! AI 助手配置（「AI 助手」页的对话底座 / 页面内设置抽屉）。
//!
//! ── 存什么 ──────────────────────────────────────────────────
//! 一块 JSON 存在配置库的 [`KEY_ASSISTANT_CONFIG`] 键下（与 `clineUpstreamHeaders`
//! 同一形态：整块进出，不为每个字段各起一个配置键）：
//!
//!   {
//!     "model": "claude-sonnet-4.5" | null,   // 对外模型名（与 /v1/models 同一口径）
//!     "mode": "confirm" | "full",      // 权限模式（改动类工具要不要先问）
//!     "systemPromptEnabled": true,
//!     "systemPrompt": "……",            // 空串 = 用内置默认（DEFAULT_SYSTEM_PROMPT）
//!     "tools": { "list_models": true, "set_account_enabled": false, ... }
//!   }
//!
//! ── 为什么独立成文件 ────────────────────────────────────────
//! `config/mod.rs` 已经装了整个设置面的读写入口，`types.rs` 装常量与结构。
//! 助手这块是自足的一整块（四个字段、一份默认值、一份内置提示词），拆出来
//! 让「它到底存了什么」在一个文件里读完，不必在四千行的 mod.rs 里翻。
//!
//! ── 工具勾选的默认值 ────────────────────────────────────────
//! 存量安装（库里没有这个键）给的是 [`DEFAULT_ENABLED_TOOLS`]：**全部九只**。
//! 助手的价值在于「真的能把事做完」，默认少给权限只会让第一次提问就撞上
//! 「这个工具没勾」；风险由确认卡兜住 —— 改动类工具默认都要用户点「执行」。
//! 用户存过一次之后以库里的值为准（哪怕存的是全不勾）。
//!
//! ── 权限模式 ────────────────────────────────────────────────
//! [`PermissionMode`] 只影响**前端要不要弹确认卡**，不改后端能力边界：工具表
//! 与执行路径完全一样。默认 `Confirm` —— 新装用户不会在不知情时被直接改配置。
//!
//! ── 与「设置页现有系统提示词」的关系 ────────────────────────
//! **无关**。设置页「网关 → 系统提示词」那一份管的是**转发给客户端的请求**
//! 怎么处理 system（`promptMode` / `promptFile`）；这里这一份只喂给 AI 助手
//! 自己的对话。两者名字像，作用域完全不同，别合并。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// 配置键（config.json / 配置库里的键名）
pub const KEY_ASSISTANT_CONFIG: &str = "assistantConfig";

/// 系统提示词正文的长度上限（按字符）。
///
/// 8000 与「模型测试」的提示词上限同一量级：助手提示词是**行为约束**，不是知识库，
/// 写不下的内容应该进工具说明；再往上也没有意义，只会每轮都白送一次 token。
pub const MAX_SYSTEM_PROMPT_CHARS: usize = 8000;

/// 全部工具 id（**与前端 `assistant-tools.ts` 的 `ASSISTANT_TOOLS` 对齐**：
/// 那里是权威定义（名称 / 参数 / 执行），这里只需要 id 列表 —— 用途只有一个：
/// 出站时把工具表展开成完整形态（见 [`AssistantConfig::status_value`]）。
/// 加一只工具 = 两边各加一处（前端定义 + 这里的 id）。
pub const ALL_TOOL_IDS: [&str; 9] = [
    "list_models",
    "list_account_balance",
    "set_model_enabled",
    "set_model_mapping",
    "set_model_capability",
    "test_model",
    "set_provider_order",
    "set_account_order",
    "set_account_enabled",
];

/// 旧版本（默认集是 6 只时）的推荐勾选。
///
/// 用于**存量迁移**：那一版界面每次保存都会把 9 项展开写回库（勾 6 假 3），
/// 于是「从没动过工具」的用户在库里也留着一份显式的 6 真 3 假。不识别它的话，
/// 新默认集（全选）对他们永远不会生效 —— 用户只会在助手页看到「3 个工具没勾」，
/// 而他从没做过这个选择。
///
/// 判据是**逐项相等**：只要用户动过任何一项，库里就不再等于这组值，按他的
/// 选择走（那是真实意图，不该被默认值覆盖）。
const LEGACY_DEFAULT_TOOLS: [&str; 6] = [
    "list_models",
    "list_account_balance",
    "set_model_enabled",
    "set_model_mapping",
    "set_model_capability",
    "test_model",
];

/// 库里的勾选表是否恰好等于「旧默认集展开后的形态」
fn is_legacy_default_tools(tools: &BTreeMap<String, bool>) -> bool {
    ALL_TOOL_IDS.iter().all(|id| {
        tools
            .get(*id)
            .copied()
            .unwrap_or_else(|| LEGACY_DEFAULT_TOOLS.contains(id))
            == LEGACY_DEFAULT_TOOLS.contains(id)
    })
}

/// 存量安装的默认勾选工具（见模块头：全部九只）。
pub const DEFAULT_ENABLED_TOOLS: [&str; 9] = ALL_TOOL_IDS;

/// 内置默认系统提示词（`systemPrompt` 为空串时用它）。
///
/// 与设置页「编辑系统提示词」弹窗里那份**逐字相同** —— 用户点「恢复默认」拿到的
/// 就是这一段，「清空保存」等价。改这里等于改所有未自定义用户的助手行为。
pub const DEFAULT_SYSTEM_PROMPT: &str = "你是 Agent2API 的内置助手，帮用户管理本机的模型转发配置。\n\n说话简洁，别铺垫。配置项一律用等宽字写出准确的 ID（模型名、账号名、提供商名），不要用「那个模型」这类含糊说法。\n\n调用改动类工具之前，先用一句话说清将要改动什么；确认卡由界面渲染，不要跳过它直接执行。\n\n拿不准的配置先去读（获取模型列表 / 查询账号余额），不要凭印象改。";

/// 权限模式：改动类工具要不要先弹确认卡。
///
/// 只影响前端流程（`confirm` 停在确认卡上、`full` 直接执行），**不扩大工具的
/// 能力边界** —— 发出去的工具表与执行路径完全一样。所以它存在配置里而不是
/// 做成一把安全开关：换台机器沿用同一个习惯即可。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// 变更前确认（默认）：只读工具直接跑，改动类先列清单等用户点「执行」
    #[default]
    Confirm,
    /// 完全访问：改动类工具直接执行，结果仍以卡片留在对话里
    Full,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Confirm => "confirm",
            PermissionMode::Full => "full",
        }
    }

    /// 宽松解析：只认字面 `full`，其余（缺失 / 写坏 / 未知值）一律回落到
    /// 更安全的那一档 —— 配置被手改坏时不该把用户悄悄推进「不再询问」。
    pub fn from_str_lenient(text: &str) -> Self {
        if text.trim().eq_ignore_ascii_case("full") {
            PermissionMode::Full
        } else {
            PermissionMode::Confirm
        }
    }
}

/// AI 助手配置快照
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantConfig {
    /// 对话模型 = **对外模型名**（与 `/v1/models` 的 `data[].id` 同一口径：原始 ID
    /// 或用户建的映射名都行）；`None` = 还没选（此时发起对话会被前端拦下并提示
    /// 去设置里选）。路由语义与普通客户端一致：同名在多家时按账号优先级主备。
    pub model: Option<String>,
    /// 思考等级（`reasoning_effort` 的取值）；`None` = 不指定 —— 此时由映射上
    /// 绑定的等级决定（若也没绑，则不向上游发任何档位字段）。
    ///
    /// 存的是**会话默认值**：聊天框里改一次就存一次，下一条消息沿用它。单次
    /// 请求可以带 `reasoning` 覆盖它（见 `api::assistant::chat` 的读取顺序）。
    pub reasoning: Option<String>,
    /// 权限模式（改动类工具要不要先问，见 [`PermissionMode`]）
    pub mode: PermissionMode,
    /// 系统提示词开关（**工具一只都没勾时会自动视为不注入**，见 `system_prompt`）
    pub system_prompt_enabled: bool,
    /// 系统提示词正文（空串 = 内置默认，见 [`DEFAULT_SYSTEM_PROMPT`]）
    pub system_prompt: String,
    /// 工具勾选表（tool id → 是否勾选）。**只存显式值**，缺省键由
    /// [`Self::tool_enabled`] 按 [`DEFAULT_ENABLED_TOOLS`] 兜底。
    pub tools: BTreeMap<String, bool>,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            model: None,
            reasoning: None,
            mode: PermissionMode::default(),
            system_prompt_enabled: true,
            system_prompt: String::new(),
            tools: BTreeMap::new(),
        }
    }
}

impl AssistantConfig {
    /// 某个工具是否勾选（用户在设置里存过该项就用存的，否则用默认集）
    pub fn tool_enabled(&self, id: &str) -> bool {
        self.tools
            .get(id)
            .copied()
            .unwrap_or_else(|| DEFAULT_ENABLED_TOOLS.contains(&id))
    }

    /// 实际注入给模型的系统提示词。
    ///
    /// 三条短路与设置页那张状态表逐条对应：
    ///   · 开关关着 → 不注入；
    ///   · **一只工具都没勾** → 不注入（用户把助手当纯聊天用时，不该再收到一堆
    ///     工具行为约束 —— 这是产品口径，不是优化）；
    ///   · 正文为空 → 用内置默认（不是不注入）。
    ///
    /// `tools_present` 由调用方给（它手里的工具表可能已经按本次请求过滤过），
    /// 本函数不自己去数 `self.tools`。
    pub fn system_prompt(&self, tools_present: bool) -> Option<String> {
        if !self.system_prompt_enabled || !tools_present {
            return None;
        }
        let text = self.system_prompt.trim();
        if text.is_empty() {
            return Some(DEFAULT_SYSTEM_PROMPT.to_string());
        }
        Some(text.to_string())
    }

    /// 序列化成存库 / 应答前端的 JSON 形态
    pub fn to_value(&self) -> Value {
        let tools: Map<String, Value> = self
            .tools
            .iter()
            .map(|(id, on)| (id.clone(), Value::Bool(*on)))
            .collect();
        json!({
            "model": self.model.clone().map(Value::String).unwrap_or(Value::Null),
            "reasoning": self.reasoning.clone().map(Value::String).unwrap_or(Value::Null),
            "mode": self.mode.as_str(),
            "systemPromptEnabled": self.system_prompt_enabled,
            "systemPrompt": self.system_prompt,
            "tools": Value::Object(tools),
        })
    }

    /// 给界面的形态：**tools 展开成完整表**（每个 id 都有明确的 true / false，
    /// 缺省按 [`DEFAULT_ENABLED_TOOLS`] 兜底），其余与 [`Self::to_value`] 相同。
    ///
    /// 为什么出站要展开、入库不展开：库里只存用户**显式改过**的取舍（这样以后改
    /// 默认集，没动过工具的存量用户能跟着走）；而界面要的是「这一项现在到底勾没勾」
    /// —— 把默认值算在界面侧等于让前端也维护一份默认集，两处迟早对不上。
    pub fn status_value(&self) -> Value {
        let mut value = self.to_value();
        let mut tools = Map::new();
        for id in ALL_TOOL_IDS {
            tools.insert(id.to_string(), Value::Bool(self.tool_enabled(id)));
        }
        value["tools"] = Value::Object(tools);
        value
    }
}

/// 从配置底稿读这块配置（缺失 / 写坏一律回落默认，绝不让助手起不来）。
///
/// 容忍度取向与 `cline_headers_from` 一致：手改库写出的类型不对的项**跳过该项**，
/// 不整块报错 —— 「有一项写坏了」不该把用户设置里的另外三项一起废掉。
pub fn from_raw(raw: &Map<String, Value>) -> AssistantConfig {
    let Some(block) = raw.get(KEY_ASSISTANT_CONFIG).and_then(Value::as_object) else {
        return AssistantConfig::default();
    };
    // 对话模型：新形态是**字符串**（对外名）；旧安装可能存过 `{provider, id}`
    // 对象（改成对外名之前的那版），取 `id` 兼容读 —— provider 在新路由语义下
    // 不再参与（同名在多家时按账号优先级，与普通客户端一致），丢掉它无信息损失
    let model = match block.get("model") {
        Some(Value::String(text)) => {
            let id = text.trim();
            (!id.is_empty()).then(|| id.to_string())
        }
        Some(Value::Object(model)) => {
            let id = model.get("id").and_then(Value::as_str).unwrap_or("").trim();
            (!id.is_empty()).then(|| id.to_string())
        }
        _ => None,
    };
    // 思考等级：空串 / 缺失 = 不指定（交给映射绑定决定）。长度上限与
    // `model_rules::normalize_reasoning` 同一口径 —— 手改库写进来的超长值
    // 按「不指定」处理，而不是原样塞进上游请求。
    let reasoning = block
        .get("reasoning")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.chars().count() <= 32)
        .map(str::to_string);
    let tools: BTreeMap<String, bool> = block
        .get("tools")
        .and_then(Value::as_object)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|(id, on)| on.as_bool().map(|on| (id.clone(), on)))
                .collect()
        })
        .unwrap_or_default();
    // 存量迁移：库里的值恰好是「旧默认集」时视为没调过，清空显式值让它跟着
    // 新默认集（全选）走。见 is_legacy_default_tools 的说明。
    let tools = if is_legacy_default_tools(&tools) { BTreeMap::new() } else { tools };
    AssistantConfig {
        model,
        reasoning,
        // 缺失 / 写坏一律回落「变更前确认」：配置被手改时不该把人悄悄推进
        // 「不再询问」那一档（见 PermissionMode::from_str_lenient）
        mode: block
            .get("mode")
            .and_then(Value::as_str)
            .map(PermissionMode::from_str_lenient)
            .unwrap_or_default(),
        // 只有字面 `false` 算关（默认开）：缺失 → true
        system_prompt_enabled: block
            .get("systemPromptEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        system_prompt: block
            .get("systemPrompt")
            .and_then(Value::as_str)
            .map(|text| clamp_chars(text, MAX_SYSTEM_PROMPT_CHARS))
            .unwrap_or_default(),
        tools,
    }
}

/// 按字符数截断（越界回落取向与设置页其它文本项一致：截断而不是拒绝，
/// 但接口层会先做一次校验并把超限告诉用户 —— 这里只是兜住手改库的情形）。
fn clamp_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}
