//! **长度截断的自动续写**：上游这一轮说到一半就停了，网关自己把它问完。
//!
//! ── 为什么需要它 ────────────────────────────────────────────
//! 上游的 turn 会**中途收尾**：输出预算用尽（`finish_reason: length` 一族）、连接被
//! 掐断、或者模型把整轮都花在思考通道里、正文只吐了一两个词。客户端拿到的是
//! **半截答案**，而它没法区分「模型就说这么多」与「被腰斩」—— 前端显示出来的
//! 是一段没有结尾标点的文字，用户以为模型坏了。
//!
//! 本模块的做法与参考实现（社区件 Trae2api-cn 的 `src/continuation.py`）一致：
//! 判出「这一轮像被截断」，就把**已经写出的那半截**当作 assistant 回合回放给上游，
//! 追一句「请继续输出」，再打一次 —— 新内容**接在同一条客户端响应后面**，
//! 而不是让客户端收到半截再自己拼。
//!
//! ── 判据：六条，按参考实现的顺序逐条对应 ────────────────────
//! 判定函数 [`looks_truncated`] 是**唯一**的一份判据清单，两条链路都读它
//! （非流式读聚合好的 chat 体，流式读 [`TurnAccumulator`] 从下发帧里攒出来的同一份
//! 三元组）。写第二份「增量版」判据是这里的头号风险：加一条判据漏改一处，
//! 就会出现「非流式续写、流式不续」这种查得出现象查不出根因的分歧。
//!   1. **工具轮永不续**（最高优先级，在 length 判定之前）：调用点的执行权在客户端，
//!      把它的 tool_calls 续写一遍等于伪造一次工具调用；
//!   2. `finish_reason` 命中 length 别名表（8 个写法，见 [`LENGTH_FINISH_ALIASES`]）；
//!   3. 正文为空但**有思考**：整轮烧在思考通道里，该再问一次（正文与思考都空 → 交给
//!      既有的空答复兜底链，不归本模块管）；
//!   4. 代码围栏 ``` 数量为**奇数**（数整篇，未闭合的围栏是硬证据）；
//!   5. 末 100 字符的括号栈失衡（先给「散文里零星括号」留 +2 容忍，再做严格配对）；
//!   6. 末 100 字符 rstrip 后落在悬挂尾部集合里（`xxx,` / `xxx|` / `xxx...` / `xxx:` 等
//!      「结构写到一半」的形态），或「长思考 + 极短正文且正文不以句末符号收尾」。
//!
//! ── 停机条件（两道）────────────────────────────────────────
//!   - 轮数上限：[`config::auto_continue_max_rounds`]（默认 3）；
//!   - **相似度闸**：新续出来的正文与上一轮逐字符位置相似度超过 0.5 就停 ——
//!     这是防「模型每次都在同一个位置吐同一小段」的死循环（参考实现同款）。
//!
//! ── 代价（必须说清）────────────────────────────────────────
//!   - **多轮 = 多次上游往返**：一轮的 prompt_tokens 含前几轮的全部历史，所以真实
//!     成本是各轮**求和**（不是覆盖）。合并口径见 [`sum_tokens`]，收口时由续写侧
//!     调一次 `report_usage` 覆盖上去（网关侧的报表与限流记账因此拿到总量）。
//!     客户端**看到的**流式 usage 帧：出现时机变了，内容不变，本模块也不合成总量帧。
//!     终帧**之前**到的 usage 帧照原样实时透传；终帧**之后**（含与终帧同帧）的那些
//!     与终帧一起被压住 —— 续写时整段丢弃，不续写时原样放行。OpenAI 的标准形态
//!     （`stream_options.include_usage`）正是把 usage 帧放在 finish **之后**，所以
//!     走上续写的那条流里，客户端只能看到**最后一轮**的 usage 帧 —— 已知不完全，
//!     但不伪造字节：合成一帧上游根本没发过的 usage，比数字不全更坏。
//!   - **同一份对话会被再发一遍**：续写轮走的是同一条转发链路（同一个
//!     `UpstreamService::forward`），因此它**会重新走一次选路** —— 可能落到另一个
//!     账号、甚至另一个供应商（只要该 Key 的提供商白名单允许）。对账号隔离要求严格
//!     的部署，这是决策信息：开这个开关等于允许「同一次客户端请求的内容出现在多个
//!     上游账号上」。
//!   - 转发阶段会被最后一轮决定；账号归属同样是「最后一次为准」——续写换到别家时，
//!     请求日志的那一行归到最后承载的那一家（与跨家降级同一口径）。
//!   - 流式路径把**终帧**（带非 null `finish_reason` 的那一帧与 `[DONE]`）压住不发，
//!     直到本轮说完才决定「放行」或「丢掉续下一轮」。正文帧仍是**实时**下发的，
//!     所以只有收尾这一拍被推迟到 EOF（通常紧跟着就到），不是整段缓冲。
//!
//! ── 不碰客户端的参数（包括 max_tokens）─────────────────────
//! 续写轮发出去的是**原始请求体 + 追加的两个回合**，`max_tokens` / `temperature` /
//! `tools` 等一律照抄。上游因为「输出预算到顶」而截断时，放宽预算是另一件事
//! （那是网关替客户端改参数，客户端下一次收到的上限就不是它自己定的了），
//! 与本模块的「把话说完」是两种干预。要做也得单独立项、单独开关。
//!
//! ── 默认关 ─────────────────────────────────────────────────
//! 开关是 `autoContinue`（见 `config::auto_continue`），关着时调用侧**根本不包装**，
//! 下发字节与接入前逐字一致。打开的理由要经得起审计：续写会多烧一份上游配额，
//! 且改的是「客户端看到什么」这种可观测行为，不是纯旁路。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap / expect / panic。

use std::io;
use std::sync::Arc;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::Stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::server::config;
use crate::server::core::key_scope::KeyScope;
use crate::server::core::upstream::{ForwardOutcome, ForwardRequest, UpstreamService, usage};
use crate::server::core::upstream::refusal;
use crate::server::logging;

/// 长度别名表：上游声明「输出预算用尽」的全部写法（参考实现的
/// `_LENGTH_FINISH_REASONS` 逐字对应）。少认一个就等于把那一族的截断整族漏掉，
/// 所以宁可收进来没见过的写法，也不要漏。
const LENGTH_FINISH_ALIASES: &[&str] = &[
    "length",
    "max_tokens",
    "max_output_tokens",
    "max_token",
    "token_limit",
    "model_context_window_exceeded",
    "context_window_exceeded",
    "context_length_exceeded",
];

/// 悬挂尾部的**单字符**集合（rstrip 之后看末字符）。
///
/// 参考实现这里是 8 条正则，逐条看完可以等价化成集合判定，不必引入正则：
///   - `,$`、`[|｜]$`、`\\$`、`//$`、`#$`、`[-–—]$`、`[:：]$` 都是「以某个字符结尾」；
///   - `//` 是「以 `/` 结尾」的子集（末尾两个斜杠时末字符也是斜杠），合并；
///   - `\.\.\.$`（省略号）单独按 `ends_with("...")` 判 —— **单个句点不算截断**，
///     正常句子以 `.` 收尾是常态，这条是集合与省略号分开的唯一理由。
const DANGLING_ENDING_CHARS: &[char] = &[
    ',', '|', '｜', '\\', '/', '#', '-', '–', '—', ':', '：',
];

/// 句末符号集合（参考实现的 `_SENTENCE_END_RE` 字符类逐字对应）。
///
/// 用途只有一个：「长思考 + 极短正文」那条判据里，正文**不以**这些符号收尾时
/// 才认为没说完。含引号的闭合形态（`”` `’` `」`）是有意收全的：中文答案常以
/// 右引号结束，漏了会把「说完了」判成半截。
const SENTENCE_ENDING_CHARS: &[char] = &[
    '.', '!', '?', '。', '！', '？', ']', ')', '}', '`', '"', '\'', '”', '’', '』', '」',
];

/// 括号配对扫描的回看长度（参考实现 `body[-100:]`）。
const TAIL_SCAN_CHARS: usize = 100;

/// 「极短正文」的上限（参考实现 `_FRAGMENT_TEXT_LIMIT`）。
const FRAGMENT_TEXT_MAX_CHARS: usize = 40;

/// 「长思考」的下限（参考实现 `_FRAGMENT_REASONING_MIN`）。
const FRAGMENT_REASONING_MIN_CHARS: usize = 200;

/// 思考与正文的倍数（参考实现 `_FRAGMENT_REASONING_RATIO`：思考至少比正文长 4 倍）。
///
/// 按参考实现的两条常量算，这一道闸**当前永不生效**：正文上限 40 字符 ⇒ 倍数门槛
/// 最高 160，而思考下限已经是 200。留着它不是留着死代码，而是它是这条判据的
/// 定义的一部分（改任何一条常量都会让它重新承重）；真要清掉它，得连 40 与 200
/// 一起改，别单独删这一行。
const FRAGMENT_REASONING_RATIO: usize = 4;

/// 相似度停机阈值（参考实现 `_DEFAULT_SIMILARITY_THRESHOLD`）。
///
/// 判据是**严格大于**：正好 0.5 时再续一轮（与参考实现一致），因为逐字符位置
/// 相似度 0.5 更像是「接着写了一段不同的话」而不是复读。
const SIMILARITY_STOP: f64 = 0.5;

/// 一轮里「终止帧之后的补帧」最多压住多少字节（见 `stream()` 的 held 累积）。
///
/// 这个缓冲的存在理由是保住帧序（有些上游在 `finish_reason` 之后才补 usage 帧，
/// 若只压终止帧就会出现 usage 在 finish 之前 —— 那种顺序变化是本模块自己造的）。
/// 但压住的量**不能由上游决定**：终止帧之后一直补帧的上游（坏掉的或恶意的）会让
/// 它无界增长。1 MiB 是量级选择：正常形态（一帧 usage、偶尔几帧）连 1/1000 都用不到，
/// 而任何超过它的上游都已经不是「补几帧」而是「刷流」。
const MAX_HELD_BYTES: usize = 1024 * 1024;

/// 续写指令（参考实现 `CONTINUE_INSTRUCTION` 逐字照抄）。
///
/// 这条文本会进上游请求体，措辞是上游侧调过的（「不要重复已经写过的内容」是
/// 防复读的一半保险，另一半是 [`SIMILARITY_STOP`]）。
pub const CONTINUE_INSTRUCTION: &str =
    "请继续输出，从你中断的地方继续，不要重复已经写过的内容，不要重新开始。";

/// 「整轮只有思考、正文为空」时用的指令（参考实现 `REASONING_ONLY_INSTRUCTION`）。
///
/// 保持英文：参考实现如此，且这条的读者是模型而非用户。
pub const REASONING_ONLY_INSTRUCTION: &str = "Your previous response contained only \
    thinking/reasoning with no actual output. Provide the actual response now - either \
    text content or a tool call.";

/// 一轮上游答复的**判据输入**：四个读数。
///
/// 两条链路（非流式的 chat 体、流式的下发帧）都先把各自的数据折成这一份，再交给
/// [`looks_truncated`] —— 折的时候各读各的字段名，判的时候共用一份清单。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnParts {
    /// 可见正文（`message.content` / 各帧 `delta.content` 拼接）
    pub text: String,
    /// 思考内容（`reasoning_content` 与 `reasoning` 两种键名都算，见聚合侧同一口径）
    pub reasoning: String,
    /// 本轮出现过工具调用
    pub has_tool_calls: bool,
    /// 本轮的 `finish_reason`（None = 上游没给终帧，例如 officeace 那种缺终止帧的流）
    pub finish: Option<String>,
}

/// `finish_reason` 是否属于「输出预算用尽」一族（大小写不敏感，与
/// `refusal::is_refusal` 同一比较口径）。
pub fn is_length_finish(finish: &str) -> bool {
    LENGTH_FINISH_ALIASES
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(finish))
}

/// 这一轮像不像**说到一半就停了**（六条判据见模块头，顺序与参考实现逐条对应）。
pub fn looks_truncated(parts: &TurnParts) -> bool {
    // 1. 工具轮永不续（在 length 判定之前，理由见模块头第 1 条）
    if parts.has_tool_calls {
        return false;
    }
    // 2. 上游自己说「预算用尽」
    if parts
        .finish
        .as_deref()
        .is_some_and(|finish| is_length_finish(finish))
    {
        return true;
    }
    let body = parts.text.as_str();
    if body.trim().is_empty() {
        // 3. 正文为空：有思考就再问一次；两者都空交给既有的空答复兜底链
        return !parts.reasoning.trim().is_empty();
    }
    // 4. 代码围栏未闭合（数整篇）
    if unclosed_code_fence(body) {
        return true;
    }
    let tail = last_chars(body, TAIL_SCAN_CHARS);
    // 5. 末段括号失衡
    if unbalanced_brackets(&tail) {
        return true;
    }
    // 6a. 悬挂尾部
    if dangling_tail(&tail) {
        return true;
    }
    // 6b. 长思考 + 极短正文且不以句末符号收尾
    stopped_after_thinking(body, &parts.reasoning)
}

/// 整轮只有思考、没有可见正文（决定追哪一条指令用，参考实现同名函数）。
pub fn looks_reasoning_only(parts: &TurnParts) -> bool {
    !parts.has_tool_calls && parts.text.trim().is_empty() && !parts.reasoning.trim().is_empty()
}

/// 逐字符**位置**相似度（参考实现 `similarity` 逐字对应）：取两者较短的长度做窗口，
/// 数同一位置上字符相同的比例。
///
/// 不是编辑距离、不是前缀长度：参考实现选这个度量是因为它便宜且对「复读同一段
/// 开头」敏感，而这正是死循环的形状。换度量等于换停机条件，别顺手「优化」。
pub fn similarity(left: &str, right: &str) -> f64 {
    let overlap = std::cmp::min(left.chars().count(), right.chars().count());
    if overlap == 0 {
        return 0.0;
    }
    let same = left
        .chars()
        .zip(right.chars())
        .filter(|(a, b)| a == b)
        .count();
    same as f64 / overlap as f64
}

/// 续出来的这一轮是不是上一轮的**复读**（超过阈值即停，见 [`SIMILARITY_STOP`]）。
pub fn is_repeat(previous: &str, next: &str) -> bool {
    similarity(previous, next) > SIMILARITY_STOP
}

/// 把请求体改写成「续写这一轮」的形状：已写出的半截当 assistant 回合回放，
/// 再追一句 user 指令（参考实现 `continued_messages` 的形态）。
///
/// 返回值 false = 这个 body 里没有 `messages` 数组（改不动，调用侧据此不续）。
/// 回放用的是 **原文**（不 trim）：判据用 trim 后的形状，但发回上游的半截要逐字
/// 是上游自己写出来的那份，否则「从你中断的地方继续」对不上它自己的语气。
pub fn append_continuation(body: &mut Value, partial: &str, reasoning_only: bool) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    if !partial.trim().is_empty() {
        messages.push(json!({"role": "assistant", "content": partial}));
    }
    let instruction = if reasoning_only {
        REASONING_ONLY_INSTRUCTION
    } else {
        CONTINUE_INSTRUCTION
    };
    messages.push(json!({"role": "user", "content": instruction}));
    true
}

/// 从非流式的 chat 完成体里读判据输入（`choices[0].message` 那一层）。
///
/// 字段名与 `aggregate::CompletionAccumulator` 写出的形状一致：`content` 是字符串
/// （上游给 null 时按空串）、`reasoning_content` 可能有、`tool_calls` 可能有、
/// `finish_reason` 恒在（缺省时聚合侧写 "stop"）。
pub fn parts_of_completion(body: &Value) -> Option<TurnParts> {
    let choice = body.get("choices").and_then(Value::as_array)?.first()?;
    let message = choice.get("message")?;
    let mut parts = TurnParts {
        text: message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        reasoning: message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        has_tool_calls: message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty()),
        finish: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .map(str::to_string),
    };
    // 少数兼容实现把思考放在 `reasoning`（聚合侧两个键都读，见 aggregate.rs 的
    // reasoning 追加分支）；只在前者缺席时取后者，避免同一份思考数两遍。
    if parts.reasoning.is_empty() {
        parts.reasoning = message
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    Some(parts)
}

/// 把各轮的**可见正文**接成一条（用 `join("")`：上游被腰斩处没有换行，
/// 中间插空格或换行会改坏 markdown 与代码块的拼接位置）。
pub fn join_text(rounds: &[String]) -> String {
    rounds.concat()
}

/// 把「到目前为止、客户端会看到的整体」组装成判据输入。
///
/// ── 为什么判定读整体而不是本轮那一段（实测踩出来的）────────────
/// 结构性判据（未闭合 ```、括号失衡、悬挂尾部）问的是「**答案**写完了没有」，
/// 而续写场景里的答案是**跨轮拼接**的一个整体：上一轮留开的那道 ``` 正是这一轮
/// 关掉的。按本轮自己那段判就会把「已经补完」读成「又断了」——
/// 2026-10-14 拿假上游实测到这条：fence 那条判据连打三轮，客户端最终看到的是
/// `…return 42\n```\n上面的函数返回 42。` **后面又接了一遍同一段**。
/// 误判的代价从来不是「少说两句」，而是把重复内容钉在答案尾巴上。
///
/// ── 为什么 `finish` 与 `has_tool_calls` 取本轮 ──────────────
/// 这两条是**本轮的事实**：`finish_reason: length` 说的是这一轮把预算打光了，
/// 工具调用也是这一轮发起的（前一轮没有不代表这一轮没有 —— 累积或覆盖都会漏掉
/// 当前这一轮的信号，而工具轮必须一口咬定不续）。
fn verdict_of(
    accumulated_text: &str,
    accumulated_reasoning: &str,
    any_tool_calls: bool,
    current: &TurnParts,
) -> TurnParts {
    TurnParts {
        text: accumulated_text.to_string(),
        reasoning: accumulated_reasoning.to_string(),
        has_tool_calls: any_tool_calls || current.has_tool_calls,
        finish: current.finish.clone(),
    }
}

/// 多轮 usage 的合并（口径见模块头「代价」条）。
///
/// 四项**全部求和**：续写的每一轮都是一次真实的上游往返，第 N 轮的 prompt 里装着
/// 前 N-1 轮的全部内容（上游就是这么计费的），`cache_read` 同样按轮发生
/// （每轮各自命中一次缓存）。求和后，请求日志与限流记账读到的才是「这条客户端
/// 请求实际花了多少」。
///
/// 与 `RequestTelemetry::report_usage` 的「最后一次为准」**不冲突**：那条服务的是
/// 同一条上游流里补发的增量帧（后到的即最终值），这里服务的是**跨轮** ——
/// 由续写侧在收口时调一次 `report_usage`，把合并值当作最后一次上报覆盖上去。
///
/// 求和走 [`usage::UsageTokens`]（而不是把两份 usage JSON 相加）是有意的：别名表
/// 与「第一个正数」的口径只有一份（`usage::extract_usage`），这里再造一份就等于
/// 允许两边的字段认知分叉。
pub fn sum_tokens(rounds: &[usage::UsageTokens]) -> usage::UsageTokens {
    rounds.iter().fold(usage::UsageTokens::default(), |mut acc, round| {
        acc.prompt += round.prompt;
        acc.completion += round.completion;
        acc.total += round.total;
        acc.cache_read += round.cache_read;
        acc
    })
}

/// 合并后的 token 数 → 一份规范 OpenAI 形状的 usage 对象。
///
/// 两个用途：喂给 `report_usage`（它的提取侧认这批键），以及写进非流式 stitched
/// 体的 `usage` 字段。后者**改了客户端看到的形状**（上游原本可能给驼峰别名），
/// 这是跨轮求和后唯一诚实的写法：没有任何一轮的原始 usage 能代表总量。
/// 只在真的续写过（≥2 轮）时才走这里，单轮响应一个字节都不动。
pub fn usage_json(tokens: &usage::UsageTokens) -> Value {
    json!({
        "prompt_tokens": tokens.prompt,
        "completion_tokens": tokens.completion,
        "total_tokens": tokens.total,
        "prompt_tokens_details": { "cached_tokens": tokens.cache_read },
    })
}

/// 从一份 chat 完成体里取 token 数（缺失 / 非对象都按全 0，与单轮口径一致）。
pub fn tokens_of_completion(body: &Value) -> usage::UsageTokens {
    body.get("usage")
        .and_then(usage::extract_usage)
        .unwrap_or_default()
}

/// ``` 围栏数量为奇数（参考实现 `_unclosed_code_fence`）。
fn unclosed_code_fence(text: &str) -> bool {
    text.matches("```").count() % 2 == 1
}

/// 取末尾至多 `n` 个**字符**（不是字节：中文按字节切会切出半个字）。
fn last_chars(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(n);
    chars[start..].iter().collect()
}

/// 末段括号是否失衡（参考实现 `_unbalanced_brackets`，两段判据都要）。
///
/// 先按**计数差**（opens > closes + 2）判：给散文与表格里零散的右括号留容忍；
/// 再做**配对走查**：末段以一个还开着的分组结束（`{a: [1, 2}` 这种）是硬证据。
///
/// ── 与参考实现**故意不同**的一处（照抄会误判）────────────────
/// 参考实现遇到「闭合符对不上最内层开括号」或「栈空时来了闭合符」就直接返回
/// truncated。那是它自己的 docstring 想抓的反面：它说「以还开着的分组结束是
/// 无歧义的」，但落盘时把「窗口里出现落单的右括号」也算了进去。而我们扫的是
/// **末 100 个字符**这个窗口 —— 一篇写完的代码答案，窗口左边界很可能正好切在
/// 某个 `{` 与它的 `}` 之间，于是结尾那个 `}` 在窗口里成了「落单的闭合符」，
/// 一条完整答案被判成半截、白花一轮配额并往答案后面接一坨重复。
/// 误判的代价（改坏客户端看到的内容）比漏判（回到今天的形态）重，所以这里
/// **只认「走完仍未闭合」**：对不上的闭合符按噪声跳过，不留栈、不点亮。
/// 参考实现举的那个例子 `{a: [1, 2}` 仍然抓到（`}` 对不上 `[` 被跳过，
/// 走到结束时 `{` 与 `[` 都还开着）。
fn unbalanced_brackets(tail: &str) -> bool {
    let opens = tail.chars().filter(|c| matches!(c, '[' | '{' | '(')).count();
    let closes = tail.chars().filter(|c| matches!(c, ']' | '}' | ')')).count();
    if opens > closes + 2 {
        return true;
    }
    let mut stack: Vec<char> = Vec::new();
    for c in tail.chars() {
        match c {
            '[' | '{' | '(' => stack.push(c),
            ')' | ']' | '}' => {
                // 配对成功才弹栈；对不上（含空栈）按散文噪声跳过，理由见函数头
                if matching_opener(c).is_some_and(|opener| stack.last() == Some(&opener)) {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    !stack.is_empty()
}

/// 闭合符对应的开括号（不是闭合符时 None）。
fn matching_opener(closer: char) -> Option<char> {
    match closer {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

/// rstrip 之后是否落在悬挂尾部形态（判据集合化见 [`DANGLING_ENDING_CHARS`]）。
fn dangling_tail(tail: &str) -> bool {
    let trimmed = tail.trim_end();
    if trimmed.ends_with("...") {
        return true;
    }
    trimmed
        .chars()
        .last()
        .is_some_and(|c| DANGLING_ENDING_CHARS.contains(&c))
}

/// 「长思考 + 极短正文且正文没说完」（参考实现 `_stopped_after_thinking`）。
///
/// 形状：模型把整轮预算烧在思考通道，正文只吐一两个词。那半截被当成最终答案
/// 交出去，用户看不出「其实什么都没写」。续一轮的代价远小于交半截。
/// 复读保护由 [`is_repeat`] 兜住（参考实现注释同义）。
fn stopped_after_thinking(text: &str, reasoning: &str) -> bool {
    let visible = text.trim();
    let thinking = reasoning.trim();
    let visible_chars = visible.chars().count();
    if visible_chars == 0 || visible_chars > FRAGMENT_TEXT_MAX_CHARS {
        return false;
    }
    let thinking_chars = thinking.chars().count();
    if thinking_chars < FRAGMENT_REASONING_MIN_CHARS {
        return false;
    }
    if thinking_chars < visible_chars * FRAGMENT_REASONING_RATIO {
        return false;
    }
    !ends_with_sentence_mark(visible)
}

/// 是否以句末符号收尾（`visible` 已 trim，所以看末字符即可，不必再允许尾随空白）。
fn ends_with_sentence_mark(visible: &str) -> bool {
    visible
        .chars()
        .last()
        .is_some_and(|c| SENTENCE_ENDING_CHARS.contains(&c))
}

/// 流式路径的判据输入累积器：从**下发帧**里攒出 [`TurnParts`]。
///
/// ── 为什么读下发帧而不是上游原始字节 ────────────────────────
/// 帧已过 `ReasoningCoalescer`：思考被合成成完整帧、纯换行保活帧已被丢弃
/// （`sse.rs` 的 strip 分支）。按上游原始字节自己切一遍会有两个后果：换行保活帧
/// 被算进正文（判据 6a 的悬挂尾部会被保活帧的 `\n` 干扰）、思考数两遍。
///
/// ── 为什么不做成 O(1) 的增量判据 ────────────────────────────
/// 判据 4 要**全文**的围栏奇偶、停机闸要**整段**文本的逐位置相似度，增量化等于
/// 为这两条另写一套近似 —— 又回到「两份清单」的风险。内存代价是「本轮正文的一份
/// 拷贝」，与 `RecordingStream` 已在攒的下行字节同量级，不是新增的大头。
#[derive(Clone, Debug, Default)]
pub struct TurnAccumulator {
    parts: TurnParts,
    /// 本轮是否已经看到终帧（带非 null `finish_reason` 的那一帧）
    saw_finish_frame: bool,
    /// 本轮是否已经看到 `[DONE]`
    saw_done: bool,
    /// 本轮是否出现过流内错误帧（`{"error": …}`）
    saw_error: bool,
    /// 本轮看到的最后一份 usage（上游可能在多帧里补发，取最后一份 = 该轮最终值，
    /// 与 `RequestTelemetry::report_usage` 同一口径；跨轮再按 [`sum_tokens`] 求和）
    usage: Option<Value>,
}

impl TurnAccumulator {
    /// 吃一帧（`ForwardStream` 交给下游的那一份：已含 `data: …\n\n` 的完整帧）。
    ///
    /// 返回这一帧**是不是**终止类帧（终帧 / `[DONE]` / 错误帧）—— 调用侧据此决定
    /// 「压住不发」还是「照常下发」。解析失败的帧按普通帧放行（判据读不到就当
    /// 没看见，与 `refusal` 的 fail-open 同一取向）。
    pub fn push_frame(&mut self, frame: &Bytes) -> bool {
        let text = String::from_utf8_lossy(frame);
        // 哨兵帧：`[DONE]`（切帧口径与 refusal 共用一份，见 frame_is_done 的注释）
        if frame_is_done(&text) {
            self.saw_done = true;
            return true;
        }
        let Some(value) = first_json_payload(&text) else {
            return false;
        };
        // usage 抄一份（旁路读取，不参与帧的构造）；用量采集的权威点仍在 `sse.rs`
        if let Some(usage_object) = value.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Some(usage_object.clone());
        }
        if value.get("error").is_some() {
            self.saw_error = true;
            return true;
        }
        let mut terminal = false;
        // usage-only 帧（choices 缺席或为空）不参与判据也不记账：用量采集点在
        // `sse.rs` 的旁路（`telemetry.report_usage`），不归本模块
        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return false;
        };
        for choice in choices {
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                self.parts.finish = Some(reason.to_string());
                self.saw_finish_frame = true;
                terminal = true;
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                self.parts.text.push_str(content);
            }
            for key in ["reasoning_content", "reasoning"] {
                if let Some(thinking) = delta.get(key).and_then(Value::as_str) {
                    self.parts.reasoning.push_str(thinking);
                }
            }
            if delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty())
            {
                self.parts.has_tool_calls = true;
            }
        }
        terminal
    }

    /// 本轮的判据输入。
    pub fn parts(&self) -> &TurnParts {
        &self.parts
    }

    /// 是否已经收到终止帧（`[DONE]` 或错误帧）—— 决定「本轮说完了」的依据之一。
    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    /// 本轮是否出现过流内错误帧。
    pub fn saw_error(&self) -> bool {
        self.saw_error
    }

    /// 本轮的 token 数（没有 usage 帧时全 0 —— 与单轮口径一致：宁可不记也不猜）。
    pub fn tokens(&self) -> usage::UsageTokens {
        self.usage
            .as_ref()
            .and_then(usage::extract_usage)
            .unwrap_or_default()
    }
}

/// 这一帧是不是 `[DONE]` 哨兵。
///
/// 切帧口径**复用 `refusal::frame_payloads`**：「找 finish_reason」「取拒答正文」
/// 与「判本轮说完」是同一批字节的三条识别路径，切帧方式必须一致 —— 否则会出现在
/// 一处算一帧、另一处算两帧的分歧（那种分歧只会表现为「偶尔少续一轮」）。
fn frame_is_done(text: &str) -> bool {
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .any(|payload| payload == "[DONE]")
}

/// 取一帧里第一个能解析成 JSON 对象的 `data:` 载荷。
fn first_json_payload(text: &str) -> Option<Value> {
    refusal::frame_payloads(text).find_map(|payload| serde_json::from_str::<Value>(payload).ok())
}

/// 下游那条流的类型（与 `ForwardOutcome::Stream` 交出的同一个形状）。
type Downstream = Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + Unpin>;

/// 续写一条客户端请求要复用的**全部**上下文。
///
/// 每一项都是 owned：续写轮是「客户端那条流还在活着的时候再打一次上游」，
/// 跨 await 与进 spawn 任务都带不动借用（转发层内部的 `ProviderContext` 是借用体，
/// 那是它**一轮之内**的事，正好是我们不能复用的那层）。
pub struct Continuation {
    /// 转发层句柄（`UpstreamService` 是 Clone 的壳，内部 Arc 共享路由与去重表）
    pub service: UpstreamService,
    /// 客户端原始 chat 体（`messages` 在内）；每轮在它之上追加续写回合
    pub base_body: Value,
    /// 客户端这一轮要的是流式还是非流式（续写轮沿用同一个模式）
    pub stream: bool,
    pub client_headers: HeaderMap,
    pub telemetry: Arc<usage::RequestTelemetry>,
    pub allowed_providers: Option<KeyScope>,
    /// 总轮数上限（**含首轮**）：`2` = 最多续写一轮
    pub max_rounds: usize,
}

/// 读开关，回答「这条请求要不要走续写包装」。
///
/// `None` 时调用侧**连包装都不做** —— 下发字节与接入前逐字一致。这不是
/// 「反正逻辑等价，包一层也无所谓」：包一层要多一次 channel 往返与每帧一次 JSON
/// 解析，开关关着也要付，换 0 收益。
///
/// **两个口径的换算在这里做掉（+1），别让它们各自漂在调用两侧**：配置读出来的
/// `autoContinueMaxRounds` 是**续写次数**（`AUTO_CONTINUE_MIN_ROUNDS` 的定义是
/// 「一次请求最多『再问一次』几回」，端点侧「只想把预算收到 1 轮」也是这个意思），
/// 而 [`Continuation::max_rounds`] 是**含首轮的总轮数**。所以设 3 = 续 3 轮（总 4 轮），
/// 设 1 = 续 1 轮。
///
/// 从前这里是 `(max >= 2).then_some(max)`，把两个口径当成了同一个：设 3 实际只续
/// 2 轮，设 1（PUT 校验放行的合法值）直接返回 `None` —— 恰好造出配置侧专门反对的
/// 那个状态（「开着开关却一轮都不续，与关掉开关是同一件事」，见
/// `AUTO_CONTINUE_MIN_ROUNDS`）。
pub fn plan() -> Option<usize> {
    if !config::auto_continue() {
        return None;
    }
    Some(total_rounds_of(config::auto_continue_max_rounds()))
}

/// 配置口径（续写次数）→ [`Continuation::max_rounds`]（含首轮的总轮数）。
///
/// 单独抽成纯函数是为了能被钉住：`plan()` 读的是全局配置，而并发用例改全局配置
/// 会互相踩（本仓已有同类教训），所以算术归算术、读配置归读配置。
fn total_rounds_of(continuations: i64) -> usize {
    continuations.clamp(1, 1_000) as usize + 1
}

#[cfg(test)]
mod round_budget_tests {
    //! 续写次数 → 总轮数 的换算。钉它是因为这里**真的错过一次**：从前 `plan()`
    //! 把两个口径当同一个（`(max >= 2).then_some(max)`），于是设 3 只续 2 轮、
    //! 设 1（PUT 校验放行的合法值）直接返回 None 把功能关掉 —— 而配置侧专门写了
    //! 「想关就去关开关，不要用数字表达关闭」（见 `AUTO_CONTINUE_MIN_ROUNDS`）。

    use super::*;

    #[test]
    fn the_configured_continuation_count_becomes_rounds_including_the_first() {
        assert_eq!(total_rounds_of(1), 2, "设 1 = 续一轮（不是关掉功能）");
        assert_eq!(total_rounds_of(3), 4, "设 3 = 续三轮（默认值，别少一轮）");
        assert_eq!(total_rounds_of(8), 9, "上限 8 = 续八轮");
        // 越界走夹紧（配置层已校验 1..=8，这里是纵深防御：直读环境变量兜底那条路）
        assert_eq!(total_rounds_of(0), 2);
        assert_eq!(total_rounds_of(-5), 2);
    }
}

impl Continuation {
    /// 非流式：把聚合好的 chat 完成体续写完整。
    ///
    /// 一轮都没续时**原样返回**那一份体（连 `usage` 都不重写）—— 「开着开关但没截断」
    /// 是绝大多数请求的形态，那条路径的输出必须与接入前逐字相同，否则「开关开着
    /// 但行为变了」没法解释。
    pub async fn complete(&self, first: Value) -> Value {
        let mut body = first;
        let mut texts: Vec<String> = Vec::new();
        let mut reasonings: Vec<String> = Vec::new();
        let mut tokens: Vec<usage::UsageTokens> = Vec::new();
        // 逐轮累加的工作体（第 3 轮的 messages 里要带第 1、2 轮各自的半截答案，
        // 而不是只带最后那一轮 —— 否则模型会把它自己写过的第一段忘掉）
        let mut working = self.base_body.clone();
        let mut rounds = 1usize;
        loop {
            // 读不出 `choices[0].message` 就不判也不续（fail-open，同 refusal）
            let Some(parts) = parts_of_completion(&body) else {
                break;
            };
            let repeat = texts
                .last()
                .is_some_and(|previous| is_repeat(previous, &parts.text));
            // 判据读「已拼各轮 + 本轮」的整体（与流式同一条口径，见 [`verdict_of`]）。
            // `any_tool_calls` 这里恒等于本轮那条：前面任何一轮出现过工具调用都会
            // 当场判「不续」并 break，走不到这一行
            let verdict = verdict_of(
                &(join_text(&texts) + &parts.text),
                &(join_text(&reasonings) + &parts.reasoning),
                parts.has_tool_calls,
                &parts,
            );
            // 复读的那一轮**也留在拼接里**：流式路径里那些字节早就实时发出去了，
            // 收不回来；这里若偷偷丢掉，同一个问题就会「stream:false 看不到重复、
            // stream:true 看得到」—— 两种模式必须给同一段答案。
            // 复读闸的作用是「不再打第四轮」，不是「把第三轮抹掉」。
            texts.push(parts.text.clone());
            reasonings.push(parts.reasoning.clone());
            tokens.push(tokens_of_completion(&body));
            if repeat || !looks_truncated(&verdict) || rounds >= self.max_rounds || self.telemetry.is_cancelled() {
                break;
            }
            // 没有 messages 数组就改不动（入口校验过，走到这里按不续处理）
            if !append_continuation(&mut working, &parts.text, looks_reasoning_only(&verdict)) {
                break;
            }
            rounds += 1;
            match self.forward_round(working.clone()).await {
                Ok(ForwardOutcome::Completion { body: next }) => {
                    // 轮次边界与流式同一句、同一落点（同样受「首次为准」限制，
                    // 说明见 `stream` 里那条注释）
                    self.telemetry.note_attempt_notice("自动续写轮（上一轮疑似被截断）");
                    body = next;
                }
                // 续写轮回的不是非流式体 / 转发失败：`body` 保持上一轮那份，
                // 客户端拿到与开关打开前一样的半截答案（不伪造收尾）
                _ => break,
            }
        }
        if texts.len() < 2 {
            return body;
        }
        // 跨轮用量求和：先覆盖遥测（每轮各自的 report_usage 是「最后一次为准」，
        // 这里把「最后一次」改写成合并值），再写进客户端看到的那份体
        let merged = sum_tokens(&tokens);
        self.telemetry.report_usage(&usage_json(&merged));
        if let Some(usage_object) = body.as_object_mut() {
            usage_object.insert("usage".to_string(), usage_json(&merged));
        }
        let Some(choice) = body
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.first_mut())
            .and_then(|choice| choice.as_object_mut())
        else {
            return body;
        };
        let Some(message) = choice.get_mut("message").and_then(Value::as_object_mut) else {
            return body;
        };
        message.insert("content".to_string(), json!(join_text(&texts)));
        let merged_reasoning = join_text(&reasonings);
        if !merged_reasoning.is_empty() {
            message.insert("reasoning_content".to_string(), json!(merged_reasoning));
        }
        body
    }

    /// 流式：把多轮拼成**一条**下游流。
    ///
    /// 形态与仓里既有的「流还活着时做异步事」一致（后台任务喂 mpsc，主路径返回
    /// `ReceiverStream`，见 `providers::kuku::chat` 等处的同一写法）—— 手工在
    /// `poll_next` 里 poll 一个 future 只有 `cancellation::CancellableStream` 一处先例，
    /// 而它不需要在轮次之间换掉整条上游流。
    ///
    /// 下发时序：正文帧**实时**转发；只有终止类帧（带非 null `finish_reason` 的帧、
    /// `[DONE]`、错误帧）压住不发，等本轮 EOF 后决定「放行」（不续写）或「丢掉」
    /// （续写下一轮）。所以开关开着但没续写时，字节的顺序与内容与接入前逐字相同，
    /// 只有最后一帧的到达时刻被推到晚一根头发丝（EOF 就紧跟在它后面）。
    pub fn stream(&self, first: Downstream) -> Downstream {
        let service = self.service.clone();
        let telemetry = self.telemetry.clone();
        let client_headers = self.client_headers.clone();
        let allowed_providers = self.allowed_providers.clone();
        let mode = self.stream;
        let max_rounds = self.max_rounds;
        let mut working = self.base_body.clone();
        let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(64);
        crate::spawn_task(async move {
            let mut current = first;
            let mut texts: Vec<String> = Vec::new();
            let mut tokens: Vec<usage::UsageTokens> = Vec::new();
            // 客户端已经看到的整体（判据读它，理由见 [`verdict_of`]）
            let mut joined = String::new();
            let mut thinking = String::new();
            let mut any_tool_calls = false;
            let mut rounds = 1usize;
            loop {
                let mut accumulator = TurnAccumulator::default();
                let mut held: Vec<Bytes> = Vec::new();
                let mut held_bytes = 0usize;
                let mut held_full = false;
                let mut client_gone = false;
                while let Some(item) = current.next().await {
                    match item {
                        Ok(frame) => {
                            // 一旦压住过终止帧，**后面所有帧都一并压住**：有些上游在
                            // `finish_reason` 之后还补一帧 usage，若只按「终止帧才压」
                            // 处理，客户端会先收到 usage 再收到 finish —— 那是本模块
                            // 自己造出来的顺序变化，比少续一轮坏得多。
                            if accumulator.push_frame(&frame) || !held.is_empty() {
                                // 压住的东西**必须有上限**：上游在终止帧之后一直补帧
                                // （坏掉的或恶意的上游）会让这个 Vec 无界增长 —— 转发层
                                // 不该给上游一个把网关内存吃干的入口。超限后不再收，
                                // 但**不改**「已压住就全压住」的形态（多出来的帧丢掉，
                                // 与续写路径丢掉整段 held 是同一种损失），也不影响
                                // 「要不要续写」的判定（判据读的是累积器，不是 held）。
                                if held_bytes + frame.len() > MAX_HELD_BYTES {
                                    if !held_full {
                                        held_full = true;
                                        logging::verbose(
                                            "Continuation",
                                            "终止帧之后的补帧超过上限，后续帧丢弃（本轮不再累积）",
                                        );
                                    }
                                } else {
                                    held_bytes += frame.len();
                                    held.push(frame);
                                }
                            } else if sender.send(Ok(frame)).await.is_err() {
                                client_gone = true;
                                break;
                            }
                        }
                        Err(error) => {
                            // `ForwardStream` 不会给出 Err（它在 poll_next 里就把错误折成
                            // 错误帧 + [DONE]）；真拿到 Err 说明来源不是它 —— 原样透传后收尾，
                            // 在这里伪造终止帧比少发一帧更坏
                            let _ = sender.send(Err(error)).await;
                            return;
                        }
                    }
                }
                if client_gone {
                    // 客户端断开：一条没人看的流不值得再多打一轮上游配额
                    return;
                }
                let parts = accumulator.parts().clone();
                tokens.push(accumulator.tokens());
                let repeat = texts
                    .last()
                    .is_some_and(|previous| is_repeat(previous, &parts.text));
                // 判据在**并入本轮之前**算：verdict 里是「前面各轮 + 本轮」的整体，
                // 也就是客户端此刻会看到的整段答案。
                // ⚠️ 必须带上 `parts.text` 自己：只传「已拼好的那些」在第一轮就是空串，
                // 于是第一轮永远判不出截断、一次都不会续（2026-10-14 假上游实测到这条）
                let verdict = verdict_of(
                    &(joined.clone() + &parts.text),
                    &(thinking.clone() + &parts.reasoning),
                    any_tool_calls,
                    &parts,
                );
                joined.push_str(&parts.text);
                thinking.push_str(&parts.reasoning);
                any_tool_calls |= parts.has_tool_calls;
                texts.push(parts.text.clone());
                let want_more = rounds < max_rounds
                    && !accumulator.saw_error()
                    && !repeat
                    && looks_truncated(&verdict)
                    && !telemetry.is_cancelled()
                    // 客户端在这一轮 EOF 到「决定续写」之间断开了。上面那处
                    // `sender.send` 失败是唯一**已经发生**的探测点，而这个窗口里
                    // 原本没有任何探测 —— 断线之后照样会白打一轮上游（带全量历史，
                    // 真金白银）。`is_closed` 是同步方法，单独一行挡掉。
                    && !sender.is_closed()
                    // 本轮**一个字节都没产出**（正文与思考都空）：没有可续的东西。
                    // 判据读的是**累积**文本，所以上一轮的半截答案还在、判据仍说截断，
                    // 相似度闸对空串也不设防（similarity("", x) 恒 0）—— 不拦就会
                    // 一路问到 `max_rounds`，每轮都白花一次往返。
                    // 注意只判「两者都空」：正文空但有思考是判据第 3 条要续的正例。
                    && !(parts.text.trim().is_empty() && parts.reasoning.trim().is_empty());
                if want_more
                    && append_continuation(&mut working, &parts.text, looks_reasoning_only(&verdict))
                {
                    rounds += 1;
                    match forward_round_with(
                        &service,
                        mode,
                        &client_headers,
                        &telemetry,
                        allowed_providers.clone(),
                        working.clone(),
                    )
                    .await
                    {
                        Ok(ForwardOutcome::Stream { stream: next, .. }) => {
                            // 轮次边界打在**这一轮**的明细上（此刻它已经是最后一条；
                            // 若这一轮内部又换了几个账号，标记落在最后那条 ——
                            // 不做逐条标注是刻意的：明细的形状归转发层管，本模块只加一句）
                            //
                            // ⚠️ 这一句**可能静默不写**：`note_attempt_notice` 是
                            // 「首次为准」，那一轮如果已经记了「账号代理不可用、本次回退
                            // 直连」，续写标记就让位给它。看不到标记不等于没续写 ——
                            // 判据是 `attempts` 与明细条数（报表口径见 `TelemetrySnapshot::attempts`）。
                            telemetry.note_attempt_notice("自动续写轮（上一轮疑似被截断）");
                            // 确定要续了才丢终止帧：上面任何回退路径都得把它们交回去
                            drop(held);
                            current = next;
                            continue;
                        }
                        Ok(ForwardOutcome::Completion { .. }) => {
                            logging::verbose("Continuation", "续写轮回的是非流式体，停止续写");
                        }
                        Err(error) => {
                            // 不 note_error：客户端仍会拿到上一轮的终止帧，这条请求
                            // 在日志里的形状与开关打开前一样（续写失败不该把成功记成失败）
                            logging::verbose(
                                "Continuation",
                                &format!("续写轮转发失败，按本轮结束收尾: {error}"),
                            );
                        }
                    }
                }
                for frame in held {
                    if sender.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                break;
            }
            if texts.len() > 1 {
                telemetry.report_usage(&usage_json(&sum_tokens(&tokens)));
            }
        });
        Box::new(ReceiverStream::new(receiver))
    }

    /// 发一轮续写请求（与首轮的差别只有 `body` 与 `dedupe_key`）。
    async fn forward_round(&self, body: Value) -> Result<ForwardOutcome, crate::server::errors::GatewayError> {
        forward_round_with(
            &self.service,
            self.stream,
            &self.client_headers,
            &self.telemetry,
            self.allowed_providers.clone(),
            body,
        )
        .await
    }
}

/// 组装并发起一轮续写请求。
///
/// `dedupe_key` 传**空串**是必须的，不是省事：首轮那条 `InFlightGuard` 挂在
/// 交给客户端的流上（流不被 drop 就不释放），第二轮若沿用同一个 key 会等首轮
/// 自己结束 —— 而首轮正在等第二轮，最长干等 45 秒（`mod.rs` 的
/// `wait_for_in_flight` / `INFLIGHT_WAIT_MS`）。空串在建槽处直接短路。
///
/// 单独抽成自由函数是因为两条链路都要用它：`complete` 在 `self` 上调，
/// `stream` 在 spawn 任务的 owned 句柄上调（那边借不到 `self`）。
async fn forward_round_with(
    service: &UpstreamService,
    stream: bool,
    client_headers: &HeaderMap,
    telemetry: &Arc<usage::RequestTelemetry>,
    allowed_providers: Option<KeyScope>,
    body: Value,
) -> Result<ForwardOutcome, crate::server::errors::GatewayError> {
    service
        .forward(ForwardRequest {
            body,
            stream,
            dedupe_key: String::new(),
            client_headers: client_headers.clone(),
            telemetry: telemetry.clone(),
            allowed_providers,
            // 与主链路同口径：续写轮也不钉账号、也不关模型闸门、也不带代理覆盖
            pinned_account: None,
            ignore_model_gate: false,
            proxy_override: None,
        })
        .await
}

#[cfg(test)]
mod truncation_criteria {
    //! 判据的六条与停机闸（模块头列的顺序逐条对应参考实现）。
    //!
    //! 每条判据都配一个**反对照**：误判的代价是「把已经写完的答案再续一遍」——
    //! 客户端会看到重复内容，还白花一轮上游配额。漏判只是回到今天的形态（半截答案），
    //! 所以这批用例的重点是「正常答案不许被点亮」。

    use serde_json::json;

    use super::*;

    fn parts(text: &str, reasoning: &str, finish: Option<&str>) -> TurnParts {
        TurnParts {
            text: text.to_string(),
            reasoning: reasoning.to_string(),
            has_tool_calls: false,
            finish: finish.map(str::to_string),
        }
    }

    /// 正常答复的抽样：五种「看着像半截但其实写完了」的形状，一条都不许点亮。
    #[test]
    fn complete_answers_are_not_flagged() {
        for text in [
            "答案是 42。",
            "Totals: 3 items, 2 shipped, 1 returned.",
            "```rust\nfn main() {}\n```",
            "配置在 `~/.qoder-cn/settings.json`，改完重启即可",
            "- 第一步：打开面板\n- 第二步：点「刷新模型」",
        ] {
            assert!(
                !looks_truncated(&parts(text, "", Some("stop"))),
                "正常答复被判成截断（会重复输出并白花一轮配额）：{text:?}"
            );
        }
    }

    /// 判据 1：工具轮永不续，**即使** finish_reason 明确说预算用尽。
    ///
    /// 反对照放在同一条用例里：同样的正文、没有 tool_calls 时必须点亮 ——
    /// 否则「工具轮短路」这条判据其实在测另一个函数。
    #[test]
    fn tool_call_turns_are_never_continued() {
        let mut with_calls = parts("我把文件读完之后", "", Some("length"));
        with_calls.has_tool_calls = true;
        assert!(!looks_truncated(&with_calls), "工具轮的执行权在客户端，不许续写");
        assert!(
            looks_truncated(&parts("我把文件读完之后", "", Some("length"))),
            "同样的正文没有 tool_calls 时应当点亮（否则上面那条断言是空跑）"
        );
    }

    /// 判据 2：length 一族的 8 个写法都要认，且大小写不敏感。
    ///
    /// 少认一个 = 那一族的截断整族漏掉；上游给的是哪个写法不是我们能选的。
    #[test]
    fn every_length_alias_counts_as_truncation() {
        for alias in LENGTH_FINISH_ALIASES {
            assert!(
                is_length_finish(alias)
                    && looks_truncated(&parts("就这样", "", Some(alias))),
                "length 别名没认出来: {alias}"
            );
        }
        assert!(is_length_finish("LENGTH"), "大小写不敏感");
        assert!(!is_length_finish("tool_calls"));
        assert!(!is_length_finish("content_filter"), "拒答归 refusal 管，不是截断");
    }

    /// 判据 3：正文空 + 有思考要续；两者都空**不续**（那是既有空答复兜底链的活）。
    #[test]
    fn empty_visible_text_depends_on_the_reasoning_channel() {
        assert!(looks_truncated(&parts("", "先分析题目，再考虑边界情况。", None)));
        assert!(!looks_truncated(&parts("   ", "", None)), "两个通道都空：不归本模块管");
        // 正文空 + 有思考 + finish 是 content_filter：**仍然判截断**。
        // 这条不是疏漏，是分工：拒答在转发层由 `refusal` 门控拦下（它看的是
        // 上游声明的 finish 集合），能走到续写的都是门控放过的那批 —— 本模块
        // 不该再拿 finish 名字做第二套判断，否则两处对同一条流各有结论。
        assert!(looks_truncated(&parts("", "思考", Some("content_filter"))));
    }

    /// 判据 4：代码围栏数量为奇数。
    #[test]
    fn an_unclosed_code_fence_is_hard_evidence() {
        assert!(looks_truncated(&parts("看这段：\n```python\ndef f():\n    return 1", "", None)));
        assert!(
            !looks_truncated(&parts("看这段：\n```python\ndef f():\n    return 1\n```", "", None)),
            "闭合的围栏不该点亮"
        );
    }

    /// 判据 5：末段括号 —— 只认「走完仍未闭合」，落单的闭合符按噪声跳过。
    ///
    /// 第二条与第三条是把两处**代价不对称**钉住：`{a: [1, 2}` 必须抓到（参考实现
    /// 举的例子），而窗口左边界切在一对括号中间产生的那个落单 `}` 必须**不**抓到
    /// （抄参考实现就会抓到，见 `unbalanced_brackets` 函数头的偏离说明）。
    #[test]
    fn bracket_checks_end_with_an_open_group_not_with_a_stray_closer() {
        assert!(!looks_truncated(&parts("先跑 (npm test) 再提交", "", None)), "配对完整：写完了");
        assert!(looks_truncated(&parts("返回 {a: [1, 2}", "", None)), "仍未闭合必须抓到");
        assert!(looks_truncated(&parts("调用 compute(a, b", "", None)));
        // 落单闭合符：整篇是「写完的一句中文」，只是括号不成对
        assert!(
            !looks_truncated(&parts("这里有个坑) 以及另一个)", "", None)),
            "窗口切出来的落单右括号不该点亮"
        );
        // 计数差超过容忍（三个开括号一个没关）
        assert!(looks_truncated(&parts("嵌套 ( ( ( 还没关", "", None)));
    }

    /// 判据 6a：悬挂尾部集合；**单个句点不算**（省略号才算）。
    #[test]
    fn dangling_tails_are_flagged_but_a_period_is_not() {
        for text in [
            "总价包括：",
            "第一行 |",
            "等等...",
            "路径是 src\\",
            "注释 #",
            "下一项 -",
            "结果是 :",
            "表格单元 ｜",
        ] {
            assert!(looks_truncated(&parts(text, "", None)), "悬挂尾部没抓到: {text:?}");
        }
        assert!(
            !looks_truncated(&parts("结果是 42.", "", None)),
            "单个句点是正常收尾，不许和省略号混起来"
        );
    }

    /// 判据 6b：长思考 + 极短正文且正文没说完。
    ///
    /// 三条反照各有分工：正文够长（越过上限）/ 思考不够长 / 正文已以句末符号收尾。
    /// 倍数那道闸在现有常量下点不亮（原因见 `FRAGMENT_REASONING_RATIO` 的注释），
    /// 所以这里不为它写断言 —— 断言一条永不生效的分支只会把它锁成「看起来有测试」。
    #[test]
    fn a_long_think_with_a_stopped_fragment_needs_one_more_round() {
        let thinking = "分".to_string().repeat(FRAGMENT_REASONING_MIN_CHARS);
        assert!(looks_truncated(&parts("好的", &thinking, None)), "整轮烧在思考里");
        assert!(
            !looks_truncated(&parts(&"好的，".repeat(20), &thinking, None)),
            "正文超过上限就不该走这条判据"
        );
        assert!(
            !looks_truncated(&parts("好的", "短思考", None)),
            "思考不够长时，短句可能就是答案"
        );
        assert!(
            !looks_truncated(&parts("收到。", &thinking, None)),
            "正文以句末符号收尾 = 说完了"
        );
    }

    /// 判据必须读「拼接后的整体」，不是本轮那一段 —— 假上游实测踩到的一条。
    ///
    /// 现场：第一轮留了一道没闭合的 ```（该续），第二轮正好把它关上并写完。
    /// 按本轮自己那段判，第二轮也是「奇数道围栏」⇒ 再打第三轮，客户端看到的
    /// 答案尾巴上就多出一整段重复。这条用例把方向钉死：
    /// **第一轮点亮、第二轮的整体必须熄灭**。
    #[test]
    fn the_verdict_reads_the_stitched_answer_not_the_last_round() {
        let first = parts("示例代码：\n```python\ndef solve():\n    return 42\n", "", Some("stop"));
        assert!(looks_truncated(&first), "第一轮留着没闭合的围栏：该续");
        let second = parts("```\n上面的函数返回 42。", "", Some("stop"));
        assert!(
            looks_truncated(&second),
            "本轮单独看仍是半截（这正是旧判据多点一次火的地方）"
        );
        let stitched = verdict_of(
            &(first.text.clone() + &second.text),
            "",
            false,
            &second,
        );
        assert!(
            !looks_truncated(&stitched),
            "拼接后那道围栏已经闭合：整段答案是写完的，不许再打一轮"
        );
        // 首轮（前面还没有任何已拼好的内容）也必须点亮 —— 这条钉的是「整体」里
        // 必须**含本轮**：只传历史会退化成空串，第一轮永远判不出截断
        let opening = verdict_of(&first.text.clone(), "", false, &first);
        assert!(
            looks_truncated(&opening),
            "首轮的判据里必须带上本轮自己那段文本"
        );
    }

    /// 工具轮信号只看本轮：前面几轮没有工具调用，不代表本轮没有。
    #[test]
    fn a_tool_call_in_the_current_round_stops_the_chain() {
        let mut current = parts("接着写", "", Some("stop"));
        current.has_tool_calls = true;
        let verdict = verdict_of("前面已经写好的整段答案", "", false, &current);
        assert!(verdict.has_tool_calls, "本轮的 tool_calls 必须并进整体判据");
        assert!(!looks_truncated(&verdict));
    }

    /// 停机闸：完全复读停、正好 0.5 不停（参考实现的严格大于）。
    #[test]
    fn the_repeat_gate_is_strictly_above_half() {
        assert!((similarity("abcd", "abcd") - 1.0).abs() < f64::EPSILON);
        assert_eq!(0.0, similarity("", "abcd"), "空串不做分母");
        assert!(is_repeat("接着写", "接着写下去"));
        // 四个字符里两个相同 = 正好 0.5 ⇒ 不判为复读
        assert!(!is_repeat("abcd", "abxy"));
        assert_eq!(0.5, similarity("abcd", "abxy"));
    }

    /// 回放形态：半截答案当 assistant 回合 + 一句 user 指令（顺序与内容都要对）。
    #[test]
    fn continuation_appends_the_partial_then_the_instruction() {
        let mut body = json!({"messages": [{"role": "user", "content": "写一首诗"}]});
        assert!(append_continuation(&mut body, "床前明月光，", false));
        let messages = body["messages"].as_array().cloned().unwrap_or_default();
        assert_eq!(3, messages.len());
        assert_eq!("assistant", messages[1]["role"]);
        assert_eq!("床前明月光，", messages[1]["content"], "回放用原文，不许 trim");
        assert_eq!("user", messages[2]["role"]);
        assert_eq!(CONTINUE_INSTRUCTION, messages[2]["content"]);
    }

    /// 正文为空时不造空的 assistant 回合，只追指令（换成思考专用那条）。
    #[test]
    fn an_empty_partial_does_not_create_an_empty_assistant_turn() {
        let mut body = json!({"messages": [{"role": "user", "content": "继续"}]});
        assert!(append_continuation(&mut body, "   ", true));
        let messages = body["messages"].as_array().cloned().unwrap_or_default();
        assert_eq!(2, messages.len(), "空正文不该占一条 assistant");
        assert_eq!(REASONING_ONLY_INSTRUCTION, messages[1]["content"]);
        // 没有 messages 数组：改不动，返回 false 且体子不被写坏
        let mut broken = json!({"prompt": "裸 prompt"});
        assert!(!append_continuation(&mut broken, "半截", false));
        assert_eq!(json!({"prompt": "裸 prompt"}), broken);
    }

    /// 非流式体的四个读数都要读得到（两个思考键、工具调用、终帧）。
    #[test]
    fn completion_parts_read_the_aggregated_shape() {
        let body = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "半截", "reasoning_content": "想了很久"},
                "finish_reason": "length",
            }],
        });
        let parts = parts_of_completion(&body).expect("聚合体的形状必须读得出");
        assert_eq!("半截", parts.text);
        assert_eq!("想了很久", parts.reasoning);
        assert_eq!(Some("length".to_string()), parts.finish);
        assert!(!parts.has_tool_calls);
        // `reasoning` 这个键名也有上游在用（聚合侧两个键都读）
        let alt = json!({"choices": [{"message": {"content": "", "reasoning": "思考"}}]});
        assert_eq!(
            "思考",
            parts_of_completion(&alt).unwrap().reasoning,
            "reasoning_content 缺席时要退回 reasoning"
        );
        let with_calls = json!({"choices": [{"message": {"tool_calls": [{"id": "1"}]}}]});
        assert!(parts_of_completion(&with_calls).unwrap().has_tool_calls);
        assert!(parts_of_completion(&json!({"choices": []})).is_none(), "没有 choice 就判不了");
    }

    fn frame(payload: Value) -> Bytes {
        Bytes::from(format!("data: {}\n\n", payload))
    }

    /// 流式侧的累积器：正文/思考/工具/终帧四类读数。
    #[test]
    fn the_stream_accumulator_reads_downstream_frames() {
        let mut acc = TurnAccumulator::default();
        assert!(!acc.push_frame(&frame(json!({"choices": [{"delta": {"content": "前半"}}]}))));
        assert!(!acc.push_frame(&frame(json!({"choices": [{"delta": {"content": "，后半"}}]}))));
        assert!(!acc.push_frame(&frame(json!({"choices": [{"delta": {"reasoning_content": "思考"}}]}))));
        assert!(!acc.push_frame(&frame(json!({"choices": [{"delta": {"tool_calls": [{"index": 0}]}}]}))));
        assert_eq!("前半，后半", acc.parts().text, "逐帧拼接");
        assert_eq!("思考", acc.parts().reasoning);
        assert!(acc.parts().has_tool_calls);
        assert!(!acc.saw_error());
        // 终帧：返回 true（调用侧据此压住不发）
        assert!(acc.push_frame(&frame(json!({"choices": [{"delta": {}, "finish_reason": "length"}]}))));
        assert_eq!(Some("length".to_string()), acc.parts().finish);
        assert!(acc.push_frame(&Bytes::from_static(b"data: [DONE]\n\n")));
        assert!(acc.saw_done());
    }

    /// 三类「不是答案」的帧：错误帧要标记、注释行与坏 JSON 不许崩也不许算终止。
    #[test]
    fn error_comment_and_unparsable_frames_are_classified() {
        let mut acc = TurnAccumulator::default();
        assert!(acc.push_frame(&frame(json!({"error": {"message": "上游断了", "type": "proxy_error"}}))));
        assert!(acc.saw_error(), "错误帧必须让调用侧别再续写");
        assert!(!acc.saw_done());

        let mut noisy = TurnAccumulator::default();
        assert!(!noisy.push_frame(&Bytes::from_static(b": keepalive\n\n")));
        assert!(!noisy.push_frame(&Bytes::from_static(b"data: not json\n\n")));
        assert!(!noisy.push_frame(&frame(json!({"choices": [{"delta": {"content": "A"}}], "x": 1}))));
        assert_eq!("A", noisy.parts().text, "坏帧只被跳过，不许把已攒的正文弄脏");
    }

    /// usage：本轮取最后一份，跨轮求和，且合并值还能被 `extract_usage` 读回来。
    ///
    /// 最后半个断言是**接口闭环**：合并走的是规范键名，而遥测的读取侧按别名表取值；
    /// 两边对不上时数字会静默变成 0（报表里就成「这条请求没花钱」）。
    #[test]
    fn usage_is_last_wins_per_round_and_summed_across_rounds() {
        let mut acc = TurnAccumulator::default();
        acc.push_frame(&frame(json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 2}})));
        acc.push_frame(&frame(json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 9, "total_tokens": 19}})));
        let round = acc.tokens();
        assert_eq!(9, round.completion, "同一轮里后到的 usage 是最终值");

        let rounds = [
            usage::UsageTokens { prompt: 10, completion: 9, total: 19, cache_read: 4 },
            usage::UsageTokens { prompt: 20, completion: 5, total: 25, cache_read: 0 },
        ];
        let merged = sum_tokens(&rounds);
        assert_eq!(30, merged.prompt, "每轮的 prompt 都含全部历史，真实成本按轮求和");
        assert_eq!(14, merged.completion);
        assert_eq!(44, merged.total);
        assert_eq!(4, merged.cache_read);
        let json = usage_json(&merged);
        assert_eq!(merged, usage::extract_usage(&json).unwrap(), "合并值必须能被提取侧读回来");
    }

    /// 拼接不留分隔符：上游被腰斩处没有换行，插字符会改坏代码块与 markdown。
    #[test]
    fn rounds_are_joined_without_separators() {
        assert_eq!("abc", join_text(&["ab".to_string(), "c".to_string()]));
        assert_eq!("x", join_text(&["x".to_string()]));
        assert_eq!("", join_text(&[]));
    }
}
