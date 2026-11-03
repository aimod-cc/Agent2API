//! 插件控制面错误的 HTTP 状态与业务码兼容解码。

use agent2api_provider_protocol as proto;
use serde::Deserialize;

use crate::server::errors::GatewayError;

#[path = "strict_errors.rs"]
pub(super) mod strict;

const HTTP_CODE_PREFIX: &str = "provider_http:";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorMetadata {
    status: i32,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    upstream_code: Option<i64>,
}

pub(super) fn control_error(error: &proto::WireError) -> Option<GatewayError> {
    let payload = error.code.as_deref()?.strip_prefix(HTTP_CODE_PREFIX)?;
    let metadata: ErrorMetadata = serde_json::from_str(payload).ok()?;
    if !(400..600).contains(&metadata.status) {
        return None;
    }
    let mut failure = GatewayError::with_status(metadata.status, error.message.clone())
        .with_optional_code(metadata.upstream_code);
    failure.code = metadata.code;
    Some(failure)
}

pub(super) fn operation_failure(method: &str, detail: &str) -> String {
    use proto::methods::gateway as methods;

    let operation = match method {
        methods::USAGE_QUERY => "余额查询",
        methods::TOKEN_ENSURE => "获取可用凭证",
        methods::TOKEN_REFRESH => "凭证续期",
        methods::TOKEN_EXPIRING => "检查凭证有效期",
        methods::MODELS_LIST => "获取模型列表",
        methods::MODELS_REFRESH => "刷新模型列表",
        methods::CHECKIN_CLAIM => "签到",
        methods::CLAIM => "福利领取",
        methods::CLAIM_STATUS => "查询福利状态",
        methods::ACCOUNT_NORMALIZE => "账号导入",
        methods::ACCOUNT_VIEW => "获取账号信息",
        methods::ACCOUNT_IDENTITY => "识别账号",
        methods::LOGIN_START => "发起登录",
        methods::LOGIN_POLL => "查询登录状态",
        methods::LOGIN_CALLBACK => "登录授权",
        methods::LOGIN_CANCEL => "取消登录",
        methods::CHAT => "构造对话请求",
        crate::server::core::providers::images::GENERATE_METHOD => "图片生成",
        crate::server::core::providers::images::EDIT_METHOD => "图片编辑",
        _ => "提供商操作",
    };
    format!("{operation}失败：{detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 插件把 413 装进 `provider_http:` 信封时，宿主必须还原出 413。
    ///
    /// issue #6 的修法两截：插件侧带状态码过线、宿主侧按 413 停在同家不再换号。
    /// 只做后者时**永远不会触发** —— 旧插件走 `into_message`，宿主看到的是 502，
    /// 于是队列里每个账号照旧白撞一遍。这条用例钉住「信封 → 状态码」这一截。
    #[test]
    fn a_body_limit_envelope_keeps_its_413_on_this_side() {
        let envelope = proto::WireError::with_code(
            "CatPaw 入口按请求体字节数限流：本次约 2000000 字节，超过约 1 MiB 上限",
            r#"provider_http:{"status":413}"#,
        );
        let failure = control_error(&envelope).expect("provider_http 信封必须被解出来");
        assert_eq!(413, failure.status_code, "状态码丢了，宿主的体积判据就失效");
        assert!(
            !failure.is_quota_limit(),
            "413 是体积问题，不能落进限额记账（那会给账号挂冷却）"
        );
        // 反面对照：没有信封的错误仍是「未分类」，不能凭空造出一个 413。
        assert!(control_error(&proto::WireError::with_code("上游失败", "internal")).is_none());
    }
}
