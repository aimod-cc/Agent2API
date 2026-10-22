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
/// 账号被自动禁用（余额不足 / Token 用量触及上限）——需要用户手动启用
pub const KIND_ACCOUNT_DISABLED: &str = "account_disabled";
/// 转发时一个可用账号都找不到（没加账号 / 全部被禁用或冷却）——全局事件
pub const KIND_NO_AVAILABLE_ACCOUNT: &str = "no_available_account";
/// 账号有积分包将到期且尚有剩余——提醒尽快用掉
pub const KIND_CREDIT_EXPIRING: &str = "credit_expiring";
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
        (
            KIND_ACCOUNT_DISABLED,
            "账号被自动禁用",
            "余额不足或 Token 用量触及上限时账号被自动禁用（不会自动恢复，需手动启用）",
        ),
        (
            KIND_NO_AVAILABLE_ACCOUNT,
            "无可用账号",
            "一次转发请求到来时没有任何可用的账号（全部禁用 / 冷却 / 无凭证 / 没加账号）",
        ),
        (
            KIND_CREDIT_EXPIRING,
            "积分即将到期",
            "账号有积分包在设定天数内到期且尚有剩余——提醒尽快用掉，避免作废",
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

    /// 账号被自动禁用（余额不足 / Token 用量触及上限）
    ///
    /// `reason` 是那条禁用判定的原文（含读数与阈值），直接透出 ——
    /// 用户据此判断「该充值还是该改阈值」。
    pub fn account_disabled(name: &str, reason: &str) -> Self {
        let detail = if reason.trim().is_empty() { "（未给出原因）" } else { reason.trim() };
        Self {
            kind: KIND_ACCOUNT_DISABLED.to_string(),
            key: name.to_string(),
            level: NotifyLevel::Warn,
            title: format!("账号已自动禁用：{name}"),
            body: format!("{detail}\n该账号已不参与转发；处理后请到「账号」页手动启用（不会自动恢复）。"),
        }
    }

    /// 转发时没有任何可用账号（全局事件，`key` 固定为空 —— 同一时刻只说一次）
    pub fn no_available_account(reason: &str) -> Self {
        let detail = if reason.trim().is_empty() { "（未给出原因）" } else { reason.trim() };
        Self {
            kind: KIND_NO_AVAILABLE_ACCOUNT.to_string(),
            // 全局事件不细分：`key` 留空，去重窗口内只说一次（见 `dedupe`）
            key: String::new(),
            level: NotifyLevel::Error,
            title: "没有可用账号".to_string(),
            body: format!("{detail}\n转发请求已经失败，请到「账号」页添加 / 启用 / 重新登录账号。"),
        }
    }

    /// 积分即将到期（`name` 是账号显示名）
    pub fn credit_expiring(name: &str, detail: &str) -> Self {
        let detail = if detail.trim().is_empty() { "（未给出明细）" } else { detail.trim() };
        Self {
            kind: KIND_CREDIT_EXPIRING.to_string(),
            key: name.to_string(),
            level: NotifyLevel::Warn,
            title: format!("积分即将到期：{name}"),
            body: format!("{detail}\n过期的积分会作废，建议尽快使用。"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个新增事件都在 `available_kinds()` 里出现 —— 否则「后端能发但界面
    /// 没得勾」会变成静默不会发的坑（见 `available_kinds` 的说明）。
    ///
    /// 这条钉住的是一个真实的接线错误面：前端设置页的勾选项**完全由这份清单
    /// 渲染**（`api::notify` 的 `notify_json`），清单漏了就等于开关不存在。
    #[test]
    fn new_kinds_are_listed() {
        let listed: Vec<&str> = available_kinds().into_iter().map(|(id, _, _)| id).collect();
        for kind in [KIND_ACCOUNT_DISABLED, KIND_NO_AVAILABLE_ACCOUNT, KIND_CREDIT_EXPIRING] {
            assert!(listed.contains(&kind), "available_kinds() 缺少 {kind}");
        }
    }

    /// 无可用账号是**全局**事件：`key` 必须为空，否则去重键会按实例分裂，
    /// 一个空池会随每个请求刷出一条通知（见 `dedupe` 的窗口语义）。
    #[test]
    fn no_available_account_is_global() {
        let event = NotifyEvent::no_available_account("池是空的");
        assert_eq!(event.kind, KIND_NO_AVAILABLE_ACCOUNT);
        assert_eq!(event.key, "", "全局事件的去重键必须为空");
        assert_eq!(event.level, NotifyLevel::Error);
    }

    /// 账号被禁用 / 积分到期是**按账号**的事件：`key` 取账号显示名，
    /// 这样同一账号在窗口内只喊一次，不同账号各自独立。
    #[test]
    fn per_account_events_key_by_name() {
        let disabled = NotifyEvent::account_disabled("小号A", "余额 0 < 阈值 1");
        assert_eq!(disabled.kind, KIND_ACCOUNT_DISABLED);
        assert_eq!(disabled.key, "小号A");
        assert_eq!(disabled.level, NotifyLevel::Warn);

        let expiring = NotifyEvent::credit_expiring("小号B", "「每日积分」将于明天到期");
        assert_eq!(expiring.kind, KIND_CREDIT_EXPIRING);
        assert_eq!(expiring.key, "小号B");
        assert_eq!(expiring.level, NotifyLevel::Warn);
    }

    /// 原因 / 明细为空时不能产出「（空）」式的空正文 —— 通知正文里至少
    /// 要有一句可读的兜底（与 `task_failed` / `credential_invalid` 同款）。
    #[test]
    fn empty_detail_gets_fallback_text() {
        let event = NotifyEvent::account_disabled("x", "   ");
        assert!(event.body.contains("未给出原因"));
        let event = NotifyEvent::credit_expiring("y", "");
        assert!(event.body.contains("未给出明细"));
    }
}
