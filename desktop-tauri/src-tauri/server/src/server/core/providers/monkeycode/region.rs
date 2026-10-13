//! MonkeyCode 的**地区**（国内版 / 国际版）：两套站点域名与两条 provider 身份。
//!
//! ── 为什么地区要成为一等公民（与 AutoClaw / Qoder / ZCode 同一条思路）──
//! MonkeyCode 是同一套客户端协议的两个**站点**（国内 `monkeycode-ai.com` /
//! 国际 `monkeycode-ai.net`），共用同一套账号接口、同一个 session cookie 名、
//! 同一套任务流协议 —— 只有站点不同。把「地区」做成账号上的一个字段的后果
//! 与那几家一模一样：地区成了**账号的属性**，界面上混在一起，「哪个账号走
//! 哪个站点」在列表里看不出来；账号库里的记录也无法按地区隔离。
//!
//! 因此按**两个 provider** 建模（`monkeycode` 国内版 / `monkeycode-intl`
//! 国际版），各自有独立的账号、清单与启停，界面上各占一个分组。本文件是
//! **地区 → 域名 / 身份 / 环境变量**的唯一事实来源，别处不要再写
//! `"https://monkeycode-ai.com"` 或 `"monkeycode-intl"` 这类字面量。
//!
//! ── 与参考资料的差异（一处必须说明的事实）───────────────────────
//! 逆向参考 `Acankao/MonkeyCodeReverseEngineer` 只覆盖了国内站 `.com`
//! （`mvp/config.py` 的 `BASE_URL` 默认值、文档里所有端点示例都是
//! `monkeycode-ai.com`）。国际站 `.net` 的**存在**由官方发布渠道确认为
//! 「MonkeyCode 在线托管版」（README / 多篇公告把 `.net` 指为 hosted 入口），
//! 但参考里**没有** `.net` 的实测端点、cookie 名或字段差异记录。
//! 本模块因此按「同一套协议、只换站点」建模（与 AutoClaw 两地同协议同值的
//! 情形一致），并把 `.net` 的 cookie 名 / 响应字段差异列为待实测项
//! （见 `endpoints.rs` 的模块头）。若将来实测出差异，改这一处即可。
//!
//! ── provider id 为什么国内版保持裸 `monkeycode` ─────────────────
//! 本家是 2026-10 新增的一家，国内版取无后缀的 `monkeycode`（用户直觉里
//! 「MonkeyCode 就是国内那个站」），国际版取 `monkeycode-intl` —— 与
//! AutoClaw / Qoder / ZCode 的命名口径一致。两个 id 一旦落进 accounts.json
//! 就是**落盘契约**，不要再改。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use crate::server::core::providers::{kind_id, ProviderKind};

/// MonkeyCode 的地区。
///
/// 顺序 = 注册表顺序（国内版在前）：`ALL` 的遍历顺序决定模型目录合并时
/// 同名模型先归谁家、以及界面上两家的先后。国内版在前与注册表、与其它
/// 两地区家的书写顺序一致。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Region {
    /// 国内版（`monkeycode-ai.com`；参考资料的覆盖面，provider id `monkeycode`）
    #[default]
    Cn,
    /// 国际版（`monkeycode-ai.net`；官方托管入口，provider id `monkeycode-intl`）
    Intl,
}

impl Region {
    /// 两个地区（注册表顺序：国内版在前）
    pub const ALL: [Region; 2] = [Region::Cn, Region::Intl];

    /// 本地区对应哪个 provider kind（地区 → 身份的**唯一**映射）
    pub fn kind(self) -> ProviderKind {
        match self {
            Self::Cn => ProviderKind::MonkeyCode,
            Self::Intl => ProviderKind::MonkeyCodeIntl,
        }
    }

    /// 本地区的 provider id（`"monkeycode"` / `"monkeycode-intl"`）
    pub fn provider_id(self) -> &'static str {
        kind_id(self.kind())
    }

    /// provider id → 地区（`monkeycode` 系之外的 id 返回 None）
    pub fn from_provider_id(provider_id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|region| region.provider_id() == provider_id)
    }

    /// 这个 kind 是不是 MonkeyCode 系（两家都算）—— 判据只在这里写一份
    pub fn from_kind(kind: ProviderKind) -> Option<Self> {
        Self::ALL.into_iter().find(|region| region.kind() == kind)
    }

    /// 地区短标识（`"cn"` / `"intl"`，用于日志与缓存键）
    pub fn id(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Intl => "intl",
        }
    }

    /// 展示名后缀（界面上跟在 `MonkeyCode` 后面的那一段）
    pub fn label(self) -> &'static str {
        match self {
            Self::Cn => "国内版",
            Self::Intl => "国际版",
        }
    }

    /// 站点 Origin（网页与 API 同域；`mvp/config.py` 的 `BASE_URL` 就是它）
    pub fn site_origin(self) -> &'static str {
        match self {
            Self::Cn => "https://monkeycode-ai.com",
            Self::Intl => "https://monkeycode-ai.net",
        }
    }

    /// 环境变量前缀：`MONKEYCODE_` / `MONKEYCODE_INTL_`。
    ///
    /// 不能两地共用一个变量名：那会让「只想给国际版配代理 / 换站点」变成
    /// 「两地一起改」（与 AutoClaw 同一条理由）。
    pub fn env_prefix(self) -> &'static str {
        match self {
            Self::Cn => "MONKEYCODE_",
            Self::Intl => "MONKEYCODE_INTL_",
        }
    }

    /// 读本地区的环境变量覆盖（空值视为未设置）。返回 None 表示无覆盖。
    pub fn env_override(self, name: &str) -> Option<String> {
        let key = format!("{}{name}", self.env_prefix());
        std::env::var(key)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
    }

    /// 出站基址：`MONKEYCODE_BASE_URL` / `MONKEYCODE_INTL_BASE_URL` 可覆盖。
    ///
    /// 覆盖只影响**本网关打上游的地址**（自建 / 测试环境），不改站点身份 ——
    /// 落盘仍是 provider id，与域名的对应关系由本文件保证。
    pub fn base_url(self) -> String {
        self.env_override("BASE_URL")
            .unwrap_or_else(|| self.site_origin().to_string())
    }

    /// 本地区账号记录 id 的**前缀**（id 生成用）。
    ///
    /// 两地的 provider id 已经把归属分开，但账号 id 在**整份账号集合里唯一**：
    /// 同一个人在两地各自注册、服务端 id 生成规则一旦同形就会撞。带上前缀之后
    /// 两地天然不相交，同一个人的两个地区账号可以并存（与 AutoClaw 同一条
    /// 理由与同一手法）。
    pub fn account_id_prefix(self) -> &'static str {
        match self {
            Self::Cn => "monkeycode-",
            Self::Intl => "monkeycode-intl-",
        }
    }
}
