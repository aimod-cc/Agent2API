//! **历史图片剥离**：把上下文里那些「已经被答过、这一问也用不上」的旧图摘掉。
//!
//! ── 两条判据（各自有出处，都不是防御性检查）─────────────────
//!
//! ① **这家不吃图**（[`StripReason::HomeRefusesImages`]）
//! 识图闸门（`providers::catalog::routing::entry_refuses_images` 那套）按目录把
//! 不吃图的家从候选里**剔掉**。那是对的，但它有一个副作用：长会话里图往往全在
//! **历史轮**（Qoder 那种 300 多条消息 / 6 MB 的上下文，截图是二十几轮前贴的），
//! 而当前这一问就是纯文本。这时"这家不能收图"其实是个可以消除的技术障碍 ——
//! 为了迁就一叠历史截图，把一次本来能在**最高优先级**那家完成的转发，推给一个
//! 优先级低得多的家。用户看到的正是这个现象：`deepseek-v4.1-flash` 明明有
//! pri=2 的 codearts，却条条走 pri=26 的 trae。
//!
//! ② **这叠历史图的体积越过预算**（[`StripReason::HistoryImagesOverBudget`]）
//! 能吃图 ≠ 该把这叠图原样重发。一手读数（dev 3066，2026-10-15）：同一份
//! **16,341,828 字节**的会话里有 169 个图片块（日志 `已转换 169 个图片块`），
//! 平均每块约 96 KB ⇒ 图几乎就是全部体积；而同一家实测能收下的最大体是
//! 2,014,731 字节。客户端每轮把全部历史重发一遍（BYOK 客户端不自动压缩上下文），
//! 于是那叠二十几轮前的截图每一轮都跟着上链一次：换不来任何本轮用得上的信息，
//! 换来的是撞上游的长度墙（业务层 400 / APIG 413）与一长串「408 客户端在转发
//! 完成前断开」。判据因此不只挂在那家能不能吃图上。
//!
//! ── 唯一的硬边界：当前这一轮的图绝不剥 ────────────────────
//! 剥图不能恢复信息。当前问题依赖那张图时（"这张报错截图第三行写了什么"），
//! 剥掉必然答错，而且错得很像成功（200 + 一段编出来的话）。这一层因此只服务
//! 一种请求：**当前这一轮里没有任何 user 消息带图片**。含图就整层不动，退回今天的
//! 行为（剔家）。这条判据是 `current_turn_needs_vision`，它是本模块存在的
//! 全部前提，不是防御性检查 —— **预算那一支也不例外**：图再大，只要是这一问
//! 要用的，就一个字节都不动。
//!
//! ── 形状：转发层只可能看到一种图 ─────────────────────────
//! 三个入口都在进本层之前折成 chat 形态（用例见 `upstream::provider_loop` 的
//! `vision_gate`）：Anthropic 的 `image` 块 → `image_url`
//! （`protocol::anthropic::image_to_chat`），Responses 的 `input_image` →
//! `image_url`（`protocol::responses`）。而 **Anthropic 的 `tool_result` 内嵌
//! 图片到不了这里** —— 入口把工具结果折成字符串（`tool_result_text`，
//! `protocol/anthropic.rs` 的拆分那段），所以本层不必、也不该认第二套形状：
//! 认了就等于假设"存在我们收不到的图"，那种假设会让剥不干净却自称剥过。
//!
//! ── 为什么原位留一句标记文本 ─────────────────────────────
//! 直接把图删干净、别的字节都不动，模型会面对一个"少了一张图但没人告诉它"的
//! 上下文，于是它可能凭空补一段描述。留一句占位文本才让它能如实回答"我看不到
//! 那张图"。每条消息**只留一句**（哪怕它原本有 3 张图）：三句重复的占位符会把
//! 上下文搞脏，而信息量与一句相同。⚠️ **占位文本按理由分开**（[`StripReason`]）：
//! "该上游不支持图像输入"在预算那一支上是假话（这家明明能吃图），模型据此劝用户
//! "换个能看图的供应商"就是这么来的。
//!
//! ── 两处开关 ────────────────────────────────────────────
//! `AGENT2API_NO_IMAGE_STRIP=1` 整层关闭，退回"剔家"；
//! `AGENT2API_NO_IMAGE_BUDGET=1` 只关**预算那一轴**（能力那一轴照旧），留给
//! 预算数值定错时的一格 —— 与 zcode 那边的 `ZCODE_REQUIRE_CAPTCHA` 同一取向：
//! 判错的兜底不该依赖发版。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；且**能不复制就不复制**
//! （没图可剥时返回 `None`，让调用点继续借用原始 body，与 `sanitize_body` 同一口径）。
//! 体积读数也守着这一条：只累加 `image_url.url` 的串长，**不序列化整份 body**
//! （十几 MB 的体序列化一遍就是十几 MB 的额外拷贝与一次 GC）。

use serde_json::{json, Value};

/// 能力那一支原位留下的占位文本。措辞要能让模型据此说"我看不到"，而不是让它以为
/// 用户在问一张不存在的图。
const MARKER_TEXT_REFUSES: &str = "[图片已在转发时移除：该上游不接受图像输入]";

/// 预算那一支的占位文本。**必须与上面那句不同**：这家是能吃图的，只是这叠历史图
/// 太占地方；写成"不支持图像输入"会让模型给用户一句错的建议。
const MARKER_TEXT_BUDGET: &str = "[图片已在转发时移除：历史图片占用过大，已省略]";

const MARKER_PART: &str = "text";

/// 历史图片的体积预算（字节）：越过它，这叠图就不再原样重放。
///
/// ── 这个数是怎么来的 ────────────────────────────────────
/// 取 **2 MiB**，锚点是我们实测过的最紧那家：loomy 能收下的整份请求体上限是
/// **2,014,731 字节**（同一家 12,642,504 字节那批全被拒）。历史图自己就越过这条线
/// 时，这叠图已经比那家能收下的**整份请求**还大 —— 无论本轮文字多短都必然撞墙，
/// 而它在别的家换来的也只是"能不能再多塞几兆"的余量，不值这个险。
/// 反面参照：codearts 的墙在 6 MiB 附近、officeace 实测 13,500,858 字节仍能过 ——
/// 所以这条预算**不是**各家墙的最小值，是"这叠图已经大到不值得为它冒险"的那条线。
///
/// ⚠️ 这是**第一版手定的数**，不是量出来的最优解（没有"在哪个阈值上答案质量开始
/// 变差"的读数）。定错了用 `AGENT2API_NO_IMAGE_BUDGET=1` 整轴关掉，别改发版。
const HISTORY_IMAGE_BUDGET_BYTES: u64 = 2 * 1024 * 1024;

/// 一次剥图的产物。
pub struct Stripped {
    /// 处理副本（调用方此后只能读它，别再与原 body 比较字节）
    pub body: Value,
    /// 摘掉了多少**张**图（不是多少条消息）—— 报给用户的读数是张数
    pub removed: usize,
    /// 为什么剥的（决定这里留下的占位文本，以及明细里那句留痕的措辞）
    pub reason: StripReason,
}

impl Stripped {
    /// 留痕要用的那一小撮（body 已经被调用方拿走，这里只留读数与理由）。
    pub fn notice(&self) -> StripNotice {
        StripNotice {
            removed: self.removed,
            reason: self.reason,
        }
    }
}

/// 这一层为什么动手。两支的**模型可见文本**不同，理由见 [`StripReason::marker_text`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripReason {
    /// 目录说这家不吃图 —— 剥了才留得住这家（判据在发送侧按目录读）
    HomeRefusesImages,
    /// 历史图片的体积越过预算 —— **与这家能不能吃图无关**
    HistoryImagesOverBudget,
}

impl StripReason {
    /// 模型看到的那句占位文本。
    ///
    /// 两支必须分开：预算那一支上这家是能吃图的，告诉模型"该上游不接受图像输入"
    /// 是假话，而它会据此给用户"换个能看图的供应商"这类错的建议。
    fn marker_text(self) -> &'static str {
        match self {
            StripReason::HomeRefusesImages => MARKER_TEXT_REFUSES,
            StripReason::HistoryImagesOverBudget => MARKER_TEXT_BUDGET,
        }
    }
}

/// 这次剥图留给**尝试明细**的那句话（唯一措辞出处）。
///
/// 为什么由调用方而不是本模块写进 telemetry：留痕挂在「本轮尝试」这条记录上，
/// 而那条记录的起头时刻各条路径不一样 —— 有状态路径（CatPaw / CodeArts / Trae）
/// 是 `send_body` 先跑、`note_attempt_started` 后调，此刻还没有可写的槽位，
/// 在变换里直接记会**静默丢掉**（-17 上线后实测到这一点：明细里的 notice 是 null，
/// 而日志确实剥了 1 张）。所以本模块只交出「剥了几张、为什么」，由三个调用点在
/// 起头之后写。
#[derive(Clone, Copy, Debug)]
pub struct StripNotice {
    /// 摘掉了多少张
    pub removed: usize,
    /// 为什么摘
    pub reason: StripReason,
}

impl StripNotice {
    pub fn describe(&self) -> String {
        match self.reason {
            StripReason::HomeRefusesImages => format!(
                "该上游不接受图像输入，已剥离历史图片 {} 张（当前这一问不依赖图片）",
                self.removed
            ),
            StripReason::HistoryImagesOverBudget => format!(
                "历史图片体积越过预算，已剥离 {} 张（当前这一问不依赖图片）",
                self.removed
            ),
        }
    }
}

/// 关闭整层的开关（读一次定死）。
fn strip_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        let raw = std::env::var("AGENT2API_NO_IMAGE_STRIP")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        matches!(raw.as_str(), "1" | "true" | "yes" | "on")
    })
}

/// 关掉**预算那一轴**的开关（能力那一轴不受它影响）。读一次定死。
fn budget_axis_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| budget_axis_disabled_from(std::env::var("AGENT2API_NO_IMAGE_BUDGET").ok().as_deref()))
}

/// 上一行的判据本体。拆开只为能测：进程环境变量是全局的，用例里改它会污染并发
/// 跑别的用例（与 `codearts::keep_historical_reasoning_from` 同一取舍）。
fn budget_axis_disabled_from(value: Option<&str>) -> bool {
    matches!(
        value.map(|raw| raw.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// 这段 `content` 里有没有图片 part。
fn has_image(content: Option<&Value>) -> bool {
    content
        .and_then(Value::as_array)
        .is_some_and(|parts| parts.iter().any(is_image_part))
}

/// 单个内容块是否是图。只认 chat 形态的 `image_url`（理由见模块头「形状」）。
fn is_image_part(part: &Value) -> bool {
    part.get("type").and_then(Value::as_str) == Some("image_url")
}

/// 请求体里**任何**位置有没有图（消息数组 + `system` 的数组形态）。
///
/// `system` 也要看：这一层的目的就是"别让这家收到图"，只扫 `messages` 会留下
/// 一个仍然带着 `data:image/...` 的 system 数组，然后照旧 400。
pub fn request_carries_images(body: &Value) -> bool {
    let in_messages = body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages
                .iter()
                .any(|message| has_image(message.get("content")))
        });
    // `system` 可以是字符串（没有图可言），也可以是被各家入口折出来的块数组
    let in_system = match body.get("system") {
        Some(Value::String(_)) => false,
        Some(other) => has_image(other.get("content").or(Some(other))),
        None => false,
    };
    in_messages || in_system
}

/// **当前这一轮**是否依赖图片：本轮里的任何消息含图即为真。
///
/// 「本轮」的边界见 [`current_turn_start`]。
///
/// 为什么不能只看"最后一条 user"（2026-10-09 改，此前就是这条判据漏的）：客户端可以
/// 把一次提问拆成**两条连续的 user**（图在第一条、正文在第二条），也可以后面再补一句
/// 上下文提醒。那时"最后一条 user"是纯文本的，刚贴的图却被当成历史剥掉 —— dev 一手：
/// codearts 回「没收到图片，看不到内容」，而这次本该剔家转给能识图那一台。
///
/// 为什么也不取"最后一条消息"：agent 会话的尾巴经常是 `assistant`（工具调用）或
/// `tool`（工具结果），拿"最后一条"判会把用户真正的那一问漏掉。
pub fn current_turn_needs_vision(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    // `current_turn_start` 只会是「某条消息的下标」或「某条之后一位」，永远 ≤ len，
    // 切片不会越界
    messages[current_turn_start(messages)..]
        .iter()
        .any(|message| has_image(message.get("content")))
}

/// 本轮的起点。三种真实形状要同时满足（少一种就会重演今天这种「刚贴的图被剥」或
/// 反向的「二十几轮前的截图永远剥不掉」）：
///
/// 1. **上一条 assistant 之后又出现了新的 user** ⇒ 模型已经接过新的一问，本轮从那一条
///    起。这一条保住了本层的用处：早期轮次里被答过的截图仍是历史。
///    `user(图) assistant(工具调用) tool user(正文)` → 起点 = 最后那条 user
/// 2. **上一条 assistant 之后没有新的 user**（尾巴只有 tool）⇒ 那一问还没答完，本轮退到
///    它自己那一条 user 上。`user(图) assistant(工具调用) tool` → 起点 = 那条 user
/// 3. **一条 assistant 都没有**（`user(图) user(正文)`，或只有一问）⇒ 整段都是本轮，
///    还没有任何东西消费过那张图。
fn current_turn_start(messages: &[Value]) -> usize {
    let is_role = |message: &Value, role: &str| {
        message.get("role").and_then(Value::as_str) == Some(role)
    };
    let Some(assistant) = messages.iter().rposition(|message| is_role(message, "assistant")) else {
        return 0;
    };
    // 情形 1：尾巴里已经有 user 消息
    if let Some(offset) = messages[assistant + 1..]
        .iter()
        .position(|message| is_role(message, "user"))
    {
        return assistant + 1 + offset;
    }
    // 情形 2：尾巴只有 tool / 其它，退回到那一条还没被答的 user
    messages[..=assistant]
        .iter()
        .rposition(|message| is_role(message, "user"))
        .unwrap_or(assistant + 1)
}

/// 这一层此刻要不要工作（= 请求里有图、且当前这一问不依赖图、且没被环境变量关掉）。
///
/// 选路侧读它决定「还留不留这家」，发送侧经 [`strip_reason_for`] 读它决定
/// 「动不动这份 body」—— 两侧共用一处判据是分叉唯一的防线：各写一遍的形态是
/// "候选池里留着这家、发送时却没剥"，症状正是本层要避免的那次 400。
///
/// ⚠️ **预算那一轴不在这里**。本函数回答的是「这叠图**能不能**剥」（结构性判据），
/// 而预算回答的是「该不该为这一家剥」—— 后者是发送侧的事。若把预算塞进来，
/// 一张小图 + 一家不吃图的家会变成"本层不该动手"，于是选路侧照旧把这家剔掉 ——
/// 那正好把能力那一支的用处整块抹掉。
pub fn strip_applies(body: &Value) -> bool {
    if strip_disabled() {
        return false;
    }
    request_carries_images(body) && !current_turn_needs_vision(body)
}

/// 「这一层该不该为**这一家**动手」+「为什么」。`None` = 一个字节都不动。
///
/// 两支的优先次序是固定的：**这家不吃图**先于**预算**。理由不是"更严重"，而是
/// 留痕与占位文本要如实 —— 一家不吃图的家同时撞上预算时，真正解释这次剥图的是
/// 它不吃图（预算那一支的措辞在那家是不准确的）。
///
/// 参数按名字传进来而不是在函数里查目录：判据（本层该动手）与事实（这家不吃图）
/// 是两件不同的事，捆在一个函数里就没法单独测。目录那侧的读法在
/// `providers::catalog::provider_refuses_images`，调用点是 `payload::send_body`。
pub fn strip_reason_for(body: &Value, home_refuses_images: bool) -> Option<StripReason> {
    if !strip_applies(body) {
        return None;
    }
    if home_refuses_images {
        return Some(StripReason::HomeRefusesImages);
    }
    if needs_budget_strip(body, budget_axis_disabled()) {
        return Some(StripReason::HistoryImagesOverBudget);
    }
    None
}

/// 预算那一轴的判据本体（把开关作为入参收进来，于是两种状态都能被用例钉住：
/// 进程环境变量是全局的，用例里改它会污染并发跑的其它用例）。
fn needs_budget_strip(body: &Value, axis_disabled: bool) -> bool {
    !axis_disabled && history_image_bytes(body) > HISTORY_IMAGE_BUDGET_BYTES
}

/// 历史轮里的图片一共占多少**字节**。
///
/// 只累加 `image_url` 的 url 串长（base64 就在那里），**不序列化整份 body** ——
/// 在这条路径上，序列化一遍十几 MB 的体就是为了拿一个数，代价是等量的拷贝与 GC。
///
/// 扫的范围与 [`strip_history_images`] 完全同源（本轮之内不算、`system` 里的算），
/// 判据本体只有 `is_image_part` 与 `current_turn_start` 两处，这里不另立一套 ——
/// 各写一遍的形态是"量出超预算却剥不掉"或反向的"剥了却没量到"，两种都静默。
fn history_image_bytes(body: &Value) -> u64 {
    fn sum(parts: &[Value]) -> u64 {
        parts
            .iter()
            .filter(|part| is_image_part(part))
            .map(|part| {
                part.get("image_url")
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(str::len)
                    .unwrap_or(0) as u64
            })
            .sum()
    }
    let mut total = 0;
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        // 本轮之内的消息不参与读数：它们本来就不在剥离范围内
        for message in messages.iter().take(current_turn_start(messages)) {
            if let Some(parts) = message.get("content").and_then(Value::as_array) {
                total += sum(parts);
            }
        }
    }
    if let Some(parts) = system_parts(body) {
        total += sum(parts);
    }
    total
}

/// `system` 那个字段的块数组形态（两种：直接是数组，或 `{content:[…]}`）。
///
/// 与 [`strip_history_images`] 里那段可变的取法形状一致（那里必须拿 `&mut`，
/// 没法复用这个只读版）；两处形状不一致的话症状是"读数算上了 system 的图、
/// 剥的时候却没碰它"。`system` 是字符串时没有图可言，返回 `None`。
fn system_parts(body: &Value) -> Option<&Vec<Value>> {
    match body.get("system") {
        Some(Value::Array(parts)) => Some(parts),
        Some(Value::Object(object)) => object.get("content").and_then(Value::as_array),
        _ => None,
    }
}

/// 剥掉历史图片，返回处理副本、摘除数与理由。
///
/// `reason` 由调用方给（[`strip_reason_for`] 是唯一出处）：它决定原位留下的那句
/// 占位文本，而"该上游不接受图像输入"与"历史图片占用过大"对模型是两句不同的话。
///
/// `None` = 一个都不该剥（判据不成立，或请求本来就没图）：调用方**零拷贝**沿用
/// 原 body，存量字节不变。这与 `sanitize::sanitize_body` 同一口径 —— 不该为了
/// "统一返回类型"而把无操作的请求复制一遍。
///
/// 逐条规则：
///   · 本轮之内的消息（[`current_turn_start`] 往后）：判据已保证它们无图，因此不触碰；
///   · 其余消息：`content` 数组里的 image part 全部移除；该消息若移除过≥1 张，
///     在**最后一个文本块之后**补一句占位文本（没有文本块就补在开头），
///     让模型先读完它真正会读的文字、再看"这里少了一张图"；
///   · `content` 是字符串的消息没有图可言，原样保留；
///   · 只移图，不动 role / tool_calls / cache_control 之类的兄弟字段。
pub fn strip_history_images(body: &Value, reason: StripReason) -> Option<Stripped> {
    if !strip_applies(body) {
        return None;
    }
    let marker = reason.marker_text();
    let mut next = body.clone();
    let mut removed = 0usize;

    // ① messages
    if let Some(messages) = next.get_mut("messages").and_then(Value::as_array_mut) {
        let turn_start = current_turn_start(messages);
        for (index, message) in messages.iter_mut().enumerate() {
            if index >= turn_start {
                // 当前这一轮：判据保证本轮没图，这里再显式跳过一次，
                // 免得将来有人改了判据而这里跟着遭殃
                continue;
            }
            let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            let taken = strip_parts(parts, marker);
            if taken == 0 {
                continue;
            }
            removed += taken;
        }
    }

    // ② system（各家入口折过来的形态有两种：块数组，或 `{content:[…]}` 对象）
    //     `system_parts` 那个只读版是同一形状的读数版（见它的说明）。
    let system_parts = {
        match next.get_mut("system") {
            Some(Value::Array(_)) => next.get_mut("system").and_then(Value::as_array_mut),
            Some(Value::Object(_)) => next
                .get_mut("system")
                .and_then(|value| value.get_mut("content"))
                .and_then(Value::as_array_mut),
            _ => None,
        }
    };
    if let Some(parts) = system_parts {
        let taken = strip_parts(parts, marker);
        if taken > 0 {
            removed += taken;
        }
    }

    if removed == 0 {
        // 判据说"有图"而这里剥出 0 张 —— 形状与本模块的认知不一致（多半是图藏在
        // 我们没扫到的位置）。**不要**交出一份自称"已经不含图"的副本：那会让
        // 选路侧以为这家能收，结果照旧 400，而日志里写着"已剥离 0 张"。
        // 如实放弃这一层，退回剔家。
        return None;
    }
    Some(Stripped {
        body: next,
        removed,
        reason,
    })
}

/// 就地把 image part 换成占位文本，并把重复的占位并成一条；返回**摘掉的张数**。
///
/// 先换后并的理由很实际：一遍扫完才知道有几张要并，而位置要留在**第一张**的地上
/// —— 第一张图常常在文本块之前，占位符留在原位最贴近"这里原本有个东西"。
/// 一张图都没有时原样返回 0，一个字节都不动（不会凭空插进一句占位文本）。
fn strip_parts(parts: &mut Vec<Value>, marker: &str) -> usize {
    let taken = parts.iter().filter(|part| is_image_part(part)).count();
    if taken == 0 {
        return 0;
    }
    for part in parts.iter_mut() {
        if is_image_part(part) {
            *part = json!({ "type": MARKER_PART, "text": marker });
        }
    }
    // 多条占位只留第一条
    let mut seen_marker = false;
    parts.retain(|part| {
        let is_marker = part.get("type").and_then(Value::as_str) == Some(MARKER_PART)
            && part.get("text").and_then(Value::as_str) == Some(marker);
        if !is_marker {
            return true;
        }
        if seen_marker {
            return false;
        }
        seen_marker = true;
        true
    });
    taken
}

#[cfg(test)]
mod strip_rules {
    //! 三条判据：当前轮是否依赖图、能剥多少、什么时候**不许**自称剥过。
    //!
    //! 用例里的形状全部照转发层的真实形状写（`image_url` 块），并且额外钉一条
    //! Anthropic 的 `tool_result` 形态 —— 那条不是"我们也认它"，而是钉住
    //! 「入口已经把它折成字符串，所以轮不到本层」这个前提（见模块头「形状」）。

    use serde_json::{json, Value};

    use super::{
        budget_axis_disabled_from, current_turn_needs_vision, history_image_bytes,
        needs_budget_strip, request_carries_images, strip_applies, strip_history_images,
        strip_reason_for, StripNotice, StripReason, HISTORY_IMAGE_BUDGET_BYTES,
        MARKER_TEXT_BUDGET, MARKER_TEXT_REFUSES,
    };

    /// 旧用例绝大多数走的是**能力那一支**（这家不吃图）：包一层，别在每个断言里
    /// 重复理由。预算那一支另有专门用例。
    fn strip_refusing(body: &Value) -> Option<super::Stripped> {
        strip_history_images(body, StripReason::HomeRefusesImages)
    }

    fn parts_with_image(text: &str) -> Vec<serde_json::Value> {
        vec![
            json!({"type": "text", "text": text}),
            json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}),
        ]
    }

    #[test]
    fn an_earlier_screenshot_is_strippable_while_a_current_one_is_not() {
        // 历史带图、当前纯文本 —— 这正是 codearts 今天被绕开的那一类请求
        let historical = json!({"model": "deepseek-v4.1-flash", "messages": [
            {"role": "user", "content": parts_with_image("看这张截图")},
            {"role": "assistant", "content": "看到了"},
            {"role": "user", "content": "只回一个字：好"},
        ]});
        assert!(request_carries_images(&historical));
        assert!(
            !current_turn_needs_vision(&historical),
            "上一条 assistant 之后又来了新的 user ⇒ 本轮只剩那一条，它没有图"
        );
        let stripped = strip_refusing(&historical).expect("这一类应当剥得动");
        assert_eq!(1, stripped.removed, "摘除数按**张**计");
        assert!(
            !request_carries_images(&stripped.body),
            "剥完之后本层的判据必须看不见任何图"
        );
        let first = stripped.body["messages"][0]["content"]
            .as_array()
            .expect("数组形态保留");
        assert_eq!(
            1,
            first
                .iter()
                .filter(|part| part["text"] == MARKER_TEXT_REFUSES)
                .count(),
            "原位留一句占位文本"
        );
        assert_eq!(
            "看这张截图", first[0]["text"],
            "真正的文本要排在占位文本之前"
        );
        assert_eq!(
            "只回一个字：好", stripped.body["messages"][2]["content"],
            "当前这一问一个字节都不动"
        );

        // 当前轮带图 —— 一律不动，退回今天的剔家
        let current = json!({"model": "m", "messages": [
            {"role": "user", "content": parts_with_image("上面那张图第三行写了啥？")},
        ]});
        assert!(
            current_turn_needs_vision(&current),
            "整个请求就一条 user，本轮就是它，它自己含图"
        );
        assert!(
            strip_refusing(&current).is_none(),
            "这种请求一个字都不该改"
        );
        assert!(!strip_applies(&current), "选路侧因此照旧把这家剔掉");
    }

    #[test]
    fn a_second_user_message_in_the_same_turn_does_not_hide_the_image() {
        // dev 2026-10-09 一手：客户端把「贴一张图」和「补一句正文」拆成两条连续的
        // user 消息。判据只看最后一条 user 时，那张刚贴的图被当成历史剥掉，
        // codearts 回「没收到图片，看不到内容」—— 而这次本该剔家转给能识图那一台。
        let body = json!({"messages": [
            {"role": "system", "content": "只答颜色"},
            {"role": "user", "content": parts_with_image("这图什么颜色")},
            {"role": "user", "content": "（补充：不用管别的）"},
        ]});
        assert!(
            current_turn_needs_vision(&body),
            "本轮里有两条 user，图在那一条里 —— 就还是本轮的图"
        );
        assert!(!strip_applies(&body), "因此整层不动，选路侧照旧剔掉不吃图的那几家");
        assert!(strip_refusing(&body).is_none(), "一个字节都不该改");

        // 正对照：同一形状里若那张图在上一条**真答过话**的 assistant 之前，仍按历史剥
        let answered = json!({"messages": [
            {"role": "user", "content": parts_with_image("看这张截图")},
            {"role": "assistant", "content": "看到了"},
            {"role": "user", "content": "图里第二行写了啥"},
            {"role": "user", "content": "（补充：不用管别的）"},
        ]});
        assert!(!current_turn_needs_vision(&answered));
        let stripped = strip_refusing(&answered).expect("这一类仍该剥得动");
        assert_eq!(1, stripped.removed);
        assert_eq!(
            "图里第二行写了啥", stripped.body["messages"][2]["content"],
            "本轮那两条连续 user 都不该被触碰"
        );
        assert_eq!(
            "（补充：不用管别的）", stripped.body["messages"][3]["content"],
            "本轮那两条连续 user 都不该被触碰"
        );
    }

    #[test]
    fn a_newer_user_message_closes_the_turn_even_when_the_answer_was_only_a_tool_call() {
        // 情形 1（保住本层的用处）：图之后模型回过话 —— 哪怕只发了 tool_calls ——
        // 并且**又出现了新的 user**，那一问就已被消费，图退回历史轮，照剥。
        let superseded = json!({"messages": [
            {"role": "user", "content": parts_with_image("这图什么颜色")},
            {"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": "结果文本"},
            {"role": "user", "content": "换个问题：今天几号"},
        ]});
        assert!(!current_turn_needs_vision(&superseded), "新的 user 已经把那一问答完了");
        let stripped = strip_refusing(&superseded).expect("这一类仍该剥得动");
        assert_eq!(1, stripped.removed);

        // assistant 带正文的同一形状（块数组形态的正文也要认）
        let answered = json!({"messages": [
            {"role": "user", "content": parts_with_image("这图什么颜色")},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "想一想"},
                {"type": "text", "text": "红色"},
            ]},
            {"role": "user", "content": "换个问题"},
        ]});
        assert!(!current_turn_needs_vision(&answered));
    }

    #[test]
    fn a_trailing_tool_turn_does_not_hide_the_current_question() {
        // agent 会话的尾巴是 assistant(工具调用) + tool(结果)：判据取"本轮"而不是
        // "最后一条消息"，否则带图的那一问会被当成历史剥掉
        let body = json!({"messages": [
            {"role": "user", "content": parts_with_image("这张图里是什么？")},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": "结果文本"},
        ]});
        assert!(
            current_turn_needs_vision(&body),
            "尾巴不是用户那一问：判据要按本轮判，那轮还没答完"
        );
        assert!(strip_refusing(&body).is_none());
    }

    #[test]
    fn three_screenshots_in_one_message_leave_one_marker() {
        let body = json!({"messages": [
            {"role": "user", "content": vec![
                json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}),
                json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,BBB"}}),
                json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,CCC"}}),
            ]},
            {"role": "assistant", "content": "看到了"},
            {"role": "user", "content": "只回一个字：好"},
        ]});
        let stripped = strip_refusing(&body).expect("历史三张图，当前轮纯文本");
        assert_eq!(3, stripped.removed, "张数照实报，别报成消息数");
        let parts = stripped.body["messages"][0]["content"]
            .as_array()
            .expect("数组");
        assert_eq!(1, parts.len(), "三条占位要并成一条");
        assert_eq!(MARKER_TEXT_REFUSES, parts[0]["text"]);
    }

    #[test]
    fn images_hidden_outside_messages_would_not_be_claimed_as_stripped() {
        // 判据说"有图"而变换剥出 0 张：宁可不剥，也不要交出一份自称不含图的副本。
        // 这里把图藏在 `messages` 之外、又不在 system 数组形态的位置（顶层自定义键），
        // 复现"形状与我们的认知不一致"那种情形
        let body = json!({
            "messages": [{"role": "user", "content": "纯文本"}],
            "additional_inputs": {"image_url": {"url": "data:image/png;base64,AAA"}},
        });
        assert!(
            !request_carries_images(&body),
            "本层的判据只覆盖它会剥的位置"
        );
        assert!(
            strip_refusing(&body).is_none(),
            "没图可剥 ⇒ 零拷贝返回 None"
        );
    }

    #[test]
    fn system_array_images_are_stripped_too() {
        // 各家入口可能把 system 折成块数组；只扫 messages 会漏下一张图，
        // 而漏掉的那张正是让这家 400 的东西
        let body = json!({
            "system": [{"type": "text", "text": "你是助手"},
                       {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}],
            "messages": [{"role": "user", "content": "只回一个字：好"}],
        });
        assert!(request_carries_images(&body), "system 里的图也要被看见");
        assert!(!current_turn_needs_vision(&body));
        let stripped = strip_refusing(&body).expect("system 的图能剥");
        assert_eq!(1, stripped.removed);
        assert!(!request_carries_images(&stripped.body));
    }

    #[test]
    fn the_home_capability_gates_the_layer() {
        // 「本层该动手」与「这家不吃图」两件事的关系，就是发送侧能力那一支的入口条件。
        // 真值表钉在这里，是为了让"候选池留着这家、发送时没剥"那种分叉一写出来就红。
        let historical = json!({"messages": [
            {"role": "user", "content": parts_with_image("看这张截图")},
            {"role": "assistant", "content": "看到了"},
            {"role": "user", "content": "只回一个字：好"},
        ]});
        let current = json!({"messages": [
            {"role": "user", "content": parts_with_image("这张图第三行写了啥？")},
        ]});
        assert_eq!(
            Some(StripReason::HomeRefusesImages),
            strip_reason_for(&historical, true),
            "不吃图的家 + 图在历史 ⇒ 剥（能力那一支）"
        );
        assert!(
            strip_reason_for(&historical, false).is_none(),
            "这家能吃图、这叠图也没超预算 ⇒ 一个字节都不动"
        );
        assert!(
            strip_reason_for(&current, true).is_none(),
            "当前轮依赖图 ⇒ 本层不动手（选路侧照旧剔家）"
        );
        assert!(
            strip_reason_for(
                &json!({"messages": [{"role": "user", "content": "纯文本"}]}),
                true
            )
            .is_none(),
            "没有图可言 ⇒ 不动手，也不该插占位文本"
        );
    }

    // ── 预算那一轴（本次新增）──────────────────────────────────
    //
    // 一手读数（dev 3066，2026-10-15）：同一份 16,341,828 字节的会话里有 169 个
    // 图片块，平均每块约 96 KB；而同一家实测能收下的最大体是 2,014,731 字节。
    // 这个形状用一条 `history_image_bytes` 就能认出来，不必真的拼一个 16 MB 的体。

    /// 造一张 url **恰好** `url_bytes` 字节的图（含 `data:` 前缀），形状与真请求一致。
    ///
    /// 判据数是 `image_url.url` 的串长，所以这里必须把前缀也算进去 —— 否则边界
    /// 用例量的是"前缀 + 填充"而它自称量的是填充（第一次就是这么差 22 字节红的）。
    fn sized_image(url_bytes: usize) -> Value {
        const PREFIX: &str = "data:image/png;base64,";
        let fill = url_bytes.saturating_sub(PREFIX.len());
        json!({
            "type": "image_url",
            "image_url": {"url": format!("{PREFIX}{}", "A".repeat(fill))},
        })
    }

    /// 历史图 + 当前轮纯文本：预算那一支唯一认得的形状。
    fn historical_with_image_bytes(bytes: usize) -> Value {
        json!({"messages": [
            {"role": "user", "content": [sized_image(bytes)]},
            {"role": "assistant", "content": "看到了"},
            {"role": "user", "content": "只回一个字：好"},
        ]})
    }

    #[test]
    fn a_huge_screenshot_pile_is_stripped_even_though_the_home_accepts_images() {
        // 能吃图 ≠ 该把二十几轮前的截图每一轮都重发一遍。
        // 体量照实测那一份写：169 块 × 约 96 KB ≈ 15 MB，这里取预算的四倍。
        let body = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize * 4);
        assert_eq!(
            Some(StripReason::HistoryImagesOverBudget),
            strip_reason_for(&body, false),
            "这家**能**吃图，但这叠历史图已经越过预算 ⇒ 仍要剥"
        );
        let stripped = strip_history_images(&body, StripReason::HistoryImagesOverBudget)
            .expect("超预算这一类必须剥得动");
        assert_eq!(1, stripped.removed);
        assert!(
            !request_carries_images(&stripped.body),
            "剥完了这一家就不该再收到图"
        );
    }

    /// 反面对照：没超预算就不许动。
    ///
    /// 少了这一条，判据写成「有历史图就剥」也会全绿 —— 那等于把能识图的家降级成
    /// 不能识图的家，用户的截图从此再也进不了上下文。
    #[test]
    fn a_history_under_budget_is_left_alone() {
        let body = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize / 4);
        assert!(
            strip_reason_for(&body, false).is_none(),
            "没超预算 + 这家能吃图 ⇒ 一个字节都不动"
        );
        assert!(strip_history_images(&body, StripReason::HomeRefusesImages).is_some(), "（同一份体在不吃图的家仍该剥：能力那一支与预算那一支互不影响）");
    }

    /// 边界：正好等于预算是"没过"（与 `>` 而不是 `>=` 同口径）。
    #[test]
    fn the_budget_boundary_is_strictly_greater_than() {
        let exact = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize);
        let over = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize + 1);
        assert_eq!(HISTORY_IMAGE_BUDGET_BYTES, history_image_bytes(&exact), "读数就是 url 串长之和");
        assert!(!needs_budget_strip(&exact, false), "正好等于预算 ⇒ 不剥");
        assert!(needs_budget_strip(&over, false), "多一个字节 ⇒ 剥");
    }

    /// 开关只关预算那一轴：能力那一轴一格都不许被它影响。
    #[test]
    fn the_budget_switch_turns_only_that_axis_off() {
        assert!(budget_axis_disabled_from(Some("1")));
        assert!(budget_axis_disabled_from(Some(" on ")));
        assert!(budget_axis_disabled_from(Some("YES")));
        assert!(!budget_axis_disabled_from(None), "缺省是开着（默认剥）");
        assert!(!budget_axis_disabled_from(Some("0")));

        let over = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize + 1);
        assert!(needs_budget_strip(&over, false), "开着 ⇒ 超预算要剥");
        assert!(!needs_budget_strip(&over, true), "关掉 ⇒ 这一轴不动手");
        assert_eq!(
            Some(StripReason::HomeRefusesImages),
            strip_reason_for(&over, true),
            "关掉预算那一轴之后，不吃图的家照旧要剥"
        );
    }

    /// 两支的**模型可见文本**必须不同。
    ///
    /// 预算那一支上这家明明能吃图，写"该上游不接受图像输入"是假话，而模型据此
    /// 给用户"换个能看图的供应商"这类错建议就是这么来的。明细里的留痕同理。
    #[test]
    fn the_two_reasons_leave_different_words() {
        assert_ne!(MARKER_TEXT_REFUSES, MARKER_TEXT_BUDGET);
        let refuses = StripNotice {
            removed: 3,
            reason: StripReason::HomeRefusesImages,
        };
        let budget = StripNotice {
            removed: 3,
            reason: StripReason::HistoryImagesOverBudget,
        };
        assert!(refuses.describe().contains("不接受图像输入"));
        assert!(budget.describe().contains("预算"));
        assert_ne!(refuses.describe(), budget.describe());
        // 占位文本真的落进 body 的是哪一句：这一条把上面那个枚举与剥出来的字节扣在一起
        let body = historical_with_image_bytes(HISTORY_IMAGE_BUDGET_BYTES as usize + 1);
        let stripped =
            strip_history_images(&body, StripReason::HistoryImagesOverBudget).expect("剥得动");
        assert_eq!(
            MARKER_TEXT_BUDGET,
            stripped.body["messages"][0]["content"][0]["text"],
            "预算那一支留下的必须是预算那句占位"
        );
    }

    /// 预算再大也不许碰**当前这一轮**：这一问就靠那张图。
    #[test]
    fn the_budget_axis_never_touches_the_current_turn() {
        let body = json!({"messages": [
            {"role": "user", "content": [sized_image(HISTORY_IMAGE_BUDGET_BYTES as usize * 2)]},
        ]});
        assert!(history_image_bytes(&body) == 0, "本轮之内的图不进读数");
        assert!(!strip_applies(&body), "含图 ⇒ 本层整层不动");
        assert!(strip_reason_for(&body, false).is_none());
        assert!(
            strip_reason_for(&body, true).is_none(),
            "不吃图的家也一样：这一问就依赖那张图，剥了必然答错"
        );
    }

    /// 闸门与剥图层必须对**同一份请求**给出一致的结论。
    ///
    /// 这是这次改动唯一的致命分叉：选路侧 `decide_image_gate` 判"图可剥"而把这家
    /// 留在候选里，发送侧却没剥干净 —— 结果就是带图的请求送进不吃图的上游，
    /// 回一句 "The request model is not multimodal"。两侧读的是同一对函数
    /// （`strip_applies` / `provider_refuses_images`），但"同一个函数"这件事本身
    /// 也要被钉住：下面这组形状各异的请求体过一遍，任何一处"判可剥却仍留图"
    /// 都会当场红。形状覆盖特意包含：图在 system、图在 assistant、一条消息多张图、
    /// 工具尾巴、纯文本无图。
    #[test]
    fn what_the_gate_keeps_the_stripper_actually_empties() {
        let img = |tag: &str| json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{tag}")}});
        let cases: Vec<(&str, Value)> = vec![
            (
                "图在历史 user，当前轮纯文本",
                json!({"messages": [
                    {"role": "user", "content": [json!({"type":"text","text":"看图"}), img("A")]},
                    {"role": "assistant", "content": "看了"},
                    {"role": "user", "content": "只回一个字"},
                ]}),
            ),
            (
                "图在 system 数组里",
                json!({
                    "system": [json!({"type":"text","text":"你是助手"}), img("B")],
                    "messages": [{"role": "user", "content": "只回一个字"}],
                }),
            ),
            (
                "图在 assistant 历史里",
                json!({"messages": [
                    {"role": "user", "content": "看这张"},
                    {"role": "assistant", "content": [img("C"), json!({"type":"text","text":"好的"})]},
                    {"role": "user", "content": "然后呢"},
                ]}),
            ),
            (
                "一条消息里三张图",
                json!({"messages": [
                    {"role": "user", "content": [img("D"), img("E"), img("F")]},
                    {"role": "assistant", "content": "看了"},
                    {"role": "user", "content": "只回一个字"},
                ]}),
            ),
            (
                "连续两条 user，图在前一条（本轮不许动手）",
                json!({"messages": [
                    {"role": "user", "content": [img("D2"), json!({"type":"text","text":"这张图"})]},
                    {"role": "user", "content": "补充：不用管别的"},
                ]}),
            ),
            (
                "尾巴是工具调用与结果，图在更早的 user",
                json!({"messages": [
                    {"role": "user", "content": [img("G"), json!({"type":"text","text":"这张报错图"})]},
                    {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
                    {"role": "tool", "tool_call_id": "c1", "content": "结果文本"},
                    {"role": "user", "content": "那我现在该改哪一行"},
                ]}),
            ),
            (
                "当前轮就带图（不许动手）",
                json!({"messages": [
                    {"role": "user", "content": [img("H"), json!({"type":"text","text":"第三行写了啥"})]},
                ]}),
            ),
            (
                "整份请求没有图",
                json!({"messages": [{"role": "user", "content": "你好"}]}),
            ),
        ];
        for (label, body) in cases {
            let strippable = strip_applies(&body);
            match strip_refusing(&body) {
                Some(stripped) => {
                    assert!(strippable, "{label}：动了 body，闸门却没判可剥 —— 反向分叉");
                    assert!(
                        !request_carries_images(&stripped.body),
                        "{label}：闸门会保留这家，而发出去的 body 里还有图（就是那次 400）"
                    );
                    assert!(stripped.removed >= 1, "{label}：声称剥过却报 0 张");
                }
                None => {
                    if request_carries_images(&body) {
                        assert!(
                            !strippable,
                            "{label}：判可剥却一颗都没剥成 —— 这种请求会被留在候选里带图上行"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_request_without_images_is_left_untouched() {
        let body = json!({"messages": [
            {"role": "system", "content": "别动我"},
            {"role": "user", "content": "只回一个字：好"},
        ]});
        assert!(!request_carries_images(&body));
        assert!(
            strip_refusing(&body).is_none(),
            "无图 ⇒ None ⇒ 调用点零拷贝"
        );
    }

    #[test]
    fn sibling_fields_survive_the_strip() {
        // 只移图：role / tool_calls / 各家内部字段都得原样留着，
        // 否则剥图会顺手把工具循环拆坏（那是比 400 严重得多的故障）
        let body = json!({"messages": [
            {"role": "user", "content": parts_with_image("看图")},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function"}],
             "_wb_kept": {"x": 1}},
            {"role": "user", "content": "然后呢"},
        ]});
        let stripped = strip_refusing(&body).expect("剥得动");
        let assistant = &stripped.body["messages"][1];
        assert_eq!(
            1,
            assistant["tool_calls"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
        );
        assert_eq!(
            1,
            assistant["_wb_kept"]["x"].as_i64().unwrap_or(0),
            "内部暂存字段不该被牵连"
        );
        assert_eq!("assistant", assistant["role"]);
    }
}
