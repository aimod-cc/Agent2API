//! KukuAI 会话三件套（`bdstoken` / `uinfo` / `uk`）的进程级缓存。
//!
//! ── 为什么需要缓存 ──────────────────────────────────────────
//! 对话链路要求每次请求带 `bdstoken/uinfo/uk`（拼进 query），它们由
//! `GET /api/genflowpro/common/userreport` 用登录 Cookie 换来（kuku2api 实测）。
//! 每次对话都打一次 userreport 既没必要也招风控 —— 参考实现缓存 600 秒，
//! 本模块同值。key 用 BDUSS（同一账号的 Cookie 不变，三件套可复用）。
//!
//! ── 线程模型 ────────────────────────────────────────────────
//! 单个 `Mutex<HashMap>`，持锁只做哈希表读写、**不跨 await**（userreport 的
//! 网络请求在锁外做：先放锁再发请求，回来再写槽 —— 避免「持锁等网络」。
//! 并发时可能重复刷新，代价是同一账号多打一次 userreport，可接受）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::credentials::{KukuCredentials, request_headers};
use super::{WEB_QUERY, BASE_URL};

/// 三件套的有效期（参考实现 600 秒，同值）
const TTL_MS: i64 = 600_000;

/// 会话三件套。
#[derive(Clone, Debug)]
pub struct TokenTriple {
    pub bdstoken: String,
    pub uinfo: String,
    pub uk: String,
}

struct TokenSlot {
    triple: TokenTriple,
    refreshed_at_ms: i64,
}

/// 进程级会话槽（key = BDUSS）
static TOKEN_SLOTS: Mutex<Option<HashMap<String, TokenSlot>>> = Mutex::new(None);

fn slots() -> std::sync::MutexGuard<'static, Option<HashMap<String, TokenSlot>>> {
    match TOKEN_SLOTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 取（必要时刷新）会话三件套。
///
/// `force = true` 跳过 TTL 直接刷新（添加账号验证时用，要的是「现在真的能用」）。
pub async fn ensure_tokens(
    credentials: &KukuCredentials,
    proxy: Option<&ResolvedProxy>,
    force: bool,
) -> Result<TokenTriple, GatewayError> {
    let key = credentials.bduss.clone();
    let now = crate::server::logging::now_ms();
    if !force {
        if let Some(triple) = cached(&key, now) {
            return Ok(triple);
        }
    }
    let triple = refresh_triple(credentials, proxy).await?;
    {
        let mut guard = slots();
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(key, TokenSlot { triple: triple.clone(), refreshed_at_ms: now });
    }
    Ok(triple)
}

/// 读缓存（未命中或过期返回 None）。
fn cached(key: &str, now: i64) -> Option<TokenTriple> {
    let guard = slots();
    let map = guard.as_ref()?;
    let slot = map.get(key)?;
    if now - slot.refreshed_at_ms >= TTL_MS {
        return None;
    }
    Some(slot.triple.clone())
}

/// 打一次 userreport 换三件套。
///
/// ── 会话自愈（2026-10-08 实测后加）─────────────────────────
/// 凭证里的 STOKEN 可能是**通行证级**的（网页登录早期版本落库的账号）或
/// 已被后续登录轮换 —— 业务接口判「未登录」（errno=-6），而余额这类通用
/// 接口只看 BDUSS，所以用户看到的是「余额正常、模型拉不下」。只要 extras
/// 里有 PTOKEN，就先向百度通行证换发一次 genflowpro STOKEN（`engine.rs`）
/// 再重试；换发失败保持原错误路径，行为与旧版一致。
async fn refresh_triple(
    credentials: &KukuCredentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<TokenTriple, GatewayError> {
    let url = format!("{BASE_URL}/api/genflowpro/common/userreport?{WEB_QUERY}");
    let mut prepared = credentials.clone();
    let mut value = {
        let headers = request_headers(&prepared);
        super::http::get_json_value(&url, &headers, proxy, Some(30_000)).await?
    };
    let mut errno = value.get("errno").and_then(Value::as_i64).unwrap_or(-1);
    if errno != 0 {
        if let Some(ptoken) = super::credentials::ptoken_of(&prepared.extras) {
            match super::engine::exchange_genflowpro_stoken(&prepared.bduss, &ptoken).await {
                Ok(stoken) => {
                    crate::server::logging::log(
                        "[KukuAI]",
                        "♻️ 会话校验未通过，已换发业务会话令牌后重试",
                    );
                    prepared.stoken = stoken;
                    let headers = request_headers(&prepared);
                    value = super::http::get_json_value(&url, &headers, proxy, Some(30_000)).await?;
                    errno = value.get("errno").and_then(Value::as_i64).unwrap_or(-1);
                }
                Err(error) => {
                    crate::server::logging::log(
                        "[KukuAI]",
                        &format!("⚠️ 换发业务会话令牌失败（{}）", error.message),
                    );
                }
            }
        }
    }
    if errno != 0 {
        let message = value
            .get("show_msg")
            .or_else(|| value.get("errmsg"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        // 走到这里的 -6 已经过一次「换发业务令牌重试」仍然失败（2026-10-08
        // 实测：换发成功即 errno=0），剩下的基本是凭证真失效（退出登录 /
        // 换号）或上游临时风控限流。
        return Err(GatewayError::with_status(
            401,
            format!(
                "KukuAI 会话校验未通过（userreport errno={errno}：{message}）——\
                 请重新登录该账号（或重新粘贴完整 Cookie）后再试；若账号无误，\
                 等 1-2 分钟再试"
            ),
        ));
    }
    let Some(data) = value.get("data") else {
        return Err(GatewayError::with_status(
            502,
            "KukuAI userreport 返回缺少 data（上游响应异常）",
        ));
    };
    let text_of = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let triple = TokenTriple {
        bdstoken: text_of("bdstoken"),
        uinfo: text_of("uinfo"),
        // uk 可能是字符串也可能是数字（kuku2api 直接 `d["uk"]`，参考实现
        // 初始值就是 0）；as_str 拿不到数字，回落转成字符串（实测 uk 空时
        // model/list 仍能通，但对话接口可能要求非空，两形态都收）。
        uk: {
            let raw = data.get("uk");
            raw.and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_string)
                .filter(|text| !text.is_empty())
                .or_else(|| raw.and_then(Value::as_i64).map(|n| n.to_string()))
                .unwrap_or_default()
        },
    };
    if triple.bdstoken.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "KukuAI userreport 返回缺少 bdstoken（上游响应异常）",
        ));
    }
    Ok(triple)
}

/// 会话 query 段（拼在接口 query 后：`&bdstoken=…&uinfo=…&uk=…`）。
pub fn token_query(triple: &TokenTriple) -> String {
    format!(
        "&bdstoken={}&uinfo={}&uk={}",
        triple.bdstoken, triple.uinfo, triple.uk
    )
}
