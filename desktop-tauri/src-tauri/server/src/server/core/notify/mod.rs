//! 通知与告警：把「出事了」推给用户（Webhook）。
//!
//! ── 为什么需要它 ────────────────────────────────────────────
//! 桌面端是**托盘常驻**的：用户不会一直盯着界面。但没有推送时，
//! 签到失败、账号凭证失效、额度耗尽这些事只能靠用户自己想起来打开看 ——
//! 而这类事的价值恰恰在「及时」：凭证失效早发现一天，就少一天转发失败。
//! 这是桌面端相对 Web 版面板的天然优势（面板关掉就什么都不会说）。
//!
//! ── 几种渠道为什么合成一套 ──────────────────────────────────
//! 飞书 / 企业微信 / 钉钉 / 通用 Webhook 的消息体**形状不同**（飞书与钉钉是
//! `{msgtype, content}`、企微是 `{msgtype, markdown}`、通用是纯文本），但
//! 「什么时候该发、发什么内容、发失败怎么办」是**完全一样**的。所以这里按
//! 「一个事件 → 各渠道各自渲染 → 各自投递」来组织：事件与内容在 `event` 层，
//! 形状差异收在 [`Channel::payload`] 一处。
//!
//! ── 投递纪律（都是为了「通知不该把事情弄得更糟」）─────────────
//!   1. **只发一次、异步发**：调用方（任务收尾、凭证失效）**不能**因为
//!      Webhook 慢或不通而卡住 —— 通知是旁路，主流程不等它。所以走
//!      `tokio::spawn`，且调用点一律用 [`notify`] 这一个入口（它内部吞掉
//!      全部错误，见那里的说明）。
//!   2. **失败不重试、不排队**：任务失败本身会退避重试（见 `task_state`），
//!      再叠一层通知重试只会让同一件事被说好几遍。丢一条通知远好过刷屏。
//!   3. **同类事件去重**：同一账号连续失效、同一任务连续失败时不重复喊 ——
//!      见 [`dedupe`] 的窗口。没有这一层，一个坏账号能在几小时内刷出上百条。
//!   4. **配置为空就是关**：没配任何渠道时 [`notify`] 直接返回，不产生
//!      任何网络请求、也不写日志（默认静默）。

pub mod config;
pub mod dedupe;
pub mod event;
/// 事件接驳：各模块的钩子 → 通知事件（要加新事件源只改这里，见文件头）
pub mod hooks;
pub mod sender;

pub use config::{NotifyConfig, NotifyChannelConfig};
pub use event::{NotifyEvent, NotifyLevel};
pub use hooks::install;

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::server::logging;

/// 总开关的内存镜像：读配置太频（每次任务收尾都要问一次）而写极少，
/// 缓存一份可以省掉每轮的 JSON 反序列化。写配置时由 [`refresh_cache`] 刷新。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 从配置刷新内存镜像。配置写入路径（`api::notify`）必须调它。
pub fn refresh_cache() {
    let config = NotifyConfig::load();
    ENABLED.store(config.enabled && !config.channels.is_empty(), Ordering::Relaxed);
}

/// 当前是否处于「可发送」状态（总开关开着且至少配了一个渠道）。
///
/// 单独暴露给调用方做**廉价短路**：任务收尾这类高频路径上，
/// 先问一句比每次都去构造事件、查重、序列化要省得多。
pub fn active() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 发一条通知。**这是唯一对外的入口**，也是唯一该被调用的函数。
///
/// 三条纪律（见模块头）在这里落地：
///   · 未启用 → 立刻返回（零开销）；
///   · 去重窗口内重复 → 丢掉；
///   · 真正投递走 `tokio::spawn`，**调用方不等结果**。
///
/// 本函数**不返回 Result**：通知发不出去是旁路故障，调用方（任务收尾、
/// 凭证维护）没有任何合理的应对动作 —— 返回一个必然被 ignore 的 Result
/// 只会让调用点多写一行 `let _ =`，且给读者「这里该处理错误」的错误暗示。
pub fn notify(event: NotifyEvent) {
    if !active() {
        return;
    }
    if dedupe::is_duplicate(&event) {
        return;
    }
    let config = NotifyConfig::load();
    if !config.enabled || config.channels.is_empty() {
        return;
    }
    // 通知是旁路：主流程不等它。失败也只在日志里留一条（见 sender）。
    tokio::spawn(async move {
        sender::deliver(&config, &event).await;
    });
}

/// 发一条测试通知（设置页的「发送测试」按钮走这里）。
///
/// 与 [`notify`] 的两处差别都是刻意的：
///   · **不走去重** —— 用户连点两次「测试」该收到两条，那是他在验证；
///   · **同步等结果** —— 界面要立刻告诉他通没通、失败原因是什么。
pub async fn notify_test(channel_index: Option<usize>) -> Result<Vec<String>, String> {
    let config = NotifyConfig::load();
    if config.channels.is_empty() {
        return Err("还没有配置任何通知渠道".to_string());
    }
    let event = NotifyEvent::test();
    let mut results = Vec::new();
    for (index, channel) in config.channels.iter().enumerate() {
        if let Some(want) = channel_index {
            if want != index {
                continue;
            }
        }
        match sender::deliver_one(channel, &event).await {
            Ok(()) => results.push(format!("{}：发送成功", channel_label(channel))),
            Err(error) => results.push(format!("{}：{}", channel_label(channel), error)),
        }
    }
    if results.is_empty() {
        return Err("没有找到指定的渠道".to_string());
    }
    Ok(results)
}

/// 渠道的可读名（日志与测试结果里用；没填名字时退回标记）
fn channel_label(channel: &NotifyChannelConfig) -> String {
    let name = channel.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    match channel.channel_type.as_str() {
        "feishu" => "飞书".to_string(),
        "wecom" => "企业微信".to_string(),
        "dingtalk" => "钉钉".to_string(),
        other => other.to_string(),
    }
}

/// 当前配置的公开形态（脱敏：Webhook URL 里的 token 段遮掉）。
///
/// 为什么连本地配置也要脱敏：这个响应会被**网页端面板**读到（同一套 API），
/// 而 URL 本身就是凭证 —— 拿到它就能往那个群里发任意消息。
pub fn public_config() -> Value {
    let config = NotifyConfig::load();
    json!({
        "enabled": config.enabled,
        "events": config.events,
        "channels": config.channels.iter().map(|channel| json!({
            "name": channel.name,
            "type": channel.channel_type,
            // 只回遮过的 URL：界面显示「已配置」，改的时候整体重填（与密码同款）
            "url": mask_url(&channel.url),
            "hasUrl": !channel.url.trim().is_empty(),
        })).collect::<Vec<_>>(),
    })
}

/// 把 Webhook URL 的**末段**（就是那个 token）遮掉。
///
/// 保留 host 与前缀是为了让用户能确认「填的是哪家的地址」——
/// 整条遮成 `****` 就分不出飞书与企微了，而那恰恰是排查时要看的。
fn mask_url(url: &str) -> String {
    let text = url.trim();
    if text.is_empty() {
        return String::new();
    }
    match text.rfind('/') {
        Some(index) if index + 1 < text.len() => {
            let (head, tail) = text.split_at(index + 1);
            // 太短的尾段（不是 token 形态）不遮，免得遮成一个没法辨认的东西
            if tail.len() <= 4 {
                return text.to_string();
            }
            format!("{}****{}", head, &tail[tail.len() - 4..])
        }
        _ => text.to_string(),
    }
}

/// 记一条通知相关的日志（统一前缀，方便在「日志」页筛）。
pub fn log(message: &str) {
    logging::log("[Notify]", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_url_keeps_host_and_tail() {
        let masked = mask_url("https://open.feishu.cn/open-apis/bot/v2/hook/abcdef123456");
        assert!(masked.starts_with("https://open.feishu.cn/open-apis/bot/v2/hook/"));
        assert!(masked.ends_with("3456"));
        assert!(masked.contains("****"));
        // 原 token 的中段不能留在结果里
        assert!(!masked.contains("abcdef"));
    }

    #[test]
    fn mask_url_short_tail_untouched() {
        assert_eq!(mask_url("http://x/y"), "http://x/y");
        assert_eq!(mask_url(""), "");
    }
}
