//! ModelScope（魔搭）的**地区**（国内站 / 国际站）：两套域名与两条 provider 身份。
//!
//! ── 上游长什么样（2026-10-16 实测，来源：CPA 的 modelscope-ratelimit 插件
//!    源码 `D:\GitHub\modelscope-ratelimit` + 真实账号 curl 验证）──────
//! 魔搭是**两个站点各一套账号体系**，同一份协议、不同的域：
//!
//! ```text
//!                推理（OpenAI 兼容）                      网页站（身份 / 魔粒 / 签到）
//!   国内  https://api-inference.modelscope.cn/v1        https://modelscope.cn
//!   国际  https://api-inference.modelscope.ai/v1        https://www.modelscope.ai
//! ```
//!
//! ── 一处**实测确认**的关键事实：一个 `ms-` key 两用 ────────────
//! 同一个 `ms-...` 令牌既作 `Authorization: Bearer` 调推理，又作
//! `Cookie: m_session_id=<key>` 调网页站的身份 / 魔粒接口 —— 两条链路都实测 200。
//! 插件源码的注释也这么写（「session 就是 api key 本身……永不过期」）。
//! 因此本家**只有一个凭证字段**，不需要像 CodeArts / OfficeAce 那样分两层。
//!
//! ── 另一处实测确认：**两个站点的账号体系是分开的** ──────────────
//! 用国内站的 key 打 `https://www.modelscope.ai/api/v1/users/login/info`
//! 得到 `401 {"Code":10019901001,"Message":"access forbidden"}`；反过来同理。
//! 而**推理端点两地都能用同一个 key 打通**（模型网关的 key 是账号级的、不分站）。
//! 所以「地区」在本家不是可有可无的标签：**签到与魔粒必须打对站点**，
//! 打错了拿到的是一个必然失败的结果。
//!
//! ── 为什么是两个 provider 而不是「一家的一个选项」（与 AutoClaw / ZCode 同一思路）──
//! 地区做成「账号上的一个字段」的后果与本仓库已修过数次的那类缺陷相同：
//! 地区成了**账号的属性**，界面上混在一起、账号记录也无法按站隔离；
//! 更要紧的是模型目录只有一份（单槽缓存 + 单条刷新排期），两个站的清单会互相覆盖。
//! 拆成两个 provider 后各家有独立的账号、清单、启停与映射，
//! 界面上各占一个分组、在添加弹窗里相邻。
//!
//! ── provider id 的取值 ──────────────────────────────────────
//! 本家是**全新接入**（没有任何存量账号），所以两个 id 都是新名字、
//! 不需要像 AutoClaw 那样为存量数据保留裸 id：
//! `modelscope`（国内）与 `modelscope-intl`（国际）。
//!
//! ── 为什么没有环境变量覆盖（与本仓库其余几家不同）──────────────
//! 别家做覆盖是历史原因（域名的多来源、代理调试、客户端内置值可被官方换掉）。
//! 魔搭这两个域名是**硬编码在官方 SDK / 客户端里的**、2026-10 实测稳定，
//! 为它们加一层 `MODELSCOPE_*_BASE_URL` 只是多一条没人会用的分支（KISS）。
//! 真要换域名时，改本文件的两处常量比找环境变量更直接。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use crate::server::core::providers::{kind_id, ProviderKind};

/// ModelScope 的地区。
///
/// 顺序 = 注册表顺序（国内版在前）：`ALL` 的遍历顺序决定模型目录合并时同名模型
/// 先归谁家、以及界面上两家的先后。**国内版在前**（用户直觉里「魔搭就是国内那个站」，
/// 与 MonkeyCode / ZCode 同一处置）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Region {
    /// 国内站（`modelscope.cn`；provider id `modelscope`）
    #[default]
    Cn,
    /// 国际站（`www.modelscope.ai`；provider id `modelscope-intl`）
    Intl,
}

impl Region {
    /// 两个地区（注册表顺序：国内版在前）
    pub const ALL: [Region; 2] = [Region::Cn, Region::Intl];

    /// 本地区对应哪个 provider kind（地区 → 身份的**唯一**映射）
    pub fn kind(self) -> ProviderKind {
        match self {
            Self::Cn => ProviderKind::Modelscope,
            Self::Intl => ProviderKind::ModelscopeIntl,
        }
    }

    /// 本地区的 provider id（`"modelscope"` / `"modelscope-intl"`）
    pub fn provider_id(self) -> &'static str {
        kind_id(self.kind())
    }

    /// provider id → 地区（本家之外的 id 返回 None）
    pub fn from_provider_id(provider_id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|region| region.provider_id() == provider_id)
    }

    /// 这个 kind 是不是本家（两家都算）—— 判据只在这里写一份
    pub fn from_kind(kind: ProviderKind) -> Option<Self> {
        Self::ALL.into_iter().find(|region| region.kind() == kind)
    }

    /// 展示名（界面上跟在 `ModelScope` 后面的那一段）
    pub fn label(self) -> &'static str {
        match self {
            Self::Cn => "国内版",
            Self::Intl => "国际版",
        }
    }

    /// 推理基址（**含** `api-inference` 主机与尾部的 `/v1`）。
    ///
    /// `build_chat_request` 在它后面拼 `/chat/completions`，
    /// `models.rs` 在它后面拼 `/models` —— 两处都不要再写域名。
    pub fn inference_base_url(self) -> &'static str {
        match self {
            Self::Cn => "https://api-inference.modelscope.cn/v1",
            Self::Intl => "https://api-inference.modelscope.ai/v1",
        }
    }

    /// 网页站基址（身份 / 魔粒余额 / 今日用量 / 签到都在这个域下，**不带尾部斜杠**）。
    ///
    /// 国际站用 `www.` 前缀（实测 `modelscope.ai` 的接口挂在 `www` 主机上，
    /// 与插件源码的 `siteBaseFromURL` 同款判定）。
    pub fn site_base_url(self) -> &'static str {
        match self {
            Self::Cn => "https://modelscope.cn",
            Self::Intl => "https://www.modelscope.ai",
        }
    }

    /// 本地区账号记录 id 的**前缀**（id 生成用）。
    ///
    /// 两地带前缀的理由与 AutoClaw 国际版那一节相同：两站的身份 id 空间互不相通，
    /// 但**同一个人在两地的 `HavanaId` 完全可能相同**（各自注册，服务端 id 生成
    /// 规则同形就会撞）。撞了的话存储层现有的跨 provider 撞 id 保护会直接报错拒绝，
    /// 「两地账号并存」就变得不可能 —— 而并存正是本功能的意义之一。
    ///
    /// 本家没有存量账号，所以国内版**不必**保持裸 id，两家都带前缀（Cline 同款）。
    pub fn account_id_prefix(self) -> &'static str {
        match self {
            Self::Cn => "modelscope-",
            Self::Intl => "modelscope-intl-",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Region;

    /// 两条身份必须与注册表里的 id 逐字对上 —— 对不上的话 `adapter_for` 会返回
    /// 错误的适配器实例（转发打到另一个站点），而这是编译期看不出来的。
    #[test]
    fn region_ids_match_the_expected_literals() {
        assert_eq!(Region::Cn.provider_id(), "modelscope");
        assert_eq!(Region::Intl.provider_id(), "modelscope-intl");
    }

    /// 互查是双射：id → region → id 必须回到原值
    #[test]
    fn region_lookup_round_trips() {
        for region in Region::ALL {
            assert_eq!(Region::from_provider_id(region.provider_id()), Some(region));
            assert_eq!(Region::from_kind(region.kind()), Some(region));
        }
        assert_eq!(Region::from_provider_id("loomy"), None);
    }

    /// 两个站点的域名必须**不同** —— 写重了会让两地账号实际打到同一个站，
    /// 是「看起来支持两地区、实际只有一家」的静默失效。
    #[test]
    fn the_two_regions_do_not_share_a_host() {
        assert_ne!(Region::Cn.inference_base_url(), Region::Intl.inference_base_url());
        assert_ne!(Region::Cn.site_base_url(), Region::Intl.site_base_url());
    }

    /// 账号 id 前缀必须**不同** —— 相同的话同一身份在两地的记录会生成同一个 id，
    /// 第二个添加的账号会撞上存储层的跨 provider 保护而加不进来。
    ///
    /// 注意这里**不**断言「互不为前缀」：`modelscope-intl-` 本就以 `modelscope-`
    /// 开头（Cline 的两个池同款）。判「是不是 ModelScope 系」一律走
    /// `Region::from_provider_id`，绝不要用 `starts_with` 前缀判据 —— 那会把国际版
    /// 误算进国内版。
    #[test]
    fn account_id_prefixes_are_distinct() {
        assert_ne!(Region::Cn.account_id_prefix(), Region::Intl.account_id_prefix());
    }
}