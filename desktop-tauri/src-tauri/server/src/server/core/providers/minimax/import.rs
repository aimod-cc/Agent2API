//! 「导入本机登录态」：从已登录的 MiniMax Code 客户端**复制**一份令牌。
//!
//! ── 客户端的令牌存在哪（2026-10-21 本机实测 + `@mavis/oauth-core` 源码）──
//! ```text
//!   ~/.minimax/auth/prod/cn/mcode-public/auth.json         ← 国内版令牌（records[*]）
//!   ~/.minimax/auth/prod/cn/mcode-public/auth-state.json   ← 国内版登录状态 + generation
//!   ~/.minimax/auth/prod/en/mcode-public/auth.json         ← 国际版令牌（同一个安装包）
//! ```
//! 目录口径来自 `@mavis/oauth-core/dist/namespace.js`：
//! `namespaceHome = join(authHome, buildEnv, region, MCODE_OAUTH_CLIENT_ID)` ——
//! **地区是路径的一层**，所以同一个客户端里国内 / 国际两份登录态各存一处。
//! 因此导入必须按 provider 的地区取目录（`Region::desktop_region_dir()`）：
//! 拿国内版的 provider 去导国际版的令牌，得到的是一份**打不通**的凭证
//! （curl 实测：两地的 `mmoat_` 令牌互不通用，会回 `invalid access token`）。
//! `records` 的键是 `${service}\0${account}`（**真 NUL 字符**分隔），
//! `account = base64url(sha256(authHome + "\0" + "mcode-public"))` —— 所以
//! **不能按固定键名取**，只能「按值找第一条带 `accessToken` 的记录」
//! （`dsh-connect-minimaxcode` 的 `parseOAuthDocument` 也是这么做的）。
//!
//! ── 为什么只读不写（重要，别「顺手」加上写回）───────────────────
//! `auth.json` 与 `auth-state.json` 各带一个 `generation` 计数器，客户端要求
//! **两者相等**，否则判定「这次登录属于别处」并要求用户重新扫码
//! （`MCodeOAuthCore.readUsableCredential`）。我们写一次就会破坏这个代际；
//! 更隐蔽的是**续期**：`refresh_token` 单次有效，我们若拿它续期，客户端手里
//! 那枚就立刻失效 —— 用户下次打开客户端会被要求重登，而他会以为是我们弄坏的。
//! ⇒ 导入只是**那一刻的快照**：之后我们用自己的刷新令牌续自己的，客户端管它自己的。
//! 两枚刷新令牌同源同轮换，谁先续谁赢，另一方会拿到 `invalid_grant` ——
//! 所以导入路径要在说明里讲清「导入后请尽量只用网关，客户端那边会各自续期」。
//!
//! ── 与 Marvis 的对比（为什么本家不需要提权）─────────────────────
//! Marvis 的令牌只在进程内存里、进程还设了受限 DACL，所以那边要 UAC 派生提权
//! 提取器；本家令牌就是一个**当前用户可读的普通 JSON 文件**，代价为零。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic，且**不写任何文件**。

use serde_json::{json, Map, Value};

use crate::server::errors::GatewayError;

use super::credentials;
use super::region::Region;

/// 客户端数据目录名（`~/.minimax`，可由 `MINIMAX_DATA_DIR` / `MAVIS_DATA_DIR` 覆盖）。
const DATA_DIR_NAME: &str = ".minimax";

/// 令牌文件相对数据目录的模板（`auth/<env>/<region>/mcode-public/auth.json`，
/// 见模块头 —— 地区是路径的一层）。
const AUTH_RELATIVE_TEMPLATE: &str = "auth/prod/{region}/mcode-public/auth.json";

/// 定位令牌文件（含客户端的两个环境变量覆盖）。
///
/// `region` 决定路径里的地区段：国内 `cn`、国际 `en`（客户端 `MAVIS_REGION`
/// 的两个取值，见 `@mavis/oauth-core/dist/namespace.js`）。
fn auth_path(region: Region) -> Option<std::path::PathBuf> {
    let base = std::env::var("MINIMAX_DATA_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("MAVIS_DATA_DIR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let home = std::env::var("USERPROFILE")
                .ok()
                .or_else(|| std::env::var("HOME").ok())?;
            Some(std::path::PathBuf::from(home).join(DATA_DIR_NAME))
        })?;
    Some(base.join(
        AUTH_RELATIVE_TEMPLATE.replace("{region}", region.desktop_region_dir()),
    ))
}

/// 从客户端令牌文档里取出第一条可用凭据。
///
/// `records` 的形状：`{"<service>\0<account>": {accessToken, refreshToken,
/// expiresAtMs, clientId, scopes, audience, generation, …}}`。
/// 键名生成于安装期，不能硬编码（见模块头）。
pub fn parse_auth_document(text: &str) -> Option<Value> {
    let document: Value = serde_json::from_str(text).ok()?;
    let records = document.get("records")?.as_object()?;
    for entry in records.values() {
        let Some(record) = entry.as_object() else {
            continue;
        };
        let access_token = record
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(access_token) = access_token else {
            continue;
        };
        let mut out = Map::new();
        out.insert("accessToken".to_string(), Value::String(access_token.to_string()));
        if let Some(refresh) = record
            .get("refreshToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            out.insert("refreshToken".to_string(), Value::String(refresh.to_string()));
        }
        if let Some(expires_at) = record
            .get("expiresAtMs")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
        {
            out.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        // JWT 里能读到的身份与到期：`expiresAtMs` 缺失时用它兜底
        let (jwt_expires, jwt_user) = credentials::decode_jwt_claims(access_token);
        if !out.contains_key("expiresAt") && jwt_expires > 0 {
            out.insert("expiresAt".to_string(), Value::from(jwt_expires));
        }
        if !jwt_user.is_empty() {
            out.insert("userId".to_string(), Value::String(jwt_user));
        }
        return Some(Value::Object(out));
    }
    None
}

/// 读本机客户端登录态（`region` 决定读哪一版：国内 `auth/prod/cn/`、国际 `auth/prod/en/`）。
///
/// 每一种失败都是一句**能指导下一步动作**的话，而不是「失败了」。
pub fn import_from_desktop(region: Region) -> Result<Value, GatewayError> {
    let Some(path) = auth_path(region) else {
        return Err(GatewayError::with_status(
            400i32,
            "找不到用户目录，无法定位 MiniMax Code 客户端的登录态；请改用粘贴访问令牌",
        ));
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            return Err(GatewayError::with_status(
                400i32,
                format!(
                    "没读到 MiniMax Code {}的登录态（{}）：客户端没装、没登录，或用了自定义数据目录。\
                     请先在客户端里登录（注意客户端按地区分别保存登录态，国内版与国际版是两个账号），\
                     或改用「粘贴访问令牌」",
                    region.label(),
                    path.display()
                ),
            ))
        }
    };
    let mut record = parse_auth_document(&text).ok_or_else(|| {
        GatewayError::with_status(
            400i32,
            format!(
                "客户端 {}的登录态文件里没有可用令牌（可能还没登录完成）：请先在客户端里完成登录",
                region.label()
            ),
        )
    })?;
    // 明确告知来源，便于日志与「这是从哪儿来的」这一类追问
    crate::server::logging::log(
        "[Accounts]",
        &format!(
            "已读取 MiniMax Code {}客户端登录态：{}",
            region.label(),
            path.display()
        ),
    );
    // 地区标记落进记录：调试时能一眼看出「这份令牌是从哪一版客户端导的」。
    // **不参与**身份判定（地区由 provider id 表达，见 `region.rs`）。
    if let Some(object) = record.as_object_mut() {
        object.insert(
            "edition".to_string(),
            Value::String(region.edition_id().to_string()),
        );
    }
    Ok(record)
}

/// 本机登录态是否**看起来**已登录（界面决定「导入」按钮是否置灰）。
pub fn desktop_available(region: Region) -> bool {
    auth_path(region)
        .map(|path| path.is_file())
        .unwrap_or(false)
}

/// 给界面看的一句话（不含路径细节，避免把用户目录带进前端日志）。
pub fn desktop_hint(region: Region) -> Value {
    json!({
        "available": desktop_available(region),
        "hint": format!(
            "从本机 MiniMax Code {}客户端读取登录态（无需管理员权限，只读不写）",
            region.label()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实文件的形状（键里带真 NUL，`\u0000` 是 JSON 对它的转义）。
    fn document() -> String {
        json!({
            "records": {
                "main\u{0}YjRlY2YwYQ": {
                    "accessToken": "header.eyJleHAiOjE3MDAwMDAwMDAsInVzZXIiOnsiaWQiOiJ1NDIifX0.sig",
                    "refreshToken": "mmort_abc",
                    "expiresAtMs": 1_700_003_600_000i64,
                    "clientId": "mcode-public",
                    "scopes": ["agent.default"],
                    "audience": "agent-backend",
                    "generation": 3
                }
            }
        })
        .to_string()
    }

    #[test]
    fn reads_the_first_usable_record_by_value() {
        // 键名是安装期生成的，不能硬编码 —— 只能按值找
        let record = parse_auth_document(&document()).expect("应能读到");
        assert!(record["accessToken"].as_str().unwrap_or("").starts_with("header."));
        assert_eq!(record["refreshToken"], json!("mmort_abc"));
        assert_eq!(record["expiresAt"], json!(1_700_003_600_000i64));
        assert_eq!(record["userId"], json!("u42"), "JWT 里的身份要一并带出");
    }

    #[test]
    fn expires_at_falls_back_to_the_jwt() {
        let text = json!({
            "records": {
                "k": {
                    "accessToken": "h.eyJleHAiOjE3MDAwMDAwMDAsInVzZXIiOnsiaWQiOiJ1NDIifX0.s"
                }
            }
        })
        .to_string();
        let record = parse_auth_document(&text).expect("应能读到");
        assert_eq!(record["expiresAt"], json!(1_700_000_000_000i64));
        assert!(record.get("refreshToken").is_none(), "没给就不写这个键");
    }

    #[test]
    fn records_without_token_are_skipped() {
        let text = json!({
            "records": {
                "a": {"refreshToken": "mmort_only"},
                "b": {"accessToken": "  "},
                "c": {"accessToken": "good-token"},
            }
        })
        .to_string();
        let record = parse_auth_document(&text).expect("应能读到 c");
        assert_eq!(record["accessToken"], json!("good-token"));
    }

    #[test]
    fn malformed_documents_yield_none() {
        assert!(parse_auth_document("not json").is_none());
        assert!(parse_auth_document("{}").is_none());
        assert!(parse_auth_document("{\"records\": {}}").is_none());
        assert!(parse_auth_document("{\"records\": []}").is_none());
    }

    /// 两个地区读**不同文件**（`auth/prod/cn/` vs `auth/prod/en/`）——
    /// 拿到另一地的令牌是一份打不通的凭证（两地令牌互不通用）。
    ///
    /// 只算路径、不碰文件系统：`auth_path` 依赖进程环境变量，测它会把
    /// 本机真实登录态卷进来（SKILL「测试不得依赖本机环境」），
    /// 所以钉住的是模板展开后的相对路径。
    #[test]
    fn each_region_reads_its_own_desktop_file() {
        let cn = AUTH_RELATIVE_TEMPLATE.replace("{region}", Region::Cn.desktop_region_dir());
        let intl = AUTH_RELATIVE_TEMPLATE.replace("{region}", Region::Intl.desktop_region_dir());
        assert_eq!(cn, "auth/prod/cn/mcode-public/auth.json");
        assert_eq!(intl, "auth/prod/en/mcode-public/auth.json");
        assert_ne!(cn, intl, "两地登录态在客户端里就是两个文件");
    }
}
