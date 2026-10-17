//! 事件接驳：把各处的「出事了」接成通知事件。
//!
//! ── 为什么单独一个文件 ──────────────────────────────────────
//! 事件源在**各自的模块里**（任务收尾在 `core::task_state`、凭证失效在
//! `core::credential_maintenance`），它们不该认识通知；而通知也不该去遍历
//! 那些模块。这个文件是两者之间**唯一**的接线处：一边是各模块暴露的钩子，
//! 一边是 [`super::notify`]。要加新事件源，只改这里 + 在 `install` 里挂上。
//!
//! ── 打扰策略：什么才值得推送 ────────────────────────────────
//! 通知的价值与它的频率成反比 —— 一条被刷屏淹没的通知等于没有。所以这里
//! 定了两条克制规则（都在 [`on_task_finish`] 里）：
//!   · **只报连续失败的第 1、3 次** —— 第 1 次说明「刚出问题」，
//!     第 3 次说明「不是偶发」。中间的每一次都推就是在刷屏，而第 4 次之后
//!     任务已经退避到很久才跑一轮（最长一天），继续报也没有新信息。
//!   · 底层还有一层 6 小时窗口去重（见 `dedupe`），兜住「同一件事换了
//!     说法又报一遍」的情况。

use super::event::NotifyEvent;
use crate::server::core::task_state;

/// 挂上全部事件源。**在服务启动时调一次**（见 `server::mod` 的初始化段）。
pub fn install() {
    task_state::observe_task_finish(on_task_finish);
    // 配置的「是否可发送」内存镜像也要先装一次，否则首次任务失败时
    // `notify::active()` 还是 false（那份镜像是 AtomicBool，默认关）
    super::refresh_cache();
}

/// 定时任务收尾的回调（由 `task_state::RunGuard::finish` 调用）。
///
/// `task_id` 是**配置键**（如 `credentialMaintenance`），不是给人看的名字 ——
/// 这里换成 `scheduled_tasks::TASKS` 里的中文标签，日志与推送里才有意义。
fn on_task_finish(task_id: &str, success: bool, summary: &str, failures: u32) {
    if success {
        return;
    }
    // 只报第 1、3 次（理由见模块头）。`failures` 是**落库后的**次数，
    // 所以这里看到 1 就是第一次失败。
    if failures != 1 && failures != 3 {
        return;
    }
    let label = task_label(task_id);
    super::notify(NotifyEvent::task_failed(&label, summary, failures as i64));
}

/// 任务 id → 中文标签。**只查一次表**并把结果缓存起来：`TASKS` 是编译期
/// 常量表，查它本身很便宜，但这条路径要在每轮任务收尾时走，缓存掉更干净。
///
/// 查不到时退回 id 本身（不让「新加的任务还没登记进 TASKS」变成一条
/// 「定时任务失败：undefined」）。
fn task_label(task_id: &str) -> String {
    crate::server::core::scheduled_tasks::TASKS
        .iter()
        .find(|task| task.id == task_id)
        .map(|task| task.label.to_string())
        .unwrap_or_else(|| task_id.to_string())
}

/// 账号凭证失效（由 `credential_maintenance` 在判定「需重新登录」时调用）。
///
/// `name` 是账号的显示名（用户能认出是哪个号），`reason` 是上游给的原因。
pub fn on_credential_invalid(name: &str, reason: &str) {
    super::notify(NotifyEvent::credential_invalid(name, reason));
}
