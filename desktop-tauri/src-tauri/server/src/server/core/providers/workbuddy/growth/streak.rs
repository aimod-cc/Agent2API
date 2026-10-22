//! WorkBuddy **连登管家**（growth 域的 streak 子系统；从参考项目
//! `internal/upstream/streak.go` + `internal/upstream/blackcat.go` 的补签部分
//! + `internal/scheduler/streak.go` 移植）。
//!
//! ── 这是什么 ────────────────────────────────────────────────
//! 上游「成长中心」的连登玩法有三层，彼此依赖：
//!
//! ```text
//! 连续登录天数（streak.days）
//!   ├─ 7d / 14d / 28d 档位：解锁后兑换发 credit / energy / 补签卡 / 抽奖次数
//!   └─ 抽奖次数：**只能**从档位兑换获得，兑完立刻可抽
//! 补签卡：对漏签的某一天补上，保住连续天数（断了要重攒 7 天）
//! ```
//!
//! 三件事的共同点：**都没人愿意天天盯着**。档位解锁当天不兑换，到下一天
//! 仍然是「已解锁未兑换」；抽奖次数不抽就烂在那里；昨天漏签不补，连登归零。
//! 所以它们被组织成「签到末尾跑一遍」的闭环（参考项目 `RunStreakBonusNow`）：
//!
//! ```text
//! ① 昨日漏签 + 有补签卡 → 补签
//! ② 拉连登完整状态 → 对每个 unlocked 档位兑换
//! ③ 拉抽奖余额 → 抽完
//! ```
//!
//! ── 为什么幂等是硬要求 ──────────────────────────────────────
//! 这段每天都会跑（可能一天跑两次：定时签到 + 手动签到）。上游对三件事都是
//! 幂等的：`locked` / `claimed` 档位再兑返回 403 / 业务错误，补签过的日期
//! 再补返回业务错误，抽奖次数扣完就是 0。因此**不做本地去重状态** ——
//! 上游状态就是唯一事实源，本地记一份只会在多实例部署时与上游分叉。
//!
//! ── 企业版与 global 的门控 ──────────────────────────────────
//! 企业版账号没有个人成长体系（`/activity/growth/streak` 上游一律 403），
//! 国际版（global）没有 CN 任务中心 —— 两者都在调用方（`run_for_accounts`）
//! 就跳过，连一次上游请求都不发。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use super::{chat_call, GrowthAccount};

/// 连登兑换档位路径（growth 域）。
const REDEEM_PATH: &str = "/activity/growth/redeem";
/// 抽奖余额（GET）与抽奖（POST）。
const LOTTERY_SUMMARY_PATH: &str = "/activity/growth/lottery/summary";
const LOTTERY_DRAW_PATH: &str = "/activity/growth/lottery/draw";
/// 连登完整状态（含档位解锁状态与补签卡余额）。
const STREAK_PATH: &str = "/activity/growth/streak";
/// 热力图（判昨日是否漏签）。
const HEATMAP_PATH: &str = "/activity/growth/heatmap";
/// 使用补签卡。
const MAKEUP_USE_PATH: &str = "/activity/growth/makeup-cards/use";

/// 三个档位（顺序即兑换顺序：小的先兑，收益确定性更高）。
pub const TIERS: [&str; 3] = ["7d", "14d", "28d"];

/// 抽奖单轮护栏：一次闭环最多抽这么多次。上游的 chances 理论上不会超过
/// 兑换所得（三档合计几次），给 20 是防御「上游返回一个离谱数字」把账号
/// 抽成筛子（参考项目同款护栏）。
pub const MAX_DRAWS: usize = 20;

// ─── 读侧：连登完整状态 ─────────────────────────────────────

/// 一个档位的解锁状态与奖励内容。
#[derive(Clone, Debug, Default)]
pub struct Tier {
    pub tier: String,
    /// `locked` / `unlocked` / `claimed`（上游原文，未识别的值原样保留）
    pub status: String,
    pub credit: i64,
    pub energy: i64,
    pub cards: i64,
    pub chances: i64,
}

impl Tier {
    /// 可兑换：已解锁且未领取。
    ///
    /// 只认 `unlocked` 而不是「非 locked 且非 claimed」：上游以后若加出
    /// `expired` 之类的新状态，按「可兑换」去试会稳定报错；只认已知的可兑换
    /// 态，遇到没见过的值就跳过并如实记进报告。
    fn redeemable(&self) -> bool {
        self.status == "unlocked"
    }
}

/// 连登完整状态（`GET /activity/growth/streak` 的 `data` 段）。
#[derive(Clone, Debug, Default)]
pub struct StreakFull {
    pub days: i64,
    /// 补签卡余额（>0 才可能补签）
    pub makeup_cards: i64,
    pub tiers: Vec<Tier>,
}

impl StreakFull {
    fn from_data(data: &Value) -> Self {
        let days = data
            .get("streak")
            .and_then(|streak| streak.get("days"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let makeup_cards = data
            .get("makeup_cards")
            .and_then(|cards| cards.get("balance"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        // 档位清单在上游有两个来源：`redemption_status.tiers[]`（带奖励明细）
        // 与 `redemption_status.tier_7d_status` 三个扁平字段（只有状态）。
        // 优先用前者（兑换要 credit/energy 读数给界面）；它缺失时用后者补出
        // 「只有状态、没有奖励读数」的档位，至少让兑换能跑。
        let status = data.get("redemption_status").cloned().unwrap_or(Value::Null);
        let mut tiers: Vec<Tier> = status
            .get("tiers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let tier = item.get("tier").and_then(Value::as_str)?.to_string();
                        let tier_status = status
                            .get(format!("tier_{tier}_status"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        Some(Tier {
                            tier,
                            status: tier_status,
                            credit: item.get("credit").and_then(Value::as_i64).unwrap_or(0),
                            energy: item.get("energy").and_then(Value::as_i64).unwrap_or(0),
                            cards: item.get("cards").and_then(Value::as_i64).unwrap_or(0),
                            chances: item.get("chances").and_then(Value::as_i64).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if tiers.is_empty() {
            for tier in TIERS {
                let status_text = status
                    .get(format!("tier_{tier}_status"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if status_text.is_empty() {
                    continue;
                }
                tiers.push(Tier { tier: tier.to_string(), status: status_text, ..Default::default() });
            }
        }
        Self { days, makeup_cards, tiers }
    }
}

/// 拉连登完整状态。
pub async fn streak_full(account: &GrowthAccount) -> Result<StreakFull, String> {
    let call = chat_call(account, "GET", STREAK_PATH, None, &[]).await?;
    if !call.ok() {
        return Err(format!(
            "连登状态查询失败（HTTP {} code={} {}）",
            call.status,
            call.code().map(|code| code.to_string()).unwrap_or_else(|| "-".to_string()),
            call.msg()
        ));
    }
    Ok(StreakFull::from_data(call.data()))
}

/// 昨日是否漏签（`GET /activity/growth/heatmap` 的 cell.score == 0）。
///
/// 热力图里没有昨天那一格 = 「查不出」，返回 false（不补签）—— 宁可漏补
/// 也不要在拿不准的时候乱用补签卡（那是稀缺资源）。
pub async fn yesterday_missed(account: &GrowthAccount) -> Result<bool, String> {
    let call = chat_call(account, "GET", HEATMAP_PATH, None, &[]).await?;
    if !call.ok() {
        return Err(format!("热力图查询失败（HTTP {}）", call.status));
    }
    let yesterday = yesterday_key();
    let missed = call
        .data()
        .get("cells")
        .and_then(Value::as_array)
        .map(|cells| {
            cells.iter().any(|cell| {
                let date = cell.get("date").and_then(Value::as_str).unwrap_or("");
                // 上游日期可能带时分秒（`2026-10-17T00:00:00`），只比前 10 位
                date.get(..10) == Some(yesterday.as_str())
                    && cell.get("score").and_then(Value::as_i64).unwrap_or(0) == 0
            })
        })
        .unwrap_or(false);
    Ok(missed)
}

/// 北京时间的昨天（`YYYY-MM-DD`）。
///
/// 用 `core::beijing` 而不是机器本地时区：上游的自然日按 UTC+8 切
/// （与签到同一口径，见 `core::beijing` 的模块头）。
fn yesterday_key() -> String {
    let now = crate::server::core::beijing::now();
    crate::server::core::beijing::date_key(now - chrono::Duration::days(1))
}

// ─── 写侧：兑换 / 补签 / 抽奖 ───────────────────────────────

/// 幂等令牌（上游前端用 `randomUUID()`，语义是「同一次点击只兑一次」）。
///
/// 格式对齐 UUID v4 的字面形态（8-4-4-4-12 小写 hex）：上游把它当不透明串，
/// 但照着前端形状发最稳。随机源用 `uuid` 之外的手段 —— 这里只需要 16 字节
/// 熵，用系统时间 + 进程内计数器混合即可（本仓没有随机数依赖，为这一个
/// 令牌引一个不值得）。
fn client_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = crate::server::logging::now_ms() as u64;
    let mut state = now ^ (seq.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut bytes = [0u8; 16];
    for byte in bytes.iter_mut() {
        // xorshift64：够用且无依赖（令牌只要求不重复，不参与任何校验）
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xff) as u8;
    }
    // UUID v4 形态（版本位 / 变体位），与前端 randomUUID 同形状
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 兑换一个档位。未解锁（403）属预期，由调用方按状态跳过。
pub async fn redeem_tier(account: &GrowthAccount, tier: &str) -> Result<(), String> {
    let call = chat_call(
        account,
        "POST",
        REDEEM_PATH,
        Some(&json!({ "tier": tier, "client_token": client_token() })),
        &[],
    )
    .await?;
    if call.ok() {
        Ok(())
    } else {
        Err(format!(
            "兑换 {tier} 失败（HTTP {} code={} {}）",
            call.status,
            call.code().map(|code| code.to_string()).unwrap_or_else(|| "-".to_string()),
            call.msg()
        ))
    }
}

/// 用补签卡补指定日期。
pub async fn use_makeup_card(account: &GrowthAccount, date: &str) -> Result<(), String> {
    let call = chat_call(
        account,
        "POST",
        MAKEUP_USE_PATH,
        Some(&json!({ "target_date": date })),
        &[],
    )
    .await?;
    if call.ok() {
        Ok(())
    } else {
        Err(format!(
            "补签 {date} 失败（HTTP {} {}）",
            call.status,
            call.msg()
        ))
    }
}

/// 当前抽奖次数（`GET /activity/growth/lottery/summary`）。
pub async fn lottery_chances(account: &GrowthAccount) -> Result<i64, String> {
    let call = chat_call(account, "GET", LOTTERY_SUMMARY_PATH, None, &[]).await?;
    if !call.ok() {
        return Err(format!("抽奖余额查询失败（HTTP {}）", call.status));
    }
    Ok(call.data().get("chances").and_then(Value::as_i64).unwrap_or(0))
}

/// 抽一次奖，返回奖品载荷原文（形状随活动期变化，透传给调用方）。
pub async fn lottery_draw(account: &GrowthAccount) -> Result<Value, String> {
    let call = chat_call(
        account,
        "POST",
        LOTTERY_DRAW_PATH,
        Some(&json!({ "client_token": client_token() })),
        &[],
    )
    .await?;
    if !call.ok() {
        return Err(format!(
            "抽奖失败（HTTP {} code={} {}）",
            call.status,
            call.code().map(|code| code.to_string()).unwrap_or_else(|| "-".to_string()),
            call.msg()
        ));
    }
    Ok(call.data().clone())
}

// ─── 闭环：单账号 ───────────────────────────────────────────

/// 单账号闭环的一步结果（界面与日志按行渲染）。
#[derive(Clone, Debug)]
pub struct StepReport {
    /// makeup / redeem / draw / skip / failed
    pub outcome: &'static str,
    pub message: String,
}

/// 单账号跑完「补签 → 兑换 → 抽奖」三件事。
///
/// 返回逐条报告（**空数组** = 三件事都无事可做，属正常态）。任何一步失败
/// 都不中断后续步骤：补签失败不该拖累兑换，兑换失败不该拖累抽奖 ——
/// 三者在上游互不依赖，且都是幂等的，下一次还会重来。
///
/// 调用方负责企业版 / global 的门控（见模块头）。
pub async fn run_once(account: &GrowthAccount) -> Vec<StepReport> {
    let mut reports: Vec<StepReport> = Vec::new();

    // ── ① 补签：昨日漏签 + 有卡才补 ──
    // 先查热力图而不是先查卡：漏签是最常见的「无事可做」态，热力图一次
    // 只读请求就能排除掉大部分轮次，不必再拉一次 streak。
    match yesterday_missed(account).await {
        Ok(true) => match streak_full(account).await {
            Ok(full) if full.makeup_cards > 0 => {
                let date = yesterday_key();
                match use_makeup_card(account, &date).await {
                    Ok(()) => reports.push(StepReport {
                        outcome: "makeup",
                        message: format!("已用补签卡补签 {date}（保连登）"),
                    }),
                    Err(error) => reports.push(StepReport {
                        outcome: "failed",
                        message: error,
                    }),
                }
            }
            Ok(full) => {
                // 有漏签但没卡：如实记一条跳过（这是用户最该知道的一类）
                reports.push(StepReport {
                    outcome: "skip",
                    message: format!(
                        "昨日漏签但无补签卡（余额 {}），连登可能中断",
                        full.makeup_cards
                    ),
                });
            }
            Err(error) => reports.push(StepReport { outcome: "failed", message: error }),
        },
        Ok(false) => {}
        Err(error) => {
            // 热力图失败不阻断后面的兑换 / 抽奖：那是两件独立的事
            reports.push(StepReport { outcome: "failed", message: error });
        }
    }

    // ── ② 兑换：逐个已解锁档位 ──
    let full = match streak_full(account).await {
        Ok(full) => Some(full),
        Err(error) => {
            reports.push(StepReport { outcome: "failed", message: error });
            None
        }
    };
    if let Some(full) = &full {
        for tier in full.tiers.iter().filter(|tier| tier.redeemable()) {
            match redeem_tier(account, &tier.tier).await {
                Ok(()) => reports.push(StepReport {
                    outcome: "redeem",
                    message: format!(
                        "兑换 {} 档（+{}c +{}e 补签卡×{} 抽奖×{}）",
                        tier.tier, tier.credit, tier.energy, tier.cards, tier.chances
                    ),
                }),
                Err(error) => reports.push(StepReport { outcome: "failed", message: error }),
            }
        }
    }

    // ── ③ 抽奖：把 chances 抽空（兑换刚发的次数已在服务端累加）──
    match lottery_chances(account).await {
        Ok(chances) if chances > 0 => {
            let limit = (chances as usize).min(MAX_DRAWS);
            let mut drawn = 0usize;
            for index in 0..limit {
                match lottery_draw(account).await {
                    Ok(prize) => {
                        drawn += 1;
                        reports.push(StepReport {
                            outcome: "draw",
                            message: format!("第 {} 抽：{}", index + 1, compact_prize(&prize)),
                        });
                    }
                    Err(error) => {
                        // 抽奖失败即停（多半是次数耗尽或活动收摊），不再往下试
                        reports.push(StepReport { outcome: "failed", message: error });
                        break;
                    }
                }
            }
            if drawn > 0 {
                reports.push(StepReport {
                    outcome: "draw",
                    message: format!("抽奖完成 {drawn} 次"),
                });
            }
        }
        Ok(_) => {}
        Err(error) => reports.push(StepReport { outcome: "failed", message: error }),
    }

    reports
}

/// 奖品载荷压成一行（日志与界面行都只要「抽到了什么」）。
///
/// 上游的 prize 形状随活动期变化（可能是对象、也可能是带嵌套奖励的数组），
/// 所以不解析结构、只取可读串并截断 —— 猜字段名会在活动换期时静默变成空。
fn compact_prize(prize: &Value) -> String {
    let text = match prize {
        Value::String(text) => text.clone(),
        Value::Null => "（空）".to_string(),
        other => other.to_string(),
    };
    let chars: Vec<char> = text.chars().take(160).collect();
    if chars.len() < text.chars().count() {
        format!("{}…", chars.into_iter().collect::<String>())
    } else {
        text
    }
}

// ─── 闭环：多账号 ───────────────────────────────────────────

/// 一个账号的闭环汇总（供签到结果行 / 日志使用）。
pub struct AccountStreakReport {
    pub account_id: String,
    pub name: String,
    /// 三件事里真正做成的（makeup / redeem / draw）
    pub done: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

/// 对一批账号跑闭环（**串行** + 账号间错峰）。
///
/// ── 为什么要串行且错峰 ──────────────────────────────────────
/// 参考项目在账号之间插 0.8s（旅行/活跃）到 2~5s（巡检）的延时：整批账号
/// 同一秒打上游是最容易被风控识别的形状。这里的动作量比签到小（通常
/// 3~5 个请求/账号），给 800ms 与参考项目的 `travelAccountDelay` 同口径。
///
/// ── 门控（为什么在这里而不是调用方）─────────────────────────
/// 企业版与 global 的判据来自账号记录，本模块拿得到 —— 放在这里，
/// 调用方（签到末尾 / 手动入口）就不必各自抄一遍。
pub async fn run_for_accounts(
    store: &crate::server::core::account_store::AccountStore,
    accounts: &[Value],
) -> Vec<AccountStreakReport> {
    /// 账号间错峰（毫秒）：与参考项目 `travelAccountDelay` 同口径。
    const ACCOUNT_GAP_MS: u64 = 800;

    let mut reports: Vec<AccountStreakReport> = Vec::new();
    let mut first = true;
    for account in accounts {
        let id = account.get("id").and_then(Value::as_str).unwrap_or("");
        let name = account
            .get("name")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .or_else(|| account.get("nickname").and_then(Value::as_str))
            .unwrap_or(id)
            .to_string();
        if id.is_empty() {
            continue;
        }
        // 门控：只服务 WorkBuddy 国内版的个人账号
        if !super::supports_growth(account) {
            continue;
        }
        if account
            .get("enterpriseId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        let Some(growth_account) = GrowthAccount::from_store(store, id) else {
            continue;
        };
        if !first {
            tokio::time::sleep(std::time::Duration::from_millis(ACCOUNT_GAP_MS)).await;
        }
        first = false;
        let steps = run_once(&growth_account).await;
        let done = steps
            .iter()
            .filter(|step| matches!(step.outcome, "makeup" | "redeem" | "draw"))
            .count();
        let failed = steps.iter().filter(|step| step.outcome == "failed").count();
        if done == 0 && failed == 0 {
            // 三件事都无事可做：不进报告（否则每轮几十条噪音）
            continue;
        }
        let messages: Vec<String> = steps.iter().map(|step| step.message.clone()).collect();
        for message in &messages {
            crate::server::logging::log(
                "[Streak]",
                &format!("{name}: {message}"),
            );
        }
        reports.push(AccountStreakReport {
            account_id: id.to_string(),
            name,
            done,
            failed,
            messages,
        });
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_redeemable_only_for_unlocked() {
        let mut tier = Tier { tier: "7d".into(), status: "unlocked".into(), ..Default::default() };
        assert!(tier.redeemable());
        tier.status = "locked".into();
        assert!(!tier.redeemable());
        tier.status = "claimed".into();
        assert!(!tier.redeemable());
        // 没见过的状态不当可兑换（避免稳定报错）
        tier.status = "expired".into();
        assert!(!tier.redeemable());
    }

    #[test]
    fn streak_full_reads_tiers_with_rewards() {
        let data = json!({
            "streak": { "days": 9 },
            "makeup_cards": { "balance": 2 },
            "redemption_status": {
                "tier_7d_status": "unlocked",
                "tiers": [
                    { "tier": "7d", "credit": 100, "energy": 5, "cards": 1, "chances": 2 },
                    { "tier": "14d", "credit": 200, "energy": 8, "cards": 1, "chances": 3 }
                ]
            }
        });
        let full = StreakFull::from_data(&data);
        assert_eq!(full.days, 9);
        assert_eq!(full.makeup_cards, 2);
        assert_eq!(full.tiers.len(), 2);
        // 7d 的状态来自扁平字段，奖励读数来自 tiers 条目
        assert_eq!(full.tiers[0].status, "unlocked");
        assert_eq!(full.tiers[0].credit, 100);
        assert_eq!(full.tiers[0].chances, 2);
        // 14d 没有对应的 tier_14d_status → 空状态，不可兑换
        assert_eq!(full.tiers[1].status, "");
        assert!(!full.tiers[1].redeemable());
    }

    #[test]
    fn streak_full_falls_back_to_flat_statuses() {
        let data = json!({
            "streak": { "days": 7 },
            "redemption_status": { "tier_7d_status": "claimed", "tier_28d_status": "locked" }
        });
        let full = StreakFull::from_data(&data);
        assert_eq!(full.tiers.len(), 2);
        assert_eq!(full.tiers[0].tier, "7d");
        assert_eq!(full.tiers[1].tier, "28d");
    }

    #[test]
    fn client_token_has_uuid_shape() {
        let token = client_token();
        let parts: Vec<&str> = token.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        // 版本位 4 / 变体位 8..b
        assert!(parts[2].starts_with('4'));
        assert!(matches!(&parts[3][0..1], "8" | "9" | "a" | "b"));
        // 连续两次不重复
        assert_ne!(token, client_token());
    }

    #[test]
    fn compact_prize_truncates_long_payloads() {
        let long = json!({ "name": "x".repeat(400) });
        let text = compact_prize(&long);
        assert!(text.chars().count() <= 161);
        assert!(text.ends_with('…'));
        assert_eq!(compact_prize(&json!(null)), "（空）");
    }
}
