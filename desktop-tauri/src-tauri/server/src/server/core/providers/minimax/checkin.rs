//! MiniMax Code 每日签到：**七天一轮，共 4000 积分**。
//!
//! ── 上游是什么（2026-10-21 实测 + `app.asar` 逆向）─────────────
//! ```text
//! GET  {gw}/minimax-cloud/api/v1/signin/status?timezone_id=Asia/Shanghai
//! POST {gw}/minimax-cloud/api/v1/signin/claim?timezone_id=Asia/Shanghai   body {}
//! → {"base_resp":{"status_code":0},"data":{…}}
//! ```
//! 两者都要「设备查询串 + 三签名头 + Bearer」（缺签名回
//! `400 invalid signature`，见 `endpoints.rs`）。
//!
//! ── 这个链路真的存在（不是猜的）───────────────────────────────
//! ① 客户端 `archon` 页的 `signin` store 直接调这两个端点
//!    （`out/_next/static/chunks/app/(pages)/(mavis)/archon/page-*.js` 的
//!    `getSigninPanel` / `claimSignin`）；
//! ② 校验器在 `@mavis/shared/dist/daily-signin.js`（七天、`status` 1–4、
//!    `claim_result` 1/2），本仓的 `balance.rs` 按它逐条复刻；
//! ③ 积分确实是它发的：台账里 `credit_type == 2` 的批次就是签到产出，
//!    `expire_at_ms` 正好是领取日 + 30 天的北京时间零点。
//!
//! ── 幂等 ────────────────────────────────────────────────────
//! 当天重复领回 `claim_result == 2`（今日已领），**不算失败** —— 如实报成功并
//! 说明是重复调用。客户端自己不自动领（卡片要用户点按钮），所以对「只用网关」
//! 的用户来说，这个签到链是白拿的 4000 积分/轮。
//!
//! ── `timezone_id` 为什么写死北京 ─────────────────────────────
//! 上游只认 IANA 名（传偏移量回 `invalid timezone_id`），而它决定「今天是哪天」。
//! 国内版账号的签到日历按北京日算；与 `welfare::today`（UTC+8）同一口径，
//! 避免界面与台账各按一个日界。
//!
//! ⚠️ **国际版也走北京日**（本实现不分地区传 `Asia/Shanghai`）：上游只要求它是
//! 一个合法的 IANA 名，此刻没有任何实测证据表明它会按账号地区拒绝 / 改写这个值，
//! 而「按地区换个时区名」是一个没有依据的分叉。真要改，凭据是「国际账号用
//! `Asia/Shanghai` 领到的批次 `expire_at_ms` 与本地日历对不上」—— 先测再改。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::account_store::AccountStore;

use super::region::Region;

pub use super::balance::{claim_signin as claim_daily_checkin, query_signin as panel};

/// 签到中心的说明文字（`api::checkin_center` 的分组说明用）。
///
/// `PROVIDER_DESC` 在前端还有一份（`checkin-page.tsx`），两处措辞刻意一致：
/// 一处给接口文档看，一处给界面看，写成两套说法会让人以为漏了一家。
pub const DESCRIPTION: &str =
    "七天一轮共 4000 积分（400/400/400/1000/400/400/1000），每份 30 天后到期；\
     当天重复领是幂等的（上游回「今日已领」）。";

/// 台账里签到的 `credit_type`（1 = 购买，2 = 签到）。
pub const CREDIT_TYPE_SIGNIN: i64 = 2;

/// 一轮签到的天数与总积分（面板校验与说明文案共用一份口径）。
pub const CYCLE_DAYS: usize = super::balance::SIGNIN_CYCLE_DAYS;
pub const CYCLE_POINTS: f64 = 4000.0;

/// 签到面板的摘要（签到中心的「今天能不能领」用它，不发二次请求）。
pub fn summarize(panel: &super::balance::SigninPanel) -> Value {
    let today = panel.today();
    serde_json::json!({
        "claimable": panel.claimable(),
        "claimed": panel.claimed(),
        "dayNo": today.map(|day| day.day_no),
        "points": today.map(|day| day.points),
        "cyclePoints": panel.cycle_points(),
        "days": panel.days.iter().map(|day| serde_json::json!({
            "dayNo": day.day_no,
            "points": day.points,
            "status": day.status,
            "isToday": day.is_today,
        })).collect::<Vec<Value>>(),
    })
}

/// 签到链是否对该账号可用（签到中心决定要不要给它勾选框）。
///
/// 只判「账号存在且启用」：本家的签到不需要额外凭证（访问令牌就够），
/// 所以没有「凭证缺失」这一档。两个地区同一判据（只是查各自的账号集合）。
pub fn available_for(region: Region, store: &AccountStore, account_id: &str) -> bool {
    store.minimax_account_record(region, account_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::providers::minimax::balance::{SigninDay, SigninPanel};

    fn panel() -> SigninPanel {
        SigninPanel {
            scene: 2,
            days: vec![
                SigninDay { day_no: 1, points: 400.0, status: 3, is_today: false },
                SigninDay { day_no: 2, points: 400.0, status: 3, is_today: false },
                SigninDay { day_no: 3, points: 400.0, status: 2, is_today: true },
                SigninDay { day_no: 4, points: 1000.0, status: 1, is_today: false },
                SigninDay { day_no: 5, points: 400.0, status: 1, is_today: false },
                SigninDay { day_no: 6, points: 400.0, status: 1, is_today: false },
                SigninDay { day_no: 7, points: 1000.0, status: 1, is_today: false },
            ],
        }
    }

    #[test]
    fn summary_reports_today_and_the_cycle() {
        let summary = summarize(&panel());
        assert_eq!(summary["claimable"], Value::Bool(true));
        assert_eq!(summary["claimed"], Value::Bool(false));
        assert_eq!(summary["dayNo"], Value::from(3));
        assert_eq!(summary["points"], Value::from(400.0));
        assert_eq!(summary["cyclePoints"], Value::from(CYCLE_POINTS));
        assert_eq!(summary["days"].as_array().map(Vec::len), Some(CYCLE_DAYS));
    }

    #[test]
    fn cycle_constants_match_the_measured_panel() {
        // 400+400+400+1000+400+400+1000 = 4000（实测一轮的总额）
        let total: f64 = panel().days.iter().map(|day| day.points).sum();
        assert_eq!(total, CYCLE_POINTS);
        assert_eq!(panel().days.len(), CYCLE_DAYS);
    }
}
