//! 智谱清言适配器：**无状态**（一次 HTTP 请求 = 一次生成），
//! 但响应帧混着增量与全文快照两种语义 —— 走翻译层按帧语义差分下发，
//! 不走会话式转发。
//!
//! ── 为什么不上有状态路线（`is_stateful` / `forward_conversation`）──
//! 本家的上游确实是「一次请求一次生成」，只是**响应帧语义**特殊：
//! `init` 帧给的是本拍新增的一小段，带 finish 的帧给的是整段全文快照，
//! 而 OpenAI 要的是统一口径的 delta。
//! 这属于「上游响应说哪套协议」，本仓的既定解法是 [`UpstreamResponse`] 加一个
//! 变体、编排层按它选翻译层（ZCode / Command Code / Antigravity 三条先例）——
//! 于是账号轮换、限额冷却、退避重试、usage 记账与取消处理全部留在编排层。
//!
//! 请求侧只多一件事：**把多轮 messages 拍平为单条文本**（`plan.rs` 有实测论证：
//! 上游只认最后一条 user 消息，原生 messages 数组里的历史与 system 全被丢弃）。
//! 拍平发生在 `build_chat_request` 里，属于请求构造，不影响无状态形态。
//!
//! ── 两条 HTTP 200 的失败通道（本家特有，见 `is_error_response`）──
//!   1. **业务码失败**：正常对话返回 `text/event-stream`，但额度不足等失败
//!      返回 `HTTP 200 + application/json`（实测 `-1001 剩余积分不足`）。
//!      靠 [`Self::is_error_response`] 拦下，交给既有错误分类与账号轮换；
//!   2. **流内错误码**：SSE 里也可能带 `error_code`（翻译状态机折成错误帧）。
//!
//! ── 能力位（README 能力表逐列对照，见模块头末段）──────────────
//!   - LLM 请求 ✓（`supports_chat`）
//!   - Token 自动续期 ✓（`supports_refresh = true`，`user/refresh` 实测可用；
//!     续期后 write-back 会覆盖轮换过的 refresh_token）
//!   - 模型列表 ✓（`operation/detail?tag=available_models` + 静态兜底）
//!   - 余额查询 ✓（`supports_usage`，积分出自 `payment-api` + 会员积分接口）
//!   - 签到 ✓（本家的「每日登录积分」= `member/daily_login_score`）
//!   - 领取类 ✓（会员积分活动领取 `score_activity_draw` + 季节性活动）
//!   签到与领取**只对登录账号有效**（访客实测 401 `user needs login`），
//!   由 `balance.rs` / `checkin.rs` 各自判形态并给可读提示。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass, UpstreamResponse,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{balance, credentials, models, plan};

/// 智谱清言适配器（无状态单例）
pub struct ChatglmAdapter;

/// 静态单例（`adapter_for(ProviderKind::Chatglm)` 返回这一个）
pub static CHATGLM_ADAPTER: ChatglmAdapter = ChatglmAdapter;

impl ProviderAdapter for ChatglmAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Chatglm
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存 + 内置兜底）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST /chatglm/mainchat-api/engine/stream`。
    ///
    /// `account` 是会话形态（`store.get_session_by_id` 的返回值），
    /// 令牌从 `auth.accessToken` 取（与各家同一约定：公开形态里只有尾号）。
    /// 设备号从 `auth.deviceId` 取 —— 它是**身份的一部分**（见 `credentials.rs`），
    /// 缺失时 `credentials::from_record` 会现生成一个（账号层负责写回）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let credentials = credentials::from_record(Some(account))?;
        let token = credentials.bearer();
        if token.trim().is_empty() {
            return Err(GatewayError::with_status(
                401,
                "智谱清言账号缺少令牌，无法转发（请重新添加该账号）",
            ));
        }
        // 模型名由编排层按映射改写后塞进 body.model（见 `upstream::payload`）；
        // 这里读出它，缺省走上游默认
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(models::default_model);
        let built = plan::build_chat_plan(body, &model);
        let headers = super::http::request_headers(
            &token,
            &credentials.device_id,
            &[("Accept", "text/event-stream")],
        );
        Ok(ChatRequestPlan {
            url: built.url,
            headers,
            body: built.body,
            response: UpstreamResponse::ChatglmSnapshot,
        })
    }

    /// 响应头是否表明上游错误。
    ///
    /// ── 本家为什么要覆写（与 ZCode 同一处境）──────────────────────
    /// 正常对话返回 `text/event-stream`；但**额度不足 / 未登录等业务失败返回
    /// `HTTP 200 + application/json`**（实测 `-1001 剩余积分不足`、
    /// `40103 user needs login`）。不覆写的话这些响应会被当成成功流交给客户端，
    /// 结果是一条「200 但内容全无」的空回答 —— 既看不到真实原因，
    /// 也不会触发账号轮换。
    ///
    /// 判定口径与 ZCode 逐字相同（MIME 参数剥离 + 大小写不敏感），
    /// 两处刻意保持一致：同一类判据在两家各写一套，迟早分叉。
    fn is_error_response(&self, status: u16, headers: &HeaderMap) -> bool {
        if !(200..300).contains(&status) {
            return true;
        }
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    }

    /// 上游错误分类。
    ///
    /// 取值链与各家一致：先取上游文案（`message` / `msg` / `error.message`），
    /// 再按状态码或**业务码**归四档：
    ///
    ///   - `401` / `403`、业务码 `40003`（缺 Authorization）/ `40103`
    ///     （`user needs login`）→ `TokenExpired`（编排层刷新后同账号重试一次；
    ///     访客凭证刷新走 `guest/access` 重新换发，见 `refresh_access_token`）；
    ///   - `429`、业务码 `-1001`（**剩余积分不足**）、`40014`/`40013`
    ///     （签名/时间戳异常，通常是本机时钟偏了）→ 前者 `QuotaLimited`
    ///      换账号；签名类归 `Fatal`（换账号也救不了，是请求本身的问题，
    ///      如实透出让用户看到原因）；
    ///   - `400` + 审核文案 → 走共用的 `content_block` 判定（不罚账号）；
    ///   - 其余 → `Fatal` 原样透传。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .or_else(|| error_body.get("msg"))
            .or_else(|| error_body.get("error"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        // 业务码可能是顶层 `status`（本家的信封字段）或 `code`
        let code = error_body
            .get("status")
            .and_then(Value::as_i64)
            .or_else(|| error_body.get("code").and_then(Value::as_i64))
            .or_else(|| error_body.get("error_code").and_then(Value::as_i64));
        let message = format!("上游返回 {status}: {raw}");

        match code {
            // 访客/登录态失效：交给编排层刷新一次（访客会重新换发凭证）
            Some(40003) | Some(40103) => {
                return UpstreamErrorClass::TokenExpired { message }
            }
            // 额度不足：本家最常见的失败（访客走 AgentMore 通道时必现）。
            // 归限额而不是致命 —— 换一个有额度的账号比直接报错有用得多，
            // 且下游按 429 退避也符合「这会儿没量了」的语义
            Some(-1001) => {
                return UpstreamErrorClass::QuotaLimited {
                    reset_at: None,
                    message: format!("{message}（该账号积分不足，请换账号或稍后再试）"),
                    upstream_code: Some(-1001),
                    status: 429,
                }
            }
            // 签名 / 时间戳异常：换账号也救不了（是请求本身的问题），
            // 如实透出并给一句可执行提示（最常见的成因是本机时钟不准）
            Some(40013) | Some(40014) | Some(40001) | Some(40004) => {
                return UpstreamErrorClass::Fatal {
                    status: 400,
                    message: format!(
                        "{message}（请求签名/设备头未通过上游校验；\
                         最常见原因是本机系统时间不准，校准后重试）"
                    ),
                    upstream_code: code,
                }
            }
            _ => {}
        }

        match status {
            401 | 403 => UpstreamErrorClass::TokenExpired { message },
            429 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: code,
                status: 429,
            },
            // 400 交给共用的内容拦截判定（本家同样有内容策略拦截）
            400 | 422 => content_block::classify_or_fatal(status, error_body, message, code),
            // 其余（含 5xx）→ 502：服务端故障，退避重试由编排层按 502 处理
            _ => UpstreamErrorClass::Fatal {
                status: if status >= 500 { 502 } else { status },
                message,
                upstream_code: code,
            },
        }
    }

    /// 取可用令牌。**访客凭证失效时自动重新换发** —— 这是本家能「零配置长期
    /// 可用」的关键：访客令牌的有效期只有 7 天（JWT `exp` 实测 86400*7 秒
    /// 量级），过期后 `user/refresh` 也不一定救得回来，而重新 `guest/access`
    /// 恒可用（换个新访客身份）。
    ///
    /// 因此这里的策略是：**有 refresh_token 就先走续期；续期失败且是访客身份，
    /// 就用同一个设备号重新换发访客凭证并写回账号**。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let record = store.chatglm_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("智谱清言账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            Ok(credentials.bearer())
        })
    }

    /// 有续期手段（`POST /chatglm/user-api/user/refresh`，实测可用）
    fn supports_refresh(&self) -> bool {
        true
    }

    /// 401 后的**强制**刷新。
    ///
    /// ── 为什么必须覆写（不能吃默认实现）────────────────────────
    /// 默认实现转调 `ensure_access_token`，而本家的 `ensure_access_token`
    /// 只读记录、不刷新（令牌有效性由上游说了算）。吃到默认实现的话，
    /// 「401 → 刷新一次 → 同账号重试」这一步会拿着**同一个**令牌再发一次，
    /// 必然再得一个 401，然后错误地被判成「这个账号的凭证废了」。
    ///
    /// ── 访客与登录账号走不同的恢复路径（本家特有）──────────────
    ///   - 登录账号：`user/refresh`（refresh_token 换新令牌，上游会轮换它，
    ///     因此必须回写账号）；
    ///   - 访客：同一个设备号重新 `guest/access` 换发一对新令牌 ——
    ///     访客令牌有 7 天有效期，过期后 `user/refresh` 未必救得回来，
    ///     而重新换发**恒可用**（换个新访客身份，免费额度照旧）。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let record = store.chatglm_account_record(account_id);
            let credentials = credentials::from_record(record.as_ref())?;
            // 访客（或没有 refresh_token 的记录）：重新换发访客凭证
            if credentials.is_guest || credentials.refresh_token.trim().is_empty() {
                let (access, refresh, uid) = super::http::guest_access(&credentials.device_id, None)
                    .await
                    .map_err(|error| {
                        GatewayError::with_status(
                            error.status_code,
                            format!("智谱清言访客凭证重新换发失败：{}", error.message),
                        )
                    })?;
                // 写回账号（设备号保持不变 —— 它是身份的一部分）
                if !account_id.is_empty() {
                    let _ = store.update_chatglm_tokens(account_id, &access, &refresh);
                    let _ = uid;
                }
                return Ok(access);
            }
            // 登录账号：refresh_token 续期（上游可能轮换 refresh_token，必须回写）
            let (access, rotated, _uid) = match super::http::refresh_token(
                &credentials.refresh_token,
                &credentials.device_id,
                None,
            )
            .await
            {
                Ok(triple) => triple,
                Err(error) => {
                    // 续期失败且是访客之外的账号：如实报（让用户重新登录）
                    return Err(GatewayError::with_status(
                        error.status_code,
                        format!(
                            "智谱清言凭证续期失败：{}（请在账号页重新添加该账号）",
                            error.message
                        ),
                    ));
                }
            };
            if !account_id.is_empty() {
                let _ = store.update_chatglm_tokens(account_id, &access, &rotated);
            }
            Ok(access)
        })
    }

    /// 凭证是否临期（维护任务据此决定要不要提前续期）。
    ///
    /// 本家的令牌有效期写在 JWT 的 `exp` 声明里 —— 直接读它，不靠猜。
    /// 判不出来（记录缺失 / 令牌形态异常）一律 false：宁可漏刷也不要凭猜测打上游。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        let Some(record) = store.chatglm_account_record(account_id) else {
            return false;
        };
        let Ok(credentials) = credentials::from_record(Some(&record)) else {
            return false;
        };
        credentials::token_expiring_soon(&credentials.bearer())
    }

    /// 有远程目录（`operation/detail?tag=available_models`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一个可用账号）的令牌。
    ///
    /// 「一个账号都没有」不是失败（那是「用户还没添加账号」的正常状态），
    /// 返回 `unchanged()` —— 与 Loomy / MonkeyCode / Command Code 同一处置。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.chatglm_account_record(account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "智谱清言模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials.bearer(), &credentials.device_id, force).await
        })
    }

    /// 有余额 / 积分查询（见 `balance.rs`）
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查询积分余额（`balance.rs`）。
    ///
    /// ── 三处读数合并成一套形状 ─────────────────────────────────
    ///   1. 会员积分活动的 `current_score`（活动期间的积分余额）；
    ///   2. `payment-api` 的会员档位（vip / svip 与到期时间）；
    ///   3. 每日登录积分的到账记录（只用于展示「签到可得」）。
    /// 访客账号这些接口一律 401 —— 如实报「访客账号没有积分体系」，
    /// 用中性标记而不是红色失败（见 `balance.rs`）。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { balance::query(store, account_id).await })
    }
}
