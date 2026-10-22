//! MiniMax Code 账号：设备码登录 / 手工粘贴落账号 + 公开形态。
//!
//! ── 两个地区 = 两家 provider（2026-10-21 补）─────────────────────
//! `provider` 是 `"minimax"`（国内版）或 `"minimax-intl"`（国际版），
//! 所有查询/写入都按 `Region` 参数化（`Region::provider_id()`）。
//! 两地账号集合独立：同一个 `user.id` 在两地各有一条记录也有意义（两份独立的
//! 积分池），所以账号 id 带地区前缀（`minimax-` / `minimax-intl-`）。
//!
//! ── 账号字段 ────────────────────────────────────────────────
//! ```text
//!   { id, provider:"minimax"|"minimax-intl", name, accessToken(= mmoat_ 令牌),
//!     refreshToken(= mmort_ 刷新令牌), expiresAt, userId, tokenTail,
//!     source, priority, enabled, addedAt, updatedAt }
//! ```
//! `accessToken` / `refreshToken` / `expiresAt` 三个键名与别家统一
//! （`session_from_record` 会把它们搬进会话的 `auth`，转发链据此取令牌）。
//!
//! ── 为什么 id 取「稳定身份」而不是令牌哈希 ──────────────────────
//! 访问令牌**每小时轮换**（`expires_in=3600`），拿它当 id 会让同一个人每小时
//! 多出一个账号。身份来源按强度递减：
//!   1. `userId`（payload 显式给，或 JWT 的 `user.id`）；
//!   2. `credentials::stable_identity` 挖出的稳定身份（JWT 的 `user.id` /
//!      `uid` / `sub`，或 `token-` / `user-` 前缀的尾段）；
//!   3. 都挖不到才退回 `sha256(令牌)`。
//!
//! **2026-10-22 修正（两条缺陷，都在这一处收敛）**：
//!   1. 线上 `mmoat_` 令牌是**不透明串**（60 字符、无 JWT 段），旧实现恒落到
//!      第 3 档 ⇒ 「重新添加同一个人 = 新增一条账号」，用户看到两个自己、
//!      旧的那条再也刷不动（其 RT 已被新登录轮换掉）。现在第 2 档承担主要
//!      职责，并且**落库前先认领既有记录**（按 `userId` / 稳定身份 / 令牌字面），
//!      避免修复本身把老账号（旧公式算出的 id）变成孤儿。
//!   2. 第 3 档对不透明令牌仍然存在（上游不给身份，没有本地解法）。补法是
//!      **把 `userId` 落进记录**：登录与添加流程都会在探活里顺带取它
//!      （`probe::ProbeOutcome.identity`），进了记录之后同一个人就恒定落在
//!      同一个 id 上。账户类接口的签名也依赖它（缺 `user_id` ⇒ `invalid signature`）。
//!   3. **认领必须双向**（2026-10-22 补，实测复现）：上一条修好的是「有新 userId、
//!      旧记录也有 userId」。而真实升级路径是「旧记录**只有**令牌哈希、本次登录
//!      才拿到 userId」—— 旧实现在这个方向上一条判据都不命中，于是①仍在列表里，
//!      ②另起一条，而①的 RT 已被②轮换掉 ⇒ 旧那条永久报登录过期。现在第 4 层
//!      判据专门覆盖它（仅 `web` / `desktop` 路径、且本地区只有一条无 userId
//!      的记录），见 `add_minimax_account` 里的长注释。
//!
//! ⚠️ 国内版的 id 必须保持 `minimax-<hash>` 这个**存量口径**（已接入那批账号
//! 就是这么存的），国际版加 `-intl` 段。改前者会让老账号变成孤儿记录。
//!
//! ── 本文件没有任何网络请求 ──────────────────────────────────
//! 全是「读-改-写」存储操作（持锁不做网络）。联网的探活在 `providers::minimax::probe`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::minimax::credentials;
use crate::server::core::providers::minimax::Region;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 输入长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_INPUT_LENGTH: usize = 16_384;

/// 从 payload 里取第一个非空字符串（候选键覆盖落盘名与粘贴形态）。
fn pick(payload: &Value, keys: &[&str]) -> String {
    for key in keys {
        let value = payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// 身份键 → 稳定的账号 id（见模块头）。
///
/// ── 哈希域**不含地区**（这是一条兼容性约束，别「顺手」加上）──────
/// 国内版是**存量** provider：已接入那批账号的 id 就是
/// `minimax-` + `sha256("minimax:account-id:v1\0" + identity)` 的前 12 位。
/// 把地区塞进哈希输入会让同一身份重新登录时算出一个**不同**的 id ⇒ 新增一条
/// 记录而不是更新（用户看到两个自己，旧的那条再也刷不动）。
///
/// 两地区不撞是靠**前缀**而不是靠哈希（`minimax-` vs `minimax-intl-`）：
/// 即使同一个 `user.id` 在两地的 digest 相同，拼上前缀后也是两个不同的 id。
/// 这与 ModelScope 的 `format!("{prefix}{user_id}")` 同一思路。
fn account_id_for(region: Region, identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"minimax:account-id:v1\0");
    hasher.update(identity.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{}{}", region.account_id_prefix(), &digest[..12])
}

/// 这条写入路径**是否已经轮换掉了旧记录的刷新令牌**。
///
/// 只有这种路径才允许「认领一条没有 userId 的旧记录」（见 `add_minimax_account`
/// 的第 4 层判据）。判据是上游行为，不是我们的偏好：
///
/// ```text
///   web       网页登录（OAuth 设备码）：换取新令牌的同时，上游让**同一个人**
///             旧的 refresh token 失效 —— 旧记录留着也刷不动，只能删
///   desktop   导入本机客户端登录态：同一台机器的同一份授权，同样是一次快照替换
///   manual    手工粘贴：用户可能在另一台机器/另一份授权上操作，旧快照**仍然有效**，
///             两条并存是对的 —— 因此绝不能让它去顶掉别人的记录
///   file      同上（导入的是别人导出的凭证文本）
/// ```
fn is_supersede_source(source: &str) -> bool {
    matches!(source.trim(), "web" | "desktop")
}

impl AccountStore {
    /// 取一条 MiniMax Code 账号记录（`region` 决定查哪一家）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条。
    pub fn minimax_account_record(&self, region: Region, account_id: &str) -> Option<Value> {
        let provider_id = region.provider_id();
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == provider_id).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, provider_id)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 MiniMax Code 账号（登录与手工粘贴的唯一落账号入口）。
    ///
    /// payload：
    ///   - `accessToken` / `access_token` / `token`：`mmoat_` 访问令牌（必填）
    ///   - `refreshToken` / `refresh_token`：`mmort_` 刷新令牌（可选，但没有它就
    ///     无法自动续期 —— 一小时后用户必须重新登录）
    ///   - `expiresAt`：访问令牌到期毫秒（可选，缺时从 JWT 的 `exp` 推）
    ///   - `userId` / `user_id` / `uid`：登录身份（可选，缺时从 JWT 的 `user.id` 推）
    ///
    /// 同一身份的记录就地更新（保留优先级与启用状态、沿用用户改过的备注名）——
    /// 这是本家的关键语义：令牌每小时轮换，重新登录应**更新**而不是新增账号。
    /// 撞到别的 provider 的 id 时报错，绝不覆写（与另外几家同一策略）。
    pub fn add_minimax_account(
        &self,
        region: Region,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let provider_id = region.provider_id();
        let access_token = pick(payload, &["accessToken", "access_token", "token"]);
        let refresh_token = pick(payload, &["refreshToken", "refresh_token"]);
        if access_token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 accessToken（MiniMax Code 访问令牌）",
                400,
            ));
        }
        if access_token.chars().count() > MAX_INPUT_LENGTH {
            return Err(AccountStoreError::new("输入过长", 400));
        }
        // JWT 里能读出身份与到期 —— **以 JWT 为准**（更权威，且手工粘贴时
        // 用户通常不会自己填 expiresAt）
        let (jwt_expires, jwt_user) = credentials::decode_jwt_claims(&access_token);
        let user_id = {
            let explicit = pick(payload, &["userId", "user_id", "uid"]);
            if explicit.is_empty() {
                jwt_user
            } else {
                explicit
            }
        };
        let expires_at = payload
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(jwt_expires);
        // 身份键：`userId` → 令牌里挖得出的稳定身份 → 令牌哈希（见模块头）。
        //
        // ── 为什么中间多出一档（2026-10-22 修）────────────────────
        // 线上令牌是不透明的 `mmoat_`（60 字符，没有 JWT 段），`decode_jwt_claims`
        // 读不出 `userId`。旧实现因此直接掉到 `sha256(accessToken)`，于是**每换
        // 一枚令牌就等于一个新账号**：用户重新登录 / 重新添加同一个人 → 列表里
        // 两条，旧的那条再也刷不动（它的 RT 已被新登录轮换掉）。
        //
        // `stable_identity` 先按 JWT 的 `user.id` / `uid` / `sub` 挖，再按
        // `token-` / `user-` 前缀挖 —— 这两种都随令牌轮换而不变。
        //
        // ⚠️ **不透明令牌仍然挖不到身份**（这是上游现实，不是可以修的 bug）。
        // 那种情况只能退回令牌哈希：这时「重新添加」仍会新增一条，但
        //   ① 令牌字面追认（下面第 2 层）能挡住重复导入同一份快照；
        //   ② 唯一能根治的办法是把 `userId` 落进记录 —— 登录/添加流程已经
        //      在探活里顺带补它（`probe::ProbeOutcome.identity`），补上之后
        //      同一个人就永远落在同一个 id 上。
        // 拒绝落库是**不能做**的取舍：那会把所有不透明令牌的用户挡在门外。
        let identity = if !user_id.is_empty() {
            format!("user:{user_id}")
        } else {
            let stable = credentials::stable_identity(&access_token);
            if stable.is_empty() {
                let mut hasher = Sha256::new();
                hasher.update(b"minimax:token-identity\0");
                hasher.update(access_token.as_bytes());
                format!("token:{:x}", hasher.finalize())
            } else {
                format!("stable:{stable}")
            }
        };
        let computed_id = account_id_for(region, &identity);

        let guard = self.guard();
        // ── 同身份复用既有 id（第二条闸）────────────────────────────
        // 为什么不能只靠哈希：**存量记录**是旧公式算出来的
        // （`minimax-<sha256("minimax:account-id:v1\0" + "token:<digest>")[..12]>`），
        // 而新公式（稳定身份）会给同一个人算出另一个 id。若不认领既有记录，
        // 修复本身就会制造一对重复账号 —— 比原缺陷更糟（用户得手工删）。
        //
        // 判据按强度递减，命中即复用既有 id（**绝不新算**）：
        //   1. 记录里的 `userId` 相等（最硬；两地区已由 provider 收窄）；
        //   2. 两边的稳定身份相等且都非空（令牌轮换也不变）；
        //   3. 令牌字面相等（同一枚 RT / 同一枚 accessToken）—— **仅当两边
        //      都挖不出稳定身份**时才算。
        //   4. **[`is_supersede_source`] 的写入路径 + 唯一候选**（2026-10-22 补，
        //      见下）。
        //
        // ── 第 3 层为什么必须加那个前提（临时验收程序当场抓到过）──────
        // 只看「令牌字面相等」的话，把 A 的令牌再粘一次会命中 **别人的记录**
        // （对方恰好也用过同一枚令牌 —— 实测复现：两个不同的人被并成一条，
        // 其中一个人凭空消失）。令牌字面相等只说明「同一份快照又来了」，
        // 不说明「这两条该合并」；只有在我们连稳定身份都没有、无从判断时，
        // 它才是唯一线索。
        //
        // ── 第 4 层为什么必要（2026-10-22 实测复现的缺陷）─────────────
        // 上面三层**全都要求两边可比**：要么两边都有 userId、要么两边都挖得出
        // 稳定身份、要么令牌字面相同。而线上最常见的升级路径恰好一条都不满足：
        //   ```text
        //   ① 旧版粘贴一枚不透明 `mmoat_` 令牌（挖不到身份）→ id = minimax-<token 哈希>
        //   ② 之后用网页登录重登（探活补上了 userId）       → 算出 user:880281 的 id
        //   ⇒ 两条记录，且**①的 RT 已被②这次登录轮换掉** ⇒ 旧那条永久刷不动
        //   ```
        // 症状是「列表里两个自己，一个还在被转发、另一个永远报登录过期」。
        // 这一层的判据是「**旧记录缺 userId、本次拿到了 userId**，且本地区只有
        // 它一条这样的记录」：
        //   · 「唯一候选」把风险压到与第 3 层同一量级 —— 有多条无法区分的记录时
        //     宁可不认领（多一条重复账号，用户可以手删）也不猜着合并（并错人，
        //     凭证串位，不可逆）；
        //   · 只对**会轮换旧 RT 的写入路径**（[`is_supersede_source`]）生效：手工
        //     粘贴一份新快照不该去顶掉别人的记录（那种场景下两条并存是正确的）。
        //
        // 令牌比较**只做相等判定、不落日志**（SKILL 2.2）。
        let incoming_stable = credentials::stable_identity(&access_token);
        let records = self.records_for_provider(&guard, provider_id);
        let same_identity = records
            .iter()
            .find(|record| {
                let field = |key: &str| {
                    record
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or("")
                };
                let record_user = field("userId");
                if !user_id.is_empty() && record_user == user_id {
                    return true;
                }
                let record_stable = credentials::stable_identity(field("accessToken"));
                if !incoming_stable.is_empty()
                    && incoming_stable == record_stable
                {
                    return true;
                }
                // 第 3 层的前提：双方都没有稳定身份
                if !incoming_stable.is_empty() || !record_stable.is_empty() {
                    return false;
                }
                if !refresh_token.is_empty() && field("refreshToken") == refresh_token {
                    return true;
                }
                field("accessToken") == access_token
            })
            .or_else(|| {
                // 第 4 层：升级路径的认领（见上方长注释）。三个前提全部满足才动：
                // ① 本次拿到了 userId；② 写入路径会轮换旧 RT；③ 本地区
                //    「没有 userId」的记录**恰好一条**。
                if user_id.is_empty() || !is_supersede_source(source) {
                    return None;
                }
                let mut orphans = records.iter().filter(|record| {
                    record
                        .get("userId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or("")
                        .is_empty()
                });
                let candidate = orphans.next()?;
                if orphans.next().is_some() {
                    // 多条无法区分的记录 —— 不猜（见注释末段）
                    let orphan_count = records
                        .iter()
                        .filter(|record| {
                            record
                                .get("userId")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .unwrap_or("")
                                .is_empty()
                        })
                        .count();
                    crate::server::logging::log(
                        "[Accounts]",
                        &format!(
                            "⚠️ MiniMax Code {}：本地区有 {orphan_count} 条没有 userId 的记录，\
                             本次登录无法判定该认领哪一条，按新账号保存（请手工删除多余的旧记录）",
                            region.label()
                        ),
                    );
                    return None;
                }
                Some(candidate)
            });
        let id = same_identity
            .as_ref()
            .map(|record| record.id().to_string())
            .unwrap_or(computed_id);

        // 先落到一个具名绑定再取引用：`match` 里直接 `.as_ref()` 会借用临时值，
        // 而它要活到函数末尾（Rust 的临时值生命周期只到语句结束）。
        let by_id = self.record_by_id(&guard, &id);
        let existing = match same_identity {
            Some(record) => Some(record),
            None => by_id.as_ref(),
        };
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != provider_id {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一个\
                         身份的 MiniMax Code {}账号（请先处理那个账号）",
                        region.label()
                    ),
                    400,
                ));
            }
        }
        // 备注名兜底：显式传入 → 既有记录里的备注名 → 「MiniMax Code <尾号>」
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                format!(
                    "MiniMax Code {}{}",
                    if region == Region::Cn { String::new() } else { format!("{} ", region.label()) },
                    token_tail_of(&access_token)
                )
            });
        let priority = match existing {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(provider_id.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing,
        );
        record.insert("accessToken".to_string(), Value::String(access_token.clone()));
        if !refresh_token.is_empty() {
            record.insert(
                "refreshToken".to_string(),
                Value::String(refresh_token.clone()),
            );
        } else if let Some(existing) = existing {
            // 手工补令牌时用户往往只给访问令牌 —— 别把既有刷新令牌丢掉
            // （丢了等于把「自动续期」这个能力从账号上摘掉）
            if let Some(previous) = existing.get("refreshToken") {
                record.insert("refreshToken".to_string(), previous.clone());
            }
        }
        if expires_at > 0 {
            record.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        if !user_id.is_empty() {
            record.insert("userId".to_string(), Value::String(user_id));
        }
        record.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&access_token)),
        );
        record.insert("source".to_string(), Value::String(source.to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(
                existing.map(StoredAccount::enabled).unwrap_or(true),
            ),
        );
        record.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .map(StoredAccount::added_at)
                    .filter(|value| *value != 0)
                    .unwrap_or(now),
            ),
        );
        record.insert("updatedAt".to_string(), Value::from(now));
        // 未知字段全量保留（用户手工加过的字段不能因为一次「更新账号」丢掉）
        let mut merged = record;
        if let Some(existing) = existing {
            for (key, value) in existing.fields() {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let saved = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!(
                "✅ MiniMax Code {}账号已保存: {record_name}（优先级 {priority}，令牌 {}）",
                region.label(),
                credentials::masked(&access_token)
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// 续期后把新的一对令牌写回同一条记录。
    ///
    /// ── 为什么单独一个方法而不是复用 `add_minimax_account` ──────────
    /// 续期拿到的新令牌里**没有** `userId`/`expiresAt` 之外的上下文，
    /// 而 `add_minimax_account` 会按身份键重算 id —— 万一 JWT 解析失败，
    /// 就会算出另一个 id 并**新增**一条账号（旧账号还留着，用户看到两个自己）。
    /// 这里按已知 id 精确更新，语义上也不该走「添加」。
    ///
    /// `refresh_token` 为空时保留原值（上游偶尔只回 `access_token`）。
    pub fn update_minimax_tokens(
        &self,
        account_id: &str,
        access_token: &str,
        refresh_token: &str,
        expires_at: i64,
    ) -> Result<(), AccountStoreError> {
        if account_id.trim().is_empty() {
            return Err(AccountStoreError::new("缺少账号 id", 400));
        }
        if access_token.trim().is_empty() {
            return Err(AccountStoreError::new("续期结果里没有访问令牌", 400));
        }
        let guard = self.guard();
        let Some(existing) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::new(
                format!("MiniMax Code 账号 {account_id} 不存在"),
                404,
            ));
        };
        // 两家都认：这是「按已知 id 回写令牌」，地区不影响它该不该写
        // （账号 id 已经唯一确定了是哪一家）。用 `Region::from_provider_id`
        // 而不是 `starts_with("minimax")` —— 后者会把国际版误算成国内版。
        if Region::from_provider_id(&existing.provider()).is_none() {
            return Err(AccountStoreError::new(
                format!("账号 {account_id} 不属于 MiniMax Code"),
                400,
            ));
        }
        let mut fields = existing.fields().clone();
        let now = logging::now_ms();
        fields.insert(
            "accessToken".to_string(),
            Value::String(access_token.trim().to_string()),
        );
        if !refresh_token.trim().is_empty() {
            fields.insert(
                "refreshToken".to_string(),
                Value::String(refresh_token.trim().to_string()),
            );
        }
        if expires_at > 0 {
            fields.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        fields.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(access_token)),
        );
        fields.insert("updatedAt".to_string(), Value::from(now));
        let saved = StoredAccount::from_map(fields);
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        Ok(())
    }

    /// 摘掉账号上的刷新令牌（续期响应里**没有**新的一枚时调用）。
    ///
    /// ── 为什么必须显式摘（2026-10-22 实测的根因）──────────────────
    /// `update_minimax_tokens` 对空 `refresh_token` 的语义是「保留原值」——
    /// 那是给「上游偶尔漏回」留的余地。但本家的 RT **单次有效**：续期请求一发出，
    /// 记录里那枚就作废了。上游这次没回新的 ⇒ 我们手里**一枚可用的 RT 都没有**，
    /// 而记录里还留着旧字符串 ⇒ 界面上 `canRefresh` 仍是 true、下一次续期拿着
    /// 死令牌去打上游拿 `invalid_grant`。用户看到的是「自动续期坏了」，真相是
    /// 「根本没有刷新令牌了，只能重新登录」。
    ///
    /// 摘掉之后 `canRefresh` 变为 false，界面与维护任务都会如实说「请重新登录」，
    /// 而不是每 5 分钟刷一次 `invalid_grant`。
    ///
    /// 返回 `Ok(true)` 表示这次真的摘掉了（幂等：本来就没有 → `Ok(false)`）。
    pub fn clear_minimax_refresh_token(
        &self,
        account_id: &str,
    ) -> Result<bool, AccountStoreError> {
        if account_id.trim().is_empty() {
            return Err(AccountStoreError::new("缺少账号 id", 400));
        }
        let guard = self.guard();
        let Some(existing) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::new(
                format!("MiniMax Code 账号 {account_id} 不存在"),
                404,
            ));
        };
        if Region::from_provider_id(&existing.provider()).is_none() {
            return Err(AccountStoreError::new(
                format!("账号 {account_id} 不属于 MiniMax Code"),
                400,
            ));
        }
        let had = existing
            .get("refreshToken")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !had {
            return Ok(false);
        }
        let mut fields = existing.fields().clone();
        // **删键而不是写空串**：`canRefresh` / `credentials_expiring` 都按
        // 「取到非空字符串」判定，写空串也满足，但留着空串会让后续读到
        // 「这是有意的空值」而多做一次判断 —— 删掉语义最干净。
        fields.remove("refreshToken");
        fields.insert(
            "updatedAt".to_string(),
            Value::from(logging::now_ms()),
        );
        let saved = StoredAccount::from_map(fields);
        // 用 `update_in_place` 而不是 `put`：后者是「删一行再插一行」，
        // 会让这条记录在分组队列里被当成**新账号**（`queue_needs_normalize`
        // 据此重排整队）—— 而这里只是摘掉一个凭证字段，账号位置必须原地不动。
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &saved))?;
        Ok(true)
    }

    /// MiniMax Code 账号的**公开形态**（进 HTTP 响应）：去掉两个令牌本体。
    ///
    /// 安全边界（SKILL 2.2）：`accessToken` 与 `refreshToken` 都必须移除 ——
    /// 后者是长期续期凭据，泄漏它等于把账号的续期能力一起送出去。
    /// `userId` 保留：它是登录身份，但不像 openid 那样能单独换令牌，
    /// 而界面「标识」列要用它（见 `accounts-domain.ts` 的 `identifier`）。
    pub fn to_minimax_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.remove("refreshToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert(
                "hasKey".to_string(),
                Value::Bool(
                    record
                        .get("accessToken")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                ),
            );
            // 界面据此决定「刷新 Token」按钮是否可用（没有刷新令牌就置灰）
            object.insert(
                "canRefresh".to_string(),
                Value::Bool(
                    record
                        .get("refreshToken")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                ),
            );
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **回归护栏**：国内版的 id 必须与拆家前**逐字相同**。
    ///
    /// 已接入那批账号的 id 就是 `minimax-<sha256("minimax:account-id:v1\0" + "user:u42")[..12]>`；
    /// 谁把地区塞进哈希输入（或改掉那个域前缀），同一身份重新登录就会算出另一个
    /// id ⇒ 新增记录而不是更新，用户看到两个自己、旧的那条再也刷不动。
    ///
    /// 这条测试是「存量兼容」的唯一守卫：它失败 = 有老账号会被抛弃。
    #[test]
    fn cn_id_matches_the_pre_split_formula() {
        let mut hasher = Sha256::new();
        hasher.update(b"minimax:account-id:v1\0");
        hasher.update(b"user:u42");
        let expected = format!("minimax-{}", &format!("{:x}", hasher.finalize())[..12]);
        assert_eq!(account_id_for(Region::Cn, "user:u42"), expected);
        // 前缀正确：不带地区段（`minimax-` 而不是 `minimax-cn-`）
        assert!(account_id_for(Region::Cn, "user:u42").starts_with("minimax-"));
        assert!(!account_id_for(Region::Cn, "user:u42").starts_with("minimax-intl-"));
    }

    /// 同一个身份在两地区必须是两个不同 id（前缀差异足够，不需要哈希参与）。
    #[test]
    fn the_two_regions_never_collide_on_the_same_identity() {
        let cn = account_id_for(Region::Cn, "user:same");
        let intl = account_id_for(Region::Intl, "user:same");
        assert_ne!(cn, intl);
        assert_eq!(intl, format!("minimax-intl-{}", &cn["minimax-".len()..]));
    }

    /// **回归护栏**：同一个身份经新公式算出的 id 必须**与令牌无关**。
    ///
    /// 这是「重新添加同一个账号被识别成不同账号」那条缺陷的核心：
    /// 旧实现落不到稳定身份时退回 `sha256(accessToken)`，令牌一轮换（每小时）
    /// 就多一条账号。这里钉的是「身份相同 ⇒ id 相同」，不给令牌留入口。
    #[test]
    fn identity_is_independent_of_the_token() {
        let identity = format!("stable:{}", credentials::stable_identity("token-880281"));
        assert_eq!(identity, "stable:token-880281");
        let a = account_id_for(Region::Cn, &identity);
        let b = account_id_for(Region::Cn, &identity);
        assert_eq!(a, b, "同一身份两次必须算出同一个 id");
        // 两个不同的人必须算出不同 id（否则会互相覆盖，比重复账号更糟）
        let other = account_id_for(Region::Cn, "stable:token-545178");
        assert_ne!(a, other);
    }

    /// ── 认领的回归护栏（2026-10-22）─────────────────────────────
    /// 这一组用例**必须真落库**：认领判据读的是「本地区现有记录」，纯函数测不到。
    /// 库用 `test_temp`（临时目录 + 结束即删），不碰本机账号库。
    mod claim_existing_record {
        use serde_json::json;

        use super::super::{is_supersede_source, AccountStore};
        use crate::server::core::providers::minimax::Region;

        /// 线上真实形态的不透明令牌（60 字符、无 JWT 段、挖不到身份）。
        const OPAQUE: &str = "mmoat_pmqO3U8yZk1sVb7Qx2Ld4Rt6Nc8Wh0Ej3Fu5Gi7Hk9Lm";

        fn store(label: &str) -> AccountStore {
            let (db, guard) = crate::server::db::test_temp::TempDb::open(label);
            // 守卫随库一起活到用例结束（Drop 时删文件），这里直接泄给整个用例
            std::mem::forget(guard);
            AccountStore::with_db(Some(db))
        }

        fn count(store: &AccountStore) -> usize {
            store
                .list_accounts()
                .get("accounts")
                .and_then(serde_json::Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0)
        }

        /// `{"user":{"id":…}}` 的最小 JWT 形态（只求 `stable_identity` 认得出）。
        fn jwt_with_user(user: &str) -> String {
            let payload = format!(r#"{{"exp":2000000000,"user":{{"id":"{user}"}}}}"#);
            let encoded = base64_url(payload.as_bytes());
            format!("h.{encoded}.s")
        }

        fn base64_url(bytes: &[u8]) -> String {
            const TABLE: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut buffer = [0u8; 3];
                buffer[..chunk.len()].copy_from_slice(chunk);
                let value =
                    ((buffer[0] as u32) << 16) | ((buffer[1] as u32) << 8) | buffer[2] as u32;
                out.push(TABLE[(value >> 18) as usize & 63] as char);
                out.push(TABLE[(value >> 12) as usize & 63] as char);
                if chunk.len() > 1 {
                    out.push(TABLE[(value >> 6) as usize & 63] as char);
                }
                if chunk.len() > 2 {
                    out.push(TABLE[value as usize & 63] as char);
                }
            }
            out
        }

        /// **缺陷复现 → 修复护栏**：先粘贴不透明令牌（只有哈希档可用），再网页登录
        /// （拿到 `userId`）。必须认领旧记录，而不是新增第二条 —— 旧记录的 RT 已被
        /// 这次登录轮换掉，留着只会「永远报登录过期」。
        #[test]
        fn a_relogin_claims_the_record_that_has_no_user_id_yet() {
            let store = store("minimax-claim-relogin");
            let first = store
                .add_minimax_account(
                    Region::Cn,
                    &json!({ "accessToken": OPAQUE, "refreshToken": "mmort_old" }),
                    None,
                    "manual",
                )
                .expect("首次添加应成功");
            let first_id = first["id"].as_str().unwrap_or_default().to_string();

            let second = store
                .add_minimax_account(
                    Region::Cn,
                    &json!({
                        "accessToken": jwt_with_user("880281"),
                        "refreshToken": "mmort_new",
                        "userId": "880281",
                    }),
                    None,
                    "web",
                )
                .expect("重登应成功");
            let second_id = second["id"].as_str().unwrap_or_default().to_string();

            assert_eq!(count(&store), 1, "同一个账号重登后不该变成两条");
            assert_eq!(first_id, second_id, "重登必须认领既有记录");
            // 认领后新凭证落在**旧 id**上，且身份补全了
            assert_eq!(second["userId"].as_str(), Some("880281"));
            assert!(
                store
                    .minimax_account_record(Region::Cn, &first_id)
                    .and_then(|record| record.get("refreshToken").cloned())
                    .and_then(|value| value.as_str().map(str::to_string))
                    .as_deref()
                    == Some("mmort_new"),
                "刷新令牌必须换成新的那枚"
            );
        }

        /// 反向：手工粘贴**不**认领别人的记录。
        ///
        /// 用户可能在另一台机器上持有一份仍然有效的授权 —— 两条并存是对的。
        /// 让它去顶掉既有记录会静默改写别人的账号（凭证串位，不可逆）。
        #[test]
        fn a_manual_paste_never_hijacks_another_record() {
            let store = store("minimax-manual-no-hijack");
            store
                .add_minimax_account(
                    Region::Cn,
                    &json!({ "accessToken": OPAQUE, "refreshToken": "mmort_old" }),
                    None,
                    "manual",
                )
                .expect("首次添加应成功");
            store
                .add_minimax_account(
                    Region::Cn,
                    &json!({
                        "accessToken": jwt_with_user("545178"),
                        "refreshToken": "mmort_other",
                        "userId": "545178",
                    }),
                    None,
                    "manual",
                )
                .expect("第二条应成功");
            assert_eq!(count(&store), 2, "手工粘贴是「再加一个」，不是「替换」");
        }

        /// 多条候选时**不猜**：宁可多一条重复账号（用户能删），也不并错人。
        #[test]
        fn several_orphans_block_the_claim() {
            let store = store("minimax-orphans-ambiguous");
            for (index, token) in ["mmoat_orphan_one_token_value", "mmoat_orphan_two_token_value"]
                .iter()
                .enumerate()
            {
                store
                    .add_minimax_account(
                        Region::Cn,
                        &json!({ "accessToken": token, "refreshToken": format!("mmort_{index}") }),
                        None,
                        "manual",
                    )
                    .expect("孤儿记录应能落库");
            }
            assert_eq!(count(&store), 2);
            store
                .add_minimax_account(
                    Region::Cn,
                    &json!({
                        "accessToken": jwt_with_user("880281"),
                        "refreshToken": "mmort_new",
                        "userId": "880281",
                    }),
                    None,
                    "web",
                )
                .expect("重登应成功");
            assert_eq!(count(&store), 3, "候选不唯一时必须新增，不许猜");
        }

        /// 两个地区各自认领，互不干扰（provider 已由 `records_for_provider` 收窄）。
        #[test]
        fn claiming_stays_inside_one_region() {
            let store = store("minimax-claim-region");
            store
                .add_minimax_account(
                    Region::Intl,
                    &json!({ "accessToken": OPAQUE, "refreshToken": "mmort_intl" }),
                    None,
                    "manual",
                )
                .expect("国际版应能落库");
            store
                .add_minimax_account(
                    Region::Cn,
                    &json!({
                        "accessToken": jwt_with_user("880281"),
                        "refreshToken": "mmort_new",
                        "userId": "880281",
                    }),
                    None,
                    "web",
                )
                .expect("国内版重登应成功");
            assert_eq!(count(&store), 2, "国内版不能认领国际版那条记录");
        }

        /// 判据本身：只有会轮换旧 RT 的路径才认领。
        #[test]
        fn only_superseding_paths_may_claim() {
            assert!(is_supersede_source("web"));
            assert!(is_supersede_source("desktop"));
            assert!(!is_supersede_source("manual"));
            assert!(!is_supersede_source("file"));
            assert!(!is_supersede_source(""));
        }
    }
}
