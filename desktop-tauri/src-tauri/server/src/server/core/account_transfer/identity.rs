//! 导入记录的身份判定：provider 校验、业务身份提取、桌面引用识别、唯一 id 与
//! 优先级分配。
//!
//! ── 为什么身份必须带 provider 作用域 ─────────────────────────
//! 各家的 id / uid 空间互相独立：CatPaw 的账号 id 就是 uid 或 loginName，
//! WorkBuddy 的 uid 也可能是同一串数字。只按 uid 匹配会把别家的记录整条覆写
//! （连凭证一起丢）。因此这里所有判定都产出「provider + 身份」二元组，
//! 由调用方按二元组匹配本机记录。
//!
//! 身份字段（各家落盘字段的事实来源见 account_store 各模块）：
//!   WorkBuddy 两地区 = uid；
//!   CatPaw / Kuku / OrcaTerm = uid（缺则 loginName；OrcaTerm 再缺账号 id）；
//!   raccoon / AutoClaw 两地区 = userId；
//!   Qoder 两地区 = 地区 + userId（添加路径 `qoder_accounts` 按「地区 + userId」
//!     判重，这里必须同口径 —— 只按 userId 会把国际版 / 中国版两条记录并成一条）；
//!   Cline 两池 = account；
//!   CodeArts = domainId + userId（同一个人不同华为云账号是两条，见该分支）；
//!   Trae = variant + uid（solo / cn 两个谱系是同一个人也是两条）；
//!   OfficeAce = accessKeyId（缺则账号 id）；
//!   Command Code = accessToken（= API Key，缺则账号 id）；
//!   Accio 两地区 = userId（缺则 accessToken，再缺账号 id）；
//!   Loomy = userId（缺则 phone，再缺账号 id）；
//!   MonkeyCode 两地区 = userId（缺则 accessToken，再缺账号 id）；
//!   Antigravity = userId（= email；缺则 refreshToken，再缺账号 id）；
//!   自定义提供商（custom- 前缀）= apiKey（缺则账号 id，与 `custom_accounts`
//!      「同 key 合并」的幂等口径一致）。
//! 剩下落到 `userId` 兜底的只有 raccoon / AutoClaw 两地区 / ZCode 两地区 ——
//! 那几家的记录确实只有 `userId` 一个身份键（ZCode / MonkeyCode 的地区已编码在
//! provider id 里，匹配又按 provider 作用域，与「地区 + userId」等效；MonkeyCode
//! 仍单列分支，因为它有一批**没有 userId** 的匿名账号，兜底会整条拒掉）。
//!
//! ── 加新家的检查清单（这里漏一支的代价是「账号导得出去、导不回来」）──
//! 只看「provider 有没有 `userId` 键」是不够的 —— 兜底读的就是 `userId`，
//! 所以**凡是不写 `userId` 的家都必须有专属分支**。2026-10-18 上午靠这条查出
//! 了 OfficeAce 与 OrcaTerm 两家的缺口（实测：导出→导入整条失败，报「缺少
//! userId」）；同日下午复查又按同一把尺子量出 **Accio / Loomy / MonkeyCode /
//! Antigravity** 四家 —— 它们写 userId，但都有一批**合法存在却没有 userId**
//! 的账号（不透明令牌解不出 id、只粘了 session、只粘了 refresh token），
//! 添加路径给这些账号安排了次级身份（同串令牌 / 手机号），导入侧却没有。
//! 加家时两件事都要做：
//!   ① `identity_of_item`（导入条目）与 `identity_of_record`（本机记录）
//!      **同时**加分支，且字段与缺省口径逐字一致；
//!   ② 用「导出→原样导入→二次导入」三步验证：第三步必须计 `updated`
//!      而不是 `added`（否则每条都判新增，会刷出一堆重复账号）。

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::{
    normalize_priority_value, DEFAULT_PRIORITY, MAX_PRIORITY, MIN_PRIORITY,
};
use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::providers::{
    is_known_provider_id, kind_id, ProviderKind, DEFAULT_PROVIDER_ID,
};

/// 桌面端实时登录态的固定账号 id（记录里按设计不落 token，凭证实时读客户端文件）。
///
/// **全局保留**：任何 provider 的导入记录用了这些 id 都跳过 —— 既避免把引用
/// 变成普通账号，也避免占住 id 让后续 importDesktop 无法创建/刷新。
///
/// AutoClaw 占两项（国内版 / 国际版各一个 id）：两地的桌面端账号是两条独立
/// 记录（读的是同一个 `auth.json`，但归不同的 provider），因此两个 id 都要
/// 保留 —— 漏了国际版那个，它的桌面端记录就能被当成普通账号导入，然后因为
/// 「不落 token」而永远 `available: false`。
pub(super) const RESERVED_DESKTOP_IDS: [&str; 4] = [
    crate::server::core::providers::raccoon::credentials::DESKTOP_ACCOUNT_ID,
    crate::server::core::providers::catpaw::credentials::DESKTOP_ACCOUNT_ID,
    crate::server::core::account_store::autoclaw_accounts::DESKTOP_ACCOUNT_ID,
    crate::server::core::account_store::autoclaw_accounts::INTL_DESKTOP_ACCOUNT_ID,
];

pub(super) fn is_reserved_desktop_id(id: &str) -> bool {
    if RESERVED_DESKTOP_IDS.iter().any(|known| *known == id) {
        return true;
    }
    // Cline 两个池的桌面端固定 id（`cline-free-desktop` / `cline-pass-desktop`，
    // 拼装规则见 `cline::credentials::desktop_account_id` —— 它返回 String，
    // 进不了上面的 const 数组，只能在这里现拼）。漏了它们，手工构造的
    // 「带 Cline 桌面 id 但不带 desktop 标记」的导入记录会占住这个 id，
    // 目标机器的「导入桌面端登录态」就建不出实时账号了。
    let suffix = crate::server::core::providers::cline::credentials::DESKTOP_ACCOUNT_SUFFIX;
    [
        kind_id(ProviderKind::ClineFree),
        kind_id(ProviderKind::ClinePass),
    ]
    .iter()
    .any(|provider| *id == format!("{provider}-{suffix}"))
}

/// 记录带 `desktop: true` 标记 = 桌面端实时登录态的引用，不是可迁移的普通账号。
pub(super) fn is_desktop_item(item: &Map<String, Value>) -> bool {
    matches!(item.get("desktop"), Some(Value::Bool(true)))
}

/// 解析记录声明的 provider。
///
/// 缺字段 / null / 空串 → 历史数据兼容为 WorkBuddy（旧导出文件没有 provider）；
/// 非空但不是注册表已知 id → Err（绝不把未知 id 当成 WorkBuddy 存进 WorkBuddy 组）。
///
/// ── 拆家前的旧导出要按地区纠正（2026-10）──────────────────────
/// WorkBuddy 拆家前只有一家，国际版账号在导出文件里长这样：
/// `{"provider": "workbuddy", "edition": "intl", ...}`。那份 provider 字段
/// 描述的是「旧版本里唯一的那一家」，不是「这条账号属于国内版」——
/// 照字面导进去会把国际版账号塞回国内版组（凭证是国际版的，转发会稳定打错
/// 站点），因此这里按记录自己的 `edition` 归位。国内版（含 edition 缺失）
/// 归位结果仍是 `workbuddy`，行为逐字不变；显式写了 `workbuddy-intl` 的
/// 新导出不受影响（那已经是拆家后的权威归属）。
///
/// **例外：`custom-` 前缀**（自定义提供商）。它是运行期数据、刻意不进注册表
/// （见 `custom_providers` 模块头），按注册表判会被整条拒掉 —— 那正是自定义
/// 账号「导得出、导不回」的根子。这里放行前缀，存在性校验由调用方在合并完
/// 导入文件里的提供商定义之后做（`custom_providers::get`）。
pub(super) fn resolve_provider(item: &Map<String, Value>) -> Result<String, String> {
    match item.get("provider") {
        None | Some(Value::Null) => Ok(DEFAULT_PROVIDER_ID.to_string()),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(DEFAULT_PROVIDER_ID.to_string());
            }
            if !trimmed.starts_with(crate::server::core::custom_providers::ID_PREFIX)
                && !is_known_provider_id(trimmed)
            {
                return Err(format!(
                    "未知的提供商 id「{trimmed}」：注册表不认识，不能当作 WorkBuddy 导入"
                ));
            }
            if trimmed == DEFAULT_PROVIDER_ID {
                let region = crate::server::core::providers::workbuddy::Region::from_edition_id(
                    item.get("edition").and_then(Value::as_str),
                );
                return Ok(region.provider_id().to_string());
            }
            Ok(trimmed.to_string())
        }
        Some(_) => Err("provider 字段必须是字符串".to_string()),
    }
}

/// Qoder 记录 / 导入条目上的地区标识（与 `qoder::Region::from_payload` 同键序：
/// `mode` 优先、`edition` 兜底）。
///
/// 三态返回：
///   - `Ok(Some(region))`：解析成功；
///   - `Ok(None)`：键缺失 → 调用方按国际版（添加路径对缺失的缺省就是国际版，
///     两处缺省必须一致，否则同一条记录两边算出的身份对不上）；
///   - `Err(())`：值存在但不是已知地区 —— 无法识别（条目失败 / 记录不参与匹配）。
fn qoder_region(fields: &Map<String, Value>) -> Result<Option<String>, ()> {
    let raw = match fields
        .get("mode")
        .filter(|value| !value.is_null())
        .or_else(|| fields.get("edition"))
    {
        Some(raw) => raw,
        None => return Ok(None),
    };
    match raw.as_str().ok_or(())?.trim() {
        "" | "global" | "intl" => Ok(Some("global".to_string())),
        "cn" => Ok(Some("cn".to_string())),
        _ => Err(()),
    }
}

/// 导入记录的业务身份；无法确定身份时 Err（该条失败，不做猜测性匹配）。
pub(super) fn identity_of_item(provider: &str, item: &Map<String, Value>) -> Result<String, String> {
    let text = |key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let uid = text("uid");
    let login_name = text("loginName");
    let user_id = text("userId");
    // 「workbuddy 系」= 国内版 + 国际版：两家身份字段同形（uid），
    // 见 `providers::workbuddy::is_workbuddy_family` 的说明
    if crate::server::core::providers::workbuddy::is_workbuddy_family(provider) {
        if uid.is_empty() {
            return Err("缺少 uid（无法标识 WorkBuddy 账号）".to_string());
        }
        return Ok(uid);
    }
    if provider == kind_id(ProviderKind::CatPaw) {
        if !uid.is_empty() {
            return Ok(uid);
        }
        if !login_name.is_empty() {
            return Ok(login_name);
        }
        return Err("缺少 uid 与 loginName（无法标识 CatPaw 账号）".to_string());
    }
    // Kuku：账号标识是 `uid`（= userreport 的 uk，本家 id 也从它派生），缺则
    // loginName —— 与 CatPaw 同一套字段口径。没有这一支会落进兜底按 `userId`
    // 取，而本家记录没有该字段：账号导得出去、导不回来（报「缺少 userId」整条
    // 失败）。
    if provider == kind_id(ProviderKind::Kuku) {
        if !uid.is_empty() {
            return Ok(uid);
        }
        if !login_name.is_empty() {
            return Ok(login_name);
        }
        return Err("缺少 uid 与 loginName（无法标识 Kuku 账号）".to_string());
    }
    // Cline 的账号标识落在 `account`（邮箱或 `usr-…` id）而不是 `userId`
    // —— 见 `account_store::cline_accounts` 的公开形态。少了这一支，
    // Cline 账号导出后再导入会因「缺少 userId」整条失败。
    // **两个池共用这一支**（判据是「属于 Cline 系」，不是某个具体 provider id）。
    if crate::server::core::account_store::is_cline_family(provider) {
        let account = text("account");
        if !account.is_empty() {
            return Ok(account);
        }
        return Err("缺少 account（无法标识 Cline 账号）".to_string());
    }
    // CodeArts：`domain_id + user_id` 两段身份 —— 与 `codearts_accounts::same_identity`
    // 的判重口径逐字一致。只按 userId 会把**同一个人不同华为云账号（域）下的两条**
    // 并成一条：导入时后一条覆盖前一条，而被覆盖那条的一次性 refresh token
    // 就此作废（不是"少一条记录"，是"烧掉一份登录凭据"）。
    if provider == kind_id(ProviderKind::CodeArts) {
        let user_id = text("userId");
        if user_id.is_empty() {
            return Err("缺少 userId（无法标识 CodeArts 账号）".to_string());
        }
        return Ok(format!("{}\u{0}{}", text("domainId"), user_id));
    }
    // Qoder：地区 + userId 两段身份（与 `qoder_accounts` 添加路径的判重口径
    // 一致 —— 只按 userId 会把同一个人在两个地区的账号并成一条）。
    // 拆家后两个地区是两家 provider（`qoder` / `qoder-intl`），判据按家族走；
    // 地区仍从记录字段读（`mode` / `edition`），与 provider id 互为印证 ——
    // 迁移保证了存量记录两者一致。
    if crate::server::core::account_store::is_qoder_family(provider) {
        if user_id.is_empty() {
            return Err("缺少 userId（无法标识 Qoder 账号）".to_string());
        }
        let region = match qoder_region(item) {
            Ok(Some(region)) => region,
            Ok(None) => {
                // 键缺失：按 **provider id** 推（拆家后它是权威；导入条目
                // 一律带 provider），provider 也不认识时按国际版兜底（与
                // 添加路径对缺失 mode 的旧缺省一致）。
                crate::server::core::providers::qoder::endpoints::Region::from_provider_id(provider)
                    .map(|region| region.id().to_string())
                    .unwrap_or_else(|| "global".to_string())
            }
            Err(()) => {
                return Err("Qoder 地区无法识别（mode / edition 必须是 global 或 cn）".to_string())
            }
        };
        return Ok(format!("{region}:{user_id}"));
    }
    // Trae：账号标识是 `uid`（与 workbuddy / catpaw 同类，落在 uid 而不是 userId），
    // 且判重口径是 **(variant, uid) 两段** —— 见 `trae_accounts::add_trae_account`：
    // 同一个人可以在 solo 与 cn 两个谱系各有一条记录，只按 uid 会把两条并成一条。
    // 少了这一支的话，本家账号导得出去、导不回来（落到下面的 userId 兜底，
    // 报「缺少 userId（无法标识 trae 账号）」整条失败）。
    if provider == kind_id(ProviderKind::Trae) {
        if uid.is_empty() {
            return Err("缺少 uid（无法标识 Trae 账号）".to_string());
        }
        let variant = text("variant");
        return Ok(format!("{}:{uid}", if variant.is_empty() { "solo" } else { &variant }));
    }
    // 自定义提供商：凭证就是身份 —— apiKey 非空时与添加路径「同 key 合并」
    // 完全同口径（添加路径的账号 id 就是 key 的 SHA-256 前缀）；空 key 没有
    // 稳定标识，退到账号 id（空 key 的记录 id 是随机的，但全局唯一仍然成立）。
    if provider.starts_with(crate::server::core::custom_providers::ID_PREFIX) {
        let api_key = text("apiKey");
        if !api_key.is_empty() {
            return Ok(api_key);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 apiKey 与 id（无法标识自定义提供商账号）".to_string());
    }
    // OfficeAce（华为云办公云）：身份落在 `accessKeyId`（**控制面临时凭据**），
    // 不是 userId —— 本家记录根本没有 `userId` 键。少了这一支会落进最下面的
    // `userId` 兜底 → 报「缺少 userId（无法标识 officeace 账号）」整条失败：
    // 账号**导得出去、导不回来**，备份恢复时静默丢号。
    //
    // ── 为什么身份不是「基址 + appKey」──────────────────────────
    // 添加路径的 id 确实是 `officeace-<hash(基址|appKey)>`（见
    // `officeace_accounts::account_id_for`），但那是**id 的生成规则**，不是
    // 导入该用的身份：导入时 id 只作新增取名建议（本机可能因全局冲突改过名），
    // 匹配必须靠记录里的业务字段。用 accessKeyId 与「同一条凭据重复导入落同
    // 一条」这个目标一致 —— 同一份华为云凭据的 accessKeyId 恒定。
    //
    // 缺 accessKeyId 时退到账号 id（与 CommandCode / 自定义家「关键字段缺失用
    // id」同口径）：id 全局唯一，至少能保证导入的记录不与别的账号互串。
    if provider == kind_id(ProviderKind::OfficeAce) {
        let access_key_id = text("accessKeyId");
        if !access_key_id.is_empty() {
            return Ok(access_key_id);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 accessKeyId 与 id（无法标识 OfficeAce 账号）".to_string());
    }
    // OrcaTerm（腾讯云 OrcaTerm）：身份是 `uid`（= userId，但**落在 uid 键上**，
    // 与 CatPaw / Kuku / Trae 同类）。本家记录没有 `userId` 键，同样会落进
    // `userId` 兜底而失败。缺 uid 时退 loginName（与 CatPaw / Kuku 同款的二级
    // 兜底），再退账号 id。
    if provider == kind_id(ProviderKind::OrcaTerm) {
        let uid = text("uid");
        if !uid.is_empty() {
            return Ok(uid);
        }
        if !login_name.is_empty() {
            return Ok(login_name);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 uid 与 loginName（无法标识 OrcaTerm 账号）".to_string());
    }
    // Command Code：凭证就是身份 —— API Key 落盘在 `accessToken`（添加路径的
    // 账号 id 就是 key 的 SHA-256 前缀，与自定义家同构），记录上没有
    // userId / uid / apiKey 键。没有这一支会落进兜底按 `userId` 取 → 报
    // 「缺少 userId」整条失败；空 token 的记录退到账号 id（与自定义家
    // 「缺 apiKey 用 id」同口径）。
    if provider == kind_id(ProviderKind::CommandCode) {
        let access_token = text("accessToken");
        if !access_token.is_empty() {
            return Ok(access_token);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 accessToken 与 id（无法标识 Command Code 账号）".to_string());
    }
    // Accio（两个地区共用一套凭证形态）：身份是「userId，缺则 accessToken」——
    // 与 `accio_accounts::add_accio_account` 的判重口径逐字一致（那边是
    // `same_user || same_token`：有 userId 按 userId 匹配，没有就按同一串
    // accessToken 匹配）。Accio 的 accessToken 是**不透明串**，解不出 userId，
    // 网页登录也可能拿不到 —— 那类账号**合法存在**（`Credentials::to_value`
    // 把空 userId 落成 null，添加路径靠 same_token 防重）。导入侧没有这一支
    // 就会落进最下面的 userId 兜底：无 userId 的 Accio 账号「导得出去、
    // 导不回来」（报「缺少 userId」整条失败，备份恢复时静默丢号）。
    // 再退账号 id（与 OfficeAce / CommandCode 同款兜底）：id 全局唯一，
    // 至少保证导入的记录不与别的账号互串。
    if crate::server::core::account_store::is_accio_family(provider) {
        if !user_id.is_empty() {
            return Ok(user_id);
        }
        let access_token = text("accessToken");
        if !access_token.is_empty() {
            return Ok(access_token);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 userId 与 accessToken（无法标识 Accio 账号）".to_string());
    }
    // Loomy：身份是「userId，缺则手机号」—— 与 `loomy_accounts::add_loomy_account`
    // 同口径（那边明确要求「userId 或手机号至少给一个」，账号 id 就是
    // `loomy-{userId|phone}`）。短信登录的响应里带 userid，但**粘贴 session**
    // 的形态允许只给手机号 —— 那类账号合法存在，落进 userId 兜底会整条失败
    // （导得出去、导不回来）。再退账号 id（同款兜底）。
    if provider == kind_id(ProviderKind::Loomy) {
        if !user_id.is_empty() {
            return Ok(user_id);
        }
        let phone = text("phone");
        if !phone.is_empty() {
            return Ok(phone);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 userId 与 phone（无法标识 Loomy 账号）".to_string());
    }
    // MonkeyCode（两个地区共用）：身份是「userId」—— 与
    // `monkeycode_accounts::add_monkeycode_account` 同口径（有 userId 按 userId
    // 判重；地区已编码在 provider id 里，匹配又按 provider 作用域，「地区 +
    // userId」两段不会串）。**没有 userId 的账号也合法存在**：上游没给 user
    // uuid 时添加路径用随机 id 且不去重（见 `anonymous_account_id` 的注释）——
    // 那类记录导得出去，落进 userId 兜底却整条失败。次级身份用 accessToken
    // （= session）：同一串 session 就是同一个账号；且添加路径对匿名账号
    // **不去重**，而导入路径必须幂等（「导出→导入→二次导入」第三步要判
    // updated），按 accessToken 匹配正好补上这一环。再退账号 id（同款兜底）。
    if crate::server::core::account_store::is_monkeycode_family(provider) {
        if !user_id.is_empty() {
            return Ok(user_id);
        }
        let access_token = text("accessToken");
        if !access_token.is_empty() {
            return Ok(access_token);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 userId 与 accessToken（无法标识 MonkeyCode 账号）".to_string());
    }
    // Antigravity：身份是「userId（= email），缺则 refreshToken」—— 与
    // `antigravity_accounts` 的 id 派生口径同源（那边是 email → refresh token
    // 摘要：email 对同一个 Google 账号稳定，重新授权换令牌不变；只粘了令牌
    // 没有 email 的账号按令牌摘要派生 id，同样合法）。落进 userId 兜底会让
    // 「只粘贴了 refresh token」的账号导得出去、导不回来 —— antigravity_accounts
    // 模块头原话说这类账号「在导出时会得到一句缺少 userId 的失败原因」，
    // 那正是要消灭的静默丢号。用 refreshToken（长期凭证）而不是 accessToken
    // （短期、刷新即变）。再退账号 id（同款兜底）。
    if provider == kind_id(ProviderKind::Antigravity) {
        if !user_id.is_empty() {
            return Ok(user_id);
        }
        let refresh_token = text("refreshToken");
        if !refresh_token.is_empty() {
            return Ok(refresh_token);
        }
        let id = text("id");
        if !id.is_empty() {
            return Ok(id);
        }
        return Err("缺少 userId 与 refreshToken（无法标识 Antigravity 账号）".to_string());
    }
    if user_id.is_empty() {
        return Err(format!("缺少 userId（无法标识 {provider} 账号）"));
    }
    Ok(user_id)
}

/// 本机记录的业务身份（与 `identity_of_item` 同一口径）；身份字段缺失时 None。
pub(super) fn identity_of_record(provider: &str, record: &StoredAccount) -> Option<String> {
    let login_name = record
        .get("loginName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if crate::server::core::providers::workbuddy::is_workbuddy_family(provider) {
        let uid = record.uid().trim().to_string();
        return (!uid.is_empty()).then_some(uid);
    }
    if provider == kind_id(ProviderKind::CatPaw) {
        let uid = record.uid().trim().to_string();
        if !uid.is_empty() {
            return Some(uid);
        }
        return (!login_name.is_empty()).then_some(login_name);
    }
    // Kuku：uid（缺则 loginName）—— 与 `identity_of_item` 同口径
    if provider == kind_id(ProviderKind::Kuku) {
        let uid = record.uid().trim().to_string();
        if !uid.is_empty() {
            return Some(uid);
        }
        return (!login_name.is_empty()).then_some(login_name);
    }
    // Trae：`uid` + variant 两段身份（与 `identity_of_item` 同口径 —— 两处
    // 缺省都必须按 `solo` 算，否则导出去的同一条账号会在判重时对不上）。
    if provider == kind_id(ProviderKind::Trae) {
        let uid = record.uid().trim().to_string();
        if uid.is_empty() {
            return None;
        }
        let variant = record
            .get("variant")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        return Some(format!("{}:{uid}", if variant.is_empty() { "solo" } else { &variant }));
    }
    // Cline：身份在 `account` 键上（与 `identity_of_item` 同一口径，两池共用）
    if crate::server::core::account_store::is_cline_family(provider) {
        let account = record
            .get("account")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        return (!account.is_empty()).then_some(account);
    }
    // Qoder：地区 + userId（与 `identity_of_item` 同口径，地区取 `mode` /
    // `edition`，键缺失按 provider id 推、再退国际版 —— 两处缺省必须一致）。
    // 地区值非法的记录视为无法识别身份（不参与匹配，导入按新增走；正常数据
    // 不会出现这种记录）。
    if crate::server::core::account_store::is_qoder_family(provider) {
        let user_id = record.user_id().trim().to_string();
        if user_id.is_empty() {
            return None;
        }
        let region = match qoder_region(record.fields()) {
            Ok(Some(region)) => region,
            Ok(None) => crate::server::core::providers::qoder::endpoints::Region::from_provider_id(provider)
                .map(|region| region.id().to_string())
                .unwrap_or_else(|| "global".to_string()),
            Err(()) => return None,
        };
        return Some(format!("{region}:{user_id}"));
    }
    // 自定义提供商：apiKey 非空即身份（同 key 合并），否则账号 id
    if provider.starts_with(crate::server::core::custom_providers::ID_PREFIX) {
        let api_key = record
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !api_key.is_empty() {
            return Some(api_key);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // OfficeAce：accessKeyId（缺则账号 id）—— 与 `identity_of_item` 同口径。
    // 两侧都必须有这一支，否则「导出条目按 accessKeyId 匹配、本机记录算不出身份」
    // 会让同一条账号每次导入都判为新增（重复账号，也是一种数据损坏）。
    if provider == kind_id(ProviderKind::OfficeAce) {
        let access_key_id = record
            .get("accessKeyId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !access_key_id.is_empty() {
            return Some(access_key_id);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // OrcaTerm：uid（缺则 loginName，再缺用账号 id）—— 与 `identity_of_item` 同口径
    if provider == kind_id(ProviderKind::OrcaTerm) {
        let uid = record.uid().trim().to_string();
        if !uid.is_empty() {
            return Some(uid);
        }
        if !login_name.is_empty() {
            return Some(login_name);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // Command Code：accessToken（= API Key）即身份，缺则账号 id ——
    // 与 `identity_of_item` 同口径
    if provider == kind_id(ProviderKind::CommandCode) {
        let access_token = record.access_token().trim().to_string();
        if !access_token.is_empty() {
            return Some(access_token);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // Accio：userId（缺则 accessToken，再缺用账号 id）—— 与 `identity_of_item`
    // 同口径。两侧都必须有这一支：只给导入条目加分支会让「条目按 accessToken
    // 算身份、本机记录算不出身份」，同一条账号每次导入都判新增（重复账号，
    // 也是一种数据损坏）。
    if crate::server::core::account_store::is_accio_family(provider) {
        let user_id = record.user_id().trim().to_string();
        if !user_id.is_empty() {
            return Some(user_id);
        }
        let access_token = record.access_token().trim().to_string();
        if !access_token.is_empty() {
            return Some(access_token);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // Loomy：userId → phone → 账号 id —— 与 `identity_of_item` 同口径
    if provider == kind_id(ProviderKind::Loomy) {
        let user_id = record.user_id().trim().to_string();
        if !user_id.is_empty() {
            return Some(user_id);
        }
        let phone = record
            .get("phone")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !phone.is_empty() {
            return Some(phone);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // MonkeyCode：userId → accessToken → 账号 id —— 与 `identity_of_item` 同口径
    if crate::server::core::account_store::is_monkeycode_family(provider) {
        let user_id = record.user_id().trim().to_string();
        if !user_id.is_empty() {
            return Some(user_id);
        }
        let access_token = record.access_token().trim().to_string();
        if !access_token.is_empty() {
            return Some(access_token);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    // Antigravity：userId（= email）→ refreshToken → 账号 id ——
    // 与 `identity_of_item` 同口径
    if provider == kind_id(ProviderKind::Antigravity) {
        let user_id = record.user_id().trim().to_string();
        if !user_id.is_empty() {
            return Some(user_id);
        }
        let refresh_token = record.refresh_token().trim().to_string();
        if !refresh_token.is_empty() {
            return Some(refresh_token);
        }
        let id = record.id().trim().to_string();
        return (!id.is_empty()).then_some(id);
    }
    let user_id = record.user_id().trim().to_string();
    (!user_id.is_empty()).then_some(user_id)
}

/// 分配一个**真正全局唯一**的账号 id：优先用导入原值，被占用时依次尝试
/// `<id>-2`、`<id>-3`…，再退到带时间戳的后备形态；实在分不出（几乎不可能）返回 None。
///
/// 为什么不能用 `user-<uid>` 这类派生值兜底：它同样可能已被别家账号占用
/// （id 空间是全局的），而覆盖他人 id 会连凭证一起丢。
pub(super) fn allocate_unique_id(preferred: &str, taken: &HashSet<String>) -> Option<String> {
    if !taken.contains(preferred) {
        return Some(preferred.to_string());
    }
    for suffix in 2..=9999 {
        let candidate = format!("{preferred}-{suffix}");
        if !taken.contains(&candidate) {
            return Some(candidate);
        }
    }
    let base = format!("{preferred}-{}", crate::server::logging::now_ms());
    if !taken.contains(&base) {
        return Some(base);
    }
    for suffix in 2..=9999 {
        let candidate = format!("{base}-{suffix}");
        if !taken.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// 在全局队列里分配空闲优先级（排在现有账号之后）。
///
/// 优先级唯一性的作用域是**全部账号**（所有提供商共用一条转发队列，见
/// `priority.rs` 模块头），调用方必须把所有账号的优先级传进来 —— 只传同
/// provider 的会算出与别家冲突的值。号段满时返回 None，**调用方必须报错**：
/// 回落默认值会造成优先级冲突（`next_free_priority` 的兜底路径就是那样，
/// 通用导入不能接受）。
pub(super) fn allocate_priority(used: &[i64]) -> Option<i64> {
    let normalized: Vec<i64> = used
        .iter()
        .map(|value| normalize_priority_value(*value))
        .collect();
    let max = normalized
        .iter()
        .copied()
        .max()
        .unwrap_or(DEFAULT_PRIORITY - 1);
    let mut candidate = max.saturating_add(1);
    while candidate <= MAX_PRIORITY && normalized.contains(&candidate) {
        candidate += 1;
    }
    if candidate <= MAX_PRIORITY {
        return Some(candidate);
    }
    (MIN_PRIORITY..=MAX_PRIORITY).find(|value| !normalized.contains(value))
}

#[cfg(test)]
mod tests {
    //! 这里只测**身份判定**本身（导入/导出两段的口径是否一致），不测整条导入链
    //! —— 后者要临时库与账号文件，`api::accounts` 那侧已有覆盖。
    use serde_json::json;

    use super::*;

    fn item(fields: serde_json::Value) -> Map<String, Value> {
        fields.as_object().expect("对象").clone()
    }

    fn record(fields: serde_json::Value) -> StoredAccount {
        StoredAccount::from_map(fields.as_object().expect("对象").clone())
    }

    #[test]
    fn trae_identity_is_variant_plus_uid_and_never_falls_through_to_userid() {
        // 回归本家那条"导得出去、导不回来"：兜底按 `userId` 取身份，
        // 而 Trae 的记录只有 `uid`（与 workbuddy / catpaw 同类）。落进兜底的
        // 症状是导入时报「缺少 userId（无法标识 trae 账号）」整条失败。
        assert_eq!(
            "solo:51029416092912",
            identity_of_item(
                "trae",
                &item(json!({"uid": "51029416092912", "variant": "solo", "userId": ""}))
            )
            .expect("应识别出身份")
        );
        assert_eq!(
            "cn:51029416092912",
            identity_of_item("trae", &item(json!({"uid": "51029416092912", "variant": "cn"}))).expect("应识别出身份"),
            "两个谱系是同一个人也是两条记录（与 add_trae_account 的判重口径一致）"
        );
        // variant 缺失时**必须**与 `identity_of_record` 的缺省同一个值，
        // 否则同一条账号在导出侧与本机侧算出两个身份，判重失效 → 重复添加。
        assert_eq!(
            "solo:777",
            identity_of_item("trae", &item(json!({"uid": "777"}))).expect("缺 variant 按 solo"),
        );
        assert!(identity_of_item("trae", &item(json!({"uid": "  ", "userId": "51029416092912"}))).is_err(), "空 uid 不能拿 userId 凑");
    }

    #[test]
    fn officeace_identity_prefers_access_key_id_then_id_on_both_sides() {
        // 回归 4db8a60：OfficeAce 的身份落在 accessKeyId（控制面临时凭据），
        // 本家记录没有 userId 键 —— 缺这一支时整条导入报「缺少 userId」，
        // 账号导得出去、导不回来。
        assert_eq!(
            "AK-1",
            identity_of_item("officeace", &item(json!({"id": "officeace-abc", "accessKeyId": "AK-1"})))
                .expect("应识别出身份")
        );
        assert_eq!(
            "officeace-abc",
            identity_of_item("officeace", &item(json!({"id": "officeace-abc"})))
                .expect("缺 accessKeyId 退账号 id")
        );
        assert_eq!(
            Some("AK-1".to_string()),
            identity_of_record("officeace", &record(json!({"id": "officeace-abc", "accessKeyId": "AK-1"})))
        );
        assert_eq!(
            Some("officeace-abc".to_string()),
            identity_of_record("officeace", &record(json!({"id": "officeace-abc"}))),
            "记录侧缺 accessKeyId 时同样退账号 id（两侧必须同口径）"
        );
    }

    #[test]
    fn orcaterm_identity_prefers_uid_then_login_name_then_id_on_both_sides() {
        // 回归 4db8a60：OrcaTerm 的 uid 落在 `uid` 键上（值是 userId 的值、
        // 但是另一个键），本家记录没有 userId 键。
        assert_eq!(
            "1001",
            identity_of_item("orcaterm", &item(json!({"id": "orcaterm-abc", "uid": "1001", "loginName": "me"})))
                .expect("应识别出身份")
        );
        assert_eq!(
            "me",
            identity_of_item("orcaterm", &item(json!({"id": "orcaterm-abc", "loginName": "me"})))
                .expect("缺 uid 退 loginName")
        );
        assert_eq!(
            "orcaterm-abc",
            identity_of_item("orcaterm", &item(json!({"id": "orcaterm-abc"}))).expect("再退账号 id")
        );
        assert_eq!(
            Some("1001".to_string()),
            identity_of_record("orcaterm", &record(json!({"id": "orcaterm-abc", "uid": "1001"})))
        );
        assert_eq!(
            Some("me".to_string()),
            identity_of_record("orcaterm", &record(json!({"id": "orcaterm-abc", "loginName": "me"})))
        );
    }

    #[test]
    fn accio_identity_prefers_user_id_then_access_token_on_both_sides() {
        // 回归「导得出去、导不回来」第二季：Accio 的 accessToken 是**不透明
        // 串**、解不出 userId，网页登录也可能拿不到 —— 添加路径
        // （`add_accio_account`）的判重是 `same_user || same_token`，无
        // userId 的账号靠 same_token 防重、合法存在（`Credentials::to_value`
        // 把空 userId 落成 null）。导入侧曾落进 userId 兜底整条失败。
        assert_eq!(
            "u-1",
            identity_of_item("accio", &item(json!({"userId": "u-1", "accessToken": "t-1"}))).expect("应识别出身份")
        );
        assert_eq!(
            "u-1",
            identity_of_item("accio-cn", &item(json!({"userId": "u-1", "accessToken": "t-1"})))
                .expect("国内版同口径（两个地区是两家 provider）")
        );
        assert_eq!(
            "t-1",
            identity_of_item("accio", &item(json!({"userId": null, "accessToken": "t-1"})))
                .expect("无 userId 按 accessToken 认身份（与 same_token 同口径）")
        );
        assert_eq!(
            "t-1",
            identity_of_item("accio-cn", &item(json!({"accessToken": "t-1"}))).expect("键缺失同理")
        );
        // 本机记录侧必须同口径：同一条账号在两侧算出同一个身份，否则每次
        // 导入都判新增（重复账号，也是一种数据损坏）
        assert_eq!(
            Some("u-1".to_string()),
            identity_of_record("accio", &record(json!({"id": "accio-intl-abc", "userId": "u-1", "accessToken": "t-1"})))
        );
        assert_eq!(
            Some("t-1".to_string()),
            identity_of_record("accio", &record(json!({"id": "accio-intl-abc", "userId": null, "accessToken": "t-1"}))),
            "userId 落盘是 null（Credentials::to_value 的口径），按 null 视为缺失"
        );
        assert_eq!(
            Some("accio-intl-abc".to_string()),
            identity_of_record("accio", &record(json!({"id": "accio-intl-abc"}))),
            "两侧关键字段都空时退账号 id（通不过导入校验，但函数要自洽）"
        );
    }

    #[test]
    fn loomy_identity_prefers_user_id_then_phone_on_both_sides() {
        // Loomy 的身份是「userId 缺则手机号」（`add_loomy_account` 明确要求
        // 「userId 或手机号至少给一个」，账号 id 就是 `loomy-{userId|phone}`）。
        // 短信登录响应带 userid，但**粘贴 session** 的形态允许只给手机号 ——
        // 那类账号合法存在，曾落进 userId 兜底整条失败。
        assert_eq!(
            "u-1",
            identity_of_item("loomy", &item(json!({"userId": "u-1", "phone": "13800000000"}))).expect("应识别出身份")
        );
        assert_eq!(
            "13800000000",
            identity_of_item("loomy", &item(json!({"phone": "13800000000"})))
                .expect("无 userId 按手机号认身份")
        );
        assert!(
            identity_of_item("loomy", &item(json!({"accessToken": "s"}))).is_err(),
            "userId 与 phone 两个都没有才该失败"
        );
        assert_eq!(
            Some("13800000000".to_string()),
            identity_of_record("loomy", &record(json!({"id": "loomy-13800000000", "phone": "13800000000", "accessToken": "s"})))
        );
        assert_eq!(
            Some("u-1".to_string()),
            identity_of_record("loomy", &record(json!({"id": "loomy-u-1", "userId": "u-1", "phone": "13800000000"})))
        );
    }

    #[test]
    fn monkeycode_identity_prefers_user_id_then_access_token_on_both_sides() {
        // MonkeyCode 以「地区 + userId」识别，地区已编码在 provider id 里
        // （匹配又按 provider 作用域，两段不会串）。上游没给 user uuid 时
        // 添加路径用随机 id 且**不去重**（`anonymous_account_id`）—— 匿名
        // 账号合法存在，导得出去却曾落进 userId 兜底整条失败。次级身份按
        // accessToken（= session）认：同一串 session 就是同一个账号，且
        // 保证「导出→导入→二次导入」第三步计 updated（添加路径对匿名账号
        // 不去重，导入路径必须幂等）。
        assert_eq!(
            "uuid-1",
            identity_of_item("monkeycode", &item(json!({"userId": "uuid-1", "accessToken": "s-1"}))).expect("应识别出身份")
        );
        assert_eq!(
            "s-1",
            identity_of_item("monkeycode", &item(json!({"accessToken": "s-1"})))
                .expect("无 userId 按 session 认身份")
        );
        assert_eq!(
            "s-1",
            identity_of_item("monkeycode-intl", &item(json!({"accessToken": "s-1"}))).expect("国际版同口径")
        );
        assert_eq!(
            Some("uuid-1".to_string()),
            identity_of_record("monkeycode", &record(json!({"id": "mc-uuid-1", "userId": "uuid-1", "accessToken": "s-1"})))
        );
        assert_eq!(
            Some("s-1".to_string()),
            identity_of_record("monkeycode-intl", &record(json!({"id": "mc-intl-anon-aabbcc", "accessToken": "s-1"}))),
            "匿名记录（没有 userId 键）按 session 算身份"
        );
    }

    #[test]
    fn antigravity_identity_prefers_user_id_then_refresh_token_on_both_sides() {
        // Antigravity 的 userId 存的是 email；「只粘贴 refresh token」的账号
        // 按「email → refresh token 摘要」派生 id、合法存在，曾落进 userId
        // 兜底整条失败（antigravity_accounts 模块头原话：那类账号「会得到
        // 一句缺少 userId 的失败原因」）。次级身份用 refreshToken（长期
        // 凭证）而不是 accessToken（短期、刷新即变）。
        assert_eq!(
            "a@b.com",
            identity_of_item("antigravity", &item(json!({"userId": "a@b.com", "refreshToken": "r-1"}))).expect("应识别出身份")
        );
        assert_eq!(
            "r-1",
            identity_of_item("antigravity", &item(json!({"refreshToken": "r-1"})))
                .expect("无 email 按 refresh token 认身份")
        );
        assert_eq!(
            "r-1",
            identity_of_item("antigravity", &item(json!({"userId": "  ", "refreshToken": "r-1"})))
                .expect("空白 userId 视为缺失，退 refreshToken")
        );
        assert_eq!(
            Some("a@b.com".to_string()),
            identity_of_record("antigravity", &record(json!({"id": "antigravity-xyz", "userId": "a@b.com", "refreshToken": "r-1"})))
        );
        assert_eq!(
            Some("r-1".to_string()),
            identity_of_record("antigravity", &record(json!({"id": "antigravity-xyz", "refreshToken": "r-1"}))),
            "email 缺失时 userId 键不写（落盘口径），记录侧退 refreshToken"
        );
    }
}
