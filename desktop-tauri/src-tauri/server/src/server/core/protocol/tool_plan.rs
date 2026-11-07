//! 工具声明形态适配：把 Responses 的几种声明形态摊平成 Chat 能认的一层。
//!
//! ── 为什么需要摊平 ──────────────────────────────────────────
//! Chat 的 `tools` 是一层扁平数组，每项是 `{type:"function", function:{…}}`；
//! 而 Responses 允许两种「分组 / 变形」声明：
//!   · `type:"custom"`：自由文本工具，没有 JSON 参数表（细节见 `freeform` 模块）；
//!   · `type:"namespace"`：命名空间，把一组工具打包在一个名字下。Codex 的
//!     Responses Lite 路径用它承载 `functions` / `clock` / `collaboration` /
//!     `mcp__*` 等工具组。
//!
//! 直接把 namespace 转发过去上游不认，丢掉更糟：模型收不到任何工具声明，
//! 只能把调用意图当正文吐出来（2026-09 那次「Codex 调不动工具」的根因）。
//!
//! ── 摊平与还原 ──────────────────────────────────────────────
//! 子工具以 `namespace__name` 发送；超长、非法字符与名称冲突用稳定后缀区分。
//! 同一 `(namespace, name)` 的重复声明合并，后出现的定义覆盖之前的定义，
//! 位置保持不变。不同分组的同名工具不能合并。
//!
//! 声明、历史调用、tool_choice 和回程都使用同一份名称映射。回程恢复原名
//! 与 `namespace` 字段，Codex 才能按自己的工具清单分派调用。
//!
//! ── defer_loading 怎么处理 ──────────────────────────────────
//! Responses 的 namespace 子工具常带 `defer_loading: true`，语义是「先不给模型
//! 完整定义，等它用 `tool_search` 搜到再加载」。上游 Chat 接口没有 tool_search，
//! 于是这里**忽略 defer_loading、把定义全部下发**：能少占一点上下文固然好，
//! 但工具根本拿不到就是彻底不可用 —— 两害相权取其轻。

mod names;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use names::{ToolName, ToolNames};
use super::{freeform, is_truthy, string_field, string_value};
use crate::server::logging;

/// `additional_tools` 输入项的 type 字面量。
///
/// Codex 的 Responses Lite 路径用它承载工具声明：顶层 `tools` 置 null，工具
/// 改放在 `input[]` 的这一个项里（见 OpenAI 文档「Add tools at a specific
/// point in the input」）。本网关的上游是 Chat 接口，没有「对话中途追加工具」
/// 的概念，所以统一提升成请求级工具声明。
const ADDITIONAL_TOOLS_ITEM: &str = "additional_tools";

/// 命名空间声明的 type 字面量
const NAMESPACE_TOOL: &str = "namespace";

/// 一次请求的工具声明经过摊平后的结果。
#[derive(Default)]
pub struct ToolPlan {
    pub declarations: Vec<Value>,
    names: ToolNames,
    custom_names: BTreeSet<String>,
}

impl ToolPlan {
    pub fn is_custom(&self, name: &str) -> bool {
        self.custom_names.contains(name)
    }

    pub fn call_name(&self, item: &Value) -> Result<String, String> {
        self.names.call_name(item)
    }
}

struct Declaration {
    tool: Value,
    identity: Option<ToolName>,
}

#[derive(Default)]
struct ToolDeclarations {
    entries: Vec<Declaration>,
    positions: BTreeMap<ToolName, usize>,
}

impl ToolDeclarations {
    fn push(&mut self, tool: Value, namespace: &str) {
        let kind = string_field(&tool, "type");
        let callable = kind.is_empty()
            || kind.eq_ignore_ascii_case("function")
            || freeform::is_custom_tool(&tool);
        let name = declaration_name(&tool);
        let identity = (callable && !name.is_empty()).then(|| ToolName::new(namespace, name));
        if let Some(identity) = &identity {
            if let Some(index) = self.positions.get(identity) {
                if let Some(existing) = self.entries.get_mut(*index) {
                    existing.tool = tool;
                    return;
                }
            }
            self.positions.insert(identity.clone(), self.entries.len());
        } else if self.entries.iter().any(|entry| entry.identity.is_none() && entry.tool == tool) {
            return;
        }
        self.entries.push(Declaration { tool, identity });
    }

    fn finish(self, request: &Value) -> ToolPlan {
        let identities = self.entries.iter().filter_map(|entry| entry.identity.clone()).collect();
        let mut plan = ToolPlan { names: ToolNames::new(identities, request), ..ToolPlan::default() };
        for mut entry in self.entries {
            if let Some(identity) = entry.identity {
                let wire = plan.names.wire_name(&identity);
                set_declaration_name(&mut entry.tool, &wire);
                if freeform::is_custom_tool(&entry.tool) {
                    plan.custom_names.insert(wire);
                }
            }
            plan.declarations.push(entry.tool);
        }
        plan
    }
}

/// 顶层声明先于 input 中追加的声明，同一工具最后出现的定义生效。
pub fn plan_tools(request: &Value) -> ToolPlan {
    let mut declarations = ToolDeclarations::default();
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        for tool in tools {
            push_tool(&mut declarations, tool);
        }
    }
    if let Some(input) = request.get("input") {
        match input {
            Value::Array(items) => {
                for item in items {
                    collect_additional_tools(&mut declarations, item);
                }
            }
            Value::Object(_) => collect_additional_tools(&mut declarations, input),
            _ => {}
        }
    }
    declarations.finish(request)
}

fn collect_additional_tools(declarations: &mut ToolDeclarations, item: &Value) {
    if !string_field(item, "type").eq_ignore_ascii_case(ADDITIONAL_TOOLS_ITEM) {
        return;
    }
    if let Some(tools) = item.get("tools").and_then(Value::as_array) {
        for tool in tools {
            push_tool(declarations, tool);
        }
    } else if let Some(names) = item.get("tool_names").and_then(Value::as_array) {
        let listed: Vec<String> = names.iter().map(string_value).collect();
        logging::log(
            "[Responses]",
            &format!(
                "⚠️ additional_tools 只给了工具名、没有定义，无法转发：{}",
                listed.join(", ")
            ),
        );
    }
}

fn push_tool(declarations: &mut ToolDeclarations, tool: &Value) {
    if string_field(tool, "type").eq_ignore_ascii_case(NAMESPACE_TOOL) {
        expand_namespace(declarations, tool);
    } else {
        declarations.push(tool.clone(), "");
    }
}

fn expand_namespace(declarations: &mut ToolDeclarations, namespace: &Value) {
    let ns_name = string_field(namespace, "name");
    let ns_desc = string_field(namespace, "description");
    let Some(children) = namespace.get("tools").and_then(Value::as_array) else {
        return;
    };
    for child in children {
        if string_field(child, "type").eq_ignore_ascii_case(NAMESPACE_TOOL) {
            logging::log(
                "[Responses]",
                &format!("⚠️ 嵌套 namespace 无法展开，已跳过：{}", string_field(child, "name")),
            );
            continue;
        }
        if declaration_name(child).is_empty() {
            continue;
        }
        let mut decl = child.clone();
        let nested = is_nested_function(&decl);
        if let Some(fields) = decl.as_object_mut() {
            fields.remove("defer_loading");
        }
        let fields = if nested {
            decl.get_mut("function").and_then(Value::as_object_mut)
        } else {
            decl.as_object_mut()
        };
        if let Some(fields) = fields {
            if !ns_desc.is_empty() {
                append_description(fields, &format!("\n\n[所属分组 `{ns_name}`] {ns_desc}"));
            }
            fields.remove("defer_loading");
        }
        declarations.push(decl, &ns_name);
    }
}

fn declaration_name(tool: &Value) -> String {
    if is_nested_function(tool) {
        tool.get("function").map(|function| string_field(function, "name")).unwrap_or_default()
    } else {
        string_field(tool, "name")
    }
}

fn set_declaration_name(tool: &mut Value, name: &str) {
    let fields = if is_nested_function(tool) {
        tool.get_mut("function").and_then(Value::as_object_mut)
    } else {
        tool.as_object_mut()
    };
    if let Some(fields) = fields {
        fields.insert("name".to_string(), Value::String(name.to_string()));
    }
}

fn append_description(fields: &mut Map<String, Value>, extra: &str) {
    let existing = fields.get("description").map(string_value).unwrap_or_default();
    fields.insert("description".to_string(), Value::String(format!("{existing}{extra}")));
}

/// 同时用于完整调用项和携带 name 的流式事件。
pub fn restore_call(item: Value, flat_name: &str, plan: &ToolPlan) -> Value {
    plan.names.restore(item, flat_name)
}

/// 工具声明在日志里的可读标签（`function:bash` / `web_search` / …）。
///
/// 丢工具时必须留下它叫什么：只报「丢了 N 条」，排查的人无从判断丢的是不是
/// 关键能力 —— 2026-09 那次 Codex 工具失效，症状是模型把调用当正文吐出来，
/// 若当时有这行日志，一眼就能定位。
pub fn tool_kind_label(tool: &Value) -> String {
    // 字符串简写（`tools: ["web_search"]`）：它自己就是名字
    if let Some(text) = tool.as_str() {
        let text = text.trim();
        return if text.is_empty() {
            "未命名工具".to_string()
        } else {
            text.to_string()
        };
    }
    let kind = string_field(tool, "type");
    // 名字两处可取：扁平写法的顶层 `name`，与嵌套 Chat 形态
    // （`{"type":"function","function":{"name":…}}`）里层的 `function.name`。
    // 只看顶层的话，标准 OpenAI 形态的工具在日志里全印成「function」，
    // 等于没报名字 —— 2026-10-01 排查「工具被整批剔除」时就吃了这个亏。
    let name = {
        let flat = string_field(tool, "name");
        if !flat.is_empty() {
            flat
        } else {
            tool.get("function")
                .map(|function| string_field(function, "name"))
                .unwrap_or_default()
        }
    };
    if name.is_empty() {
        if kind.is_empty() { "未命名工具".to_string() } else { kind }
    } else if kind.is_empty() || kind.eq_ignore_ascii_case("function") {
        name
    } else {
        format!("{kind}:{name}")
    }
}

/// 工具声明是否是「已经嵌套好的」Chat 形态（客户端混用两种形态时原样保留）
pub fn is_nested_function(tool: &Value) -> bool {
    string_field(tool, "type").eq_ignore_ascii_case("function") && tool.get("function").is_some()
}

/// 扁平 function 声明 → 嵌套 Chat 形态（`{type, name, parameters}` →
/// `{type:"function", function:{name, parameters}}`）。名字缺失时返回 `None`。
pub fn nested_from_flat(tool: &Value) -> Option<Value> {
    let name = string_field(tool, "name");
    if name.is_empty() {
        return None;
    }
    let parameters = tool
        .get("parameters")
        .filter(|value| is_truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut function = Map::new();
    function.insert("name".to_string(), Value::String(name));
    if let Some(description) = tool.get("description").filter(|value| is_truthy(value)) {
        function.insert("description".to_string(), description.clone());
    }
    function.insert("parameters".to_string(), parameters);
    // strict 是 Responses 的字段，Chat 的 function 里也认（部分上游支持）
    if let Some(strict) = tool.get("strict") {
        function.insert("strict".to_string(), strict.clone());
    }
    Some(json!({ "type": "function", "function": Value::Object(function) }))
}
