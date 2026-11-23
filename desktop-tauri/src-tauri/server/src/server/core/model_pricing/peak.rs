//! 分时（峰谷）计费时段判断：DeepSeek / Ollama 峰谷时段与提供商分时倍率窗口。

use chrono::{Datelike, FixedOffset, Local, TimeZone, Timelike, Weekday};

use super::types::{parse_clock_minutes, RateWindow};

pub const PEAK_FACTOR: f64 = 2.0;

/// 2026 年国务院法定节假日日期表（北京时间 `MM-DD`），用于 DeepSeek 工作日峰谷排除。
const CN_HOLIDAYS_2026: &[&str] = &[
    "01-01", "01-02", "01-03",
    "02-15", "02-16", "02-17", "02-18", "02-19", "02-20", "02-21", "02-22", "02-23",
    "04-04", "04-05", "04-06",
    "05-01", "05-02", "05-03", "05-04", "05-05",
    "06-19", "06-20", "06-21",
    "09-25", "09-26", "09-27",
    "10-01", "10-02", "10-03", "10-04", "10-05", "10-06", "10-07",
];

fn is_cn_holiday(utc_ms: i64) -> bool {
    let Some(offset) = FixedOffset::east_opt(8 * 3600) else {
        return false;
    };
    let chrono::LocalResult::Single(cn) = offset.timestamp_millis_opt(utc_ms) else {
        return false;
    };
    if cn.year() != 2026 {
        return false;
    }
    let md = format!("{:02}-{:02}", cn.month(), cn.day());
    CN_HOLIDAYS_2026.contains(&md.as_str())
}

/// 给定模型和提供商是否受官网峰谷时段倍增影响。
pub fn peak_kind_for(provider_id: &str) -> &'static str {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "deepseek",
        "ollama" | "ollama-cloud" => "ollama",
        _ => "",
    }
}

/// 判断给定 UTC 毫秒时间戳是否落入指定峰谷档的高峰期。
pub fn is_peak_at(peak: &str, ts_ms: i64) -> bool {
    if peak.is_empty() || ts_ms <= 0 {
        return false;
    }
    let chrono::LocalResult::Single(utc) = chrono::Utc.timestamp_millis_opt(ts_ms) else {
        return false;
    };
    if matches!(utc.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let hour = utc.hour();
    match peak {
        "ollama" => (12..18).contains(&hour),
        "weekdays" => (1..4).contains(&hour) || (6..10).contains(&hour),
        "deepseek" => ((1..4).contains(&hour) || (6..10).contains(&hour)) && !is_cn_holiday(ts_ms),
        _ => false,
    }
}

/// 命中提供商分时倍率时段时返回倍率；第一条命中的时段胜出，未命中返回 `None`。
///
/// 跨午夜时段（如 `22:00` - `02:00`）按开始时刻所属星期计算。
pub fn rate_window_at(windows: &[RateWindow], ts_ms: i64) -> Option<f64> {
    if windows.is_empty() || ts_ms <= 0 {
        return None;
    }
    let chrono::LocalResult::Single(local) = Local.timestamp_millis_opt(ts_ms) else {
        return None;
    };
    let minute_of_day = (local.hour() * 60 + local.minute()) as i32;
    let day = local.weekday().num_days_from_sunday() as u8;
    for window in windows {
        let Some(start) = parse_clock_minutes(&window.start) else { continue };
        let Some(end) = parse_clock_minutes(&window.end) else { continue };
        if start == end {
            continue;
        }
        let (inside, effective_day) = if start < end {
            (minute_of_day >= start && minute_of_day < end, day)
        } else {
            let in_window = minute_of_day >= start || minute_of_day < end;
            let eff_day = if minute_of_day < end { (day + 6) % 7 } else { day };
            (in_window, eff_day)
        };
        if !inside {
            continue;
        }
        if !window.days.is_empty() && !window.days.contains(&effective_day) {
            continue;
        }
        if window.rate > 0.0 {
            return Some(window.rate);
        }
    }
    None
}
