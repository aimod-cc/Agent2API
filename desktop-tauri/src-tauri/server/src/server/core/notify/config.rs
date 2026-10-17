//! 通知配置的读写（落 config.json 的 `notify` 字段）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一个通知渠道。
///
/// `channel_type` 决定**消息体形状**（飞书 / 企微 / 钉钉各自的约定），
/// 见 `sender::Channel::payload`。填 `generic` 则按纯文本 POST。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotifyChannelConfig {
    /// 用户起的名字（界面显示、日志里用）
    #[serde(default)]
    pub name: String,
    /// 渠道类型：`feishu` / `wecom` / `dingtalk` / `generic`
    #[serde(rename = "type", default)]
    pub channel_type: String,
    /// Webhook 地址（**本身就是凭证**，对外响应里必须遮掉，见 `mod::public_config`）
    #[serde(default)]
    pub url: String,
}

/// 通知配置。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotifyConfig {
    /// 总开关。**默认关**：没配好就发是打扰，而且新装用户根本没有渠道
    #[serde(default)]
    pub enabled: bool,
    /// 订阅的事件名清单（空 = 全部订阅，见 `event::is_subscribed`）
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub channels: Vec<NotifyChannelConfig>,
}

/// 配置在 config.json 里的键名。
pub const CONFIG_KEY: &str = "notify";

impl NotifyConfig {
    /// 从当前配置快照里读出。形状不对（老配置 / 被人手改成别的类型）时
    /// **当作没配**而不是报错 —— 通知是旁路，读不出来就静默不发，
    /// 绝不能让一个坏字段把任务收尾这类主流程带崩。
    pub fn load() -> Self {
        let raw = crate::server::config::current().raw().get(CONFIG_KEY).cloned();
        match raw {
            Some(value) => Self::parse(&value),
            None => Self::default(),
        }
    }

    /// 解析一份 JSON（形状不对时逐字段兜底，不整体丢弃 —— 一个坏渠道不该
    /// 让其余渠道一起失效）。
    pub fn parse(value: &Value) -> Self {
        let object = match value.as_object() {
            Some(map) => map,
            None => return Self::default(),
        };
        let enabled = object.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        let events = object
            .get("events")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let channels = object
            .get("channels")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    // 逐条独立解析：单条坏掉只丢那一条
                    .filter_map(|item| serde_json::from_value::<NotifyChannelConfig>(item.clone()).ok())
                    .filter(|channel| !channel.url.trim().is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self { enabled, events, channels }
    }

    /// 序列化回 JSON（写盘用）
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}
