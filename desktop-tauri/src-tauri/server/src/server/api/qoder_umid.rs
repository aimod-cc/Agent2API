//! Qoder UMID 组件（设备风控身份）的状态与安装：
//!
//! - `GET  /api/qoder-umid` → `risk::status`（available / source / installSupported / installing）
//! - `POST /api/qoder-umid/install` → 下载安装（仅 Linux；Windows / macOS 走
//!   本机客户端 / qodercli，无需下载）
//!
//! 为什么是独立一条而不是塞进账号路由：组件是**机器级**的（一次安装服务全部
//! 国际版账号），与任何一条账号记录无关；塞进 `/api/accounts/{id}/…` 会让
//! 「组件在不在」看起来像账号的状态。
//!
//! 消费方：签到中心在 Qoder 国际版分组的说明里展示组件状态，Linux 且未安装
//! 时给「一键安装」按钮（安装耗时以分钟计 —— 要下载 ~10MB 的 npm 包并解出
//! 二进制，前端按下后轮询 GET 看 installing 即可）。

use axum::extract::State;
use axum::response::Response;

use crate::server::core::providers::qoder::risk;
use crate::server::errors::management_error;
use crate::server::http::ok_json;
use crate::server::ServerState;

/// GET /api/qoder-umid
pub async fn get_status(_state: State<ServerState>) -> Response {
    ok_json(risk::status(crate::server::core::providers::qoder::endpoints::Region::Global))
}

/// POST /api/qoder-umid/install
pub async fn install(State(_state): State<ServerState>) -> Response {
    match risk::install_component().await {
        Ok(manifest) => {
            crate::server::logging::log("[Qoder]", "UMID 组件安装完成（来自签到中心）");
            ok_json(manifest)
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}
