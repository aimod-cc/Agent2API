//! 面板登录校验设置；服务端密钥只接受写入，不随响应回传。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;

use crate::server::config::{self, captcha::CaptchaSettings};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

pub async fn get_captcha(State(state): State<ServerState>) -> Response {
    let config = config::current();
    ok_json(CaptchaSettings::from_config(&config).management_json(&config, state.local_panel))
}

pub async fn put_captcha(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    if let Err((status, message)) = config::captcha::save(object) {
        return errors::management_error(status, message);
    }
    let config = config::current();
    let settings = CaptchaSettings::from_config(&config);
    let (desktop, web) = config.captcha_settings();
    logging::log(
        "[Security]",
        &format!(
            "机器人校验：{}，桌面端{}、网页端{}",
            settings.provider,
            if desktop { "开启" } else { "关闭" },
            if web { "开启" } else { "关闭" },
        ),
    );
    ok_json(settings.management_json(&config, state.local_panel))
}
