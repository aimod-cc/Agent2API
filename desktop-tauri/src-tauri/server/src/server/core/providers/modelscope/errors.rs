//! ModelScope 的**账号级失败判据**：三类不该被当成「速率窗口」的拒绝。
//!
//! ── 为什么要单独一个文件 ────────────────────────────────────
//! 魔搭把「账号条件不满足」和「速率窗口撞上」都用 429 / 403 表达，**只能靠文案
//! 区分** —— 而两者的正确处置完全不同：前者在充值 / 完成验证之前**一直**不可用
//! （按天数冷却），后者等一会儿就好（分钟级冷却）。
//!
//! 判据来源：CPA 的 `modelscope-ratelimit` 插件（源码
//! `D:\GitHub\modelscope-ratelimit`，`internal/ratelimit/apply.go` 的
//! `IsInsufficientBalance` / `IsUnverifiedAccount`，两处都是线上实测得来），
//! 本文件照同一套文案子串做匹配。
//!
//! ── 三类是什么（附上游真实报文）──────────────────────────────
//! ```text
//!   ① 魔粒耗尽   429 {"error":{"message":"insufficient balance","request_id":"…"}}
//!       账号的魔粒用完了。充魔粒之前这把令牌确实不可用 —— 插件按「当日禁用」处理，
//!       跨过午夜再试一次（充了值就恢复）。
//!   ② 未验证邮箱 403 正文含 "email verification" / "verified accounts" / "/my/settings/account"
//!       账号没完成邮箱验证，魔粒推理接口直接拒绝服务，充魔粒也救不回来 ——
//!       唯一出路是去账号设置页完成验证。
//! ```
//!
//! ── 本网关怎么用这两条 ──────────────────────────────────────
//! 两类都 → `QuotaLimited` 且 `reset_at` 给**下一个北京零点**（`mark_account_limited`
//! 会据此把账号冷却到那个时刻，跨天自动解冻）。
//!
//! ── 插件里还有一条，本网关**有意不照搬**（说明理由，免得将来被当成遗漏）──
//! 插件把 `503 + "model_not_ready"`（上游 vLLM 还在加载权重）当作**整家不可用**摘掉。
//! 本网关**不做**：那件事与账号无关（本家每把令牌都会被同样拒绝），罚账号是错的 ——
//! 而 agent2api 也没有「按 provider 摘家」的接口，照搬只能退化成「随便找个账号记一笔
//! 限额」。这里的默认处置是 `Fatal`（不罚账号，如实回给客户端、由客户端重试），
//! 那正是这条失败应有的语义。代价是编排层换号前会各撞一次，但 `model_not_ready`
//! 是**短暂**状态（权重加载完就好），撞几次的代价远小于把一个好账号记成限额。
//!
//! ⚠️ **判据一律大小写不敏感的子串匹配，且只在「这次请求已经失败」的分支上调用** ——
//! 子串匹配本身不区分「上游报的错」与「模型自己生成的内容」，正常响应里若出现这些
//! 字样会被误判（插件源码在 `IsInputContentInspectionFailed` 上方写了同一条警告）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// 一整天（毫秒）
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 北京时间的固定偏移（UTC+8）。
///
/// 用**固定偏移**而不是机器时区：网关可能跑在 UTC 容器里（`.3` 上的部署就是），
/// 而魔搭的一整套「当日」口径（签到、魔粒发放、日额度）都是按北京时间算的 ——
/// 「明天零点」必须是北京的那个。与 `codearts::welfare` 的同名常量同一处置。
const DAY_ZONE_OFFSET_SECONDS: i32 = 8 * 3600;

/// 一次失败是否意味着**账号魔粒耗尽**（429 + `insufficient balance`）。
///
/// 只看文案不看状态码也可以，但两样都看更稳（插件源码的 `IsInsufficientBalance`
/// 同样要求 429）—— 别的 4xx 里出现这串字的场合不存在，多一道闸门不损失什么。
pub fn is_insufficient_balance(status: u16, message: &str) -> bool {
    status == 429 && contains_ci(message, "insufficient balance")
}

/// 一次失败是否意味着**账号未完成邮箱验证**（403 + 三选一的特征串）。
///
/// **必须同时看状态码与文案**（插件源码的注释写明了理由）：403 还有别的权限语义，
/// 只看状态码会把无关失败误判成「账号未验证」，而那会让用户去查一个不存在的问题。
pub fn is_unverified_account(status: u16, message: &str) -> bool {
    if status != 403 {
        return false;
    }
    ["email verification", "verified accounts", "/my/settings/account"]
        .iter()
        .any(|marker| contains_ci(message, marker))
}

/// 大小写不敏感的子串匹配（空值一律 false）
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

/// `now_ms` 之后最近的**北京时间零点**（毫秒时间戳，纯函数）。
///
/// 给「魔粒耗尽 / 账号未验证 ⇒ 冷却到明天」用：这两种状态在充值 / 完成验证之前
/// 一直不可用，而魔搭的日口径是北京零点 —— 跨过那个点再试一次，既不会在当天
/// 反复白撞，也不会把账号永久钉死（用户当天处理完之后，次日自动回来）。
///
/// 边界：恰好落在零点上时给**次日**零点（刚过零点，下一个重置点是一整天之后）。
/// 用 `div_euclid` 而不是 `/`：负数时刻（1970 前）在 Rust 里默认向零取整会算错整天边界。
pub fn next_day_boundary_ms(now_ms: i64) -> i64 {
    let shifted = now_ms + i64::from(DAY_ZONE_OFFSET_SECONDS) * 1000;
    let start_of_today = shifted.div_euclid(DAY_MS) * DAY_MS;
    start_of_today + DAY_MS - i64::from(DAY_ZONE_OFFSET_SECONDS) * 1000
}

#[cfg(test)]
mod tests {
    use super::{is_insufficient_balance, is_unverified_account, next_day_boundary_ms};

    /// 上游真实报文里的文案（插件测试夹具逐字同款）
    #[test]
    fn the_measured_insufficient_balance_body_is_recognised() {
        assert!(is_insufficient_balance(429, "insufficient balance"));
        // 大小写与前后缀都不影响
        assert!(is_insufficient_balance(429, "Insufficient Balance"));
        assert!(is_insufficient_balance(
            429,
            "{\"error\":{\"message\":\"insufficient balance\",\"request_id\":\"x\"}}"
        ));
        // 必须是 429：别的状态码配这串字不是「魔粒耗尽」的语义
        assert!(!is_insufficient_balance(400, "insufficient balance"));
        assert!(!is_insufficient_balance(429, "rate limit exceeded"));
    }

    /// 403 的两种语义必须分开 —— 把「未验证邮箱」当成「令牌失效」会让用户
    /// 白白去重新粘贴一个本来就好好的令牌
    #[test]
    fn the_unverified_account_markers_are_recognised() {
        assert!(is_unverified_account(
            403,
            "please complete email verification before using inference"
        ));
        assert!(is_unverified_account(403, "only verified accounts can call this API"));
        assert!(is_unverified_account(403, "visit https://modelscope.cn/my/settings/account"));
        // 裸 403（没有任何特征串）不是这一条
        assert!(!is_unverified_account(403, "access forbidden"));
        assert!(!is_unverified_account(401, "email verification"));
    }

    /// 北京零点：与 codearts 的同名函数同一口径（那里有自己的测试守着）
    #[test]
    fn the_day_boundary_is_the_coming_beijing_midnight() {
        // 2026-10-16 12:00 北京 = 04:00 UTC → 时间戳 1792123200000
        let noon_cst = 1792123200000i64;
        let boundary = next_day_boundary_ms(noon_cst);
        // 距北京零点还有 12 小时
        assert_eq!(boundary - noon_cst, 12 * 60 * 60 * 1000);
        // 再问一次已经落在零点上的时刻：给次日零点（不是同一刻）
        assert_eq!(next_day_boundary_ms(boundary), boundary + 24 * 60 * 60 * 1000);
    }

    /// 空串不得命中任何一条（`contains_ci` 的 needle 非空，但上游可能给出空 message）
    #[test]
    fn an_empty_message_matches_nothing() {
        assert!(!is_insufficient_balance(429, ""));
        assert!(!is_unverified_account(403, ""));
    }
}