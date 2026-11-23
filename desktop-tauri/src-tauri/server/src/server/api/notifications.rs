//! 系统通知设置与测试发送 API（`/api/notifications`）。

use axum::body::Bytes;
use axum::response::Response;
use serde_json::json;

use crate::server::core::notifications::{self, NotificationSettings};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;

pub async fn get_notifications() -> Response {
    ok_json(NotificationSettings::current().management_json())
}

pub async fn put_notifications(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let settings = match notifications::save(object) {
        Ok(settings) => settings,
        Err((status, message)) => return errors::management_error(status, message),
    };
    logging::log(
        "[Notify]",
        &format!(
            "通知设置已更新：总开关{}（登录成功={}，登录失败={}，自动签到失败={}）",
            if settings.enabled { "开启" } else { "关闭" },
            if settings.events.login_success { "开" } else { "关" },
            if settings.events.login_failure { "开" } else { "关" },
            if settings.events.checkin_failure { "开" } else { "关" },
        ),
    );
    ok_json(settings.management_json())
}

pub async fn test_notification(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    match notifications::channels::send_test(object).await {
        Ok(channel) => {
            logging::log("[Notify]", &format!("✅ 测试通知已发送（{channel}）"));
            ok_json(json!({
                "sent": true,
                "channel": channel,
            }))
        }
        Err((status, message)) => {
            logging::log("[Notify]", &format!("❌ 测试通知发送失败：{message}"));
            errors::management_error(status, message)
        }
    }
}
