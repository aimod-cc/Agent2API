//! MiniMax Code 的**地区**（国内版 / 国际版）：三套域名与两条 provider 身份。
//!
//! ── 上游长什么样（2026-10-21 实测 + `app.asar` 常量表）──────────
//! 客户端自己就把「地区」当成**第一等的运行期维度**，不是可有可无的标签：
//! ```js
//! // node_modules/@mavis/config/dist/config.js
//! function getRuntimeRegion() {
//!   const val = process.env.MAVIS_REGION;                 // 'cn' | 'en'
//!   if (isValidRuntimeRegion(val)) return val;
//!   if (isElectronProcess()) {
//!     const locale = process.env.NEXT_PUBLIC_LOCALE;
//!     if (locale === 'zh') return 'cn';                    // 中文安装包 → 国内
//!     if (locale === 'en') return 'en';
//!   }
//!   return 'en';
//! }
//! const PRESET_BASE_URLS = {
//!   'cn-prod': 'https://agent.minimax.cn/mavis/api/v1/llm/v1',
//!   'en-prod': 'https://agent.minimax.io/mavis/api/v1/llm/v1',
//! };
//! // node_modules/@mavis/oauth-core/dist/endpoint-config.js
//! const ACCOUNT_ORIGINS = {
//!   cn: { prod: 'https://account.minimax.cn' },
//!   en: { prod: 'https://account.minimax.io' },   // 国际版的授权服务器是另一个域
//! };
//! // node_modules/@mavis/oauth-core/dist/namespace.js
//! namespaceHome = auth/<buildEnv>/<region>/mcode-public     // 两地登录态各存一处
//! ```
//! 于是三个域都按地区分叉：
//! ```text
//!              网关（推理 + 账户 + 签到）              授权服务器
//!   国内  https://agent.minimax.cn/mavis/…        https://account.minimax.cn
//!   国际  https://agent.minimax.io/mavis/…        https://account.minimax.io
//! ```
//!
//! ── ★ 实测证据（2026-10-21，**用真令牌**做的对照矩阵）─────────────
//! ```text
//!   [令牌]           agent.minimax.io   agent.minimaxi.com   agent.minimax.cn
//!   国内（真）            200                200                 200
//!   国际（真）            200              **401**              **401**
//! ```
//! ⇒ **`.io` 才是国际站，`.minimaxi.com` 只认国内令牌。** 两者不是同一套服务。
//!
//! ── ★★ 大陆直连 `.io` 会 307 —— 这是**地域重定向**，不是「没服务」──
//! ```text
//!   大陆出口 → GET agent.minimax.io/…  → 307 → agent.minimaxi.com/…
//!             ↳ 跟着跳过去 = 拿到国内集群的应答 ⇒ 国际令牌必然 401
//!   海外出口 → GET agent.minimax.io/…  → 200（直接服务，不跳）
//! ```
//! 客户端自己也是 `redirect: 'error'`（**不跟随**），正是为了不被这次地域
//! 重定向带偏。我们这边同理：**国际版请求必须不跟随 3xx**，否则
//! 「出口 IP 不在国际站服务范围」会被误报成「令牌失效」。
//!
//! ⚠️ 部署含义：国际版账号要求网关的**出口 IP 在境外**（或上游放行）。
//! 国内直连只会拿到 307 ⇒ 要么配代理（`egress` 的系统代理会生效），
//! 要么明确接受「国际版在当前网络下不可用」，而**不该**把它翻成「请重新登录」。
//!
//! 授权服务器仍是 `account.minimax.io`（设备码与续期都走它，实测 200；
//! `verification_uri` 原样透传 —— 改写会让用户看到的域与页面证书不一致）。
//! 续期域名与网关无关：**令牌换到了 ≠ 能用**，两件事分开看。
//!
//! ── 协议层：两地区同形 ──────────────────────────────────────
//! Anthropic Messages / 同一套 MD5 签名 / 同一套设备码 grant / 同一个
//! `client_id=mcode-public`（该常量在 `@mavis/oauth-core/dist/contracts.js`
//! 里全局唯一，不按地区分），**只有域名不同**。
//!
//! ── ⚠️ 郊区与权威域：本仓打 `.minimaxi.com`，依据如下 ──────────────
//! 客户端常量表把国际版 prod 写成 `.io`（`PRESET_BASE_URLS['en-prod']`，
//! 且 `DOMAIN_URL` 在 `isEn` 分支下也是它），而 `.minimaxi.com` 被列在
//! `LEGACY_MANAGED_PRESET_BASE_URLS` 里。单看常量表会以为该用 `.io`，
//! 但**实测结论是两者都在服务，只是 `.io` 会按 IP 做地理重定向**：
//! ```text
//!   GET agent.minimax.io/mavis/api/v1/models
//!        从中国大陆直连          → 307 → agent.minimaxi.com（Akamai，X-From: ak）
//!        经海外出口（代理）      → 200（直接服务，不跳）
//!   GET agent.minimaxi.com/mavis/api/v1/models  → 200（两地 IP 都直连可用）
//! ```
//! ⇒ 本仓**直接打 `.minimaxi.com`**：它对两种 IP 都是一跳到位，省掉那次
//!   307（跨域重定向还会丢 `Authorization`，实测带 `--location-trusted`
//!   保留后账户端点仍不通）。这是「选一个两边都通的地址」，不是「`.io` 坏了」。
//!
//! `verification_uri` 仍按上游原样透传（授权页在 `account.minimax.io`，
//! 那是上游给的地址，不该由我们改写 —— 改写会让用户看到的域与页面证书不一致）。
//! 清单缓存单列一条 [`Region::catalog_urls`] 就是为这件事：`.io` 那条一旦被
//! 上游摘掉，缓存里还留着一句「往 .io 拉」，症状是重启后国际版清单一夜消失。
//!
//! ── ⚠️ 国际版的账户/推理 401：**不是本仓实现问题**（实测已排除）────
//! 用本机客户端 `auth/prod/cn` 的真实国内令牌与库里的真实国际版令牌做过
//! 完整矩阵对照（3 个域名 × `region=cn|en` × 余额/身份两条端点，同一份签名）：
//! ```text
//!   [令牌]              域名          余额/身份
//!   国内（客户端）      cn-gw         ✅200 / ✅200
//!   国内（客户端）      intl-legacy   ✅200 / ✅200
//!   国内（客户端）      intl-io       (307) / ✅200（跟随跳转后）
//!   国际（本仓登录）    cn-gw          401  /  401  {"error":"invalid access token"}
//!   国际（本仓登录）    intl-legacy    401  /  401
//!   国际（本仓登录）    intl-io       (307) /  401
//! ```
//! 即：签名实现、路径、域名、`region` 取值四层都已用真令牌逐格验证无误
//! （国内令牌在**同一条命令**下全通），差异只在**令牌本身**。
//! 设备码申请与轮询接口本身正常（`authorization_pending` 是预期响应），
//! 续期接口也正常（200，发回全新令牌）——**但新令牌同样被网关判为无效**。
//!
//! ⇒ 结论：这取决于上游是否为该账号开通了 `agent-backend` 受众的账户/推理
//! 权限（疑似「新注册的国际账号尚未开通 MiniMax Code 权益」）。
//! **不要再改签名、域名或参数** —— 那三层已被真令牌矩阵排除。
//! 复现命令：`python _recon/minimax-probe.py matrix --intl`
//!
//! ── 为什么是两个 provider 而不是「一家的一个选项」────────────────
//! 与本仓 ModelScope / AutoClaw / ZCode 同一思路，且理由比它们更硬：
//!   1. **登录态在两个地区各存一处**（客户端 `auth/<env>/<region>/mcode-public/`），
//!      说明上游把两地当两套账号；
//!   2. **授权服务器不同域** ⇒ 国内拿到的 `mmoat_` 令牌不可能被国际网关接受
//!      （反之亦然）。地区若只是账号上的一个字段，转发链每处都要再读一次它，
//!      读漏一处的症状是「请求打到另一个地区、401」；
//!   3. **模型目录只有一份**（单槽缓存 + 单条刷新排期）⇒ 两个地区的清单会互相
//!      覆盖。这与拆 ModelScope 时那个具体故障逐字相同。
//! 拆成两个 provider 之后，账号集合、清单缓存、刷新排期、签到进度全部按地区
//! 独立，界面上也各占一个分组。
//!
//! ── provider id 的取值（存量兼容）─────────────────────────────
//! 国内版沿用裸 id `minimax`、国际版用 `minimax-intl`。理由是本家**已有存量账号**
//! （2026-10-21 起接入的国内版账号一律记在 `provider: "minimax"` 上），
//! 改名会让它们变成「未知 provider」而被界面隐藏。这与 ModelScope 那次
//! 「全新接入、两家都带前缀」的处置不同，别照抄。账号 id 前缀也按同一口径：
//! 国内版保持裸 `minimax-`（存量 id 原样有效），国际版 `minimax-intl-`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use crate::server::core::providers::{kind_id, ProviderKind};

/// MiniMax Code 的地区。
///
/// 顺序 = 注册表顺序（国内版在前）：`ALL` 的遍历顺序决定模型目录合并时同名模型
/// 先归谁家、以及界面上两家的先后。**国内版在前**（与 ModelScope / MonkeyCode /
/// ZCode 同一处置：先接入、也是绝大多数用户的那一版）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Region {
    /// 国内版（`agent.minimax.cn`；provider id `minimax`）
    #[default]
    Cn,
    /// 国际版（`agent.minimaxi.com`；provider id `minimax-intl`）
    Intl,
}

impl Region {
    /// 两个地区（注册表顺序：国内版在前）
    pub const ALL: [Region; 2] = [Region::Cn, Region::Intl];

    /// 本地区对应哪个 provider kind（地区 → 身份的**唯一**映射）
    pub fn kind(self) -> ProviderKind {
        match self {
            Self::Cn => ProviderKind::Minimax,
            Self::Intl => ProviderKind::MinimaxIntl,
        }
    }

    /// 本地区的 provider id（`"minimax"` / `"minimax-intl"`）
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

    /// 展示名（界面上跟在 `MiniMax Code` 后面的那一段）
    pub fn label(self) -> &'static str {
        match self {
            Self::Cn => "国内版",
            Self::Intl => "国际版",
        }
    }

    /// 客户端里的地区标识（`MAVIS_REGION` 的两个取值，见模块头）。
    ///
    /// 用在本仓登录任务与 `endpoints::resolve_edition` 的口径对齐上
    /// （登录任务的 `edition` 字段是 `"cn"` / `"intl"`）。
    pub fn edition_id(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Intl => "intl",
        }
    }

    /// 网关基址（推理 + 账户 + 签到都挂在这个域下，**不带尾部斜杠**）。
    ///
    /// ⚠️ **国际版必须用 `.io`**（2026-10-21 实测纠正，之前写 `.minimaxi.com` 是错的）：
    /// 这两个域**不是同一套服务**，`.minimaxi.com` 只认国内令牌：
    /// ```text
    ///   国际令牌 → agent.minimax.io     → 200（余额 / 身份 / 签到 / 推理 全通）
    ///   国际令牌 → agent.minimaxi.com   → 401 {"error":"invalid access token"}
    ///   国际令牌 → agent.minimax.cn     → 401
    ///   国内令牌 → agent.minimax.cn     → 200
    ///   国内令牌 → agent.minimaxi.com   → 200（另一个入口，同一套服务）
    /// ```
    /// 之所以之前误判：大陆直连 `.io` 会回 **307 → `.minimaxi.com`**，
    /// 于是「跟着跳转」看到的是国内集群，把国际令牌判成无效。**必须不跟随跳转**
    /// （客户端自己也是 `redirect: 'error'`）—— 这个 307 是**地域重定向**，
    /// 说明「你当前的出口 IP 不在国际站的服务范围」，而不是「国际站没服务」。
    ///
    /// ⇒ 部署提示：国际版账号要求网关的**出口 IP 在境外**（或上游放行），
    /// 否则连 `.io` 只会拿到 307，而跟着跳到 `.minimaxi.com` 必然是 401。
    pub fn gateway_url(self) -> &'static str {
        match self {
            Self::Cn => "https://agent.minimax.cn",
            Self::Intl => "https://agent.minimax.io",
        }
    }

    /// 授权服务器基址（设备码 / 换令牌 / 续期）。
    ///
    /// 国际版是 `account.minimax.io`（与网关**不同域**，实测 200 —— 别照国内版
    /// 的规律把它推成 `account.minimaxi.com`）。
    pub fn auth_host(self) -> &'static str {
        match self {
            Self::Cn => "https://account.minimax.cn",
            Self::Intl => "https://account.minimax.io",
        }
    }

    /// 模型目录的候选地址（**按优先级**，第一个成功即止）。
    ///
    /// 国际版列两条：`.io`（国际站，与 `gateway_url` 一致）在前、`.minimaxi.com`
    /// 在后。目录端点**免鉴权且两边同形**，所以用国内入口作兜底不涉及凭据安全
    /// （它只决定「模型清单从哪儿拉」，不决定令牌发往哪里）；国内版一条就够。
    ///
    /// 与 `gateway_url` 分开写而不是拼出来的理由：目录端点是**免鉴权**的公共
    /// 清单，会被落盘缓存成「往哪儿拉」，域名变化时改这里一处即可。
    pub fn catalog_urls(self) -> &'static [&'static str] {
        match self {
            Self::Cn => &["https://agent.minimax.cn"],
            Self::Intl => &["https://agent.minimax.io", "https://agent.minimaxi.com"],
        }
    }

    /// 目录缓存槽名（`catalog_cache::SCOPE_*`）。
    ///
    /// 两个地区各一格：单格的话两地的清单会互相覆盖（拆家前那个故障）。
    pub fn catalog_scope(self) -> &'static str {
        match self {
            Self::Cn => crate::server::core::providers::catalog_cache::SCOPE_MINIMAX,
            Self::Intl => crate::server::core::providers::catalog_cache::SCOPE_MINIMAX_INTL,
        }
    }

    /// 账号 id 前缀（见模块头「provider id 的取值」）。
    ///
    /// 国内版是裸 `minimax-`（**存量账号的 id 就是这样**，不能改）；国际版带
    /// `-intl` 段，让同一个人在两地的记录生成不同 id —— 否则第二个添加的账号
    /// 会撞上存储层的跨 provider 保护而加不进来。
    ///
    /// ⚠️ 判「是不是 MiniMax 系」一律走 [`Self::from_provider_id`]，
    /// **绝不要用 `starts_with("minimax")`**：那会把国际版误算进国内版。
    pub fn account_id_prefix(self) -> &'static str {
        match self {
            Self::Cn => "minimax-",
            Self::Intl => "minimax-intl-",
        }
    }

    /// 客户端凭据目录里的**地区段**（`auth/<buildEnv>/<region>/mcode-public/`）。
    ///
    /// ⚠️ 与 `edition_id()` **不同**：客户端用的是 `cn` / `en`，而界面上的版本名是
    /// `cn` / `intl`。证据是客户端自己的拼装（`@mavis/oauth-core/dist/namespace.js`）：
    /// ```js
    /// const namespaceHome = join(authHome, input.buildEnv, input.region, MCODE_OAUTH_CLIENT_ID)
    /// ```
    /// 而 `input.region` 的取值域就是 `getRuntimeRegion()` 的 `'cn' | 'en'`
    /// （同一份 asar 的 `PRESET_BASE_URLS` 用的也是 `cn-prod` / `en-prod`）。
    /// 本机实测目录同样只有 `auth/prod/cn/`。
    ///
    /// 拿 `intl` 去拼会得到一个**不存在的路径** ⇒ 「导入本机登录态」对国际版
    /// 永远报「没读到客户端登录态」，而用户明明登录着 —— 被自己的用例
    /// `each_region_reads_its_own_desktop_file` 抓住了。
    pub fn desktop_region_dir(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Intl => "en",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Region;

    /// 两条身份必须与注册表里的 id 逐字对上 —— 对不上的话 `adapter_for` 会返回
    /// 错误的适配器实例（转发打到另一个地区），而这是编译期看不出来的。
    ///
    /// ⚠️ 国内版必须是裸 `minimax`：存量账号记的就是这个 id，改了等于把老账号
    /// 变成「未知 provider」。
    #[test]
    fn region_ids_match_the_expected_literals() {
        assert_eq!(Region::Cn.provider_id(), "minimax");
        assert_eq!(Region::Intl.provider_id(), "minimax-intl");
    }

    /// 互查是双射：id → region → id 必须回到原值
    #[test]
    fn region_lookup_round_trips() {
        for region in Region::ALL {
            assert_eq!(Region::from_provider_id(region.provider_id()), Some(region));
            assert_eq!(Region::from_kind(region.kind()), Some(region));
        }
        assert_eq!(Region::from_provider_id("marvis"), None);
        // 前缀判据的反面例子：`minimax-intl` 不能被 `from_provider_id` 认成国内版
        assert_eq!(
            Region::from_provider_id("minimax-intl"),
            Some(Region::Intl)
        );
    }

    /// 两个地区的三套域名必须**各自不同** —— 写重了会让两地账号实际打到同一个站，
    /// 是「看起来支持两地区、实际只有一家」的静默失效。
    ///
    /// 授权服务器尤其容易写错：国际版的授权服务器是 `account.minimax.io`，
    /// 不是网关域 `agent.minimaxi.com` 的同名兄弟。
    #[test]
    fn the_two_regions_do_not_share_a_host() {
        assert_ne!(Region::Cn.gateway_url(), Region::Intl.gateway_url());
        assert_ne!(Region::Cn.auth_host(), Region::Intl.auth_host());
        assert_ne!(Region::Cn.catalog_scope(), Region::Intl.catalog_scope());
    }

    /// 国际版的权威域是 `agent.minimaxi.com`（`.io` 只回 307），且候选表里
    /// 第一条就是它 —— 顺序反了会多一跳、并依赖上游继续保留那个重定向。
    #[test]
    fn the_intl_catalog_prefers_the_canonical_host() {
        let urls = Region::Intl.catalog_urls();
        // `.io` 是国际站（2026-10-21 用真令牌实测：`.io` 200、`.minimaxi.com` 401）。
        // 目录端点免鉴权、两边同形，所以 `.minimaxi.com` 留作兜底是安全的 ——
        // 它只决定「清单从哪儿拉」，不决定令牌发往哪里。
        assert_eq!(urls.first().copied(), Some("https://agent.minimax.io"));
        assert!(
            urls.contains(&"https://agent.minimaxi.com"),
            "保留国内入口作为目录兜底"
        );
        assert_eq!(
            Region::Cn.catalog_urls(),
            &["https://agent.minimax.cn"],
            "国内版只有一个域，不要列冗余候选"
        );
        // 目录的首选必须与网关一致，否则会「清单从 A 拉、请求打 B」——
        // 拆家后这两者各写一份，最容易在这里分叉。
        assert_eq!(
            urls.first().copied(),
            Some(Region::Intl.gateway_url()),
            "目录首选域必须等于网关域"
        );
    }

    /// 授权服务器与网关必须是**不同主机**（国内 `account.minimax.cn` vs
    /// `agent.minimax.cn`：同后缀不同子域，拼错就会把设备码打到网关上）。
    #[test]
    fn auth_hosts_are_not_the_gateways() {
        for region in Region::ALL {
            assert_ne!(region.auth_host(), region.gateway_url());
            assert!(
                region.auth_host().starts_with("https://"),
                "OAuth 端点只走 https：{}",
                region.auth_host()
            );
        }
    }

    /// 账号 id 前缀必须**不同** —— 相同的话同一身份在两地的记录会生成同一个 id，
    /// 第二个添加的账号会撞上存储层的跨 provider 保护而加不进来。
    #[test]
    fn account_id_prefixes_are_distinct() {
        assert_ne!(Region::Cn.account_id_prefix(), Region::Intl.account_id_prefix());
        // 国内版保持存量口径（老账号的 id 就是 `minimax-<hash>`）
        assert_eq!(Region::Cn.account_id_prefix(), "minimax-");
    }

    /// **国际版网关必须是 `.io`，绝不能被改回 `.minimaxi.com`。**
    ///
    /// 这条是 2026-10-21 一次真实误判的护栏：当时看到大陆直连 `.io` 回 307
    /// 跳 `.minimaxi.com`，就以为「`.io` 没服务、跟着跳才对」，于是把网关写成
    /// `.minimaxi.com` —— 结果国际版账号的余额稳定 401，被误报成「登录态过期」，
    /// 用户反复重登三次都没用。真相是**两个域不是同一套服务**：
    /// ```text
    ///   国际令牌 → agent.minimax.io    → 200
    ///   国际令牌 → agent.minimaxi.com  → 401 invalid access token
    /// ```
    /// 那个 307 是**地域重定向**（出口 IP 不在国际站服务范围），不是「没服务」。
    /// 有人若想把域改回去，先跑这条测试看它为什么拦着。
    #[test]
    fn the_intl_gateway_is_the_io_host_not_the_legacy_one() {
        assert_eq!(
            Region::Intl.gateway_url(),
            "https://agent.minimax.io",
            "国际站是 .io；.minimaxi.com 只认国内令牌（会稳定 401）"
        );
        assert_eq!(Region::Cn.gateway_url(), "https://agent.minimax.cn");
        // 两地的网关必须不同域，否则拆家等于没拆
        assert_ne!(Region::Cn.gateway_url(), Region::Intl.gateway_url());
    }
}
