//! 请求来源客户端识别（下游 `User-Agent` → 客户端类型）。
//!
//! ── 它回答什么问题 ──────────────────────────────────────────
//! 请求日志的「客户端」列要能一眼看出「这条请求是哪个下游发来的」：
//! 同一个网关会被 Claude Code、Codex、ZCode、CherryStudio 等好几个客户端
//! 混着用，排障时「这条是 Claude Code 发的还是 Codex 发的」是最先要分清的一维。
//!
//! ── 判定口径：前缀匹配，规则表照抄 OmniProxy 的 `classifyClient` ──────
//! 一条 UA 只在**开头**匹配（CherryStudio 例外，见下）：
//!   - `Codex Desktop/` / `codex_vscode/` / `codex-tui/` → `Codex`
//!     （三种形态都是 Codex，归一类而不是各占一行：客户端列要读的是
//!      「谁在发」，不是「哪个构建形态」）
//!   - `ZCode/` → `ZCode`
//!   - `claude-cli/` → `Claude Code`
//!   - `deepseek-harness/` → `deepseek-harness`
//!   - `GitHubCopilotChat/` → `GitHub Copilot`
//!   - 任意位置含 `CherryStudio/` → `CherryStudio`（它的 UA 会带一段运行时
//!     前缀，`Electron/… CherryStudio/…` 这种，前缀匹配抓不到）
//!   - 有 UA 但不属于以上任何一类 → `other`
//!   - 无 UA（空串 / 全空白）→ 空串（「不知道是谁发的」，不猜「其它」）
//!
//! 值本身就是**展示值**（`Codex` / `Claude Code` 这类产品名不翻译），
//! 只有 `other` 由前端翻成「其它」。与 OmniProxy 逐字一致 —— 两边的
//! 请求日志列因此可以用同一套判读经验。
//!
//! ── 为什么是纯函数 ──────────────────────────────────────────
//! 识别只依赖 UA 字符串，不需要请求上下文。采集点（各入口 handler）自己
//! 从请求头取 UA 再调这里，存储层收到的是**已经判好的类型**（存字符串，
//! 不存原始 UA）：明细里留一份原始 UA 会随每个客户端的版本号变化而膨胀，
//! 而「是谁」这一维只需要归类后的结果（与 OmniProxy 存 `client_type` 同一取舍）。

/// UA 无匹配但非空时的归类值（前端显示为「其它」）。
pub const CLIENT_OTHER: &str = "other";

/// 从下游 `User-Agent` 识别客户端类型；无 UA 返回空串。
///
/// 大小写敏感是**有意**的：产品 UA 的固定前缀就长这样（`ZCode/`、
/// `claude-cli/`），放宽到不敏感会让 `zcode/…` 这类非产品 UA 也命中 ——
/// 宁可多落几个 `other`，也不要给出一个可能错的归类。
pub fn classify(ua: &str) -> String {
    let ua = ua.trim();
    if ua.is_empty() {
        return String::new();
    }
    // Codex 的三种形态（桌面端 / VS Code 插件 / TUI）归一类
    if ua.starts_with("Codex Desktop/")
        || ua.starts_with("codex_vscode/")
        || ua.starts_with("codex-tui/")
    {
        return "Codex".to_string();
    }
    if ua.starts_with("ZCode/") {
        return "ZCode".to_string();
    }
    if ua.starts_with("claude-cli/") {
        return "Claude Code".to_string();
    }
    if ua.starts_with("deepseek-harness/") {
        return "deepseek-harness".to_string();
    }
    if ua.starts_with("GitHubCopilotChat/") {
        return "GitHub Copilot".to_string();
    }
    // 唯一一条子串匹配：CherryStudio 的 UA 前面还有运行时段（见模块头）
    if ua.contains("CherryStudio/") {
        return "CherryStudio".to_string();
    }
    CLIENT_OTHER.to_string()
}
