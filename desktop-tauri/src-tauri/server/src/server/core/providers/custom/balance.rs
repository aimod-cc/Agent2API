//! 自定义提供商的**余额查询**：七种方案的编排（认证、打哪个地址、读成什么）。
//!
//! ── 为什么自定义家也要有余额查询 ─────────────────────────────
//! 内置家各有一条自己的余额链路（`providers/*/balance.rs`），而自定义家
//! （含「预置 API」卡片建的那些）此前一律没有 —— 余额列永远显示「—」。
//! 但用户接的很多上游**自己就有余额接口**（DeepSeek 的 `/user/balance`、
//! NewAPI 的 `/api/user/self`、OpenCode Go 的 `/v1/usage`…），把它们接进来，
//! 自定义上游就和内置家一样能看余额、能自动查询、能被限制器按余额跳过。
//!
//! ── 方案从哪来（不是用户手选的）──────────────────────────────
//! 预置卡片在创建时把方案写进**提供商记录**的 `balanceScheme`
//! （见 `custom_providers` 的说明），读取链路只认这一个事实来源：
//! 方案 = 「这一家是谁」的派生，用户不需要（也不该）在界面上选它。
//! 手工新建的家没有这个键，也就没有余额查询 —— 猜域名只会把别人的接口
//! 当成它的打。
//!
//! ── 七种方案（移植来源 OmniProxy `server/src/utils/providerBalance.ts`；国际站为自研补充）──
//! | 方案 | 打哪里 | 认证 | 主读数 |
//! |---|---|---|---|
//! | `sub2api` | `{base}/v1/usage` | 账号 API Key（Bearer） | 剩余额度（`quota.limit/used`，单位默认 USD） |
//! | `new_api` | `{base}/api/user/self` + `{base}/api/status` | **控制台访问令牌**（Bearer + 可选 `new-api-user` 头） | `data.quota`，按 `/api/status` 的显示口径换算成货币 |
//! | `deepseek` | `{base}/user/balance` | 账号 API Key（Bearer） | `balance_infos[].total_balance`（多币种） |
//! | `opencode` | `{base}/v1/usage` | 账号 API Key（Bearer） | rolling / weekly / monthly 三窗口剩余百分比 |
//! | `sensenova` | `platform.sensenova.cn` 控制面（与 base 无关） | **控制台账号密码**换 JWT（见 `sensenova_auth`） | 通用池 / Flash-Lite 池各 5 小时 / 7 天窗口剩余百分比 + 活动返赠 |
//! | `zhipu` | `bigmodel.cn` 控制台（与 base 无关） | **控制台令牌** | 账户余额（人民币元） |
//! | `zhipu_zai` | `api.z.ai` 控制面（与 base 无关） | 账号 API Key 铸 **SIGN 签名**（见 `zhipu_sign_jwt`） | 账户余额（美元） |
//!
//! 前四个打的是**用户填的 base**；后三个打的是**各自的控制台域**（余额记在
//! 控制台账户上，与推理端点不是同一个站点）—— 这是这一层唯一容易搞错的地方。
//! 智谱两站同一条账户报告接口，但鉴权（控制台令牌 vs API Key 签名）与币种
//! （人民币 vs 美元）都不同，所以是两个方案而不是一个方案的两个参数。
//!
//! ── 凭证存哪（与内置家的区别）────────────────────────────────
//! 内置家的查询凭证多数就是转发凭证（token 一把通用）；这七家里有三家
//! 要**另一份**凭证（NewAPI 的访问令牌、商汤的控制台账号密码、智谱国内站的
//! 控制台令牌），落在账号记录的 `balanceQuery` 键里（见 `custom_accounts`）。
//! sub2api / deepseek / opencode / zhipu_zai 四家用账号自己的 API Key，
//! 不需要额外配置 —— 与用户「DeepSeek 不用额外填东西」的口径一致。
//!
//! ── 出口与超时 ──────────────────────────────────────────────
//! 出网走**账号自己的代理**（`credential.proxy`）：余额接口打的就是这条账号
//! 访问上游的那条线路，用户在那里配的代理对它是同一件事（与内置家「各链路
//! 自成一套」的差异在于：自定义上游的域是用户自己填的，网关无从判断它该不该
//! 走代理，跟随转发线路是最不意外的口径）。超时 20 秒 —— `egress` 的默认
//! read_timeout 是 600 秒（给 SSE 的），不设总超时会让一个挂住的查询把界面
//! 转到天荒地老。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap / expect / panic。

use serde_json::Value;
use url::Url;

use crate::server::core::account_store::custom_accounts::{CustomBalanceQuery, CustomCredential};
use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http::send_raw;
use crate::server::core::custom_providers::{
    self, BALANCE_SCHEME_DEEPSEEK, BALANCE_SCHEME_NEW_API, BALANCE_SCHEME_OPENCODE,
    BALANCE_SCHEME_SENSENOVA, BALANCE_SCHEME_SUB2API, BALANCE_SCHEME_ZHIPU,
    BALANCE_SCHEME_ZHIPU_ZAI,
};
use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::adapter::usage_not_configured;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::balance_parse as parse;
use super::sensenova_auth;

/// 余额请求超时（与内置家的余额实现同一量级：15~20 秒）
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 智谱控制台域（余额记在控制台账户上，与推理端点无关）
const ZHIPU_CONSOLE_BASE: &str = "https://bigmodel.cn";

/// 智谱国际站（z.ai）的账户报告接口。`/api` 这一段是**服务路径**的一部分
/// （biz 网关挂在 `api.z.ai/api` 下），不是推理前缀；地址同样固定，与
/// 提供商 base 无关（余额记在控制台账户上）。
const ZHIPU_ZAI_BALANCE_URL: &str =
    "https://api.z.ai/api/biz/account/query-customer-account-report";

/// 余额查询方案（与提供商记录的 `balanceScheme` 一一对应）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    Sub2api,
    NewApi,
    DeepSeek,
    Opencode,
    Sensenova,
    Zhipu,
    ZhipuZai,
}

impl Scheme {
    /// 方案 id → 枚举（未知 id 给 None；id 的合法性由 `custom_providers` 保证）
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            BALANCE_SCHEME_SUB2API => Some(Scheme::Sub2api),
            BALANCE_SCHEME_NEW_API => Some(Scheme::NewApi),
            BALANCE_SCHEME_DEEPSEEK => Some(Scheme::DeepSeek),
            BALANCE_SCHEME_OPENCODE => Some(Scheme::Opencode),
            BALANCE_SCHEME_SENSENOVA => Some(Scheme::Sensenova),
            BALANCE_SCHEME_ZHIPU => Some(Scheme::Zhipu),
            BALANCE_SCHEME_ZHIPU_ZAI => Some(Scheme::ZhipuZai),
            _ => None,
        }
    }

    /// 展示名（错误与日志文案里说清是哪一条链路）
    pub fn label(self) -> &'static str {
        match self {
            Scheme::Sub2api => "Sub2API",
            Scheme::NewApi => "NewAPI",
            Scheme::DeepSeek => "DeepSeek",
            Scheme::Opencode => "OpenCode Go",
            Scheme::Sensenova => "商汤日日新",
            Scheme::Zhipu => "智谱",
            Scheme::ZhipuZai => "智谱（z.ai 国际）",
        }
    }

    /// 缺凭证时告诉用户「去哪儿补、补什么」（拼进 `usage_not_configured`）
    fn credential_hint(self) -> &'static str {
        match self {
            Scheme::Sub2api | Scheme::DeepSeek | Scheme::Opencode | Scheme::ZhipuZai => {
                "账号自己的 API Key"
            }
            Scheme::NewApi => "查询设置里的访问令牌（Access Token）",
            Scheme::Sensenova => "查询设置里的账号与密码",
            Scheme::Zhipu => "查询设置里的控制台令牌",
        }
    }
}

/// 这一家有没有余额查询能力：自定义家 + 提供商记录里登记了方案。
///
/// 消费方是 `usage_query` 的批量过滤与账号公开形态（前端据此渲染余额按钮与
/// 「查询设置」段）—— 两处必须同判据，所以只有这一个入口。
pub fn supports_usage(provider_id: &str) -> bool {
    custom_providers::is_custom_provider_id(provider_id)
        && custom_providers::balance_scheme_of(provider_id).is_some()
}

/// 查询某账号的余额（归一化形状见 `ProviderAdapter::query_usage` 文档）。
///
/// 失败语义与内置家一致：`Err` 表示这次查询失败（由调用方写进结果的
/// `error` 字段，批量查询不因单账号失败中断）；「凭证还没配」用
/// [`usage_not_configured`] 的 400 + 机器可识别 code，前端显示成中性提示。
pub async fn query_usage(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let credential = store
        .custom_credential_by_id(account_id)
        .ok_or_else(|| GatewayError::with_status(404, "账号不存在"))?;
    let Some(provider_id) = store.custom_account_provider(account_id) else {
        return Err(GatewayError::with_status(
            501,
            "该账号不是自定义提供商的账号，无法用余额方案查询",
        ));
    };
    let scheme_id = custom_providers::balance_scheme_of(&provider_id).ok_or_else(|| {
        GatewayError::with_status(
            501,
            format!(
                "该提供商没有登记余额查询方案（{provider_id}）：请用「预置 API」里的卡片重建，\
                 或在账号设置里补上查询凭证"
            ),
        )
    })?;
    let scheme = Scheme::from_id(&scheme_id).ok_or_else(|| {
        GatewayError::with_status(
            501,
            format!("未知的余额查询方案 {scheme_id}（记录可能被手改过）"),
        )
    })?;
    let provider = custom_providers::get(&provider_id).unwrap_or(Value::Null);
    let label = provider
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| scheme.label().to_string());
    // 账号可以覆盖 base（记录里的 `baseUrl` 覆盖项），缺省回落提供商的
    let base_url = credential
        .base_url_override
        .clone()
        .or_else(|| {
            provider
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let query = store
        .custom_balance_query_by_id(account_id)
        .unwrap_or_else(empty_query);
    let proxy = credential.proxy.as_ref();
    let label = label.as_str();
    // 错误与日志文案一律用**提供商的展示名**（用户在界面上看到的那一个），
    // 而不是方案名 —— 同一种方案的两个人可能有完全不同的名字
    let what = |kind: &str| format!("{label} {kind}");

    match scheme {
        Scheme::Sub2api => {
            let key = require_api_key(&credential, scheme, label)?;
            let url = build_url(&base_url, Api::Usage)?;
            let payload = get_json(&url, &bearer(&key), proxy, &what("余额")).await?;
            parse::parse_sub2api(&payload).ok_or_else(|| invalid_response(label))
        }
        Scheme::DeepSeek => {
            let key = require_api_key(&credential, scheme, label)?;
            // DeepSeek 的余额接口在站点根下一级（不是 `/v1` 那条推理前缀）
            let url = build_url(&base_url, Api::DeepSeekBalance)?;
            let payload = get_json(&url, &bearer(&key), proxy, &what("余额")).await?;
            parse::parse_deepseek(&payload).ok_or_else(|| invalid_response(label))
        }
        Scheme::Opencode => {
            let key = require_api_key(&credential, scheme, label)?;
            let url = build_url(&base_url, Api::Usage)?;
            let payload = get_json(&url, &bearer(&key), proxy, &what("用量")).await?;
            parse::parse_opencode(&payload).ok_or_else(|| invalid_response(label))
        }
        Scheme::NewApi => query_new_api(&base_url, &query, proxy, label).await,
        Scheme::Zhipu => query_zhipu(&query, proxy, label).await,
        Scheme::ZhipuZai => query_zhipu_zai(&credential, proxy, label).await,
        Scheme::Sensenova => query_sensenova(store, &credential, &query, proxy, label).await,
    }
}

/// NewAPI：账户余额（`/api/user/self`）+ 显示口径（`/api/status`）。
///
/// 状态接口是**公开**的（不鉴权），拿不到只影响「原始配额怎么显示成钱」，
/// 不该让整次查询失败 —— 那时退回原始配额 + 单位 `quota`（源实现同口径）。
async fn query_new_api(
    base_url: &str,
    query: &CustomBalanceQuery,
    proxy: Option<&ResolvedProxy>,
    label: &str,
) -> Result<Value, GatewayError> {
    let scheme = Scheme::NewApi;
    let token = query.access_token.trim();
    if token.is_empty() {
        return Err(usage_not_configured(label, scheme.credential_hint()));
    }
    let self_url = build_url(base_url, Api::NewApiSelf)?;
    let status_url = build_url(base_url, Api::NewApiStatus)?;
    let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
    let user_id = query.user_id.trim();
    if !user_id.is_empty() {
        headers.push(("new-api-user".to_string(), user_id.to_string()));
    }
    let account = get_json(&self_url, &headers, proxy, &format!("{label} 账户余额")).await?;
    let Some((remaining, used)) = parse::parse_new_api_account(&account) else {
        return Err(invalid_response(label));
    };
    let status = get_json(&status_url, &[], proxy, &format!("{label} 站点状态"))
        .await
        .ok();
    let display = status.as_ref().and_then(parse::new_api_display);
    Ok(parse::new_api_document(
        remaining,
        used,
        display.as_ref(),
        &account,
        status.as_ref(),
    ))
}

/// 智谱：`bigmodel.cn` 控制台的账户余额（金额是人民币元）。
///
/// 地址**固定在控制台域**（与提供商 base 无关）：智谱的余额记在开放平台账户
/// 上，推理端点（`open.bigmodel.cn/api/…` 或国际站 `api.z.ai`）不提供余额接口。
/// 令牌是控制台登录 Cookie `bigmodel_token_production` 的值（源实现同口径）。
async fn query_zhipu(
    query: &CustomBalanceQuery,
    proxy: Option<&ResolvedProxy>,
    label: &str,
) -> Result<Value, GatewayError> {
    let scheme = Scheme::Zhipu;
    let token = query.token.trim();
    if token.is_empty() {
        return Err(usage_not_configured(label, scheme.credential_hint()));
    }
    let url = format!("{ZHIPU_CONSOLE_BASE}/api/biz/account/query-customer-account-report");
    // 这个接口认的是**裸令牌**头（不是 Bearer），照源实现原样发
    let headers = vec![("Authorization".to_string(), token.to_string())];
    let payload = get_json(&url, &headers, proxy, &format!("{label} 账户余额")).await?;
    parse::parse_zhipu(&payload, "CNY", "智谱控制台令牌无效或已过期")
        .map_err(|message| GatewayError::with_status(502, message))
}

/// 智谱国际站（z.ai）：同一条账户报告接口，金额是**美元**。
///
/// ── 凭证为什么不是控制台令牌 ────────────────────────────────
/// z.ai 的控制台类接口只认**签名凭证**：把账号 API Key（`{id}.{secret}` 形态）
/// 现场铸成智谱式 SIGN JWT（见 [`zhipu_sign_jwt`]），裸 API Key 会被拒
/// （上游在缺 / 坏 Authorization 时都回信封 `code:1001`，实测如此）。API Key
/// 本来就是转发要填的，这一支因此**不需要额外配置** —— 与 deepseek / opencode
/// 同一档，界面不出现任何输入框。
async fn query_zhipu_zai(
    credential: &CustomCredential,
    proxy: Option<&ResolvedProxy>,
    label: &str,
) -> Result<Value, GatewayError> {
    let scheme = Scheme::ZhipuZai;
    let key = require_api_key(credential, scheme, label)?;
    let jwt = zhipu_sign_jwt(&key).ok_or_else(|| {
        GatewayError::with_status(
            400,
            format!(
                "{label} 的 API Key 不是智谱的「id.secret」形态，无法用它查询余额：\
                 请到 z.ai 控制台重新生成 API Key"
            ),
        )
    })?;
    // 同国内站：认的是**裸签名**头（不是 Bearer）
    let headers = vec![("Authorization".to_string(), jwt)];
    let payload = get_json(
        ZHIPU_ZAI_BALANCE_URL,
        &headers,
        proxy,
        &format!("{label} 账户余额"),
    )
    .await?;
    parse::parse_zhipu(&payload, "USD", "智谱 API Key 签名无效或已过期")
        .map_err(|message| GatewayError::with_status(502, message))
}

/// 把智谱的 API Key（`{id}.{secret}` 形态）铸成控制台类接口认的**签名凭证**
/// （官方 JWT 鉴权那套：HS256、header 带 `sign_type: SIGN`）。
///
/// 三段都是 base64url（**无填充**）：header 固定为
/// `{"alg":"HS256","sign_type":"SIGN"}`，payload 是
/// `{api_key: <id 半段>, exp, timestamp}`（毫秒；exp 一小时后 —— 服务端只验
/// 「没过期」，每次查询现铸一把，不需要缓存与续期），签名用 secret 半段对
/// `header.payload` 做 HMAC-SHA256。铸不出（Key 没有 `id.secret` 的点分形态）
/// 给 None —— 调用方把它当成一条可读的配置错误。
fn zhipu_sign_jwt(api_key: &str) -> Option<String> {
    use base64::Engine;
    use hmac::Mac;
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let (id, secret) = api_key.trim().split_once('.')?;
    let (id, secret) = (id.trim(), secret.trim());
    if id.is_empty() || secret.is_empty() {
        return None;
    }
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = engine.encode(br#"{"alg":"HS256","sign_type":"SIGN"}"#);
    let now = chrono::Utc::now().timestamp_millis();
    let payload = engine.encode(
        serde_json::to_vec(&serde_json::json!({
            "api_key": id,
            "exp": now + 3_600_000,
            "timestamp": now,
        }))
        .ok()?,
    );
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("{header}.{payload}").as_bytes());
    let signature = engine.encode(mac.finalize().into_bytes());
    Some(format!("{header}.{payload}.{signature}"))
}

/// 商汤日日新：登录票（缓存优先）→ 控制面的双积分池用量。
///
/// 票的续期策略与源实现一致：缓存里有**没过期**（提前 60 秒算过期）的票就直接
/// 用，否则用账号密码重登一次并把新票写回记录（下一次查询就不必再登）。
/// 重登失败是**查询失败**（502）而不是「未配置」—— 用户已经把账号密码配好了，
/// 失败的原因在上游（密码错、风控、网络），文案要如实反映。
async fn query_sensenova(
    store: &AccountStore,
    credential: &CustomCredential,
    query: &CustomBalanceQuery,
    proxy: Option<&ResolvedProxy>,
    label: &str,
) -> Result<Value, GatewayError> {
    let scheme = Scheme::Sensenova;
    let mut jwt = query.sensenova_jwt.trim().to_string();
    if jwt.is_empty() || !sensenova_auth::jwt_reusable(&jwt) {
        let username = query.username.trim();
        if username.is_empty() || query.password.is_empty() {
            return Err(usage_not_configured(label, scheme.credential_hint()));
        }
        let result = sensenova_auth::login(username, &query.password, proxy)
            .await
            .map_err(|message| GatewayError::with_status(502, message))?;
        // 回写缓存：写失败只影响下一次查询（那次会再登一次），不能让它把
        // 一次成功的余额读取变成失败
        if let Err(error) = store.store_custom_balance_jwt(
            &credential.account_id,
            &result.access_token,
            result.expires_at_ms,
        ) {
            logging::verbose(
                "[Usage]",
                &format!(
                    "商汤登录票缓存写入失败（账号 {}）：{}",
                    credential.account_name, error.message
                ),
            );
        }
        jwt = result.access_token;
    }
    let tenant = sensenova_auth::jwt_tenant_id(&jwt).ok_or_else(|| {
        GatewayError::with_status(502, format!("{label} 的登录票里没有 tenant_id，无法定位账户"))
    })?;
    let account_id_param: String =
        url::form_urlencoded::byte_serialize(tenant.as_bytes()).collect();
    let url = format!(
        "{}/console/v1/tokenplan/pool-usage?account_id={account_id_param}",
        sensenova_auth::PLATFORM_LITE_BASE
    );
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {jwt}")),
        (
            "Referer".to_string(),
            format!("{}/console", sensenova_auth::PLATFORM_BASE),
        ),
    ];
    let payload = get_json(&url, &headers, proxy, &format!("{label} 用量")).await?;
    parse::parse_sensenova(&payload).ok_or_else(|| invalid_response(label))
}

/// 这一家的余额接口在 base 上追加哪段路径（源实现 `buildQueryUri` /
/// `buildNewApiUri` 的同一套判定）。
///
/// 为什么要「判定而不是直接拼」：用户填 base 的习惯不统一 —— 有的填到
/// `https://host`、有的填到 `https://host/v1`、还有的把接口整条路径都填上。
/// 三种都要能用，所以先看末尾是不是已经是目标路径，是就不动。
#[derive(Clone, Copy)]
enum Api {
    /// `{base}/v1/usage`（sub2api 与 OpenCode Go 共用）
    Usage,
    /// `{base}/user/balance`（DeepSeek；推理前缀 `/v1` 要去掉）
    DeepSeekBalance,
    /// `{base}/api/user/self`（NewAPI 账户余额）
    NewApiSelf,
    /// `{base}/api/status`（NewAPI 站点状态，公开接口）
    NewApiStatus,
}

/// base + 目标接口 → 完整地址（base 非法时报 400，文案指向「提供商设置」）。
fn build_url(base_url: &str, api: Api) -> Result<String, GatewayError> {
    let trimmed = base_url.trim();
    let invalid = || {
        GatewayError::with_status(
            400,
            "该提供商的 Base URL 不可用（余额查询需要它）：请在「提供商」设置里检查地址",
        )
    };
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = Url::parse(&candidate).map_err(|_| invalid())?;
    if url.host_str().unwrap_or("").is_empty() {
        return Err(invalid());
    }
    let mut parts: Vec<String> = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    match api {
        Api::Usage => {
            if !ends_with(&parts, &["v1", "usage"]) {
                if parts.last().map(String::as_str) == Some("v1") {
                    parts.push("usage".to_string());
                } else {
                    parts.push("v1".to_string());
                    parts.push("usage".to_string());
                }
            }
        }
        Api::DeepSeekBalance => {
            if !ends_with(&parts, &["user", "balance"]) {
                if parts.last().map(String::as_str) == Some("v1") {
                    parts.pop();
                }
                parts.push("user".to_string());
                parts.push("balance".to_string());
            }
        }
        Api::NewApiSelf => append_api_path(&mut parts, &["api", "user", "self"]),
        Api::NewApiStatus => append_api_path(&mut parts, &["api", "status"]),
    }
    url.set_path(&format!("/{}", parts.join("/")));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

/// NewAPI 的 `/api/...` 路径追加（末尾已是 `api` 时不重复那一段）
fn append_api_path(parts: &mut Vec<String>, suffix: &[&str]) {
    if ends_with(parts, suffix) {
        return;
    }
    if parts.last().map(String::as_str) == Some("v1") {
        parts.pop();
    }
    if parts.last().map(String::as_str) == Some("api") && suffix.first() == Some(&"api") {
        parts.extend(suffix.iter().skip(1).map(|part| (*part).to_string()));
    } else {
        parts.extend(suffix.iter().map(|part| (*part).to_string()));
    }
}

/// 路径段是否以 `suffix` 结尾
fn ends_with(parts: &[String], suffix: &[&str]) -> bool {
    if parts.len() < suffix.len() {
        return false;
    }
    parts[parts.len() - suffix.len()..]
        .iter()
        .zip(suffix)
        .all(|(part, expected)| part == expected)
}

/// 发一次 GET 并读出 JSON（超时、错误文案、错误码分流都在这里收口）。
async fn get_json(
    url: &str,
    headers: &[(String, String)],
    proxy: Option<&ResolvedProxy>,
    what: &str,
) -> Result<Value, GatewayError> {
    let response = send_raw("GET", url, None, headers, proxy, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            GatewayError::with_status(
                502,
                format!("{what}请求失败：{}", egress::describe_error_detail(&error)),
            )
        })?;
    if !response.ok {
        // 401 / 403 原样透出（「令牌不对 / 没权限」与「上游挂了」是两件事），
        // 其余按 502 收敛 —— 与各家余额实现的错误分类同一取向
        let status = if matches!(response.status, 401 | 403) {
            response.status as i32
        } else {
            502
        };
        let detail = describe_payload_error(&response.payload);
        return Err(GatewayError::with_status(
            status,
            format!("{what}被上游拒绝（HTTP {}）{detail}", response.status),
        ));
    }
    response.payload.ok_or_else(|| {
        GatewayError::with_status(502, format!("{what}响应不是有效 JSON"))
    })
}

/// 上游错误响应里的一句话（`error.message` / `message` / `msg` / `error`）
fn describe_payload_error(payload: &Option<Value>) -> String {
    let Some(payload) = payload else { return String::new() };
    let message = payload
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .or_else(|| payload.get("msg").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match message {
        Some(text) => format!("：{}", parse::shorten(text, 160)),
        None => String::new(),
    }
}

/// 账号 API Key（sub2api / deepseek / opencode 三家用它查余额）
fn require_api_key(
    credential: &CustomCredential,
    scheme: Scheme,
    label: &str,
) -> Result<String, GatewayError> {
    let key = credential.api_key.trim();
    if key.is_empty() {
        return Err(usage_not_configured(label, scheme.credential_hint()));
    }
    Ok(key.to_string())
}

/// Bearer 头
fn bearer(token: &str) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), format!("Bearer {token}"))]
}

/// 「上游响应读不出余额」的统一报错（502：是上游那边的问题，不是配置缺失）
fn invalid_response(label: &str) -> GatewayError {
    GatewayError::with_status(
        502,
        format!("{label} 的余额响应结构异常（读不出余额字段）"),
    )
}

/// 空查询配置（记录里没有 `balanceQuery` 键时的读数口径）
fn empty_query() -> CustomBalanceQuery {
    CustomBalanceQuery {
        user_id: String::new(),
        access_token: String::new(),
        username: String::new(),
        password: String::new(),
        token: String::new(),
        sensenova_jwt: String::new(),
        sensenova_jwt_expires_at: 0,
    }
}
