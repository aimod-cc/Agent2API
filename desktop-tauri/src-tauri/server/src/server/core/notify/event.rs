//! 通知事件：**发什么**（与「怎么发」解耦，见模块头的说明）。

use serde::{Deserialize, Serialize};

/// 事件的严重度（决定前缀图标与用词，不影响投递）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifyLevel {
    /// 出错了：需要用户动手
    Error,
    /// 需要注意但还没坏
    Warn,
    /// 中性信息
    Info,
}

impl NotifyLevel {
    /// 事件名前缀（通道消息体里的图标）
    pub fn icon(self) -> &'static str {
        match self {
            Self::Error => "🔴",
            Self::Warn => "🟠",
            Self::Info => "🔵",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }
}

/// 一条待发的通知事件。
///
/// `kind` 是**去重的键**（见 `dedupe`）：同类事件在窗口内只发一次，
/// 所以它必须稳定描述「哪一类事」，**不能带每次都变的东西**
/// （时间戳、错误原文里的随机 id）—— 那样每条都会被当成新事件。
#[derive(Clone, Debug)]
pub struct NotifyEvent {
    /// 事件类型（与配置里 `events` 的订阅名对应，也是去重键的一部分）
    pub kind: String,
    /// 去重时的二级键（同一类事件里的**具体对象**：账号 id / 任务名）。
    /// 空串表示「这一类不再细分」。
    pub key: String,
    pub level: NotifyLevel,
    /// 标题（一句话说清发生了什么）
    pub title: String,
    /// 正文（细节、建议动作；可以为空）
    pub body: String,
}

/// 已支持的事件名（配置里的订阅清单就用这些；界面照此渲染勾选项）。
pub const KIND_TASK_FAILED: &str = "task_failed";
pub const KIND_CREDENTIAL_INVALID: &str = "credential_invalid";
pub const KIND_TEST: &str = "test";

/// 全部可选事件（顺序即界面展示顺序）。
///
/// 让界面从后端拿这份清单而不是自己写死：加事件时只改这里，
/// 界面自动多出一个勾选项 —— 否则「后端能发但界面没得勾」会变成静默不会发的坑。
pub fn available_kinds() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            KIND_TASK_FAILED,
            "定时任务失败",
            "签到 / 凭证维护 / 模型刷新等定时任务整轮失败时推送（含失败原因）",
        ),
        (
            KIND_CREDENTIAL_INVALID,
            "账号凭证失效",
            "账号登录态被上游作废（需重新登录）时推送",
        ),
    ]
}

/// 这条事件是否被订阅。`events` 为空 = 全部订阅（与「没勾就是全不勾」相反，
/// 因为默认值必须是「配好渠道就能收到东西」—— 否则用户配完渠道却什么都没发生，
/// 会以为功能坏了）。
pub fn is_subscribed(configured: &[String], kind: &str) -> bool {
    configured.is_empty() || configured.iter().any(|item| item == kind)
}

impl NotifyEvent {
    /// 定时任务整轮失败
    pub fn task_failed(task_name: &str, summary: &str, failures: i64) -> Self {
        let detail = if summary.trim().is_empty() { "（无详情）" } else { summary.trim() };
        Self {
            kind: KIND_TASK_FAILED.to_string(),
            key: task_name.to_string(),
            level: NotifyLevel::Error,
            title: format!("定时任务失败：{task_name}"),
            body: format!("连续失败 {failures} 次。\n{detail}"),
        }
    }

    /// 账号凭证失效（需重新登录）
    pub fn credential_invalid(name: &str, reason: &str) -> Self {
        let detail = if reason.trim().is_empty() { "（上游未给出原因）" } else { reason.trim() };
        Self {
            kind: KIND_CREDENTIAL_INVALID.to_string(),
            key: name.to_string(),
            level: NotifyLevel::Error,
            title: format!("账号凭证失效：{name}"),
            body: format!("{detail}\n建议到「账号」页重新登录该账号。"),
        }
    }

    /// 测试事件（设置页「发送测试」用；不参与去重）
    pub fn test() -> Self {
        Self {
            kind: KIND_TEST.to_string(),
            key: String::new(),
            level: NotifyLevel::Info,
            title: "Agent2API 通知测试".to_string(),
            body: "如果你看到这条消息，说明这个渠道配置正确。".to_string(),
        }
    }

    /// 拼成一段纯文本（通用渠道直接发它；Markdown 渠道也以它为主体）
    pub fn plain_text(&self) -> String {
        let mut text = format!("{} {}", self.level.icon(), self.title);
        if !self.body.trim().is_empty() {
            text.push('\n');
            text.push_str(self.body.trim());
        }
        text
    }
}
