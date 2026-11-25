//! 导入记录的身份判定、provider 校验、桌面引用识别、唯一 id 与优先级分配。
//! 业务身份与重新登录共用 account_store::account_identity，provider 是独立作用域。
//! CodeArts 的域、Trae 的版本以及两地、两池归属不能被账号 ID 或缺省值替代。

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::{
    normalize_priority_value, DEFAULT_PRIORITY, MAX_PRIORITY, MIN_PRIORITY,
};
use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::providers::{
    is_known_provider_id, kind_id, ProviderKind, DEFAULT_PROVIDER_ID,
};

/// 「这条记录带凭证吗」的已知凭证键（任一非空即算有）。
///
/// 不按 provider 枚举，而是列出全仓
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
    "raccoon-desktop",
    // CatPaw 的实现已搬到插件，但 `desktop-auth` 这个固定 id 是**落盘契约**
    // （存量记录仍在库里）—— 导入时同样不能被一条普通账号占住。
    "desktop-auth",
    crate::server::core::account_store::autoclaw_accounts::DESKTOP_ACCOUNT_ID,
    crate::server::core::account_store::autoclaw_accounts::INTL_DESKTOP_ACCOUNT_ID,
];

pub(super) fn is_reserved_desktop_id(id: &str) -> bool {
    if RESERVED_DESKTOP_IDS.iter().any(|known| *known == id) {
        return true;
    }
    // Cline 两个池的桌面端固定 id（`cline-free-desktop` / `cline-pass-desktop`）。
    // 漏了它们，手工构造的「带 Cline 桌面 id 但不带 desktop 标记」的导入记录会
    // 占住这个 id，目标机器的「导入桌面端登录态」就建不出实时账号了。
    // 后缀字面量写在这里而不是引用插件的常量：网关不再持有 Cline 的实现，
    // 而这两个 id 是**落盘契约**（存量记录仍在库里），必须留在网关侧。
    [
        kind_id(ProviderKind::ClineFree),
        kind_id(ProviderKind::ClinePass),
    ]
    .iter()
    .any(|provider| *id == format!("{provider}-desktop"))
}

/// 记录带 `desktop: true` 标记 = 桌面端实时登录态的引用，不是可迁移的普通账号。
pub(super) fn is_desktop_item(item: &Map<String, Value>) -> bool {
    let provider = item.get("provider").and_then(Value::as_str).unwrap_or(DEFAULT_PROVIDER_ID);
    crate::server::core::account_store::account_identity::reference(provider, item)
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
pub(crate) fn resolve_provider(item: &Map<String, Value>) -> Result<String, String> {
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
            if trimmed == DEFAULT_PROVIDER_ID && !item.get("pluginSecret").is_some_and(|value| !value.is_null()) {
                let region = crate::server::core::providers::workbuddy::Region::from_edition_id(
                    item.get("edition").and_then(Value::as_str),
                );
                return Ok(region.provider_id().to_string());
            }
            if crate::server::core::account_store::is_qoder_family(trimmed) {
                let record = StoredAccount::from_map(item.clone());
                if let Some(region) = crate::server::core::account_store::qoder_accounts::migrated_region(&record) {
                    return Ok(region.provider_id().to_string());
                }
            }
            Ok(trimmed.to_string())
        }
        Some(_) => Err("provider 字段必须是字符串".to_string()),
    }
}


/// 导入记录的业务身份；无法确定身份时 Err（该条失败，不做猜测性匹配）。
pub(super) fn identity_of_item(provider: &str, item: &Map<String, Value>) -> Result<String, String> {
    if is_desktop_item(item) {
        return crate::server::core::account_store::account_identity::text(item, &["id"])
            .ok_or_else(|| "桌面端账号缺少独立 id".to_string());
    }
    crate::server::core::account_store::account_identity::for_import(provider, item)
}

/// 本机记录的业务身份（与 `identity_of_item` 同一份判据）；身份缺失时 None。
#[cfg(test)]
pub(super) fn identity_of_record(provider: &str, record: &StoredAccount) -> Option<String> {
    identity_of_item(provider, record.fields()).ok()
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
            "两个谱系是同一个人也是两条记录（与落账号一侧的判重口径一致）"
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
            "modelscope", "atomcode", "lobsterai", "zcode",
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
