//! 凭证回写、限额标记与启动迁移（从 store.rs 拆出，单文件行数约定）。
//!
//!   update_account_tokens  账号刷新成功后回写新 token
//!   mark_rate_limited      记录账号×模型的限额状态（上游 429 / code 6004）
//!   clear_rate_limit       清除某模型的限额记录
//!   import_legacy_session  旧版单账号 auth.json 迁移（仅账号列表为空时）
//!   clear_all_rate_limits  清除账号全部模型的限额记录（账号页「全部清除」）
//!   migrate_startup        启动时一次性数据迁移（provider 惰性补齐 + 优先级全局化
//!                          + 去重 + Cline 拆池改名）
//!   renumber_globally      优先级重编号内核（**全局一条队列**，migrate_startup 私有）
//!
//! 这些方法都是「低频、写入型」操作：放在单独文件是为了让 CRUD 文件聚焦在
//! 用户可见的账号操作上，避免两者混在一起时看不清哪些是启动期做的事。
//!
//! ── 写入粒度：凭证/限额回写只碰一行（本切片的改造）──────────
//! 本文件里五个「改一条记录」的写入方法（token 回写、限额标记、清除某模型
//! 限额、清除全部限额、签到）在改造前各自「读全量 → 改一条 → 写全量」，而
//! 它们恰好是**最频繁**的账号写入 —— 每次 token 刷新、每个 429、每次签到
//! 都会触发。现在一律走 `record_by_id` + `sql::update_in_place`：
//! 只读一行、只写一行。
//!
//! ── 几条迁移为什么合成一次写入 ──────────────────────────────
//! `provider` 惰性补齐（架构文档 §3.2）、优先级去重（原 `migrate_priorities`）
//! 与 Cline 拆池改名都在启动时、都在同一份账号集合上改字段。若各自读一次、
//! 写一次，后面那条迁移读到的就是前面写过的库 —— 结果没错，但会多几次全表
//! 扫描与写入，而且「补 provider 时把已经去重过的优先级又读成并列」这类交叉
//! 影响不容易看清。因此它们由 `migrate_startup` 串起来：**一次读、一次改、
//! 一次写**。
//!
//! 拆池改名会**改主键**（`id` 从 `cline-…` 变成 `cline-free-…`）。改名本身
//! 只改内存里的 state（`migrate_cline_accounts` 不碰库），落库统一交给 `save`：
//! `save_state` 是**按 id 比对**的 —— 新 id 在库里找不到对应行，走 INSERT；
//! 旧 id 在目标状态里找不到，走 DELETE。于是改名天然表现为「删旧行 + 插新行」，
//! 不需要为它专门写一条改主键的 UPDATE。
//! 整批仍在**一个事务**里（`save_state` 自己开）：编号与改名要么全成、
//! 要么全不成 —— 中断留下「部分账号改了 id、部分没改」的中间态会让同一批
//! 账号散在两个 provider 组里。

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::account_store::priority::{
    normalize_priority_value, renumber_consecutively, PriorityAssignment,
};
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{
    json_number, AccountState, StoredAccount, PRIORITY_SCOPE_GLOBAL,
};
use crate::server::core::account_store::store::AccountStore;
use crate::server::core::account_store::store_util::{token_tail_of, truncate_chars};
use crate::server::core::providers::{provider_index, DEFAULT_PROVIDER_ID};
use crate::server::logging;

/// ── 限额冷却的参数（2026-11 参考 magpie / CLIProxyAPI 的做法）──────────
/// 上游给的恢复时间再短也不低于它：亚秒级的 `Retry-After` 会让账号在同一轮
/// 请求里就解封、被重新选中、再撞一次（CLIProxyAPI #5265 的重试风暴）。
const RATE_LIMIT_FLOOR_MS: f64 = 30_000.0;
/// 上游没给恢复时间时的兜底冷却，也是退避的基数（沿用旧行为的 10 分钟，
/// 首撞仍是它；连续撞按 [`RATE_LIMIT_CAP_MS`] 封顶增长）。
const RATE_LIMIT_FALLBACK_MS: f64 = 10.0 * 60_000.0;
/// 退避上限：连续撞时冷却的最长值。
const RATE_LIMIT_CAP_MS: f64 = 30.0 * 60_000.0;
/// 遗忘窗口：冷却结束后这么久内又撞算「同一次」连击（次数 +1）；更久之后
/// 再撞算新的一轮，从头计（避免零星抖动累积成大退避）。
const RATE_LIMIT_FORGET_MS: f64 = 30.0 * 60_000.0;
/// 上游**明确给的**恢复时刻的宽松上限：防上游手滑给出离谱值（magpie 的
/// `longestQuota` 同理）。它只用于「上游明确值」，兜底退避另有更低的上限。
const RATE_LIMIT_GIVEN_MAX_MS: f64 = 8.0 * 24.0 * 60.0 * 60_000.0;

impl AccountStore {
    // ─── 凭证回写与限额标记 ──────────────────────────────────

    /// 标记 / 清除账号的「需要重新登录」状态（`needsRelogin` + `needsReloginReason`）。
    ///
    /// ── 这个标记解决什么（issue #215）────────────────────────────
    /// 上游把会话连同 refresh token 一起注销（12153）之后，网关侧再刷多少次都
    /// 不会好。没有这个标记时，凭证维护每轮重试、每轮失败，日志刷屏，而用户
    /// 始终不知道「该去重新登录」。标记之后：
    ///   - 凭证维护跳过它（不再白打上游）；
    ///   - 界面显示「需要重新登录」而不是「刷新失败」；
    ///   - 用户重新登录成功时清掉（见 `AuthService::clear_needs_relogin`）。
    ///
    /// `reason` 给 `None` = 清除标记。账号不存在返回 false。
    pub fn set_needs_relogin(&self, id: &str, reason: Option<&str>) -> bool {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        match reason.filter(|text| !text.is_empty()) {
            Some(reason) => {
                record.set("needsRelogin", Value::Bool(true));
                record.set("needsReloginReason", Value::String(reason.to_string()));
            }
            None => {
                record.remove("needsRelogin");
                record.remove("needsReloginReason");
            }
        }
        record.set_updated_at(logging::now_ms());
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    pub fn suspend_upstream_account(&self, id: &str, reason: &str) -> Result<(), String> {
        let guard = self.guard();
        let mut record = self.record_by_id(&guard, id).ok_or_else(|| "账号不存在".to_string())?;
        record.set_enabled(false);
        record.set("upstreamBlockedReason", Value::String(truncate_chars(reason, 300)));
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record)).map_err(|error| error.to_string())
    }

    pub fn cooldown_upstream_account(&self, id: &str, until: i64, reason: &str) -> Result<(), String> {
        let guard = self.guard();
        let mut record = self.record_by_id(&guard, id).ok_or_else(|| "账号不存在".to_string())?;
        if until <= logging::now_ms() { return Ok(()); }
        record.set("upstreamCooldownUntil", Value::from(until));
        record.set("upstreamBlockedReason", Value::String(truncate_chars(reason, 300)));
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record)).map_err(|error| error.to_string())
    }

    /// 账号是否被标记为「需要重新登录」（凭证维护据此跳过）。
    pub fn needs_relogin(&self, id: &str) -> bool {
        self.read_record(id).and_then(|record| record.get("needsRelogin").and_then(Value::as_bool))
            .unwrap_or(false)
    }

    /// 记一次「上游回了 12153」并返回**累计次数**（账号不存在返回 0）。
    ///
    /// ── 为什么不是一次就判死（issue #215 评论区的实测教训）────────
    /// 参考项目（workbuddy2api-panel）在自己的注释里记了这条：旧实现「一次 12153
    /// 即 Disable」，事后排查发现**13 个被禁用的号全部 refresh 成功** —— 它们都是
    /// 误判的受害者。12153 会被网络抖动、上游闪断、refresh 竞态临时触发，一次失败
    /// 就永久杀号会误杀健康账号。
    ///
    /// 因此这里只累计、不判定；阈值判断留在调用方（`AuthService`），
    /// 达阈值才标 `needsRelogin`。
    pub fn note_refresh_token_invalid(&self, id: &str) -> i64 {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return 0;
        };
        let streak = record
            .get("refreshTokenInvalidStreak")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .saturating_add(1);
        record.set("refreshTokenInvalidStreak", Value::from(streak));
        record.set_updated_at(logging::now_ms());
        let _ = self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record));
        streak
    }

    /// 清掉 12153 连续计数（刷新成功 = 账号被证明还活着）。
    ///
    /// 与 `note_refresh_token_invalid` 成对：只要有一次刷新成功，就说明
    /// 之前那些 12153 都是抖动，计数归零，避免「隔几天抖一次」慢慢攒到阈值。
    pub fn clear_refresh_token_invalid(&self, id: &str) {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return;
        };
        if record.get("refreshTokenInvalidStreak").is_none() {
            return;
        }
        record.remove("refreshTokenInvalidStreak");
        record.set_updated_at(logging::now_ms());
        let _ = self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record));
    }

    /// 账号刷新成功后回写新 token（账号不存在返回 false）
    pub fn update_account_tokens(
        &self,
        id: &str,
        access_token: Option<&str>,
        refresh_token: Option<&str>,
        expires_at: Option<f64>,
        refresh_expires_at: Option<f64>,
    ) -> bool {
        let _guard = self.guard();
        // 只读目标那一行：这是刷新链路每次都走的写入（token 一到期就触发），
        // 改造前它要把全部账号的 JSON 解析一遍只为改其中一条
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        if let Some(token) = access_token.filter(|value| !value.is_empty()) {
            record.set("accessToken", Value::String(token.to_string()));
            record.set("tokenTail", Value::String(token_tail_of(token)));
        }
        if let Some(token) = refresh_token.filter(|value| !value.is_empty()) {
            record.set("refreshToken", Value::String(token.to_string()));
        }
        if let Some(value) = expires_at.filter(|value| *value > 0.0) {
            record.set("expiresAt", json_number(value));
        }
        if let Some(value) = refresh_expires_at.filter(|value| *value > 0.0) {
            record.set("refreshExpiresAt", json_number(value));
        }
        record.set_updated_at(logging::now_ms());
        // 原地更新：凭证回写不改账号在列表里的位置（与旧实现「在下标上改字段」一致）
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    /// 记录账号对某模型的限额状态（上游 429 / code 6004）。
    ///
    /// ── 冷却时长怎么定（四条纪律，2026-11 参考 magpie / CLIProxyAPI）──────
    /// 1. **只延不缩**：新冷却不得缩短一条还没结束的旧冷却 —— 一条 1 分钟的 429
    ///    冷却盖掉片刻前半小时的额度冷却，账号一解封就被重新选中、再撞、再落条
    ///    短冷却，一轮一轮循环（magpie #3289 的原形，也是本仓 #451 的症状）。
    ///    唯一例外见第 4 条。
    /// 2. **下限**：上游给的恢复时间再短也不低于 [`RATE_LIMIT_FLOOR_MS`] ——
    ///    亚秒级的 `Retry-After` 会让账号在同一轮请求里就解封（CLIProxyAPI #5265）。
    /// 3. **退避**：上游没给恢复时间时按连击次数指数退避（[`RATE_LIMIT_FALLBACK_MS`]
    ///    ×2ⁿ，封顶 [`RATE_LIMIT_CAP_MS`]）。连击 = 上一条冷却结束后
    ///    [`RATE_LIMIT_FORGET_MS`] 内又撞；更久之后再撞算新的一轮，从头计。
    /// 4. **来源优先**：上游**明确给的**恢复时刻优先于**我们猜的**（退避 / 兜底）
    ///    —— 猜测只是上游话语的替身，上游真给了就以它为准（哪怕更早）。
    ///
    /// resetAt 为恢复时间戳；缺失时按第 3 条退避（首撞仍是旧的 10 分钟兜底）。
    ///
    /// ── 冷却键的最终形态：provider × 账号 × 模型（Agent2API W2b-T3 确认）──
    /// 落盘位置是 `accounts[i].rateLimits[model] = {status, code, resetAt, message, at, strikes, guessed}`，
    /// 即**冷却键挂在账号记录内部**，而每条账号记录只属于一个 provider
    /// （`record.provider()` 是单值）。所以「provider 维度」已经天然成立，
    /// **不需要改数据结构**：`accounts[i]` 这一个下标就同时确定了 provider 与账号，
    /// 加上 `rateLimits` 里的模型名，键即 `(provider, account_id, model)`。
    /// （`strikes` / `guessed` 是本次新增的两个内部字段：连击次数、本次冷却是否
    /// 来自「上游没给时间」的猜测。旧记录没有它们时按 0 / false 读，即「首撞、
    /// 非猜测」，退化为只延不缩 + 10 分钟兜底。）
    ///
    /// 选路时候选集合跨提供商（全局队列），但限额判定仍是「这条账号记录对这个
    /// 模型」—— 记录内的键天然不会串到别的账号上。
    pub fn mark_rate_limited(
        &self,
        id: &str,
        model: &str,
        status: i64,
        code: Option<i64>,
        reset_at: Option<f64>,
        message: &str,
    ) -> Option<Value> {
        let _guard = self.guard();
        // 只读目标那一行。这一条在改造前是全量写盘里**最亏**的一个：一次 429
        // 就把 20 条账号记录整份重写一遍，而它只是给其中一条加一个 rateLimits 键。
        let mut record = self.record_by_id(&_guard, id)?;
        let now = logging::now_ms();
        let now_f = now as f64;
        let prev = record
            .get("rateLimits")
            .and_then(|limits| limits.get(model))
            .cloned();
        // 上游**明确给的**恢复时刻（晚于当下才算数；过去的时刻没有意义）
        let given = reset_at.filter(|value| value.is_finite() && *value > now_f);
        // 上一条冷却及其连击计数（旧记录缺这两个字段时按 0 / false 读）
        let prev_reset_at = prev
            .as_ref()
            .and_then(|entry| entry.get("resetAt"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let prev_guess = prev
            .as_ref()
            .and_then(|entry| entry.get("guessed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // 连击：上一条冷却（还没结束或刚结束不久）之后 FORGET 窗口内又撞 → +1
        let strikes = if prev.is_some() && now_f - prev_reset_at <= RATE_LIMIT_FORGET_MS {
            prev.as_ref()
                .and_then(|entry| entry.get("strikes"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            1
        };
        // 本次冷却的结束时刻（未与旧冷却合并前）
        let fresh = match given {
            // 上游明确给的：只做下限与宽松上限，其余尊重上游
            Some(given_at) => {
                given_at.clamp(now_f + RATE_LIMIT_FLOOR_MS, now_f + RATE_LIMIT_GIVEN_MAX_MS)
            }
            // 没给：指数退避（2ⁿ 用不着太深，30 步已远超上限）
            None => {
                let steps = (strikes.saturating_sub(1)).min(30) as i32;
                let backoff = RATE_LIMIT_FALLBACK_MS * 2f64.powi(steps);
                now_f + backoff.clamp(RATE_LIMIT_FLOOR_MS, RATE_LIMIT_CAP_MS)
            }
        };
        // 只延不缩（第 1 条），除非这次是上游明确值、而旧那条只是我们猜的
        // （第 4 条：上游的话语取代猜测的替身，哪怕更早）
        let until = if given.is_some() && prev_guess {
            fresh
        } else {
            fresh.max(prev_reset_at)
        };
        let entry = json!({
            "status": if status == 0 { 429 } else { status },
            "code": code.map(Value::from).unwrap_or(Value::Null),
            "resetAt": json_number(until),
            "message": truncate_chars(message, 300),
            "at": now,
            "strikes": strikes,
            "guessed": given.is_none(),
        });
        let mut limits = match record.get("rateLimits") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        limits.insert(model.to_string(), entry.clone());
        record.set("rateLimits", Value::Object(limits));
        record.set_updated_at(now);
        if self
            .with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_err()
        {
            return None;
        }
        Some(entry)
    }

    /// 清除账号对某模型的限额记录（该模型请求成功时调用）。
    ///
    /// 冷却键形态见 `mark_rate_limited`：键在账号记录内，provider 由账号唯一确定。
    pub fn clear_rate_limit(&self, id: &str, model: &str) -> bool {
        if !self.route_account(id).is_some_and(|account| {
            account.get("rateLimits").and_then(|limits| limits.get(model)).is_some()
        }) {
            return false;
        }
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        let Some(Value::Object(limits)) = record.get("rateLimits") else {
            return false;
        };
        if !limits.contains_key(model) {
            return false;
        }
        let mut limits = limits.clone();
        limits.remove(model);
        if limits.is_empty() {
            record.remove("rateLimits");
        } else {
            record.set("rateLimits", Value::Object(limits));
        }
        record.set_updated_at(logging::now_ms());
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    /// 清除账号**全部**模型的限额记录（账号页限流明细的「全部清除」）。
    /// 返回被清掉的模型数；账号不存在或本来就没有记录返回 0。
    pub fn clear_all_rate_limits(&self, id: &str) -> usize {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return 0;
        };
        let count = match record.get("rateLimits") {
            Some(Value::Object(limits)) => limits.len(),
            _ => 0,
        } + usize::from(record.get("upstreamCooldownUntil").is_some());
        if count == 0 {
            return 0;
        }
        record.remove("rateLimits");
        record.remove("upstreamCooldownUntil");
        if record.enabled() { record.remove("upstreamBlockedReason"); }
        record.set_updated_at(logging::now_ms());
        if self
            .with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_err()
        {
            return 0;
        }
        count
    }

    /// 刷新核对用的凭证指纹：`expiresAt` 与 `updatedAt`（毫秒）。
    ///
    /// `refresh_access_token` 返回 Ok 只说明「这次调用没报错」—— 适配器完全可能
    /// 什么都没刷到就返回成功（AutoClaw 实测连续 28 小时空转，issue #407）。
    /// 调用方在刷新前后各取一次这个指纹，两者都没推进就按空刷处理。
    ///
    /// 用两个字段而不是只看 `expiresAt`：过期时间在记录顶层不一定存在
    /// （小浣熊在 JWT 的 `exp`、部分家根本不写），而任何一次真正的写回都会
    /// 推进 `updatedAt`。
    pub fn credential_stamp(&self, id: &str) -> Option<(f64, f64)> {
        let _guard = self.guard();
        let record = self.record_by_id(&_guard, id)?;
        let expires_at = record
            .get("expiresAt")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        Some((expires_at, record.updated_at() as f64))
    }

    /// 一次性迁移：清掉**按请求名（映射别名）记下的限额键**（幂等，可重复调用）。
    ///
    /// ── 修的是什么（2026-09 的事故）─────────────────────────────
    /// 映射（`modelRules.mappings`）会在发送前把请求名改写成上游真名，而上游按
    /// **真名**记额度。旧实现在记账与判定两侧都用了请求名，于是：
    ///
    /// - 别名请求撞限额后，冷却写在别名键上（如 `gpt-5.6-luna`）；
    /// - 之后用真名（`deepseek-v4.1-flash`）请求时读的是真名键 —— 读不到那条
    ///   冷却，于是照样选中这个已经限额的账号，白撞一次 429；
    /// - 账号页如实展示那堆别名键，看起来像「这个账号对三个模型都限流了」，
    ///   其实只有一个额度（实测三个键的 `resetAt` 完全相同）。
    ///
    /// 修好读写两侧（`routing::CooldownKeys` / `payload::SendBody::wire_model`）
    /// 之后，这些**存量别名键**会变成永不命中的孤儿记录：判定侧不再读它们，
    /// 但它们仍会出现在账号页的「限流」列里（前端如实渲染后端给的键），
    /// 用户看到的就是一条点不掉的假限流。所以升级时必须清一次。
    ///
    /// ── 为什么是「删除」而不是「改写成真名」──────────────────────
    /// 看起来把别名键改名成真名更「保信息」，但那会**把冷却时间平白延长**：
    /// 别名键上的 `resetAt` 来自「用别名请求时」上游返回的那次 429，它与真名键
    /// 上那条记录本就是同一份额度的两种写法（实测 `resetAt` 完全一致）。改名会
    /// 在真名键已存在时二选一（丢一条或覆盖另一条），两种结果都不比删除更准。
    /// 而冷却只是「先别用这个账号」的短期建议 —— 删掉后最坏情况是**多试一次**
    /// 上游、再撞一次 429 重新落一条正确的冷却；比留一条永远错位的假记录好。
    ///
    /// ── 判据为什么必须**按账号所属的家**逐条算（这里最容易写错）──────
    /// 直觉写法是「键名出现在映射的 alias 表里 → 清掉」，但那会**误删合法的
    /// 冷却**：`alias` 允许与上游 id 同名（同名映射是主备的正式用法，见
    /// `model_rules` 模块头），所以 `deepseek-v4.1-flash` 既是 raccoon / cline
    /// 那边的映射别名，**同时**也是 WorkBuddy 的原生模型名。对着 WorkBuddy 的
    /// 账号看到这个键就删，会把一条真实的额度冷却抹掉 —— 那个账号之后会被反复
    /// 选中、反复撞 429，正是本次要修的那类症状反过来再犯一遍。
    ///
    /// 正确的判据是**转发侧那个函数本身**：把键名当成请求名，问
    /// `wire_target_for_provider(键, 该账号的家)` 会发出什么名字 ——
    ///
    /// - 发出的是**键名自己**（该家原生承载它，或存在同名映射）→ 它是真名，
    ///   这条冷却是合法的，留着；
    /// - 发出的是**别的名字**（键名被映射改写掉了）→ 键名不是上游真名，
    ///   这条冷却错位，清掉。
    ///
    /// 与判定 / 写入两侧同一套口径（都走 `wire_target_for_provider`），
    /// 不存在第三份「什么算真名」的实现。
    ///
    /// 不碰 `resetAt` 已过期的条目 —— 那些本来就不显示、也不参与判定，
    /// 让它们自然留着（与 `rate_limit_reset_at` 的「过期即未限额」同一口径，
    /// 不值得为它们多写一次库）。
    ///
    /// 返回被清理的 `(账号名, 键)` 摘要（供日志），没有可清理的返回空表。
    ///
    /// ── 为什么分两段（先快照判定、再持锁落盘）而不是全程持锁 ─────────
    /// 判定要调 `wire_target_for_provider`，它会扫各家的模型清单。虽然那些清单
    /// 当前都是**纯内存**的（`global_catalog` / 各家的 `models::list`），持着
    /// 账号锁去调它们眼下不会死锁 —— 但那是一条**隐式约定**：将来谁让某家的
    /// `list_models` 读一次账号库，这里就会变成「自己等自己」的硬死锁（`std`
    /// 的 Mutex 不可重入，且 release 是 panic=abort）。所以判定一律在锁外做完，
    /// 落盘只做「按 id 删几个键」这一件确定的事。
    pub fn migrate_rate_limit_keys(&self) -> Vec<String> {
        // ① 快照（内部自取自放锁）：公开形态里就有判定要的全部字段
        let snapshot = self.list_accounts();
        let accounts = crate::server::core::routing::accounts_of(&snapshot);
        if accounts.is_empty() {
            return Vec::new();
        }
        let now = logging::now_ms();
        // ② 锁外判定：这个键在该家是不是真名（判据见函数头）
        let mut plan: Vec<(String, String, Vec<String>)> = Vec::new();
        for account in &accounts {
            let Some(id) = account.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())
            else {
                continue;
            };
            let Some(Value::Object(limits)) = account.get("rateLimits") else {
                continue;
            };
            let provider = crate::server::core::routing::provider_of(account).to_string();
            let stale: Vec<String> = limits
                .iter()
                .filter(|(key, entry)| {
                    let future = entry
                        .get("resetAt")
                        .and_then(Value::as_f64)
                        .map(|value| value > now as f64)
                        .unwrap_or(false);
                    if !future || key.is_empty() {
                        return false;
                    }
                    // 键名当请求名问一次：发出去的不是它自己 → 它不是真名
                    let wire = crate::server::core::providers::catalog::wire_target_for_provider(
                        key,
                        &provider,
                        None,
                    )
                    .model;
                    !wire.eq_ignore_ascii_case(key)
                })
                .map(|(key, _)| key.clone())
                .collect();
            if !stale.is_empty() {
                let who = account
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(id)
                    .to_string();
                plan.push((id.to_string(), who, stale));
            }
        }
        if plan.is_empty() {
            return Vec::new();
        }
        // ③ 持锁落盘：只按计划删键，不再做任何解析
        let _guard = self.guard();
        let mut state = self.load(&_guard);
        let mut cleaned: Vec<String> = Vec::new();
        for record in state.accounts.iter_mut() {
            let Some((_, who, stale)) = plan.iter().find(|(id, _, _)| id == record.id()) else {
                continue;
            };
            // 克隆一份再改：`record.get` 是共享借用，后面 `record.remove` /
            // `record.set` 要可变借用，两者不能同时活着（与 `clear_rate_limit`
            // 同一处理：那里也是先 clone 出 limits 再写回）
            let Some(Value::Object(current)) = record.get("rateLimits") else {
                continue;
            };
            let mut limits = current.clone();
            let mut hit = false;
            for key in stale {
                // 只删计划里那几个；键可能已被别的路径清掉（快照与落盘之间有
                // 时间差），`remove` 的返回值正好用来确认真的删掉了
                if limits.remove(key).is_some() {
                    hit = true;
                    cleaned.push(format!("{who}: {key}"));
                }
            }
            if !hit {
                continue;
            }
            if limits.is_empty() {
                record.remove("rateLimits");
            } else {
                record.set("rateLimits", Value::Object(limits));
            }
            record.set_updated_at(now);
        }
        if cleaned.is_empty() {
            return cleaned;
        }
        if let Err(error) = self.save(&state, &_guard) {
            logging::log(
                "[Accounts]",
                &format!("❌ 限流键迁移落库失败（下次启动会重试）: {error}"),
            );
            return Vec::new();
        }
        cleaned
    }

    /// Trae 历史模型 ID 清理：账号上的限额冷却键、限制器条件里的模型名
    /// 按目录缓存给出的旧名映射改成原始 ID。
    ///
    /// 只处理 provider 为 `trae` 的记录；**只改映射里确实出现过的旧名**，
    /// 不按后缀猜——上游真名恰好以 `-solo` 结尾时不能被误伤。
    /// 一次读、一次差异写（与 `migrate_startup` 同一取向）；没有改动时不落库。
    pub fn migrate_trae_model_ids(&self, mapping: &HashMap<String, String>) -> usize {
        if mapping.is_empty() {
            return 0;
        }
        let _guard = self.guard();
        let mut state = self.load(&_guard);
        let now = logging::now_ms();
        let mut changed = 0usize;
        for record in state.accounts.iter_mut() {
            if !record.provider().eq_ignore_ascii_case("trae") {
                continue;
            }
            let mut touched = false;
            if let Some(Value::Object(limits)) = record.get("rateLimits").cloned() {
                let mut next = Map::new();
                for (key, value) in limits {
                    let base = mapping.get(&key.trim().to_ascii_lowercase());
                    if base.is_some() {
                        touched = true;
                    }
                    next.entry(base.cloned().unwrap_or(key)).or_insert(value);
                }
                if touched {
                    if next.is_empty() {
                        record.remove("rateLimits");
                    } else {
                        record.set("rateLimits", Value::Object(next));
                    }
                }
            }
            for field in ["limiterGroups", "limiters"] {
                let Some(mut value) = record.get(field).cloned() else {
                    continue;
                };
                if rewrite_models_arrays(&mut value, mapping) {
                    record.set(field, value);
                    touched = true;
                }
            }
            if touched {
                record.set_updated_at(now);
                changed += 1;
            }
        }
        if changed > 0 {
            if let Err(error) = self.save(&state, &_guard) {
                logging::log(
                    "[Accounts]",
                    &format!("❌ Trae 旧模型 ID 迁移落库失败（下次启动会重试）: {error}"),
                );
                return 0;
            }
        }
        changed
    }

    /// 记录账号**今天已签到**（登录页那枚按钮据此置灰）。
    ///
    /// `at` 是这次签到成功的毫秒时间戳。判定「今天签过没」由**读侧**按自然日比
    /// （`checkinAt` 落在今天即算签过），所以这里只负责如实落时间 —— 不需要
    /// 在写入时判重，跨过 0 点后同一个字段自然失效，不必任何定时器去重置。
    ///
    /// 与限额标记同族：都是「账号级的事实、低频写、失败不致命」。调用方是签到
    /// 链路，写盘失败**不**影响签到结果本身（上游那边积分已经领到了），所以返回
    /// bool 而不是 Result —— 调用方拿到 false 时最多记一条日志，绝不因此把
    /// 一次成功的签到报成失败。
    pub fn mark_checkin(&self, id: &str, at: i64) -> bool {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        record.set_checkin_at(at);
        // 注意**不**动 updatedAt：那是「记录被改过」的时间，会显示在账号页的
        // 「更新于 …」上；签到不是用户改配置那种「更新」，写进去会让这一栏
        // 在每天自动签到后集体跳动一次。
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    /// 记录成长中心**连登天数**（WorkBuddy 国内版；只读回读的结果）。
    ///
    /// 存下来是为了让签到中心 / 账号页不用每次打开都打一发上游
    /// （`GET /activity/growth/streak` 是只读查询，但几十个账号就是一串请求）。
    /// 与 [`Self::mark_checkin`] 同一口径：账号级事实、低频写、失败不致命
    /// （返回 bool，调用方最多记一条日志）。
    ///
    /// **不**动 `updatedAt`：连登是上游状态的回读，不是用户改配置。
    /// 值没变时不写库 —— 签到链每次都会回读，同一天内多次签到不该反复落盘。
    pub fn mark_streak_days(&self, id: &str, days: i64) -> bool {
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        let existing = record
            .fields()
            .get("streakDays")
            .and_then(Value::as_i64);
        if existing == Some(days) {
            return true;
        }
        record
            .fields_mut()
            .insert("streakDays".to_string(), Value::from(days));
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    /// 记下「这个账号的新手任务（一次性福利）已全部结算」的快照
    /// （账号记录上的 `onboardingSettled`；形状与读法见
    /// [`StoredAccount::set_onboarding_settled`] 与
    /// `crate::server::core::providers::onboarding_memory`）。
    ///
    /// 与 `mark_checkin` / `mark_onboarding_grant_batch` 同一口径：账号级事实、
    /// 低频写、写盘失败不致命（返回 bool；上游那边奖励已经结清，调用方最多记
    /// 一条日志）。**不**动 updatedAt（理由同上）。
    ///
    /// 形状校验只到「对象或 Null」为止：对象 = 写入（内容是各家自己的任务行
    /// `{at, tasks, earned, total}`，这里不做跨家白名单 —— 唯一调用方就是那几家
    /// 的 onboarding 模块，键名在那边随任务表定义）；**Null = 清除**（上游出了
    /// 新一期活动之类的新事实时，记忆失效要能抹掉，读侧把 Null 当「没有」）。
    /// 其余形态（数组 / 字符串 / 数字）整批拒绝，免得往账号记录里塞进读不出来的
    /// 脏值。
    ///
    /// 与现存值相同时（含「本来就没什么可清」的 Null）**直接返回 true，不写库**：
    /// 没领完的账号每次状态查询都会带着 `unclaimed > 0` 走到「清除」那一步，
    /// 值没变也写一次盘是纯浪费 —— 这条链真正要写的只有结算那一次。
    pub fn mark_onboarding_settled(&self, id: &str, snapshot: Value) -> bool {
        if !(snapshot.is_object() || snapshot.is_null()) {
            return false;
        }
        let _guard = self.guard();
        let Some(mut record) = self.record_by_id(&_guard, id) else {
            return false;
        };
        let existing = record.fields().get("onboardingSettled");
        if existing == Some(&snapshot) || (snapshot.is_null() && existing.is_none()) {
            return true;
        }
        record.set_onboarding_settled(snapshot);
        self.with_conn(&_guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }

    // ─── 迁移 ────────────────────────────────────────────────

    /// 启动时的一次性数据迁移（**唯一入口**，bootstrap 只调它）。
    ///
    /// 四步在同一份账号集合上完成、只写一次库：
    ///   ① provider 惰性补齐（缺失/空值一律补 workbuddy）；
    ///   ② 优先级作用域从「按 provider 各排各的队」迁到「全局一条队列」——
    ///      库里没有 `priorityScope: "global"` 标记时做一次：按旧版实际的转发
    ///      顺序（provider 路由优先级 → 组内优先级 → 加入时间）排好，再连续编号，
    ///      于是升级前后实际先用哪个账号完全一致，用户不会感到突变；
    ///   ③ 全局去重（手工编辑出的并列号）；
    ///   ④ Cline 拆池改名（`provider: "cline"` + `pool` 字段 → 两家里的某一家）；
    ///   ⑤⑥ WorkBuddy / Qoder 国际版拆家（只改 provider）；
    ///   ⑦ OfficeAce 账号名自愈（从记录里躺着的 refreshToken 解显示名）；
    ///   ⑧ 智谱清言令牌从嵌套 `auth` 抬到顶层。
    ///
    /// 顺序上 ④ 必须在 ①②③ **之后**：前几步按 provider 分组时看到的还是旧 id
    /// （一条 `cline` 组），改名后这一组被拆成两条；若反过来先改名，优先级整队
    /// 会在两条新组上各排一遍，得出与升级前不同的相对顺序。放最后则与
    /// 「先整好旧队、再改名」等价 —— 名字变了，号码与顺序原样。
    /// 返回 `{providerAdded, priorityChanged, assignments}` 供启动日志展示。
    ///
    /// ── 为什么整批必须在一个事务里（本切片的要点）──────────────
    /// ②③④ 都会改多条记录的 `priority`（甚至 `id`，见拆池改名）。这些编号是
    /// 「整队重排一次的结果」，中断在任何一步都会留下**编号断裂或重号**的中间态
    /// —— 而重号直接破坏「优先级全局唯一」这条不变量，下一次写入就会撞上
    /// 409，用户看到的是「明明没改过却报优先级被占用」。
    /// 因此整批走 `save`（`sql::save_state` 自开一个事务）：要么全部生效，
    /// 要么一条都不生效，中断后下次启动从头再来一遍（幂等）。
    ///
    /// ── 写入粒度 ────────────────────────────────────────────────
    /// 读仍然是全量（①②③④ 的算法本身要遍历全部账号），但**写**是按差异的：
    /// `save_state` 只对 `data` 真的变了的行发 UPDATE，因此「已经迁移过的库」
    /// 每次启动是零次写入（`provider_added == 0 && assignments.is_empty()
    /// && !scope_migrated && cline_renamed == 0` 的早退分支连事务都不开）。
    pub fn migrate_startup(&self) -> Value {
        let _guard = self.guard();
        let mut state = self.load(&_guard);

        // ① provider 惰性补齐（架构文档 §3.2）：缺失/空值一律补 workbuddy
        let mut provider_added = 0usize;
        for record in state.accounts.iter_mut() {
            if record.provider_explicit().is_none() {
                record.set_provider(DEFAULT_PROVIDER_ID);
                provider_added += 1;
            }
        }
        if provider_added > 0 {
            logging::log(
                "[Accounts]",
                &format!("🏷️  已为 {provider_added} 个历史账号补全 provider 字段（{DEFAULT_PROVIDER_ID}）"),
            );
        }

        // ② / ③ 优先级：首次进入全局队列时按旧顺序整队；之后只在有并列时整队
        let scope_migrated = state.priority_scope.as_deref() != Some(PRIORITY_SCOPE_GLOBAL);
        let assignments = Self::renumber_globally(&mut state, scope_migrated);
        if scope_migrated {
            state.priority_scope = Some(PRIORITY_SCOPE_GLOBAL.to_string());
        }

        // ④ Cline 拆池改名（见 `migrate_cline_accounts`）。放在最后：
        // 它改的是 id，而 ②③ 的整队要按**旧** id 分组看
        let cline_renamed = Self::migrate_cline_accounts(&mut state);

        // ⑤ WorkBuddy 国际版拆家（见 `migrate_workbuddy_intl_accounts`）。
        // 只改 `provider`、**不改 id**，因此放在 ②③ 之后没有顺序约束；
        // 排在 Cline 之后只是让「拆家类迁移」聚在一起。
        let workbuddy_moved = Self::migrate_workbuddy_intl_accounts(&mut state);

        // ⑥ Qoder 地区归位：兼容旧记录，并恢复有明确国内版证据的错分账号。
        // 只改 provider，不改作为请求历史引用键的账号 id。
        let qoder_moved = super::qoder_accounts::migrate_accounts(&mut state.accounts);

        // ⑦ OfficeAce 账号名自愈（存量记录的 name 曾是十六进制 id，显示名躺在
        // refreshToken 的 claims 里）已随实现搬到插件：插件在登录时就把名字取全，
        // 而网关不再解析这家令牌的 claims。存量记录的名字原样保留 —— 它们是
        // 用户看得见的历史，不该因为一次升级被改写。

        // ⑧ 智谱清言令牌从嵌套 `auth` 抬到顶层
        // （见 `chatglm_accounts::migrate_nested_tokens`）：首版把令牌写进了嵌套
        // `auth`，而 `has_credentials()` 只读顶层 —— 不抬的话那几条存量记录会一直
        // 被静默过滤掉（模型管理里看不到这一家、余额报「没有可用凭证」）。
        let chatglm_healed =
            super::chatglm_accounts::migrate_nested_tokens(&mut state.accounts);
        if chatglm_healed > 0 {
            logging::log(
                "[Accounts]",
                &format!(
                    "🔧 已修复 {chatglm_healed} 个智谱清言账号的令牌存储位置（嵌套 auth → 顶层，\
                     修复后即可正常选路与查询余额）"
                ),
            );
        }

        let codearts_limiters = super::codearts_accounts::migrate_limiters(&mut state.accounts);

        if provider_added == 0
            && assignments.is_empty()
            && !scope_migrated
            && cline_renamed == 0
            && workbuddy_moved == 0
            && qoder_moved == 0
            && chatglm_healed == 0
            && codearts_limiters == 0
        {
            return json!({
                "providerAdded": 0,
                "priorityChanged": false,
                "assignments": [],
            });
        }
        // 整批一次写入（内部是差异写 + 一个事务，见上面的说明）。
        // 拆池改名会改主键，所以 `save_state` 的「删除 + 新增」两个分支都会被
        // 用到（旧 id 的行删掉、新 id 的行插入）—— 这正是它按 id 比对而不是
        // 直接 UPDATE 的原因。副作用是被改名的记录会落到列表末尾；账号数组的
        // 顺序没有任何消费方（界面与选路都自己按优先级排），可以接受。
        if let Err(error) = self.save(&state, &_guard) {
            logging::log("[Accounts]", &format!("❌ 启动迁移落库失败: {error}"));
            return json!({
                "providerAdded": 0,
                "priorityChanged": false,
                "assignments": [],
            });
        }
        if codearts_limiters > 0 {
            logging::log("[Accounts]", &format!("已移除 {codearts_limiters} 个 CodeArts 账号的过期活动 Token 限制；无其他规则时改用总积分保护"));
        }
        if cline_renamed > 0 {
            logging::log(
                "[Accounts]",
                &format!(
                    "🔀 Cline 已拆为 Cline Free / Cline Pass 两家，{cline_renamed} 个账号已按额度池归位"
                ),
            );
        }
        if workbuddy_moved > 0 {
            logging::log(
                "[Accounts]",
                &format!(
                    "🔀 WorkBuddy 已拆为国内版 / 国际版两家，{workbuddy_moved} 个国际版账号已归位\
                     （凭证、优先级、限流记录与账号 id 均未改动）"
                ),
            );
        }
        if qoder_moved > 0 {
            logging::log(
                "[Accounts]",
                &format!(
                    "🔀 Qoder 已按地区归位 {qoder_moved} 个账号\
                     （凭证、优先级、限流记录与账号 id 均未改动）"
                ),
            );
        }
        if !assignments.is_empty() {
            logging::log(
                "[Accounts]",
                &format!(
                    "🔢 优先级已{}（{} 个账号重新编号，转发顺序保持不变）",
                    if scope_migrated { "合并为全局一条队列" } else { "去重" },
                    assignments.len()
                ),
            );
        } else if scope_migrated {
            logging::log("[Accounts]", "🔢 优先级已标记为全局一条队列（号码无需调整）");
        }
        json!({
            "providerAdded": provider_added,
            "priorityChanged": !assignments.is_empty(),
            "assignments": assignments
                .iter()
                .map(|item| json!({
                    "id": item.id,
                    "name": item.name,
                    "from": item.from,
                    "to": item.to,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// 把历史明文账号行**整列加密**落库（幂等，可重复调用）。
    ///
    /// ── 它补的是什么 ────────────────────────────────────────────
    /// 本轮改造起 `accounts.data` 写的是 `enc1:` 密文（见 `sql.rs` 模块头），但
    /// **升级前已经写进库的行**仍是明文 JSON。`sql::decode_data` 兼容明文，那些行
    /// 照常能读 —— 那是过渡期的安全网，不是最终形态：只要还有明文行，拷走库文件
    /// 就带走全部凭证。所以启动时补一次「就地加密」，把过渡态收敛掉。
    ///
    /// ── 为什么排位在 `migrate_startup` 之后（见 `account_bootstrap::run`）────
    /// 那一轮会按差异重写改了字段的行（补 provider、整队、拆池改名…），写出的
    /// 已经是密文；剩下没被它碰到、仍是明文的历史行由本函数收尾。反过来的顺序
    /// 也能得到同一结果（`decode_data` 两种都读得出来），排后面只是让「先做语义
    /// 迁移、再做形态迁移」这条读起来更顺。
    ///
    /// ── 幂等与失败语义都由 SQL 层承担 ───────────────────────────
    /// 判据是数据自证（`sql::encrypt_legacy_rows` 逐行看 `data` 是不是 `enc1:`），
    /// 没有 kv 标记；整批在一个事务里，要么全成、要么一行都不动。**失败不阻断
    /// 启动**：密钥文件建不出来之类的失败在这里只记一条日志、返回，下次启动重试
    /// —— 而因为 `decode_data` 兼容明文，本次账号照常可用，绝不把用户挡在门外。
    pub fn encrypt_legacy_accounts(&self) {
        let _guard = self.guard();
        // 用 `with_conn_mut` 而不是 `with_conn`：整批加密要在一个事务里
        // （见 `sql::encrypt_legacy_rows` 的说明），而事务需要可变的连接借用。
        match self.with_conn_mut(&_guard, |conn| sql::encrypt_legacy_rows(conn)) {
            // 0 条 = 库里没有明文行（全新库 / 已迁过的库），不记日志
            Ok(0) => {}
            Ok(count) => {
                logging::log(
                    "[Accounts]",
                    &format!("🔒 已加密 {count} 个历史账号的凭证"),
                );
            }
            Err(error) => {
                // 这条日志必须在 `with_conn_mut` **之外**打：记日志本身要写库
                // （logs 表），而 `with_conn_mut` 持着那条唯一的数据库连接 ——
                // 在它内部再记日志就是「自己等自己」的硬死锁（`std` 的 Mutex
                // 不可重入，release 构建还是 panic=abort）。
                logging::log(
                    "[Accounts]",
                    &format!(
                        "⚠️  历史账号凭证加密未完成（下次启动会自动重试，本次账号照常可用）: {error}"
                    ),
                );
            }
        }
    }

    /// 把「插件落库但顶层缺身份字段」的账号补齐（幂等，可重复调用）。
    ///
    /// ── 它补的是什么（issue #380）──────────────────────────────
    /// 插件登录 / 插件落库写的是 `pluginSecret` + `pluginPublic`，顶层没有
    /// `uid` / `edition` / `endpoint` 等字段。而网关的计费、签到、余额几条
    /// 读路径只认顶层 —— 那批账号「对话正常、签到全报找不到账号记录」。
    ///
    /// 写侧（`plugin_identity::mirror_identity`）已经在新写入时镜像了，本函数
    /// 只收尾**升级前已经在库里**的那批。读侧也做了回落（`StoredAccount::uid`
    /// 等），所以即使这一步没跑成，功能也是好的 —— 它只是把字段真正落到盘上，
    /// 让导出的 JSON 与其它按顶层字段工作的地方也一致。
    ///
    /// ── 幂等与失败语义 ──────────────────────────────────────────
    /// 判据是数据自证：只有「顶层缺、子对象里有」时才写，没有 kv 标记。
    /// 一次读一次写（`migrate_startup` 同款）；没有要补的记录时不写库。
    pub fn backfill_plugin_identity(&self) {
        let guard = self.guard();
        let mut state = self.load_locked(&guard);
        let mut changed = 0usize;
        for record in state.accounts.iter_mut() {
            // 只处理插件落库形态（有 pluginSecret 或 pluginPublic）：内置记录
            // 的顶层字段本来就是权威，不该被子对象覆盖。
            let has_plugin_side = record.get("pluginSecret").is_some_and(|value| !value.is_null())
                || record.get("pluginPublic").is_some_and(|value| !value.is_null());
            if !has_plugin_side {
                continue;
            }
            let before = record.to_value();
            let provider = record.provider();
            super::plugin_identity::mirror_identity(record, &provider);
            if record.to_value() != before {
                changed += 1;
            }
        }
        if changed == 0 {
            return;
        }
        match self.save_locked(&state, &guard) {
            Ok(()) => {
                drop(guard);
                logging::log(
                    "[Accounts]",
                    &format!("🔧 已为 {changed} 个插件账号补齐顶层身份字段（uid / edition 等）"),
                );
            }
            Err(error) => {
                drop(guard);
                logging::log(
                    "[Accounts]",
                    &format!(
                        "⚠️  插件账号身份字段回填未完成（下次启动会自动重试，本次账号照常可用）: {}",
                        error.message
                    ),
                );
            }
        }
    }

    /// Cline 拆池改名：`provider: "cline"` + `pool` 字段 → 两家里的某一家，
    /// 原地改 state（不落盘）。返回改名的记录条数。
    ///
    /// ── 为什么这一步不能省 ──────────────────────────────────────
    /// `"cline"` 这个 provider id **已经不存在**（拆分后是 `cline-free` /
    /// `cline-pass`）。不迁移的后果不是「少一条设置」而是**账号静默消失**：
    /// 每处按 provider 找账号的路径（`accounts_for_provider` 的选路、账号页
    /// 的分组、适配器按 id 找人）都拿不到它 —— 但它还在文件里，用户会看到
    /// 「账号明明在，转发却说没有可用账号」。
    ///
    /// ── 池怎么判 ────────────────────────────────────────────────
    /// 读记录里的 `pool` 字段，判据与口径**逐条沿用拆分前的实现**
    /// （`cline::models::Pool::parse`：`free` → 免费池，其余/缺失/非法 → 订阅池）。
    /// 用同一个函数而不是重写一个 match 是刻意的：池的归属直接决定用户能看到
    /// 哪些模型，迁移前后必须逐字一致，否则升级一次就换了池。
    ///
    /// ── id 为什么要一起改 ──────────────────────────────────────
    /// 旧 id 形如 `cline-usr-…`，**不带池**。若不改，同一个 Cline 账号之后想
    /// 两个池各加一份时，第二次添加会算出同一个 id 而被当成「更新同一条」，
    /// 第二个池永远加不进去。改成 `<provider>-<原后缀>`（`cline-free-usr-…`）
    /// 之后两条才能并存 —— 与 `cline_accounts::record_id` 的拼法一致。
    ///
    /// 改名的判据是**旧 provider 值**而不是 id 前缀：id 是用户可见标识，
    /// 有可能被手工改成别的样子（导出再导入也会重排），只有 provider 字段
    /// 是这一版写进去的事实。
    ///
    /// `pool` 字段在最后**删掉**：池已经是 provider 身份的一部分，留着它
    /// 就多了一处可能与 provider 不一致的事实（而读它的代码已经删光了）。
    ///
    /// 幂等：改完 provider 就不再命中第一条判据，再跑一遍无事发生。
    fn migrate_cline_accounts(state: &mut AccountState) -> usize {
        /// 拆分前的 Cline provider id（只出现在这里）
        const LEGACY_CLINE_ID: &str = "cline";
        let mut renamed = 0usize;
        for record in state.accounts.iter_mut() {
            if record.provider() != LEGACY_CLINE_ID {
                continue;
            }
            let pool = crate::server::core::providers::cline::models::Pool::parse(
                record.get("pool").and_then(Value::as_str).unwrap_or(""),
            );
            let provider = pool.provider_id();
            let id = record.id().to_string();
            // 已经被改成目标前缀的（理论不可能，但比对代价极低）就只换 provider，
            // 不重复叠前缀 —— 否则会拼出 `cline-free-free-…` 这种废物 id
            let new_id = if id.starts_with(&format!("{provider}-")) {
                id
            } else if let Some(rest) = id.strip_prefix(&format!("{LEGACY_CLINE_ID}-")) {
                format!("{provider}-{rest}")
            } else {
                format!("{provider}-{id}")
            };
            record.set_provider(provider);
            record.set("id", Value::String(new_id));
            record.remove("pool");
            renamed += 1;
        }
        renamed
    }

    /// WorkBuddy 国际版拆家：把 `provider == "workbuddy"` 且 `edition == "intl"`
    /// 的记录归到 `workbuddy-intl`（2026-10，见 `providers::workbuddy::region`）。
    ///
    /// ── 为什么不改 id（与 `migrate_cline_accounts` 的差别）──────────
    /// Cline 那次必须改 id：两个池共用 `cline-...` 的 id 空间，不拆就分不开。
    /// 本家不需要 —— 两个地区的 uid 是各自上游生成的 uuid，不会撞；而 id 是
    /// `requests.account_id` 与 `request_daily` 的引用键，改名会让用户在请求
    /// 日志与报表里再也对不上自己那个账号（详见 `Region` 模块头那段论证）。
    ///
    /// ── 判据为什么是 `edition` 而不是账号别的字段 ────────────────
    /// 拆家前「账号属于哪个地区」这件事**只有** `edition` 一个表达（端点、
    /// UA、登录站点全部由它派生），所以它就是这次迁移的全部依据。
    /// `edition` 缺失（旧数据 / 手改）一律按国内版 —— 与 `resolve_edition`
    /// 的兜底同口径（`StoredAccount::edition()` 返回 None，这里不动它）。
    ///
    /// ── 幂等 ────────────────────────────────────────────────────
    /// 判据是数据本身（`provider == workbuddy && edition == intl`）：迁完就没有
    /// 这样的记录，再跑一遍返回 0、不写库。与 `migrate_startup` 的其它步骤
    /// 同一原则（见 `db::migrate` 模块头「数据自己有没有」那段）。
    fn migrate_workbuddy_intl_accounts(state: &mut AccountState) -> usize {
        /// 拆家前的唯一 WorkBuddy provider id（只出现在这里）
        const LEGACY_WORKBUDDY_ID: &str = "workbuddy";
        let mut moved = 0usize;
        for record in state.accounts.iter_mut() {
            if record.provider() != LEGACY_WORKBUDDY_ID || record.has_plugin_secret() {
                continue;
            }
            let region = crate::server::core::providers::workbuddy::Region::from_edition_id(
                record.edition().as_deref(),
            );
            let provider = region.provider_id();
            if provider == LEGACY_WORKBUDDY_ID {
                continue;
            }
            record.set_provider(provider);
            moved += 1;
        }
        moved
    }

    /// 全局重新连续编号，**原地改 state**（不落盘）。
    ///
    /// `legacy_order = true`（首次从「按家分队」迁到全局）：排序键是旧版的实际转发
    /// 顺序 —— (旧路由优先级, 优先级, 加入时间)。旧路由优先级取 config.json 里
    /// `providerRoute` 的历史值，没有则按注册表顺序（与旧默认值 10/20/30/40 同序）。
    /// 这一档**无论有没有并列都整队**：跨家的号码原本互不相干（各家都有 P100），
    /// 即便碰巧不重号，数值顺序也未必等于旧的实际顺序。
    ///
    /// `legacy_order = false`（之后每次启动）：只在全局存在并列时按
    /// (优先级, 加入时间) 整队 —— 与单上游时代的去重逻辑逐条一致。
    ///
    /// 整队后整份数组按新顺序落盘。返回变更清单（未变化的项不列入）。
    fn renumber_globally(state: &mut AccountState, legacy_order: bool) -> Vec<PriorityAssignment> {
        if state.accounts.is_empty() {
            return Vec::new();
        }
        if !legacy_order {
            let mut unique: Vec<i64> = state
                .accounts
                .iter()
                .map(|record| normalize_priority_value(record.priority()))
                .collect();
            let total = unique.len();
            unique.sort_unstable();
            unique.dedup();
            if unique.len() == total {
                return Vec::new();
            }
        }

        let legacy_route = config::legacy_provider_route();
        let route_rank = |record: &StoredAccount| -> u32 {
            let provider = record.provider();
            legacy_route
                .iter()
                .find(|(id, _)| *id == provider)
                .map(|(_, rank)| *rank)
                .unwrap_or_else(|| (provider_index(&provider).unwrap_or(usize::MAX / 2) as u32 + 1) * 10)
        };
        let mut sorted = state.accounts.clone();
        if legacy_order {
            sorted.sort_by_key(|record| (route_rank(record), record.order_key()));
        } else {
            sorted.sort_by_key(StoredAccount::order_key);
        }
        let mut numbering: Vec<(String, String, i64)> = sorted
            .iter()
            .map(|record| (record.id().to_string(), record.name(), record.priority()))
            .collect();
        let assignments = renumber_consecutively(&mut numbering);
        // 只回写「确实改变」的账号：原本就没有 priority 字段的账号迁移后依然没有
        // （生效值仍是默认 100），不会凭空多出一批字段
        for assignment in &assignments {
            if let Some(record) = sorted.iter_mut().find(|record| record.id() == assignment.id) {
                record.set_priority(assignment.to);
            }
        }
        state.accounts = sorted;
        assignments
    }

    /// 旧版单账号 auth.json 迁移：仅当账号列表尚无任何账号时导入一次。
    ///
    /// 返回导入后的公开形态（未迁移时返回 None，对应 Node 版返回 null）。
    pub fn import_legacy_session(&self, legacy: &Value) -> Option<Value> {
        let access_token = legacy
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if access_token.is_empty() {
            return None;
        }
        {
            let _guard = self.guard();
            // 只问「账列表是否为空」—— 投影列 COUNT，不读任何记录。
            // 查询失败时按「非空」处理（保守跳过）：本函数是启动期的一次性迁移，
            // 库不可用时不该顺势去写（`add_account` 会返回 500），而「跳过」与
            // 旧实现「读不到文件就当空、于是尝试导入」的差别只在库坏掉时出现 ——
            // 那时整条账号链路都不可用，由别处的 ❌ 日志说明。
            match self.with_conn(&_guard, |conn| sql::count_all(conn)) {
                Ok(count) if count == 0 => {}
                Ok(_) => return None,
                Err(error) => {
                    logging::log(
                        "[Accounts]",
                        &format!("⚠️  读取账号数失败，跳过旧登录态迁移: {error}"),
                    );
                    return None;
                }
            }
        }
        let uid = legacy
            .get("account")
            .and_then(|account| account.get("uid"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if uid.is_empty() {
            logging::log("[Accounts]", "旧登录态缺少 uid，跳过迁移");
            return None;
        }
        let nickname = legacy
            .get("account")
            .and_then(|account| account.get("nickname"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let payload = json!({
            "auth": legacy.get("auth").cloned().unwrap_or(Value::Null),
            "account": legacy.get("account").cloned().unwrap_or(Value::Null),
            "prefixPath": legacy.get("prefixPath").cloned().unwrap_or(Value::Null),
            "endpoint": legacy.get("endpoint").cloned().unwrap_or(Value::Null),
            "platform": legacy.get("platform").cloned().unwrap_or(Value::Null),
            "edition": legacy.get("edition").cloned().unwrap_or(Value::Null),
        });
        // 旧登录态（`auth.json`）是单上游时代的产物：那时进程只有一个上游，
        // 地区由 `WORKBUDDY_EDITION` 决定，因此这里**按 payload 里的 edition
        // 归位**（缺失回落国内版，历史行为不变）。
        let provider = crate::server::core::providers::workbuddy::Region::from_edition_id(
            payload.get("edition").and_then(Value::as_str),
        )
        .provider_id();
        match self.add_account(&payload, nickname.as_deref(), Some(provider)) {
            Ok(account) => Some(account),
            Err(error) => {
                logging::log("[Accounts]", &format!("❌ 旧登录态迁移失败: {error}"));
                None
            }
        }
    }
}

/// 递归改写 `limiterGroups` / `limiters` 里所有 `models` 数组，并去重。
fn rewrite_models_arrays(value: &mut Value, mapping: &HashMap<String, String>) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            for (key, child) in object.iter_mut() {
                if key == "models" {
                    if let Some(items) = child.as_array_mut() {
                        for item in items.iter_mut() {
                            if let Some(text) = item.as_str() {
                                if let Some(base) =
                                    mapping.get(&text.trim().to_ascii_lowercase())
                                {
                                    *item = Value::String(base.clone());
                                    changed = true;
                                }
                            }
                        }
                        let before = items.len();
                        let mut seen = HashSet::new();
                        items.retain(|item| {
                            item.as_str()
                                .map(|text| seen.insert(text.to_ascii_lowercase()))
                                .unwrap_or(true)
                        });
                        changed |= items.len() != before;
                    }
                } else if rewrite_models_arrays(child, mapping) {
                    changed = true;
                }
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= rewrite_models_arrays(item, mapping);
            }
            changed
        }
        _ => false,
    }
}
