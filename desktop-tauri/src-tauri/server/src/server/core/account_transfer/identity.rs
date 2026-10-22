//! 导入记录的身份判定：provider 校验、业务身份提取、桌面引用识别、唯一 id 与
//! 优先级分配。
//!
//! ── 为什么身份必须带 provider 作用域 ─────────────────────────
//! 各家的 id / uid 空间互相独立：CatPaw 的账号 id 就是 uid 或 loginName，
//! WorkBuddy 的 uid 也可能是同一串数字。只按 uid 匹配会把别家的记录整条覆写
//! （连凭证一起丢）。因此这里所有判定都产出「provider + 身份」二元组，
//! 由调用方按二元组匹配本机记录。
//!
//! ── 判据只此一份：`identity_from_fields` ─────────────────────
//! 导入条目（导出文件里的一条 JSON）与本机记录（`StoredAccount`）的身份**必须
//! 用同一个函数算**：两边各写一份的旧实现里，同一条账号在导出侧与本机侧可能
//! 算出两个身份（缺省值不一致、字段优先级不一致），判重随即失效 —— 表现是
//! 重复添加或该合并的没合并。现在两侧都只调 [`identity_from_fields`]，它读的
//! 是同一张字段表，口径不可能再分叉。
//!
//! ── 新增一家 provider 要改这里吗：不用 ───────────────────────
//! 只有**身份语义确实特殊**的家才在 [`identity_from_fields`] 里占一条分支
//! （WorkBuddy 的 uid 必填、Qoder 的「地区 + userId」、Trae 的「variant + uid」
//! 这类 —— 少一段就会把两条真实账号并成一条）。其余全部走
//! [`GENERIC_IDENTITY_KEYS`] 的通用链：按固定顺序取第一个非空字段。
//! 新家只要把身份落在这些常见字段上（userId / uid / loginName / account /
//! apiKey / accessToken / modelAppKey…），**本文件一行都不用改**。
//!
//! 这条通用链是「每加一家就要回来补分支」这个维护负担的**根治手段**：漏补的
//! 旧症状（`缺少 userId` 整条失败，账号导得出去、导不回来）不会再出现。
//! 2026-10 的 OfficeAce 就是那样漏的 —— 它的记录只有 `modelAppKey`。

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::{
    normalize_priority_value, DEFAULT_PRIORITY, MAX_PRIORITY, MIN_PRIORITY,
};
use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::providers::{
    is_known_provider_id, kind_id, ProviderKind, DEFAULT_PROVIDER_ID,
};

/// 通用身份链：**按顺序**取第一个非空字符串。
///
/// ── 顺序是怎么定的 ──────────────────────────────────────────
/// 先「稳定的业务身份」（userId / uid / loginName / account / email / phone），
/// 再「凭证即身份」的家（apiKey / accessToken / jwt / modelAppKey），最后才是
/// 账号 id。id 放最后是因为它是**派生值**：各家的添加路径都用身份算出 id
/// （`user-<uid>`、`qoder-<region>-<sha256>`、`<provider>-<hash>`…），所以同一条
/// 账号在另一台机器上算出的 id 也一致 —— 拿它兜底能救回「身份字段缺失但 id
/// 完好」的记录，而不会把不同账号错并（id 在全局唯一）。
///
/// 顺序本身就是口径：导入条目与本机记录读同一张表，谁在前谁在后**两边一样**，
/// 判重因此不可能因字段优先级漂移而失效。加字段时追加在合适的位置即可，
/// 但别插在已有字段前面 —— 那会改变存量文件的匹配结果。
const GENERIC_IDENTITY_KEYS: &[&str] = &[
    "userId",
    "uid",
    "loginName",
    "account",
    "email",
    "phone",
    "username",
    "apiKey",
    "accessToken",
    "jwt",
    "modelAppKey",
    "id",
];

/// 「这条记录带凭证吗」的已知凭证键（任一非空即算有）。
///
/// 与 [`GENERIC_IDENTITY_KEYS`] 同一思路：不按 provider 枚举，而是列出全仓
/// 出现过的凭证落盘键。旧实现只认 `accessToken` / `refreshToken`，于是
/// OfficeAce（`modelAppKey` + `modelAppSecret`）、自定义家（`apiKey`）、
/// ZCode（`jwt`）这些账号会被整条判成「缺少凭证」—— 导得出去、导不回来。
///
/// 新增一家时若凭证落在这些键之外（比如某个 `xxxToken`），**要么复用现成键，
/// 要么在这里补一行**；漏补的症状仍是「导出正常、导入全失败」，但这次只需要
/// 在一个数组里加一项，不必再改判定逻辑。
pub(super) const CREDENTIAL_KEYS: &[&str] = &[
    "accessToken",
    "refreshToken",
    "apiKey",
    "modelAppKey",
    "modelAppSecret",
    "secretAccessKey",
    "securityToken",
    "jwt",
    "accessKeyId",
    "secretKey",
];

/// 桌面端实时登录态的固定账号 id（记录里按设计不落 token，凭证实时读客户端文件）。///
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

/// 从一张字段表算业务身份 —— **导入条目与本机记录共用这一份**（见模块头）。
///
/// 返回 `Ok(identity)`；无法确定身份时 `Err(原因)`（导入侧据此让该条失败，
/// 不做猜测性匹配）。
///
/// ── 特殊家为什么还要分支 ────────────────────────────────────
/// 通用链只按字段顺序取值，覆盖不了三类**语义**约束：
///   1. **必填**：WorkBuddy 缺 uid 时不能拿别家字段凑（凑出来的身份在两侧
///      口径不同，判重会错）；
///   2. **多段身份**：Qoder 的「地区 + userId」、Trae 的「variant + uid」、
///      CodeArts 的「domain + userId」—— 只取一段会把同一个人在两地 / 两谱系 /
///      两域下的两条真实账号并成一条，而被覆盖那条的凭证（尤其一次性 refresh
///      token）就此作废；
///   3. **取值优先级与通用链不同**：CatPaw / Kuku 是 uid 优先、loginName 兜底，
///      而通用链里 userId 排在 uid 前面 —— 这两家的记录里 `userId` 是**别的东西**
///      （CatPaw 的 `userId` 是 payload 别名，不是本家身份）。
///
/// 除此之外**一律走通用链**：新增一家 provider 时，只要身份落在
/// [`GENERIC_IDENTITY_KEYS`] 里就不必改本文件。
fn identity_from_fields(provider: &str, fields: &Map<String, Value>) -> Result<String, String> {
    let text = |key: &str| {
        fields
            .get(key)
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
    // CatPaw / Kuku：uid 优先、loginName 兜底。两家记录里的 `userId` 不是本家
    // 身份（CatPaw 的是 payload 别名），所以**不能**交给通用链 —— 通用链里
    // userId 排在 uid 前，取到的会是一个与本机侧不同的值。
    if provider == kind_id(ProviderKind::CatPaw) || provider == kind_id(ProviderKind::Kuku) {
        if !uid.is_empty() {
            return Ok(uid);
        }
        if !login_name.is_empty() {
            return Ok(login_name);
        }
        return Err("缺少 uid 与 loginName（无法标识该账号）".to_string());
    }
    // CodeArts：`domain_id + user_id` 两段身份 —— 与 `codearts_accounts::same_identity`
    // 的判重口径逐字一致。只按 userId 会把**同一个人不同华为云账号（域）下的两条**
    // 并成一条：导入时后一条覆盖前一条，而被覆盖那条的一次性 refresh token
    // 就此作废（不是"少一条记录"，是"烧掉一份登录凭据"）。
    if provider == kind_id(ProviderKind::CodeArts) {
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
        let region = match qoder_region(fields) {
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
    if provider == kind_id(ProviderKind::Trae) {
        if uid.is_empty() {
            return Err("缺少 uid（无法标识 Trae 账号）".to_string());
        }
        let variant = text("variant");
        return Ok(format!("{}:{uid}", if variant.is_empty() { "solo" } else { &variant }));
    }

    // ── 通用链：新增一家 provider 时这里不用改（见模块头）──────────
    for key in GENERIC_IDENTITY_KEYS {
        let value = text(key);
        if !value.is_empty() {
            return Ok(value);
        }
    }
    Err(format!("缺少可用的身份字段（无法标识 {provider} 账号）"))
}

/// **编译期护栏**：新增一家 provider 时必须在这里表态。
///
/// ── 它解决什么问题 ──────────────────────────────────────────
/// 这套导入导出最贵的维护成本不是写分支，而是**忘记**写分支 —— 症状是
/// 「导出正常、导入整条失败」，而且只有用户真的去导才会发现。2026-10 的
/// OfficeAce 就是这样漏的（记录里只有 `modelAppKey`，落到旧的 `userId`
/// 兜底直接报「缺少 userId」）。
///
/// 这个 `match` 的臂**不参与任何运行时判断**，唯一作用是：`ProviderKind`
/// 新增一个变体时，编译器在这里报 `non-exhaustive patterns`，逼着作者回答
/// 「这家走通用链够不够」。回答完把新家写进 [`Generic`](IdentityShape::Generic)
/// 那一臂即可 —— 一行，不需要改判定逻辑。
///
/// 返回值只用于让 match 有产出（也便于测试断言覆盖齐全），运行时**不读**。
#[allow(dead_code)]
fn identity_shape_of(kind: ProviderKind) -> IdentityShape {
    use IdentityShape::{Generic, NeedsBranch};
    match kind {
        // ── 身份语义特殊、必须走专属分支的家 ────────────────────
        // 判据见 `identity_from_fields` 的分支注释：必填、多段身份、或取值
        // 优先级与通用链不同（CatPaw / Kuku 的 `userId` 不是本家身份）。
        ProviderKind::WorkBuddy
        | ProviderKind::WorkBuddyIntl
        | ProviderKind::CatPaw
        | ProviderKind::Kuku
        | ProviderKind::CodeArts
        | ProviderKind::Qoder
        | ProviderKind::QoderIntl
        | ProviderKind::Trae => NeedsBranch,
        // ── 其余全部走通用身份链 ────────────────────────────────
        // 新增一家 provider 时：**先默认加在这里**。只有当它的身份确实不能用
        // 通用链表达（出现上面三类约束）时才移到 NeedsBranch，并在
        // `identity_from_fields` 里补分支。
        ProviderKind::Raccoon
        | ProviderKind::AutoClaw
        | ProviderKind::AutoClawIntl
        | ProviderKind::ClineFree
        | ProviderKind::ClinePass
        | ProviderKind::Accio
        | ProviderKind::AccioCn
        | ProviderKind::Zcode
        | ProviderKind::ZcodeIntl
        | ProviderKind::Loomy
        | ProviderKind::MonkeyCode
        | ProviderKind::MonkeyCodeIntl
        | ProviderKind::CommandCode
        | ProviderKind::Antigravity
        | ProviderKind::OfficeAce
        | ProviderKind::OrcaTerm
        | ProviderKind::Chatglm
        | ProviderKind::Modelscope
        | ProviderKind::ModelscopeIntl
        | ProviderKind::Atomcode
        | ProviderKind::LobsterAi
        // 千问办公 / QClaw：身份都落在通用链的常见字段上 —— 前者 `userId`
        // （+ `accessToken` 兜底），后者 `uid`（+ 网关令牌摘要兜底，见
        // `account_store::qclaw_accounts`）。无「必填 / 多段 / 优先级不同」
        // 三类约束，因此不需要专属分支（与上游对 OfficeAce / OrcaTerm 的
        // 处置一致 —— 同步上游时本地那几支专属分支已随之删除）。
        | ProviderKind::Qwenwork
        | ProviderKind::Qclaw
        // Marvis（腾讯马维斯）：身份落在通用链的 `openid` 上（本家的登录身份，
        // 换令牌还是同一个人）。无「必填 / 多段 / 优先级不同」三类约束 ——
        // `openid` 是必填项这一点由 `marvis_accounts::add_marvis_account` 的
        // 入参校验负责，不是身份链要判的事。
        | ProviderKind::Marvis
        // MiniMax Code（两个站点）：身份落在通用链的 `userId` 上（JWT 的
        // `user.id`，令牌每小时轮换但身份不变）。无「必填 / 多段 / 优先级不同」
        // 三类约束。两个地区都要列 —— 漏一个会让国际版账号的导入判重失效
        // （同一个号被重复添加成两条）。
        | ProviderKind::Minimax
        | ProviderKind::MinimaxIntl => Generic,
        // Coze（扣子）：身份落在通用链的 `uid` 上（passport 的 `user_id_str`，
        // 公开形态与 KukuAI 同形）。无「必填 / 多段 / 优先级不同」三类约束 ——
        // uid 由 `add_coze_account` 用 cookie 尾兜底，记录里恒非空。
        // 注意它**不**像 KukuAI 那样走专属分支：KukuAI 需要它是因为导出导入
        // 要带三件套，而 Coze 的整包 Cookie 就在 `accessToken` 那一列，
        // 通用链原样搬运即可。
        ProviderKind::Coze => Generic,
    }
}

/// [`identity_shape_of`] 的两种形态（只为让穷尽 match 有产出，运行时不读）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum IdentityShape {
    /// 走 [`identity_from_fields`] 里的专属分支
    NeedsBranch,
    /// 走 [`GENERIC_IDENTITY_KEYS`] 通用链
    Generic,
}

/// 导入记录的业务身份；无法确定身份时 Err（该条失败，不做猜测性匹配）。
pub(super) fn identity_of_item(provider: &str, item: &Map<String, Value>) -> Result<String, String> {
    identity_from_fields(provider, item)
}

/// 本机记录的业务身份（与 `identity_of_item` 同一份判据）；身份缺失时 None。
pub(super) fn identity_of_record(provider: &str, record: &StoredAccount) -> Option<String> {
    identity_from_fields(provider, record.fields()).ok()
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

    /// **新增 provider 不必改本文件**：身份落在通用链的常见字段上即可。
    ///
    /// 锁的是「每加一家就要回来补分支」这个维护负担的根治 —— 旧实现里
    /// OfficeAce（只有 `modelAppKey`）落到 `userId` 兜底，报「缺少 userId」
    /// 整条失败，账号导得出去、导不回来。
    #[test]
    fn generic_chain_covers_providers_without_a_branch() {
        // OfficeAce：记录里只有 modelAppKey（+ modelAppSecret），没有 uid/userId
        assert_eq!(
            "app-key-1",
            identity_of_item(
                "officeace",
                &item(json!({"modelAppKey": "app-key-1", "modelAppSecret": "s", "id": "officeace-x"}))
            )
            .expect("通用链应认出 modelAppKey")
        );
        // 自定义家：apiKey 即身份（与「同 key 合并」的添加路径同口径）
        assert_eq!(
            "sk-abc",
            identity_of_item(
                "custom-openai",
                &item(json!({"apiKey": "sk-abc", "id": "custom-openai-1"}))
            )
            .expect("apiKey 即身份")
        );
        // Command Code：key 落在 accessToken
        assert_eq!(
            "user_abc",
            identity_of_item("commandcode", &item(json!({"accessToken": "user_abc"})))
                .expect("accessToken 即身份")
        );
        // 身份字段全缺时才退到 id（派生值，但全局唯一，救得回「身份缺失但 id 完好」的记录）
        assert_eq!(
            "atomcode-1",
            identity_of_item("atomcode", &item(json!({"id": "atomcode-1"}))).expect("退到 id")
        );
        // 什么都没有 = 无法识别（导入该条失败，不做猜测性匹配）
        assert!(identity_of_item("officeace", &item(json!({"name": "无身份"}))).is_err());
    }

    /// 两侧**必须**算出同一个身份：这是判重能不能成立的前提。
    ///
    /// 旧实现把同一套规则在 `identity_of_item` 与 `identity_of_record` 里各写
    /// 一遍，任何一处缺省值漂移都会让同一条账号匹配不上（重复添加）。现在
    /// 两侧共用一个函数，这条测试钉住「同一份字段 → 同一个答案」。
    #[test]
    fn item_and_record_agree_on_every_provider() {
        for provider in [
            "workbuddy", "workbuddy-intl", "raccoon", "catpaw", "autoclaw", "autoclaw-intl",
            "qoder", "qoder-intl", "cline-free", "cline-pass", "codearts", "trae", "loomy",
            "kuku", "commandcode", "antigravity", "officeace", "orcaterm", "chatglm",
            "modelscope", "modelscope-intl", "atomcode", "lobsterai", "zcode", "minimax",
            // MiniMax Code 国际版：与国内版同一套身份链（`userId`），但 provider id
            // 不同 —— 漏了它，导入国际版账号时「条目与记录算出的身份」没人钉住。
            "minimax-intl",
        ] {
            let fields = item(json!({
                "id": "acc-1",
                "provider": provider,
                "uid": "u-1",
                "userId": "u-1",
                "loginName": "name-1",
                "account": "acc-1@example.com",
                "apiKey": "key-1",
                "accessToken": "token-1",
                "modelAppKey": "app-1",
                "variant": "solo",
                "mode": "cn",
            }));
            let from_item = identity_of_item(provider, &fields);
            let record = StoredAccount::from_map(fields);
            let from_record = identity_of_record(provider, &record);
            assert_eq!(
                from_item.ok(),
                from_record,
                "provider={provider}：导入条目与本机记录算出的身份必须一致"
            );
        }
    }

    /// 特殊家仍走专属分支（通用链不能替代它们）。
    #[test]
    fn special_providers_keep_their_multi_segment_identity() {
        // CodeArts：domain + userId 两段（只按 userId 会把两个域下的账号并成一条）
        assert_eq!(
            "dom-a\u{0}u-1",
            identity_of_item("codearts", &item(json!({"domainId": "dom-a", "userId": "u-1"})))
                .expect("两段身份")
        );
        // Qoder：地区 + userId 两段
        assert_eq!(
            "cn:u-1",
            identity_of_item("qoder", &item(json!({"userId": "u-1", "mode": "cn"})))
                .expect("两段身份")
        );
        // CatPaw：uid 优先（通用链里 userId 排在 uid 前，会取错值）
        assert_eq!(
            "uid-1",
            identity_of_item("catpaw", &item(json!({"uid": "uid-1", "userId": "alias-1"})))
                .expect("uid 优先")
        );
    }
}
