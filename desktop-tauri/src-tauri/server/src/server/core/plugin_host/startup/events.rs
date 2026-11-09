use serde_json::{json, Value};

use super::{Entry, Phase};
use crate::server::logging;

pub(super) struct Event {
    level: &'static str,
    message: String,
    data: Value,
}

impl Event {
    pub(super) fn emit(self) {
        logging::console_line("[Plugin]", &self.message);
        logging::log_event(self.level, "plugin", &format!("[Plugin] {}", self.message), Some(self.data));
    }
}

pub(super) fn name(entry: &Entry) -> String {
    if entry.name.trim().is_empty() { entry.package_name.clone() } else { entry.name.clone() }
}

pub(super) fn entry_event(entry: &Entry, boot_id: &str) -> Event {
    let status = &entry.startup;
    let summary = match status.phase {
        Phase::Ready => "插件已就绪",
        Phase::Timeout => "插件启动超时",
        Phase::Blocked => "插件暂不可用，等待运行环境恢复",
        _ => "插件启动失败",
    };
    let duration = status.updated_at.saturating_sub(status.started_at.unwrap_or(status.updated_at)).max(0);
    let name = name(entry);
    Event {
        level: if status.phase.failed() { "error" } else if status.phase == Phase::Blocked { "warn" } else { "info" },
        message: format!("{name}：{summary}"),
        data: json!({
            "source": "plugin", "action": "plugin.startup", "summary": summary,
            "pluginId": entry.id, "pluginName": name, "packageName": entry.package_name,
            "phase": status.phase, "durationMs": duration, "steps": status.steps,
            "detail": status.error, "routine": status.phase == Phase::Ready,
            "bootId": boot_id, "attempt": status.attempt,
        }),
    }
}

pub(super) fn summary_event(entries: &[&Entry], boot_id: &str, attempt: u64) -> Event {
    let ready = entries.iter().filter(|entry| entry.startup.phase == Phase::Ready).count();
    let failed = entries.iter().filter(|entry| entry.startup.phase.failed()).count();
    let blocked = entries.len().saturating_sub(ready + failed);
    let started = entries.iter().filter_map(|entry| entry.startup.started_at).min();
    let finished = entries.iter().map(|entry| entry.startup.updated_at).max().unwrap_or(0);
    let duration = started.map(|started| finished.saturating_sub(started).max(0));
    Event {
        level: if failed > 0 { "error" } else if blocked > 0 { "warn" } else { "info" },
        message: if failed == 0 && blocked == 0 {
            format!("插件启动完成：{ready} 个已就绪")
        } else {
            format!("插件启动结束：{ready} 个已就绪，{failed} 个失败，{blocked} 个等待恢复")
        },
        data: json!({
            "source": "plugin", "action": "plugin.startup.summary",
            "summary": if failed == 0 && blocked == 0 { "插件启动完成" } else { "插件启动存在异常" },
            "readyCount": ready, "failedCount": failed, "blockedCount": blocked,
            "durationMs": duration, "bootId": boot_id, "attempt": attempt,
            "plugins": entries.iter().map(|entry| json!({
                "pluginId": entry.id, "pluginName": name(entry), "phase": entry.startup.phase,
            })).collect::<Vec<_>>(),
        }),
    }
}
