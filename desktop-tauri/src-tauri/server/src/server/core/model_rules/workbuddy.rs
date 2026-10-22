//! WorkBuddy 的模型规则：默认启用白名单种子 + **拆家迁移**（2026-10）。
//!
//! ── 为什么单独一个文件 ──────────────────────────────────────
//! 与 `model_rules/cline.rs` 同一理由：这一家的规则逻辑（白名单种子 + provider
//! 拆分的存量迁移）自成一块，而 `mod.rs` 已经远超本项目的单文件行数约定
//! （1349 行）。拆出来之后 `mod.rs` 只留「与哪一家无关」的规则内核，各家的
//! 私有知识各占一个文件。`pub use` 让调用方仍写
//! `model_rules::seed_workbuddy_defaults` / `model_rules::migrate_workbuddy_split`。
//!
//! ── 本文件里的两条事实（改动前先读）──────────────────────────
//!   1. **规则按 `(provider, id)` 记账**，而 WorkBuddy 拆家后是两个 provider
//!      （`workbuddy` / `workbuddy-intl`）—— 因此「白名单种子」必须按家各跑一遍
//!      （见 `seed_workbuddy_defaults` 的 `provider` 参数）；
//!   2. **存量规则只有 `workbuddy` 一份**（拆家前不存在第二家），迁移要把它们
//!      **复制**一份给国际版 —— 复制而不是改名，因为国内版仍然是那个 id。
//!      只改名的写法（Cline 那次）在这里会把国内版的规则整批搬走。
//!
//! 消费者的契约不变：两条函数都幂等、无改动时**不落盘**（启动路径上只读不写
//! 的成本必须为零），返回 `Option<String>` 只为给调用方一句日志。

use super::{current, save};

/// WorkBuddy 清单的**默认启用白名单**：已停用（改为按可见性判），保留常量作
/// 文档与迁移依据。
///
/// ── 为什么停用（2026-10-17，与 Qoder 同一轮）───────────────────
/// 这条白名单只允许 3 个模型默认启用，其余一律默认禁用。它的前提是「清单里
/// 混着大量不该广告的名字，靠人工挑几个最安全的」。现在那个前提被**真正的
/// 判据**取代了：`/v3/config` 的 `agents[cli].models` 是官方客户端的 picker
/// 名单（实测 18 条，而 `data.models` 有 60 条），按它标 `visibility` 之后，
/// 「该关什么」由上游说了算。
///
/// 两套机制叠在一起会有个恶果：白名单关掉的那些**本来可见**的模型（`glm-5.3`
/// / `kimi-k3-1` / `minimax-m3` …）在管理页显示「可见」、开关却是关的 ——
/// 用户看到的是自相矛盾的状态。与 Qoder 那次修的是同一个问题。
///
/// 保留这个常量（而不是删掉）有两个用途：一是迁移要按它的历史形态判断
/// 「谁被误关过」；二是将来若要恢复人工白名单，这里有现成的形状。
pub const WORKBUDDY_DEFAULT_ENABLED: &[&str] = &["hy3", "hy4-preview-f", "deepseek-v4.1-flash"];

/// WorkBuddy 清单的**默认规则种子**：已停用，不再关任何模型。
///
/// ── 为什么不再调 `seed_default_enabled`（2026-10-17）─────────────
/// 与 [`super::seed_qoder_defaults`] 完全同因：该关的由可见性判据负责
/// （[`super::seed_hidden_disabled`]，调用点在 `providers::adapter`），白名单
/// 再关一遍只会把**可见的也关掉**。
///
/// 保留函数外壳是为了不惊动调用点（`core::models` 的 `apply_remote` 与
/// `providers::adapter::seed_current_workbuddy_defaults` 都还在调它）。
///
/// ⚠️ 存量用户盘上已有旧白名单种的 `disabled` 记录，靠
/// [`super::migrate_workbuddy_default_enabled`] 一次性解开。
pub fn seed_workbuddy_defaults(_provider: &str, _ids: &[String]) -> Option<String> {
    None
}

/// 拆家前后的 provider id（迁移专用；别处一律走 `workbuddy::Region`）
const LEGACY_PROVIDER: &str = "workbuddy";

/// WorkBuddy 拆家的**存量规则迁移**（2026-10）。
///
/// ── 迁什么、为什么是「复制」─────────────────────────────────
/// 拆家前「WorkBuddy 的规则」这一份数据同时覆盖国内版与国际版（只有一个
/// provider，也就只有一份规则）。拆家后国内版仍是 `workbuddy`，国际版是新 id，
/// 于是那份规则的**适用范围少了一半** —— 不迁的话：
///   - 用户手动启用过的模型在国际版组里回到「默认禁用」（种子会把它关掉），
///     而广告视图是**门禁**，客户端点它直接 400 `model_not_found`；
///   - 用户建的别名在国际版下失效（别名带 provider 归属）；
///   - 手动登记的「这家还有这个模型」在国际版模型页消失。
///
/// 因此把 `workbuddy` 名下的五类条目都照顾到：`disabled`（启停）、
/// `mappings`（别名 / 发送名改写 / 思考等级绑定）、`custom`（手动登记的模型）、
/// `capabilities`（能力位覆盖）、`seeded`（种子标记）。
///
/// ── 为什么「复制」而不是「改名」（与 Cline 那次的差别）───────────
/// `migrate_cline_split` 把旧 `cline:` 键**改名**到两个池：那时旧 id 被废弃，
/// 规则本来就该整体搬家。本家的 `workbuddy` 仍然存在（国内版就是它），
/// 改名会把国内版的规则整批搬走、留下一个没有任何规则的空家。
///
/// ── **只跑一次**（调用方的责任，别单独调这个函数）──────────────
/// 复制类迁移没有 Cline 改名那种自识别性：拆家之后用户在国内版做的一次启停
/// （「这个模型我只想在国内版用」）会被下一次运行again 同步给国际版，反复
/// 覆盖用户的明确意图。因此它由 `account_bootstrap` 与另外两条拆家迁移
/// **共用一个一次性标记**（`config::workbuddy_split_migrated`）统一编排。
///
/// 新家自己的规则（用户在拆家后给国际版单独设的）一个字节都不动。
pub fn migrate_workbuddy_split() -> (SplitMigrationOutcome, Option<String>) {
    let intl = crate::server::core::providers::workbuddy::Region::Intl.provider_id();
    let mut rules = current();
    let mut summary: Vec<String> = Vec::new();

    // 国内版「有意见的模型 id」= 它的**种子标记** ∪ 它的**启停条目**。
    //
    // 为什么两者都要：种子标记是「见过这个模型」的凭据，而启停条目里可能有
    // 种子里没有的 id —— 老配置的纯 id 兼容（`is_seeded` 对 workbuddy / raccoon
    // 保留的旧版形态）会让一批模型记不上种子键，但用户对它们的启停决定
    // （比如手动关掉白名单里的 `hy3`）**就是**国内版的明确状态，必须一起继承。
    // 只按种子取会让这些 id 悄悄漏掉，症状是「国际版里 hy3 开着、国内版关着」。
    let mut known_ids: Vec<String> = rules
        .seeded
        .iter()
        .filter_map(|key| key.strip_prefix("workbuddy:"))
        .map(|rest| rest.split('#').next().unwrap_or(rest).to_string())
        .collect();
    for entry in rules.disabled.iter() {
        if entry.provider.as_deref() != Some(LEGACY_PROVIDER) {
            continue;
        }
        if !known_ids.iter().any(|id| id.eq_ignore_ascii_case(&entry.id)) {
            known_ids.push(entry.id.clone());
        }
    }

    // ① 启停列表：让国际版的启停状态**等于**国内版（拆家前的实际行为）。
    //
    // ── 为什么是「同步」而不是「把国内版的条目复制一份」──────────
    // 启动顺序上，国际版的种子很可能**已经跑过**（`restore_cached_catalogs`
    // 排在 `account_bootstrap` 之前）：它会按白名单把大部分模型默认禁用。
    // 若这里只做「目标家还没有才添加」的复制，那些「已被种子禁用」的 id 就会
    // 被判成「已存在」而跳过 —— 用户手动启用过的模型在国际版里留在禁用状态，
    // 而广告视图是门禁，客户端点它直接 400 `model_not_found`。
    // 因此以国内版为准：它禁用的，国际版也禁用；它没禁用的，把国际版的
    // 禁用条目**删掉**（把种子的默认值纠正回来）。
    //
    // 只动**国际版专属**的条目：provider 为 None 的历史全局条目对两家都生效
    // （那种情况下 `cn_disabled` 也为真，走不到删除分支），不能顺手删掉。
    let mut changed = 0usize;
    for id in &known_ids {
        let cn_disabled = rules
            .disabled
            .iter()
            .any(|entry| entry.matches(LEGACY_PROVIDER, id));
        let intl_disabled = rules
            .disabled
            .iter()
            .any(|entry| entry.matches(intl, id));
        if cn_disabled && !intl_disabled {
            rules.disabled.push(super::RuleEntry::new(Some(intl), id));
            changed += 1;
        } else if !cn_disabled && intl_disabled {
            rules.disabled.retain(|entry| {
                !(entry
                    .provider
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(intl))
                    && entry.id.eq_ignore_ascii_case(id))
            });
            changed += 1;
        }
    }
    if changed > 0 {
        summary.push(format!("启停同步 {changed} 条"));
    }

    // ② 别名 / 发送名映射（`(alias, target, provider)` 三元组唯一）。
    //    种子机制不碰映射表，因此这里「目标家还没有才添加」就是完整语义。
    let mut added = 0usize;
    let copies: Vec<super::Mapping> = rules
        .mappings
        .iter()
        .filter(|mapping| mapping.provider.as_deref() == Some(LEGACY_PROVIDER))
        .filter(|mapping| {
            !rules.mappings.iter().any(|existing| {
                existing.alias.eq_ignore_ascii_case(&mapping.alias)
                    && existing.target.eq_ignore_ascii_case(&mapping.target)
                    && existing
                        .provider
                        .as_deref()
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(intl))
            })
        })
        .cloned()
        .collect();
    for mut mapping in copies {
        mapping.provider = Some(intl.to_string());
        rules.mappings.push(mapping);
        added += 1;
    }
    if added > 0 {
        summary.push(format!("映射 {added} 条"));
    }

    // ③ 手动登记的模型（`(provider, id)`）
    let mut added = 0usize;
    let copies: Vec<super::CustomModel> = rules
        .custom
        .iter()
        .filter(|item| item.provider.eq_ignore_ascii_case(LEGACY_PROVIDER))
        .filter(|item| !rules.custom.iter().any(|existing| existing.matches(intl, &item.id)))
        .cloned()
        .collect();
    for mut item in copies {
        item.provider = intl.to_string();
        rules.custom.push(item);
        added += 1;
    }
    if added > 0 {
        summary.push(format!("自定义模型 {added} 条"));
    }

    // ④ 能力位覆盖（`(provider, id)`）
    let mut added = 0usize;
    let copies: Vec<super::CapabilityOverride> = rules
        .capabilities
        .iter()
        .filter(|item| item.provider.eq_ignore_ascii_case(LEGACY_PROVIDER))
        .filter(|item| !rules.capabilities.iter().any(|existing| existing.matches(intl, &item.id)))
        .cloned()
        .collect();
    for mut item in copies {
        item.provider = intl.to_string();
        rules.capabilities.push(item);
        added += 1;
    }
    if added > 0 {
        summary.push(format!("能力位 {added} 条"));
    }

    // ⑤ 种子标记：`workbuddy:<id>[#alias:<别名>]` → 再补一条 `workbuddy-intl:<同一段>`。
    //    少了它，下一轮种子会把同步过去的「启用」状态又按白名单关掉
    //    （见函数头）。同样是「补缺失」，因此与启动顺序无关。
    let mut added = 0usize;
    let prefix = format!("{LEGACY_PROVIDER}:");
    let intl_prefix = format!("{intl}:");
    let copies: Vec<String> = rules
        .seeded
        .iter()
        .filter_map(|key| key.strip_prefix(&prefix))
        .map(|rest| format!("{intl_prefix}{rest}"))
        .filter(|key| !rules.seeded.iter().any(|existing| existing.eq_ignore_ascii_case(key)))
        .collect();
    for key in copies {
        rules.seeded.push(key);
        added += 1;
    }
    if added > 0 {
        summary.push(format!("种子标记 {added} 条"));
    }

    if summary.is_empty() {
        return (SplitMigrationOutcome::Nothing, None);
    }
    if !save(&rules) {
        // 规则没落盘：调用方据此**不写**一次性标记，下次启动重来一次 ——
        // 比「标记写了、规则没写」安全（后者会让用户的启停状态永久停在
        // 半迁移的位置）。
        return (
            SplitMigrationOutcome::Failed,
            Some(format!(
                "⚠️  WorkBuddy 拆家的模型规则同步未能落盘（{}）：下次启动会重试",
                summary.join("；")
            )),
        );
    }
    (
        SplitMigrationOutcome::Done,
        Some(format!(
            "🔀 WorkBuddy 已拆为国内版 / 国际版两家，存量模型规则已同步（{}）\
             —— 两家的启用、别名与能力位保持一致",
            summary.join("；")
        )),
    )
}

/// 一次拆家规则迁移的结果（调用方据此决定要不要写下一次性标记）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitMigrationOutcome {
    /// 没有可继承的内容（全新安装 / 国内版本来就没规则）—— 可以写标记
    Nothing,
    /// 已迁移且落盘成功 —— 可以写标记
    Done,
    /// 有内容要迁但**没落盘** —— **不要**写标记，下次启动重来
    Failed,
}

impl SplitMigrationOutcome {
    /// 这次的结果是否允许写下「已迁移」标记
    pub fn allows_marking_done(self) -> bool {
        !matches!(self, Self::Failed)
    }
}
