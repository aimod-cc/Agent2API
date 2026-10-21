//! 智谱清言请求构造：**把 OpenAI 的多轮 messages 拍平成单条文本**。
//!
//! ── 为什么必须拍平（2026-10 实测，这不是偷懒）──────────────────
//! `mainchat-api/engine/stream` 的 `messages` **只认最后一条 user 消息**，
//! 前面的历史与 `system` 全部被丢弃。实测对照（同一组 messages，只差拍平与否）：
//!
//! ```text
//!   原生数组 [user:记住88 / assistant:好的 / user:数字是几]
//!     → 上游答「我不知道你最喜欢的数字是多少哦」         ← 历史被丢弃
//!   拍平单条 "<|user|>…<|assistant|>…<|user|>…<|assistant|>"
//!     → 上游答「88」                                      ← 历史生效
//!
//!   原生数组 [system:只回答字母A / user:介绍你自己]
//!     → 上游答「你好！我是一个由Z.ai训练的大语言模型…」   ← system 被丢弃
//!   拍平单条（system 折成 <|sytstem|> 段）
//!     → 上游答「A」                                       ← system 生效
//! ```
//!
//! 所以「拍平」不是权宜之计，而是这一家**唯一能用**的多轮表达方式
//! （与 KukuAI 的 `build_prompt`、参考实现的 `messagesPrepare` 同一取舍）。
//!
//! ── 拍平格式（逐字复刻参考实现，包括那个拼错的标记）────────────
//! ```text
//!   <|sytstem|>      ← 注意：参考实现里 system 就是这个拼写（少个 s），
//!                      上游按这个形态训练/解析，**不要"顺手修正"**
//!   <|user|>
//!   <|assistant|>
//!   <|assistant|>    ← 末尾恒追加一个，提示模型该它说话了
//! ```
//! 但实测发现**上游其实不依赖这些标记**（把标记去掉、直接拼接也能得到正确结果）；
//! 保留它们是为了贴齐已知可用的形态，不是上游的硬要求。
//!
//! ── 标记会漏进官方客户端的聊天记录（用户实测反馈，必读）────────────
//! 拍平后的那段文本不只是"给模型看的提示词"，它**就是上游存下来的那条用户
//! 消息本身**：官方客户端会把 `<|user|>` / `<|assistant|>` 原样显示在气泡里
//! （实测截图：用户从本网关发「你好」，客户端里那条消息变成
//! `<|user|>\n你好\n<|assistant|>`）。因此：
//!
//!   · **只有一段用户输入时一律原样发**（见 [`plain_single_message`]）——
//!     单轮请求没有任何历史要表达，套标记纯粹是白脏一次对话记录。实测不带
//!     任何标记的单条消息上游照常正确回答（这本来就是它的常规形态）。
//!   · 多轮 / 带 system 时仍然必须拍平（见上：数组形态的历史与 system 都被
//!     丢弃）。那种情况下标记是**功能的代价**：要么历史与 system 失效，
//!     要么对话记录里出现标记 —— 两害相权取其轻，选后者并在这里写明。
//!
//! 另外还有几条实测否证（别再试了）：`meta_data.system_prompt`、
//! 顶层 `system_prompt` / `system`、`meta_data.system` **全都被忽略**
//! （用「无论问什么都只回答字母A」的 system 逐个试过，五个位置都答了正常
//! 自我介绍）。所以 system 不能靠换字段绕开，只能折进正文。
//!
//! ── 其它实测结论（写进实现，别照搬别家的假设）──────────────────
//!   - `content` **必须是数组**（`[{type:"text", text}]`）；用字符串直接 400
//!     `invalid param`；
//!   - 多模态块（`image_url` 等）混在数组里**不报错**，但上游是否采纳未验证 ——
//!     本实现只取 text 块，其余类型丢弃（不假装支持图片）；
//!   - `conversation_id` 可省略、`meta_data` 可省略（都实测通过），
//!     本实现恒发空串/最小 meta_data 以贴合官方形态；
//!   - `selected_model` 换值确实换模型（`glm-5.3` vs `glm-5.3-flash` 上游都认）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::protocol::{content_parts, string_field};

use super::endpoints;

/// 拍平历史上的 role 标记（**注意 system 的拼写**，见模块头）
const MARK_SYSTEM: &str = "<|sytstem|>";
const MARK_USER: &str = "<|user|>";
const MARK_ASSISTANT: &str = "<|assistant|>";

/// 单条消息拍平出来的最大字符数（防止把超长历史整段推给上游被拒）
const MAX_FLAT_CHARS: usize = 200_000;

/// 把一条消息的 content 摊成纯文本（只取 text 块，其余类型丢弃）。
///
/// 与 `protocol::content_text` 的差别：这里**只认 `text` 类型**，
/// 不认 `input_text` / `output_text`（那是 Responses API 的块名，
/// 本家上游没这个概念，混进来只会污染历史）。
fn message_text(content: &Value) -> String {
    match content {
        // 字符串形态：上游拒收（400 invalid param），但客户端可能这么传 ——
        // 在**入站**消化掉，不当错误往外抛（下游是 OpenAI 协议，字符串是合法的）
        Value::String(text) => text.clone(),
        other => {
            let mut out = String::new();
            for part in content_parts(other) {
                if let Some(text) = part.as_str() {
                    out.push_str(text);
                    continue;
                }
                if string_field(part, "type").eq_ignore_ascii_case("text") {
                    out.push_str(&string_field(part, "text"));
                }
            }
            out
        }
    }
}

/// 拍平一批 OpenAI messages 成一条上游消息的文本。
///
/// 空消息（内容全空白）会被跳过：上游对空段没有意义，留着只会稀释上下文。
/// 末尾恒追加 `<|assistant|>`（见模块头）。
pub fn flatten_messages(messages: &[Value]) -> String {
    let mut out = String::new();
    for message in messages {
        let role = string_field(message, "role");
        let text = message_text(message.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        let mark = match role.as_str() {
            "system" | "developer" => MARK_SYSTEM,
            "assistant" => MARK_ASSISTANT,
            // user 与一切未知 role（tool 等）都当作用户侧输入：
            // 上游没有工具消息的概念，丢掉会让上下文断裂，当作输入最不容易出错
            _ => MARK_USER,
        };
        if out.chars().count() >= MAX_FLAT_CHARS {
            break;
        }
        out.push_str(mark);
        out.push('\n');
        out.push_str(&text);
        out.push('\n');
    }
    out.push_str(MARK_ASSISTANT);
    out.push('\n');
    out
}

/// 只有一段用户输入时给出的**原样文本**（否则 `None`，交给 [`flatten_messages`]）。
///
/// ── 判据 ────────────────────────────────────────────────────
/// 跳过空白消息之后，剩下的必须**恰好一条**，且它的 role 不是
/// `system` / `developer` / `assistant` —— 那三种在拍平里带着语义
/// （system 段与"这句话是模型说的"），省掉标记就不成立了。
///
/// ── 为什么要这么做（见模块头）────────────────────────────────
/// 拍平文本就是上游存下来的那条用户消息，标记会被官方客户端显示出来。
/// 单轮请求（面板的「测试」按钮、只发一条消息的客户端）没有任何历史要表达，
/// 原样发既不影响结果，又不脏对话记录。
///
/// 返回的是**未 trim 的原文**：那是用户自己敲的字，一个空格都不该替他改。
fn plain_single_message(messages: &[Value]) -> Option<String> {
    let mut found: Option<String> = None;
    for message in messages {
        let text = message_text(message.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let role = string_field(message, "role");
        if matches!(role.as_str(), "system" | "developer" | "assistant") {
            return None;
        }
        found = Some(text);
    }
    found
}

/// 从客户端请求体里读多轮消息。
///
/// 两种形态都吃：`messages`（Chat Completions）优先；
/// 没有 `messages` 时退回 `prompt` / `input`（Responses 与简化客户端会这么发）。
fn read_messages(body: &Value) -> Vec<Value> {
    if let Some(list) = body.get("messages").and_then(Value::as_array) {
        if !list.is_empty() {
            return list.clone();
        }
    }
    // 退回 prompt（字符串 / 数组两种形态）
    if let Some(prompt) = body.get("prompt") {
        match prompt {
            Value::String(text) if !text.trim().is_empty() => {
                return vec![json!({ "role": "user", "content": text })];
            }
            Value::Array(items) => {
                let mut out: Vec<Value> = Vec::new();
                for item in items {
                    if let Some(text) = item.as_str() {
                        out.push(json!({ "role": "user", "content": text }));
                    } else if item.is_object() {
                        out.push(item.clone());
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

/// 一次上游对话请求的构造结果
pub struct ChatPlan {
    pub url: String,
    /// 上游请求体（已是本家信封）
    pub body: Value,
}

/// 构造 `POST /chatglm/mainchat-api/engine/stream` 的请求体。
///
/// `model` 是**上游真名**（已由编排层按映射改写）：本家上游认
/// `selected_model`（如 `glm-5.3` / `glm-5.3-flash`）。空模型走上游自己的默认
/// （`result.default` 那一组）。
pub fn build_chat_body(body: &Value, model: &str) -> Value {
    let messages = read_messages(body);
    // 单轮原样发、多轮才拍平（理由见模块头：标记会显示在官方客户端里）
    let flat = plain_single_message(&messages)
        .unwrap_or_else(|| flatten_messages(&messages));
    // reasoning_effort：客户端显式传了就透传（上游认这个键，取值 "")/thinking/
    // deep_thinking 三档由 `meta_data.chat_mode` 与 `reasoning_effort` 共同表达）
    let chat_mode = body
        .get("chat_mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let reasoning_effort = body
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let is_networking = body
        .get("is_networking")
        .or_else(|| body.get("enable_search"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut meta = json!({
        "platform": endpoints::APP_PLATFORM,
        "chat_mode": chat_mode,
        "selected_model": model,
        "is_networking": is_networking,
    });
    if !reasoning_effort.is_empty() {
        if let Some(object) = meta.as_object_mut() {
            object.insert(
                "reasoning_effort".to_string(),
                Value::String(reasoning_effort),
            );
        }
    }
    json!({
        "conversation_id": "",
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": flat }],
        }],
        "meta_data": meta,
    })
}

/// 组装完整计划（URL + 体）
pub fn build_chat_plan(body: &Value, model: &str) -> ChatPlan {
    ChatPlan {
        url: endpoints::url(endpoints::PATH_ENGINE_STREAM),
        body: build_chat_body(body, model),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_text(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }] })
    }

    /// 多轮拍平：三段历史都在、顺序正确、末尾补 assistant 标记
    #[test]
    fn flattens_multi_turn_history() {
        let flat = flatten_messages(&[
            user_text("我最喜欢的数字是 88"),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "好的" }] }),
            user_text("是多少？"),
        ]);
        assert!(flat.starts_with("<|user|>\n我最喜欢的数字是 88\n"));
        assert!(flat.contains("<|assistant|>\n好的\n"));
        assert!(flat.ends_with("<|user|>\n是多少？\n<|assistant|>\n"));
    }

    /// system 段用上游认的那个拼写（少一个 s 是刻意的）
    #[test]
    fn system_mark_keeps_upstream_spelling() {
        let flat = flatten_messages(&[
            json!({ "role": "system", "content": [{ "type": "text", "text": "只回答A" }] }),
            user_text("你好"),
        ]);
        assert!(flat.starts_with("<|sytstem|>\n只回答A\n"), "实际: {flat}");
        assert!(!flat.contains("<|system|>"), "不要'修正'上游的拼写: {flat}");
    }

    /// developer role 并入 system（OpenAI 新协议的角色）
    #[test]
    fn developer_role_maps_to_system() {
        let flat = flatten_messages(&[
            json!({ "role": "developer", "content": [{ "type": "text", "text": "规则" }] }),
            user_text("问题"),
        ]);
        assert!(flat.starts_with("<|sytstem|>\n规则\n"));
    }

    /// content 是字符串时也能消化（上游拒收字符串，在入站就摊平）
    #[test]
    fn string_content_is_flattened() {
        let flat = flatten_messages(&[json!({ "role": "user", "content": "纯字符串" })]);
        assert!(flat.contains("纯字符串"));
    }

    /// 非 text 块被丢弃（不假装支持图片）
    #[test]
    fn non_text_parts_are_dropped() {
        let flat = flatten_messages(&[json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "看这张图" },
                { "type": "image_url", "image_url": { "url": "https://example.com/a.png" } },
            ],
        })]);
        assert!(flat.contains("看这张图"));
        assert!(!flat.contains("example.com"), "图片块不应混进文本: {flat}");
    }

    /// 空消息被跳过（不留空段稀释上下文）
    #[test]
    fn empty_messages_are_skipped() {
        let flat = flatten_messages(&[
            user_text("   "),
            json!({ "role": "assistant", "content": [] }),
            user_text("真问题"),
        ]);
        assert_eq!(flat.matches("<|user|>").count(), 1);
        assert!(flat.contains("真问题"));
    }

    /// 请求体形态：content 必须是数组、带 meta_data.selected_model
    #[test]
    fn builds_upstream_envelope() {
        let body = build_chat_body(
            &json!({ "messages": [{ "role": "user", "content": "hi" }] }),
            "glm-5.3-flash",
        );
        assert_eq!(body["conversation_id"], json!(""));
        // content 必须是数组（字符串会被上游 400）
        assert!(body["messages"][0]["content"].is_array());
        assert_eq!(body["messages"][0]["content"][0]["type"], json!("text"));
        assert_eq!(body["meta_data"]["selected_model"], json!("glm-5.3-flash"));
        assert_eq!(body["meta_data"]["platform"], json!("pc"));
    }

    /// 客户端显式传的档位与联网开关透传
    #[test]
    fn passes_through_effort_and_networking() {
        let body = build_chat_body(
            &json!({
                "messages": [{ "role": "user", "content": "hi" }],
                "chat_mode": "zero",
                "reasoning_effort": "deep_thinking",
                "is_networking": true,
            }),
            "glm-5.3",
        );
        assert_eq!(body["meta_data"]["chat_mode"], json!("zero"));
        assert_eq!(body["meta_data"]["reasoning_effort"], json!("deep_thinking"));
        assert_eq!(body["meta_data"]["is_networking"], json!(true));
    }

    /// 不传档位时不塞空字段（保持最小 meta_data）
    #[test]
    fn omits_effort_when_absent() {
        let body = build_chat_body(&json!({ "messages": [{ "role": "user", "content": "hi" }] }), "m");
        assert!(body["meta_data"].get("reasoning_effort").is_none());
    }

    /// 退回 prompt（没有 messages 的简化客户端）
    #[test]
    fn falls_back_to_prompt() {
        let body = build_chat_body(&json!({ "prompt": "直接问" }), "m");
        let text = body["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(text.contains("直接问"));
    }

    /// URL 指向 engine/stream
    #[test]
    fn plan_targets_engine_stream() {
        let plan = build_chat_plan(&json!({ "messages": [user_text("x")] }), "m");
        assert!(plan.url.ends_with("/chatglm/mainchat-api/engine/stream"));
    }

    /// **单轮请求不套任何标记**（用户实测反馈：客户端里出现了 `<|user|>`）。
    ///
    /// ── 锁的是真实发生过的现象 ──────────────────────────────────
    /// 从面板「测试」发一句「你好」，官方客户端的聊天记录里那条消息变成
    /// `<|user|>\n你好\n<|assistant|>` —— 因为拍平文本就是上游存下来的
    /// 用户消息本身。单轮没有历史要表达，原样发即可（实测不带标记上游照常答）。
    #[test]
    fn single_message_goes_out_verbatim() {
        let body = build_chat_body(&json!({ "messages": [user_text("你好")] }), "m");
        let text = body["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert_eq!(text, "你好", "单轮必须原样发，一个标记都不该有");
        assert!(!text.contains("<|"), "标记会显示在官方客户端里: {text}");

        // 前后夹着空消息也仍然算单轮（空段本来就被跳过）
        let body = build_chat_body(
            &json!({ "messages": [
                user_text("   "),
                json!({ "role": "assistant", "content": [] }),
                user_text("你好"),
            ] }),
            "m",
        );
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("你好"));
    }

    /// 原样发时**不 trim 用户自己的字**，也不因为别的 role 而误判
    #[test]
    fn verbatim_keeps_user_whitespace_and_tolerates_unknown_roles() {
        let body = build_chat_body(
            &json!({ "messages": [user_text("  空行保留\n\n段落  ")] }),
            "m",
        );
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("  空行保留\n\n段落  "),
            "用户自己敲的空白不该被网关改掉"
        );
        // 未知 role（tool 等）按用户侧输入处理 —— 单轮时同样原样发
        let body = build_chat_body(
            &json!({ "messages": [{ "role": "tool", "content": "工具结果" }] }),
            "m",
        );
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("工具结果"));
    }

    /// 带 system / 多轮时**仍然拍平**（标记有语义，不能省）
    #[test]
    fn multi_turn_or_system_still_flattens() {
        for messages in [
            // 多条用户消息
            json!([user_text("一"), user_text("二")]),
            // system + 用户
            json!([
                { "role": "system", "content": [{ "type": "text", "text": "只回答A" }] },
                user_text("你好"),
            ]),
            // assistant 历史
            json!([
                user_text("记住88"),
                { "role": "assistant", "content": [{ "type": "text", "text": "好的" }] },
                user_text("是多少"),
            ]),
        ] {
            let body = build_chat_body(&json!({ "messages": messages }), "m");
            let text = body["messages"][0]["content"][0]["text"]
                .as_str()
                .unwrap_or("");
            assert!(
                text.contains("<|"),
                "多轮 / 带 system 必须拍平（否则历史与 system 会被上游丢弃）: {text}"
            );
        }
    }
}
