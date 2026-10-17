//! LobsterAI（网易有道「有道龙虾」）适配器：账号管理（浏览器登录）+ **无状态**
//! 推理转发 + 目录 / 余额 / 每日签到。
//!
//! ── 上游长什么样（细节见各子模块的模块头）──────────────────────
//! ```text
//!   登录门户   https://lobsterai.youdao.com        浏览器授权页（手机号 / 微信）
//!   API        https://lobsterai-server.youdao.com  换码 / 续期 / 目录 / 对话 / 积分 / 活动
//! ```
//! 对话端点是 **OpenAI 兼容的原生协议**且**只收 SSE**（`stream:false` 回
//! HTTP 500，`endpoints.rs` 的模块头有实测）：本网关的转发入口恒以 `stream:true`
//! 请求上游，所以这条要求天然满足。本家**无状态**（一次 HTTP 请求 = 一次回答），
//! `is_stateful` / `supports_chat` 走默认值，转发链只需 `build_chat_request`。
//!
//! ── 与别家不同的五处取舍（都写在各自模块头，这里给索引）─────────
//!   1. **登录是「回调带回一次性 code」那一套**（`supports_web_login = true`）：
//!      授权在门户完成、302 回本机 loopback，网关用 code 换令牌（`oauth.rs`）——
//!      不像 AtomCode 那样轮询判完成。所以 `build_login_url` /
//!      `exchange_login_code` 三个钩子是**真的要实现**的。
//!   2. **续期要一整组身份载荷件**（`uuid` + 两个 keyfrom），且
//!      `latestKeyfrom` **永不更新** —— 只存两个 token 的账号续不了期
//!      （`credentials.rs` 的模块头有这条约束的一手来源）。
//!   3. **额度是积分**，读数在 `/api/user/profile-summary`（**不是**
//!      `/api/user/quota`，那个只报 300 免费额度）—— 见 `usage.rs`。
//!   4. **签到是三步协议且幂等靠客户端**（活动位 → 上下文两道预检 →
//!      check_in + uuid4 幂等键）—— 见 `checkin.rs`。
//!   5. **错误分类里「余额耗尽」是最主要的失败模式**，而且它可能以
//!      `HTTP 200` + 流内错误帧下发 —— 见 `errors.rs`（含一处**已知缺口**）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

pub mod chat;
pub mod checkin;
pub mod credentials;
pub mod endpoints;
pub mod errors;
pub mod models;
pub mod oauth;
pub mod usage;
pub mod version;

use std::sync::OnceLock;

use axum::http::HeaderMap;
use serde_json::{Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{ProviderKind, refresh_flight};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 适配器实例（`adapter_for(ProviderKind::LobsterAi)` 返回这一个）。
///
/// **无状态**：它不持有任何数据 —— 凭证在账号存储、目录在 `models.rs` 的进程级
/// 句柄、登录轮次在 `oauth.rs` 的 pending 表（都可重建）。
pub struct LobsterAiAdapter;

/// 静态单例
pub static LOBSTERAI_ADAPTER: LobsterAiAdapter = LobsterAiAdapter;

impl ProviderAdapter for LobsterAiAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LobsterAi
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存 + 19 条静态兜底）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST {api}/api/proxy/v1/chat/completions`（OpenAI 协议，body 只改
    /// 四处，见 `chat.rs` 的模块头）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        chat::build_chat_request(account, body, client_headers)
    }

    /// 上游错误分类（判定顺序本身是契约，见 `errors::classify_gateway_error`）：
    ///   1. 402 / 正文含余额耗尽关键词 → `QuotaLimited`（换号 + 冷却）；
    ///   2. 40100 / 40101 → `TokenExpired`（只能重新登录）；
    ///   3. 429 → `QuotaLimited`；404 / 5xx → `Fatal`；
    ///   4. 其余 4xx → 共用内容拦截分类。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        errors::classify_gateway_error(status, error_body)
    }

    /// 有远程目录（`GET /api/models/available`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一条启用的账号）的令牌。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（脚本 / CI 不该看到一个红色失败，与 AtomCode / Loomy
    /// 同口径）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.lobsterai_account_record(account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "LobsterAI 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials, force).await
        })
    }

    /// 取可用访问令牌。
    ///
    /// ⚠️ **这里不发网络请求**（本方法是同步语义的取值口，且会在转发路径上被调）：
    /// 临期只记一行提示，真正的续期由维护链路（`supports_refresh` +
    /// `refresh_access_token`）与 401 后的强制刷新负责 —— 与 `atomcode/mod.rs`
    /// 同一取舍。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.lobsterai_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("LobsterAI 账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.expiring() {
                logging::verbose(
                    "[LobsterAI]",
                    "账号令牌已过期或临期，将按需续期（续期会轮换 refresh token）",
                );
            }
            Ok(credentials.access_token)
        })
    }

    /// 有续期接口（`POST /api/auth/refresh`）
    fn supports_refresh(&self) -> bool {
        true
    }

    /// 强制续期（401 之后 / 维护链路）：刷新并**把新令牌写回记录**。
    ///
    /// ── 为什么必须写回，以及为什么必须单飞 ───────────────────────
    /// 续期**会轮换** refresh token：上游同时下发新 access 与新 refresh，旧
    /// access 立即失效。丢掉新的 refresh 等于让账号退化成必须重新登录；两个并发
    /// 续期互相作废（后写覆盖前者）—— 因此走本仓统一的 `refresh_flight::Table`
    /// 做单飞，键含**两个令牌的指纹**（令牌一变就是新的一轮，与 Cline 同一口径）。
    ///
    /// ⚠️ **`latestKeyfrom` 绝不覆盖**：续期只回写 access / refresh / expiresAt，
    /// 载荷件三件套保持登录时那一份（`credentials.rs` 的模块头：那是唯一经过
    /// 生产验证的形态，Go 与 TS 都这么做并注明「不是为了省事」）。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.lobsterai_account_record(account_id);
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.refresh_token.is_empty() {
                return Err(GatewayError::with_status(
                    401,
                    "LobsterAI 账号没有 refresh_token，无法续期，请重新登录",
                ));
            }
            let key = format!(
                "{}:{}:{}:{}",
                store.file_string(),
                account_id,
                refresh_flight::fingerprint(&credentials.access_token),
                refresh_flight::fingerprint(&credentials.refresh_token),
            );
            match LOBSTERAI_FLIGHTS
                .get_or_init(refresh_flight::Table::new)
                .join(&key)
            {
                refresh_flight::Join::Waiter(waiter) => {
                    waiter.wait().await.map(|fresh| fresh.access_token)
                }
                refresh_flight::Join::Leader(leader) => {
                    let result = refresh_and_save(store, &credentials).await;
                    leader.finish(result.clone());
                    result.map(|fresh| fresh.access_token)
                }
            }
        })
    }

    /// 临期判定：`expiresAt`（落盘时算好）或 JWT 的 `exp`，到期前 1 小时算临期
    /// （见 `credentials::LobsterCredentials::expiring`）。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        if account_id.is_empty() {
            return false;
        }
        match store.lobsterai_account_record(account_id) {
            Some(record) => credentials::from_record(Some(&record))
                .map(|credentials| credentials.expiring())
                .unwrap_or(false),
            None => false,
        }
    }

    /// 有积分概念（`GET /api/user/profile-summary`，见 `usage.rs`）
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查积分：`GET /api/user/profile-summary` 的归一化。
    /// 401 由调用方走统一的「刷新后重试」链路。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move { usage::query_usage(store, account_id).await })
    }

    /// 本家**没有**「默认模型」概念（不指定模型时按目录 / 上游默认）
    fn supports_default_model(&self) -> bool {
        false
    }

    /// 本家**是**「回调带回一次性 code」那一套（授权页 → 本机 loopback →
    /// 网关换码），因此三个登录钩子都要实现（与 Accio 同形，见模块头）。
    fn supports_web_login(&self) -> bool {
        true
    }

    /// 授权地址：`{portal}/portal#/login?source=electron&redirect_uri=<本机回调>&state=…`
    ///
    /// 回调地址由本进程的监听端口拼出（`ServerState::bootstrap` 时写入，
    /// 见 `oauth::set_loopback_port`）。端口未知时返回 None —— 上层文案会把
    /// 「未能生成授权地址」透给用户，比拼一个必然 404 的地址好。
    fn build_login_url(&self) -> Option<(String, String)> {
        let return_url = oauth::redirect_uri()?;
        Some(oauth::begin_login(&return_url))
    }

    /// 用回调里的一次性 `code` 换令牌并落账号。
    ///
    /// `state` 在这里再校验一次（深度防御：回调路由已比对过，但真正的凭据是
    /// pending 表里那份 —— 取不到就是「不是本进程发起的那一轮」）。
    ///
    /// `uuid` 与两个 keyfrom 由**客户端生成**（TS `lobsterai-oauth.ts:82-84`）：
    /// `uuid` 是这次登录的安装标识、`firstKeyfrom` 是登录时刻（毫秒）、
    /// `latestKeyfrom` 取当前时刻。三者随换码下发、续期时要原样回传，所以一定要
    /// 落进账号（`add_lobsterai_account` 会存下来）。
    fn exchange_login_code<'a>(
        &'a self,
        store: &'a AccountStore,
        code: &'a str,
        state: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let Some(_pending) = oauth::take_pending(state) else {
                return Err(GatewayError::with_status(
                    404,
                    "这次登录已取消或已过期，请重新发起",
                ));
            };
            // 版本号是换码体的必填字段；解析结果同时写进转发的版本头快照
            let client_version = chat::prime_client_version().await;
            let keyfrom = logging::now_ms().max(0).to_string();
            let uuid = uuid_v4();
            let bundle = oauth::exchange(code, &uuid, &keyfrom, &keyfrom, &client_version).await?;

            // 落账号：字段用本仓驼峰名（`add_lobsterai_account` 两种拼法都认，
            // 但驼峰与账号记录里存下来的形态一致，便于人工核对）
            let mut payload = Map::new();
            payload.insert(
                "accessToken".to_string(),
                Value::String(bundle.access_token.clone()),
            );
            if !bundle.refresh_token.trim().is_empty() {
                payload.insert(
                    "refreshToken".to_string(),
                    Value::String(bundle.refresh_token.clone()),
                );
            }
            payload.insert("uuid".to_string(), Value::String(uuid));
            payload.insert("firstKeyfrom".to_string(), Value::String(keyfrom.clone()));
            payload.insert("latestKeyfrom".to_string(), Value::String(keyfrom));
            if !bundle.uid.trim().is_empty() {
                payload.insert("uid".to_string(), Value::String(bundle.uid.clone()));
            }
            if !bundle.user_id.trim().is_empty() {
                payload.insert("userId".to_string(), Value::String(bundle.user_id.clone()));
            }
            if !bundle.nickname.trim().is_empty() {
                payload.insert("name".to_string(), Value::String(bundle.nickname.clone()));
            }
            if bundle.expires_in > 0 {
                payload.insert("expiresIn".to_string(), Value::from(bundle.expires_in));
            }

            let account = store
                .add_lobsterai_account(&Value::Object(payload), None)
                .map_err(|error| GatewayError::with_status(error.status_code, error.message))?;
            let account_id = account
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            logging::log("[Login]", "✅ LobsterAI 网页登录完成，账号已加入列表");
            Ok(account_id)
        })
    }
}

/// 续期单飞表（进程级；键含两个令牌的指纹，令牌一变就是新的一轮）
static LOBSTERAI_FLIGHTS: OnceLock<refresh_flight::Table<credentials::LobsterCredentials>> =
    OnceLock::new();

/// 真的去打一次续期，并把**轮换后的令牌**写回账号记录。
///
/// 抽成自由函数是为了让单飞分支的「Leader 职责」只出现在一处：
/// 取版本 → 发请求 → 落盘 → 返回新凭证。**只回写 access / refresh / expiresAt**，
/// 载荷件三件套不动（见 `refresh_access_token` 的说明）。
async fn refresh_and_save(
    store: &AccountStore,
    credentials: &credentials::LobsterCredentials,
) -> Result<credentials::LobsterCredentials, GatewayError> {
    let client_version = chat::prime_client_version().await;
    let bundle = oauth::refresh(credentials, &client_version).await?;
    // 过期时刻：刚续期成功，以此刻为基准算（`expires_in` 缺失时退回读 JWT 的 exp）
    let expires_at =
        credentials::expires_at_from(logging::now_ms(), bundle.expires_in, &bundle.access_token);
    // 轮换语义：refresh token 非空才算轮换（上游可能省略，那表示沿用旧的那一个）
    let rotated = !bundle.refresh_token.trim().is_empty();
    // 落盘失败**不**把这次续期当成功：新的 access 已经在手上、但记录里还是旧的，
    // 下次仍会走一次续期（而旧的 refresh 可能已被轮换作废）。如实上报。
    let written = store.update_account_tokens(
        &credentials.id,
        Some(bundle.access_token.as_str()),
        rotated.then(|| bundle.refresh_token.as_str()),
        (expires_at > 0).then_some(expires_at as f64),
        None,
    );
    if !written {
        return Err(GatewayError::with_status(
            502,
            "LobsterAI 续期成功但写回账号失败（账号记录不存在？）：请重新登录",
        ));
    }
    logging::log("[LobsterAI]", "令牌已续期并写回账号（refresh token 已轮换）");
    Ok(credentials::LobsterCredentials {
        id: credentials.id.clone(),
        access_token: bundle.access_token.clone(),
        refresh_token: if rotated {
            bundle.refresh_token.clone()
        } else {
            credentials.refresh_token.clone()
        },
        expires_at,
        // 载荷件三件套保持登录时那一份（**不覆盖**）
        uid: credentials.uid.clone(),
        user_id: credentials.user_id.clone(),
        nickname: credentials.nickname.clone(),
        uuid: credentials.uuid.clone(),
        first_keyfrom: credentials.first_keyfrom.clone(),
        latest_keyfrom: credentials.latest_keyfrom.clone(),
    })
}

/// 生成一个 UUID v4 字符串（`8-4-4-4-12` 带横线）。
///
/// 与 `chatglm::sign::uuid_v4` **同形**（随机源是 `getrandom`，本项目已是直接
/// 依赖；随机源失败时退回时间戳派生的形态 —— 仍是 UUID 形态，比发一个形态不对
/// 的值好）。两个消费点：签到体的幂等键（`checkin.rs`）与登录的 `uuid` 载荷件。
/// 本模块自带一份而不是跨 provider 引用，是为了不让两家在签名工具上演进出耦合。
pub(crate) fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let millis = logging::now_ms().max(0) as u128;
        bytes[..16].copy_from_slice(&millis.to_le_bytes()[..16]);
    }
    // 版本位（第 7 字节高 4 位 = 0100）与变体位（第 9 字节高 2 位 = 10）
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let part = |start: usize, end: usize| hex.get(start..end).unwrap_or("").to_string();
    format!(
        "{}-{}-{}-{}-{}",
        part(0, 8),
        part(8, 12),
        part(12, 16),
        part(16, 20),
        part(20, 32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 适配器声明的能力集（这份断言存在的意义：改动能力位时会被迫读一遍模块头
    /// 里「为什么」的那几节）。
    #[test]
    fn adapter_declares_the_expected_capabilities() {
        let adapter = &LOBSTERAI_ADAPTER;
        assert_eq!(adapter.kind(), ProviderKind::LobsterAi);
        assert!(adapter.supports_chat(), "上游就是 OpenAI 协议，必然能转发");
        assert!(!adapter.is_stateful(), "一次 HTTP 请求 = 一次回答");
        assert!(adapter.supports_model_refresh(), "有 /api/models/available 目录接口");
        assert!(adapter.supports_refresh(), "有 /api/auth/refresh 续期接口");
        assert!(adapter.supports_usage(), "有 /api/user/profile-summary 积分读数");
        assert!(!adapter.supports_default_model(), "本家没有默认模型概念");
        // 本家**是**「回调带回一次性 code」那一套（与 AtomCode 相反）。
        assert!(adapter.supports_web_login());
    }

    /// 授权地址要指向门户的 hash 路由，且回调是本机 loopback。
    /// （端口未登记时 `redirect_uri()` 给 None —— 那时 `build_login_url` 也返回
    /// None，本用例只在端口已知时才断言。）
    #[test]
    fn login_url_points_at_the_portal_when_the_port_is_known() {
        if oauth::redirect_uri().is_none() {
            // 端口未知时**必须**返回 None，而不是拼一个必然 404 的地址
            assert!(LOBSTERAI_ADAPTER.build_login_url().is_none());
            return;
        }
        let (url, state) = LOBSTERAI_ADAPTER.build_login_url().expect("端口已知应给出授权地址");
        assert!(url.contains("/portal#/login"), "{url}");
        assert!(url.contains("source=electron"), "{url}");
        assert!(url.contains("redirect_uri="), "{url}");
        assert!(!state.is_empty());
        // 登记过的 state 可被兑一次（登录回调要按它找回那一轮）
        assert!(oauth::take_pending(&state).is_some());
    }

    /// 幂等键 / 登录 uuid 的形态是 UUID v4（版本位 4、变体位 8/9/a/b）。
    #[test]
    fn uuid_v4_is_shaped_correctly() {
        let key = uuid_v4();
        let parts: Vec<&str> = key.split('-').collect();
        assert_eq!(parts.len(), 5, "{key}");
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{key}"
        );
        assert!(parts[2].starts_with('4'), "版本位不是 4：{key}");
        assert!(
            matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "变体位不对：{key}"
        );
    }
}
