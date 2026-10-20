//! 机器人校验开关：`GET/PUT /api/captcha`。
//!
//! ── 这个开关做什么 ──────────────────────────────────────────
//! 控制面板登录 / 注册端点的 ALTCHA proof-of-work 校验（见 `server::altcha`）：
//! 开启时，登录页必须先领题、浏览器后台算完、随请求提交 payload 才能进门；
//! 关闭时两个端点不再校验（challenge 端点也回 400）。**默认开启** ——
//! 登录与注册是公开的认证边界，脚本暴破 / 抢注的门槛宁可多一道不可少一道。
//!
//! ── 为什么分两端 ────────────────────────────────────────────
//! 桌面壳与 headless 共用同一份后端，但暴露面完全不同：桌面端登录页只服务
//! 本机（以及开了局域网访问后的同网段设备），headless 的登录页摆在真实网络上。
//! 前者嫌算力开销时可以单独关掉，后者关掉就等于放开暴破门槛。因此两个开关
//! 彼此独立，由 `ServerState.local_panel` 决定登录端点读哪一个。
//!
//! ── 契约（GET 与 PUT 的响应同形）─────────────────────────────
//! ```text
//! { captchaDesktop: bool, captchaWeb: bool, localPanel: bool }
//! ```
//! `localPanel` 是**本进程**的形态（桌面壳 true / headless false），设置页据此
//! 把「你正在用的这一端」标出来；两个布尔值都是可写项，PUT 允许只给其中一个
//! （部分更新，与 `/api/retry` 同一约定）。旧键 `captchaEnabled` 仍被接受
//! （同时写两端），兼容还没升级的客户端 / 脚本。
//!
//! 旧版的单开关 `captchaEnabled` 只在读取时作为两端的回落默认值使用，见
//! `config::captcha_switch_from` —— 升级时用户原本关掉的开关不会自己打开。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

fn captcha_json(state: &ServerState) -> Value {
    let (desktop, web) = config::current().captcha_settings();
    json!({
        "captchaDesktop": desktop,
        "captchaWeb": web,
        "localPanel": state.local_panel,
    })
}

/// GET /api/captcha —— 两端的开关状态 + 本进程形态
pub async fn get_captcha(State(state): State<ServerState>) -> Response {
    ok_json(captcha_json(&state))
}

/// PUT /api/captcha —— body `{captchaDesktop?: bool, captchaWeb?: bool}`（或旧键
/// `captchaEnabled` 同时写两端）。
pub async fn put_captcha(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    // 逐项解析：给 null 与「没给」同义（部分更新），给了非布尔值则报错 ——
    // 静默忽略会让界面以为写成功了
    fn field(
        object: &serde_json::Map<String, Value>,
        key: &str,
    ) -> Result<Option<bool>, Response> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(errors::management_error(
                400,
                format!("{key} 必须是布尔值"),
            )),
        }
    }
    let desktop = match field(object, config::KEY_CAPTCHA_DESKTOP) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let web = match field(object, config::KEY_CAPTCHA_WEB) {
        Ok(value) => value,
        Err(response) => return response,
    };
    // 旧客户端的 `captchaEnabled`：两端一起写（旧界面本来就只有一张开关）
    let legacy = match field(object, config::KEY_CAPTCHA_ENABLED) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let desktop = desktop.or(legacy);
    let web = web.or(legacy);
    if desktop.is_none() && web.is_none() {
        return errors::management_error(
            400,
            "缺少 captchaDesktop / captchaWeb 字段（旧客户端可用 captchaEnabled 同时设置两端）",
        );
    }
    if !config::set_captcha_patch(desktop, web) {
        return errors::management_error(500, "写入配置失败，请重试");
    }
    let (desktop_now, web_now) = config::current().captcha_settings();
    logging::log(
        "[Security]",
        &format!(
            "机器人校验：桌面端{}、网页端{}",
            if desktop_now { "开启" } else { "关闭" },
            if web_now { "开启" } else { "关闭" },
        ),
    );
    ok_json(captcha_json(&state))
}
