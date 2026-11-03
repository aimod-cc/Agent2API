/**
 * 账号行的**渲染参数**：类型、兜底表、插件申报的读取与归一。
 *
 * ── 为什么单独一个文件 ──────────────────────────────────────
 * 这些参数原先住在 `accounts-domain.ts` 里（那个文件已经承载筛选、分组、限流
 * 文案），而它们回答的是另一个问题：**这一行怎么画**。更要紧的是事实来源变了
 * —— 以前是一张网关前端的常量表，现在是插件在握手时申报（协议里的
 * `AccountFeatures`）。搬家后这里只负责「读插件申报 + 兜底」这一件事。
 *
 * ── 查询顺序 ────────────────────────────────────────────────
 *  1. **插件申报**（`/api/session` 的 `accountFeatures`）—— 插件最清楚自己
 *     那条账号该显示什么；
 *  2. **`PROVIDER_FEATURES`** —— 旧插件不申报时的兜底，保证升级过程中画得对；
 *  3. **`GENERIC_FEATURES`** —— 谁都没说时的最保守档。
 *
 * 等所有插件都升级完，第 2 层可以整张删掉（删之前确认申报覆盖了表里每一个 id）。
 */

import { shared, type LimiterRule } from './accounts-shared'

export type ProviderFeatures = {
  /** 这一家有没有余额 / 积分查询概念 */
  usage: boolean
  /** 有没有国内 / 国际版概念（决定提供商徽章是否拼版本后缀、有效期读哪个字段） */
  edition: boolean
  /** 账号标识落在记录里的哪些键（按顺序取第一个有值的：uid / userId / account） */
  identifier: readonly string[]
  /** 有效期落在记录里的哪些键（expiresAt / tokenExpiresAt / tokenExpireAt） */
  expiry: readonly string[]
  /**
   * 主凭证**长期有效**（没有固定到期时刻）：有效期列显示「长期有效」。
   *
   * 与 `expiry: []` 的区别是**语义**而不是排版：空数组只说「这一列没有
   * 可读的字段」，读起来像「记录里缺数据」；`longLived: true` 是明确断言
   * 「凭证不会到期，不必关心有效期」。两者都让 `tokenExpiryOf` 返回 0。
   *
   * 判据只认上游事实，两类可以登记：
   *   1. 主凭证不过期、短期 access token 由网关自动续期 —— Antigravity 的
   *      Google refresh token、智谱清言的 refresh token；
   *   2. 令牌本身没有有效期概念 —— ModelScope 的 `ms-` 令牌、Command Code 的
   *      `user_` key。
   * **别把「无续期、只能重登」的家放进来**（Loomy 14 天 / MonkeyCode 30 天 /
   * CatPaw 会话 Cookie）：它们的凭证有固定寿命，只是过期后救不回来。
   */
  longLived?: boolean
  /** 「这家的账号以邮箱报名字」（Qoder / AutoClaw 国际版 / Accio）。两处消费：
   *  displayNameOf（**未设备注**账号的主名口径，设过备注的以备注名为主）与
   *  models-fetch-modal 的 accountLabel（拉取模型的账号下拉） */
  emailAsName?: boolean
  /**
   * 有没有「领体验套餐」这个动作（只有 ZCode 两家）。
   *
   * 界面**不再消费这一位**：账号页那颗「领套餐」按钮已撤，入口统一在签到中心的
   * 「活动福利」卡（判据是后端插件申报的 `plans` 位，见 `api::checkin_center`）。
   * 留着它是协议字段的镜像（`AccountFeatures.claim`），排障时仍能看出这一家
   * 是否申报了领取能力。
   */
  claim?: boolean
  /**
   * 有没有「使用哪个套餐（上游通道）」这个设置（只有 ZCode 两家）。
   *
   * 它回答的是「这个账号的请求走哪条上游通道」：编码套餐（自己买的订阅，
   * 开放平台的 OpenAI 端点）还是活动套餐（官方限时发放的额度，`zcode.z.ai`
   * 的 Anthropic 端点）。两者是**两份独立额度**，而「套餐已到期」这类拒绝
   * 只由其中一条给出 —— 所以它是设置项而不是自动探测（见后端
   * `providers::zcode::plan` 的模块头）。
   */
  planChannel?: boolean
  /**
   * 余额列要不要用「套餐徽标 + 点击明细弹层」的两行形态（只有 CodeArts）。
   *
   * 总积分读数只回答余额，套餐权益与分项积分在弹层展开；按能力位声明这个入口。
   */
  usageDetail?: boolean
  /**
   * 余额格的读数旁边要不要挂「积分明细」入口（逐份积分包：面额 / 已用 / 剩余 +
   * 领取 → 到期）—— 已登记的有 WorkBuddy 两家、Trae、OfficeAce、AutoClaw 两家、
   * ZCode 两家、Qoder 两家、Loomy、小浣熊。
   *
   * 与 `usageDetail` 的差别是**动不动排版**：那个会把这格换成「套餐徽标 + 读数
   * 按钮」的整套两行形态；这些家的读数本身（单行读数的「可用 N」、主额度桶的套餐名 +
   * 进度条 + 剩 x / 共 y）已经回答了用户最常问的那句，明细是**加一层**而不是
   * 换一层 —— 换过去主额度桶那格会丢掉进度条。
   *
   * 判据同样按能力位而不是形状探测：后端 `credits[]` 是这些家（以及以后愿意
   * 照这个行形状出数据的家）的**承诺** —— 点开一定有逐份读数，不是点开一个空框。
   */
  creditDetail?: boolean
  /**
   * `credits[]` 里带**逐份到期时刻**（`expireAt`），可以按「快到期优先」排序。
   *
   * 与 `creditDetail` 分开是因为真实分布是分裂的：有明细 ≠ 明细里有到期。
   * WorkBuddy / Trae / OfficeAce 的逐份读数带 `expireAt`；小浣熊只给余额一个数、
   * ZCode 的纯比例窗口连总量都没有、Qoder 的桶级口径没有时刻、Loomy /
   * ModelScope 的两行都是常驻额度。
   */
  creditExpiry?: boolean
  /**
   * 这一家「并发上限」的默认值（>0 = 本家**没有**「不限」这一档）。
   * CodeArts 的 3 是上游硬顶（超过直接回 HTTP 400，且那是账号级冲突、不降级换号），
   * 所以那一家把 `0` 解释成「按默认 3」而不是「不做并发过滤」；后端同一口径写在两处
   * （公开形态把没配过的报成 3、准入闸按 3 判），界面上说了就得对上。
   */
  concurrencyDefault?: number
  /**
   * 记录上没有 `limiters` 也没有 `lowBalance` 时的缺省限制规则。
   *
   * 由插件申报（`AccountFeatures::default_limiters`），网关只在推导缺省值时
   * 用它；用户显式配过的规则永远优先。`undefined` = 插件没申报 → 走下面的
   * `PROVIDER_FEATURES` 兜底。
   */
  defaultLimiters?: readonly LimiterRule[]
}

/**
 * provider 能力表 —— **兜底用**，插件申报优先（见 `providerFeatures`）。
 *
 * ── 为什么现在只是兜底 ──────────────────────────────────────
 * 这张表原本是唯一事实来源，于是「加一家插件」必须回头改网关前端。这些参数
 * 全是上游事实（标识存在哪个键、有效期在哪个键、余额有没有明细），属于插件
 * 自己的知识，所以已迁到协议里由插件握手时申报（`AccountFeatures`）。
 *
 * 保留它是为了**渐进升级**：旧插件不申报，网关必须继续能把它画对。所以查询
 * 顺序是「插件申报 → 本表 → GENERIC_FEATURES」。等所有插件都升级完，本表
 * 可以整张删掉（删之前确认 `accountFeatures` 覆盖了下面每一个 id）。
 *
 * 为什么是「按 provider 查表」而不是在渲染处写 if：账号页的每个分支（余额按钮、
 * 版本后缀、标识字段名）都要问同一个问题 ——「这家有没有这个概念」。
 * 散在各处写 if 的话，加一家就要翻一遍全文件，漏掉一处不报错、只静默少一个按钮。
 *
 * （签到**不在**这张表里：账号页已无签到按钮，签到中心的分组与可签判定在
 * 后端 —— `core::auto_checkin` 的 CHECKIN_PROVIDERS 与 `billing::checkin::
 * supports_checkin` 是唯一口径，各家的签到链路事实也记在那边的模块头里。）
 *
 * usage 各家都是 true（余额查询已扩到全部提供商），各由自己的适配器实现；前端只回答
 * 「这一家有没有这个概念」。CatPaw 的余额接口要单独配一个网页会话凭证（token2），
 * 没配置时后端返回可识别的「未配置」、余额列显示成中性提示 —— 所以它的按钮照样渲染，
 * 用户才有「去配置」的入口。
 */
const PROVIDER_FEATURES: Record<string, ProviderFeatures> = {
  // `creditDetail: true` —— 余额格读数旁边挂「积分明细」：后端 `query_usage` 在既有
  //   积分简报之外还出 `credits[]`（逐份积分包的面额 / 已用 / 剩余 / 领取 / 到期，
  //   见 providers::workbuddy::adapter 的 query_usage）。两地区能力一致，
  //   两项都要登记（漏一个那一家就掉进 GENERIC_FEATURES）。
  //   enterprise 账号没有资源包、后端不挂 `credits`，界面按空数组自然退回原样。
  workbuddy: { usage: true, creditDetail: true, creditExpiry: true, edition: true, identifier: ['uid'], expiry: ['expiresAt'] },
  // WorkBuddy 国际版（拆家后的第二家，见 providers::workbuddy::region）：
  //   `edition: false` 是因为注册名「WorkBuddy 国际版」自带地区，再拼一次会
  //   得到「WorkBuddy 国际版 国际版」（`editionSuffix` 虽有「名字已含就不拼」
  //   的兜底，这里直接写 false 更清楚，与 `zcode-intl` 同款）。
  'workbuddy-intl': { usage: true, creditDetail: true, creditExpiry: true, edition: false, identifier: ['uid'], expiry: ['expiresAt'] },
  // `creditDetail: true` —— 四个钱包桶（每日积分 / 每月会员积分 / 奖励积分 /
  //   充值积分）各一行：上游只给余额一个数，没有面额 / 已用 / 到期（缺的键不出），
  //   明细比读数多的只是「钱在哪个桶里」。
  raccoon: { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: ['tokenExpiresAt'] },
  catpaw: { usage: true, edition: false, identifier: ['uid'], expiry: ['tokenExpiresAt'] },
  // AutoClaw 两个地区能力完全一致，差别只在域名；两项都必须登记 —— 漏了哪一项，
  // 那一家就会掉进 GENERIC_FEATURES（症状：余额按钮消失、标识列显示成空）
  // `creditDetail: true` —— 逐钱包余额各一行，另加一条「即将过期」（上游给的是
  //   相对时长文案，原样带出，不换算成时刻）。两个地区都要带上。
  autoclaw: { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: ['tokenExpiresAt'] },
  'autoclaw-intl': { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: ['tokenExpiresAt'], emailAsName: true },
  // Qoder 两个地区能力完全一致（拆家后是两家 provider，查表按 id 精确匹配，
  // 只登记一个会让国际版掉进 GENERIC_FEATURES）；`edition` 列仍在 —— 两家各是
  // 单一地区，但公开形态仍带 edition 供徽章显示
  // `creditDetail: true` —— 三个桶各一行（套餐额度 / 个人资源包 / 组织资源包），
  //   桶级口径、没有时刻；两个地区都要带上。
  qoder: { usage: true, creditDetail: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  'qoder-intl': { usage: true, creditDetail: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  // 千问办公两版：与 Qoder 同一套账号形态（余额 / 逐桶明细 / 版本徽章 /
  // 以邮箱报名字）。插件已经申报了同一份，这里是旧插件版本的兜底。
  qwenwork: { usage: true, creditDetail: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  'qwenwork-intl': { usage: true, creditDetail: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  // Cline 两条键：同一家上游按计费通道拆成两个 provider，账号形态完全一样（见
  // providers::cline::models）。查表按 id 精确匹配，只登记一个会让另一家掉进兜底
  'cline-free': { usage: true, edition: false, identifier: ['account'], expiry: ['expiresAt'] },
  'cline-pass': { usage: true, edition: false, identifier: ['account'], expiry: ['expiresAt'] },
  // Accio 两个地区：额度可查（上游只给用量百分比）、有地区概念
  accio: { usage: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  'accio-cn': { usage: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'], emailAsName: true },
  // ZCode 两个地区：本家的运营动作是「限时套餐领取」（claim 位）。
  // `usage: true` 对应 providers::zcode::balance —— 它是一条**候选令牌链**：
  //   有套餐 JWT 时读 billing 网关的 `/zcode-plan/billing/balance`（余额桶 + 到期），
  //   否则（或 JWT 被拒时）用 accessToken 读开放平台监控接口的窗口限额
  //   （每 N 小时 / 每周的剩余比例，见 providers::zcode::monitor）。
  //   两把凭证都没有时后端回可识别的「未配置」，余额列显示成中性提示而不是一片红，
  //   所以这颗按钮照样渲染。
  // `claim: true` 是协议里的领取能力位。账号页**已不再**用它出按钮（那颗「领套餐」
  //   已撤，入口在签到中心「活动福利」卡）；这里照旧申报，保持与插件协议一致。
  //   ZCode 的活动形态是每天一份新套餐（plan_id 带日期段），领过之后那一行显示
  //   「今天已领 N 份」、次日自动恢复。
  // expiry 取 expiresAt 是给 add_zcode_account 的契约（落账号时要写访问令牌的过期时间）
  // `creditDetail: true` —— 逐额度桶一行（总量 / 已用 / 剩余，认得出归属时还带套餐名）；
  //   纯比例窗口（监控通道那条链）只有 `remainingPercent`、三个数全缺，整行跳过
  //   （见 zcode::balance 的 credits_of）—— 那一档和现在一样不出入口。两个地区都要登记。
  zcode: { usage: true, creditDetail: true, claim: true, planChannel: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'] },
  'zcode-intl': { usage: true, creditDetail: true, claim: true, planChannel: true, edition: true, identifier: ['userId'], expiry: ['expiresAt'] },
  // CodeArts（华为云 AI 代码助手）。各位各有出处，别照着别家抄：
  // `usage: true` —— 套餐统计返回总积分与分项积分，读不到时不补零。
  // `usageDetail: true` —— 余额列的明细弹层形态（套餐徽标 + 点击逐项读数）：本家
  //   的计量表一行放不下，「可用 N 积分」是加总，两个权益不同的账号看不出差别。
  // 「领福利」不在能力表里：入口已整体迁到「签到中心」（api::checkin_center 按
  //   provider 组装福利行，ui/codearts-welfare.js 的流程两边共用），账号页不再有
  //   这颗按钮 —— 判据只留后端一处，界面不再查表。
  // `edition: false` —— 没有版本/地区概念：region 固定在 cn-north-4 且必须与 token
  //   签发地一致，不是用户可选项；`login_type`（WEB/IDE）也不是版本，别塞进这一列。
  // `expiry: ['expiresAt']` —— 临时凭据约一小时到期，这一列对本家**是主要信息**。
  codearts: {
    usage: true, usageDetail: true, edition: false,
    identifier: ['userId'], expiry: ['expiresAt'],
    concurrencyDefault: 3,
  },
  // Trae（只有 SOLO 那一家）：`edition: false` 是事实 —— 国内 SOLO 与国际版是**两套协议**
  // 而不是一个地区的两种拼法，国际版将来接入时另立 provider id，不把它做成账号字段。
  // `usage: true` 对应 providers::trae::usage（上游两份账：ide_user_ent_usage 的权益包/
  // 积分池 + ide_user_pay_status 的快请求与 SOLO 并发）。
  // `creditDetail: true` —— 同一份 ent_usage 里还读得到**逐份权益包**的
  //   `start_time` / `expire_time`（领取 → 到期），后端摊成 `credits[]`
  //   （见 providers::trae::usage 的 credit_rows）。签到发的包会按天堆起来，
  //   哪一份先作废只有这份清单答得上来。
  trae: { usage: true, creditDetail: true, creditExpiry: true, edition: false, identifier: ['uid'], expiry: ['expiresAt'] },
  // Loomy（讯飞）。各位各有出处：
  // `usage: true` 对应 providers::loomy::balance —— 余额是**双账户**（永久积分 +
  //   每日赠送积分，读 `GET /api/v2/points/records` 的摘要字段），界面上两张卡都列。
  // `edition: false` —— 单一入口（手机验证码登录），没有国际版/地区概念。
  // `expiry: ['']`（**不登记 longLived**）—— 上游登录响应只有
  //   `{session, userid, phone}`，**没有到期时刻**。以前按「登录时刻 + 14 天」
  //   估算一个值填进 `expiresAt` 当有效期显示，结果账号用满两周就红着写
  //   「已过期」，而上游照常放行（实测估算到期后仍能签到、能转发）。
  //   现在不估算、也不展示：失效只认上游的 `020002` / `100002`，那时网关给账号
  //   落 `needsRelogin`，界面显示「需要重新登录」。也不登记 `longLived` ——
  //   session 确实会失效，只是失效时刻不可知。
  // `creditDetail: true` —— 双账户各一行（永久积分 / 每日赠送积分），后者标 `daily`
  //   → 界面写「每日重置」而不是到期（读到 0 也不折进「已用完」，明天回满）。
  loomy: { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: [''] },
  // KukuAI（百度文库库库 AI）。各位各有出处：
  // `usage: true` —— 余额/积分查询（GET /bizapi/gfpro/getgfvipremain 的
  //   totalPoint，见后端 providers::kuku::balance）。
  // `edition: false` —— 单一入口（粘贴 Cookie / 导入本机登录态 / 网页登录），
  //   没有国际版。
  // `identifier: ['uid']` —— 账号标识存 uid（userreport 的 uk）。
  // `expiry: ['']`（**不登记 longLived**）—— BDUSS 是登录态 Cookie，没有可读的
  //   到期时刻；但**不能**写成「长期有效」：上游随时可能让它失效（参考实现的
  //   故障排查里「BDUSS invalid」是最常见的一条），而且本家没有续期接口，
  //   失效只能重登。这一列对它是「没有可显示的事实」，不是「不会过期」。
  kuku: { usage: true, edition: false, identifier: ['uid'], expiry: [''] },
  // OfficeAce（华为云果办 / OfficeClaw）。各位各有出处：
  // `usage: true` —— 额度/积分要**控制面签名**（V11-HMAC-SHA256 打 `/v1/subscription`），
  //   那条链已经实现（`officeace/balance.rs` + 适配器 `query_usage`），所以余额按钮与
  //   积分列都开。⚠️ 只导入网关 Basic 那一对、没有控制面凭据的账号查不到余额，
  //   服务端回 400 `usage_not_configured`，界面按「未配置查询」如实显示（不是报错）。
  // `edition: false` —— 单一入口（自助 OAuth 浏览器登录 / 粘贴网关 Basic 凭据），
  //   没有国际版。
  // `expiry: ['expiresAt']` —— 控制面临时凭据约 2 小时到期；只导入网关 Basic
  //   凭据时没有这个字段，界面不渲染「有效期」（那一半本来也不过期）。
  // `identifier: ['id']` —— 账号标识渲染的是**账号 id**（`officeace-<hash>`，
  //   由网关基址 + app key 派生），不是任何凭据本身：面板上显示 `modelAppKey`
  //   会把网关 key 露在界面上（`identifierOf` 读的是字段**值**，不只是标签）。
  // `creditDetail: true` —— 逐条套餐配额 + 逐条奖励活动各一行（奖励活动含已过期的
  //   —— 弹层把它与「已用完」一起收进末尾折叠组），另加一条联网搜索（单位「次」，
  //   与文档级的「积分」不同）。
  officeace: { usage: true, creditDetail: true, creditExpiry: true, edition: false, identifier: ['id'], expiry: ['expiresAt'] },
  // Antigravity（Google AI IDE）。各位各有出处：
  // `usage: true` —— 额度是**两个模型组 × 两个窗口**（5 小时 / 每周）的剩余比例，
  //   见后端 providers::antigravity::balance（retrieveUserQuotaSummary）；
  //   上游只给比例、没有绝对数，钱包行走纯比例读数（`remainingPercent` +
  //   `balanceView`）。余额列**这四条都画**（逐窗口读数 `windows[]`，与 OpenCode Go
  //   同一形状；一格四条紧凑背景条）—— Gemini 两条 + Claude/GPT 两条，后者正是
  //   issue #288 要看的那份额度（此前只画最紧的一条，等于把第三方那组盖掉了）。
  //   悬停提示逐条列出全名与重置时刻，另带订阅档徽标（Pro / Ultra / Free）。
  // `edition: false` —— 单一入口（Google OAuth 粘贴 / 网页登录），没有地区概念。
  // `identifier: ['userId']` —— 身份键存的就是 email（见 account_store::antigravity_accounts 模块头）。
  // `expiry: ['']` + `longLived: true` —— refresh token 不过期（Google 侧的
  //   refresh token 没有固定寿命，只在撤销授权 / 连续 6 个月不用 / 撞上同一
  //   client 的 100 个上限时才失效），access token 约 1 小时、由网关提前
  //   15 分钟自动续期（见 providers::antigravity::credentials 的模块头）。
  antigravity: { usage: true, edition: false, identifier: ['userId'], expiry: [''], longLived: true },
  // 智谱清言（chatglm.cn）。各位各有出处：
  // `usage: true` —— 余额是**会员积分**（`member-api` 的活动状态 `current_score`，
  //   外加 `payment-api` 的会员档位），见后端 `providers::chatglm::balance`。
  //   ⚠️ **访客账号没有积分体系**：那些接口对访客一律 401，后端回 400
  //   `usage_not_configured`（文案说明「访客本来就没有积分」），界面按「未配置查询」
  //   如实显示而不是报错 —— 与 OfficeAce 只导 Basic 凭据那一档同一处置。
  // `creditDetail: true` —— 逐份读数（会员积分 + 会员档位两行），主读数带活动进度。
  // `edition: false` —— 单一站点（国际站是另一个产品 Z.AI/ZCode，已按独立 provider
  //   建模），本家没有地区/版本之分。
  // `identifier: ['uid']` —— 账号标识存 uid（JWT 的 `uid` 声明；访客也有）。
  // `expiry: ['']` + `longLived: true` —— 登录账号的 refresh token 由网关自动
  //   续期（`user/refresh` 实测可用），访客则由网关用**同一个设备号**重新
  //   `guest/access` 换发，所以「什么时候到期」对用户没有行动价值。
  //   ⚠️ 访客令牌本身有 7 天量级的寿命，换发后身份是否延续由上游决定（设备号
  //   不变是网关能做的全部）；这里说的是「不需要用户操心有效期」，不是
  //   「令牌永不过期」。
  chatglm: { usage: true, creditDetail: true, edition: false, identifier: ['uid'], expiry: [''], longLived: true },
  // ModelScope（魔搭）的两个站点（`modelscope` / `modelscope-intl`）：两条同一份
  //   —— 账号字段、余额形态、签到链路两地完全一致，差别只在域名（那是转发与
  //   凭证层的事，这里不体现）。
  // `usage: true` —— 额度是魔粒钱包（`available_balance`），外加「今日已用调用数」。
  //   `creditDetail: true` 挂出余额详情：可用 / 冻结 / 今日已用都是 snapshot 读数，
  //   不作为积分包计算耗尽、到期或剩余比例。
  // `edition: false` —— 地区由 provider id 直接表达（两家 provider、两个分组），
  //   不需要再渲染一个地区列。
  // `identifier: ['userId']` —— 身份键存的是上游的 HavanaId
  //   （见 account_store::modelscope_accounts 模块头）。
  // `expiry: ['']` + `longLived: true` —— `ms-` 令牌是长期有效的 API key
  //   （插件源码原话「session 就是 api key 本身……永不过期」，实测也没有任何
  //   refresh / renew 接口），没有到期时刻可算。
  modelscope: { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: [''], longLived: true },
  'modelscope-intl': { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: [''], longLived: true },
  // AtomCode（AtomGit / GitCode）。各位各有出处：
  // `usage: true` —— 额度是 `status-v2` 的**每窗口调用次数**（免费档实测 200 次 /
  //   5 小时），已接 `query_usage`（`providers/atomcode/usage.rs`），
  //   所以余额按钮与积分列都开。
  // `edition: false` —— 单一入口（没有国际版）。
  // `identifier: ['userId']` —— AtomGit 用户 id（落盘时写在 `userId` 字段）。
  // `expiry: ['expiresAt']` —— 令牌**不是 JWT**，过期时刻由 `createdAt + expiresIn`
  //   推出后落盘（见 `providers/atomcode/credentials.rs` 的模块头）。
  atomcode: { usage: true, edition: false, identifier: ['userId'], expiry: ['expiresAt'] },
  // Command Code（`api.commandcode.ai`）。此前**漏登记**，于是它掉进
  // GENERIC_FEATURES：余额按钮不显示（后端 `supports_usage` 本来就是 false，
  // 那一项是巧合对上了）、标识字段按 `userId` 读成空（本家公开形态里没有
  // userId，账号 id 是 key 的哈希），有效期列还会去读一个永远不存在的
  // `tokenExpiresAt`。
  // `usage: false` —— 本家**没有余额查询**：`/alpha/billing/*` 只在添加账号时
  //   用来探活（见 providers/commandcode/adapter.rs 的模块头），所以这里如实
  //   关掉（与后端 `supports_usage` 的默认 false 同一口径）。
  // `identifier: ['id']` —— 没有可读的用户标识：账号 id 由 key 的 SHA-256 前
  //   12 位拼成（`commandcode-<hash>`，见 account_store::commandcode_accounts
  //   的模块头），所以标识列显示这个 id 本身，与 officeace 同一处置。
  // `expiry: ['']` + `longLived: true` —— 单个静态 API Key，上游没有 refresh
  //   接口、key 本身也没有到期时刻（见 credentials.rs 模块头）。
  commandcode: { usage: false, edition: false, identifier: ['id'], expiry: [''], longLived: true },
  // LobsterAI（网易有道「有道龙虾」）。各位各有出处：
  // `usage: true` —— 余额是**积分**（`GET /api/user/profile-summary`，见后端
  //   `providers::lobsterai::endpoints` 的 `PROFILE_SUMMARY_PATH`），所以余额按钮
  //   与积分列都开。
  // `creditDetail: true` —— 同一份 profile-summary 还带**逐份积分包**（参考实现的
  //   `creditItems[]` → 面额 / 已用 / 剩余 / 到期，见 Go 参考 `credits.go` 的
  //   `creditPackage`），正是 `credits[]` 那行的形状。与 ZCode 的纯比例窗口不同，
  //   这里每份都有绝对值，能摊成逐行读数。⚠️ 这一位是**承诺**：后端
  //   `query_usage` 不出 `credits` 时入口不挂（accounts-panels 的
  //   `creditRows.length` 判据），所以现在开它是安全的；后端若不实现逐份清单，
  //   把这一位删掉即可，界面自动退回单行读数。
  // `edition: false` —— 单一站点（`lobsterai.youdao.com`），没有国际版 / 地区之分，
  //   所以不拼版本后缀（与 orcaterm / atomcode 同一处置）。
  // `identifier: ['userId']` —— 身份键取 `userId`：后端落账号时的身份口径是
  //   `userId → uid → 令牌尾段`，账号 id 也由它拼成（`lobsterai-<identity>`，
  //   见 `account_store::lobsterai_accounts`），列上与 id 的后缀对得上。
  // `expiry: ['expiresAt']` —— 落盘写的是毫秒 `expiresAt`（显式 > createdAt +
  //   expiresIn > JWT 的 `exp`），与 atomcode 同一条教训。
  lobsterai: { usage: true, creditDetail: true, edition: false, identifier: ['userId'], expiry: ['expiresAt'] },
  // MonkeyCode（长亭科技）的两个站点。此前**漏登记**（后端 `PROVIDERS` 有、本表没有），
  // 于是它掉进 GENERIC_FEATURES：余额列一律显示「—」（`supportsUsage` 走兜底 false），
  // 标识列按 `userId` 读成空，有效期列去读一个不存在的 `tokenExpiresAt`。
  // `usage: false` —— 本家**确实没有余额查询**：适配器 `supports_usage` 保持默认 false
  //   （见 `providers/monkeycode/adapter.rs` 的模块头），这是「上游没有这个接口」，
  //   不是「还没接」。所以这里如实关掉，与后端同一口径。
  // `identifier: ['userId']` —— 公开形态里有 `userId`（见
  //   `account_store::monkeycode_accounts::to_monkeycode_public_account`）。
  // `expiry: ['expiresAt']` —— 公开形态给的是 `expiresAt`（session cookie 的 30 天硬限制，
  //   见 `monkeycode/credentials.rs`），**不是** `tokenExpiresAt`。
  // `edition: false` —— 地区已经由 provider id 表达（两家、两个分组），再拼一次后缀
  //   会得到「MonkeyCode 国际版 国际版」。两个站点都要登记，漏一个就掉回兜底。
  monkeycode: { usage: false, edition: false, identifier: ['userId'], expiry: ['expiresAt'] },
  'monkeycode-intl': { usage: false, edition: false, identifier: ['userId'], expiry: ['expiresAt'] },
  // OrcaTerm（腾讯云遨驰终端）。此前同样**漏登记** —— 后端 `supports_usage` 与
  // `query_usage` 都实现了（`DescribeUserBillingStatus` 的月度 AI 配额，见
  // `providers/orcaterm/balance.rs`），README 的支持矩阵也打了 ✓，但前端查表
  // 查不到它：余额按钮、余额列、批量查询三处一起静默失效。
  // `usage: true` —— 有余额概念（`available` + `unit: "次"` 的归一形状）。
  // `identifier: ['uid']` —— 公开形态给的是 `uid`（`orcaterm_accounts` 的公开形态）。
  // `expiry: ['tokenExpireAt']` —— ⚠ 与别家都不同的字段名（不是 `tokenExpiresAt`）：
  //   公开形态里就叫 `tokenExpireAt`，写错只会让有效期列一直显示「—」。
  // `edition: false` —— 单一入口（桌面端登录态 / 粘贴令牌），没有地区概念。
  // 没有登记 `creditDetail`：`query_usage` 只给 `available` 与 subscription，
  // 不出逐份 `credits[]`，挂入口会点开一个空框。
  orcaterm: { usage: true, edition: false, identifier: ['uid'], expiry: ['tokenExpireAt'] },
}

/**
 * 未登记 provider 的兜底能力：不显示余额 / 版本 —— 这两个都是 provider 私有
 * 概念，未知的家不该被假定拥有。标识字段假定成 userId，取不到时明细行自动少一项。
 */
const GENERIC_FEATURES: ProviderFeatures = {
  usage: false, edition: false, identifier: ['userId'], expiry: ['tokenExpiresAt'],
  emailAsName: false,
}

/**
 * 插件申报的账号行参数（`provider id → 申报`），由 `/api/session` 带回。
 *
 * ── 为什么是一份模块级缓存而不是 React 状态 ─────────────────
 * 读它的地方大多在渲染函数之外（`identifierOf` / `tokenExpiryOf` /
 * `supportsUsage` 这些纯函数被 app.js、report.js、models-fetch-modal 直接
 * 调用），没有组件树可以挂 state。而它每次 `/api/session` 刷新才变一次 ——
 * 换一份新 Map 再通知就够，不必细粒度订阅。
 *
 * ── 为什么空表是安全状态 ────────────────────────────────────
 * 首屏（`/api/session` 还没回来）、旧版后端（不认这个字段）、没装插件 ——
 * 三种情况都是空表，此时全部走 `PROVIDER_FEATURES` 兜底，行为与改造前一致。
 */
let declaredFeatures: ReadonlyMap<string, ProviderFeatures> = new Map()
/** 上一次归一时用的原始对象（引用比较：`/api/session` 换一份才重归一） */
let declaredSource: unknown = null

/**
 * 收下后端带回的申报表（`/api/session` 的 `accountFeatures`）。
 *
 * 形状是协议里 `AccountFeatures` 的 serde 输出（snake_case）。这里做一次
 * 归一：字段缺省按 `GENERIC_FEATURES` 填，数组字段容忍单个字符串（旧版
 * 后端 / 手写配置），这样消费侧不必到处判形态。
 */
export function adoptDeclaredFeatures(raw: unknown): void {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return
  const next = new Map<string, ProviderFeatures>()
  for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!id || !value || typeof value !== 'object' || Array.isArray(value)) continue
    next.set(id, normalizeDeclared(value as Record<string, unknown>))
  }
  declaredFeatures = next
}

/**
 * 当前申报表，**跟着 `/api/session` 自动更新**。
 *
 * ── 为什么在这里懒同步而不是让 app.js 主动推 ─────────────────
 * 读它的地方分布在账号页、报表、模型弹窗，甚至不在 React 里的全局对象上；
 * 让每个入口自己记得「先推一次」是不可能维持的（漏一处就是静默走兜底表）。
 * 这里按引用比较：`/api/session` 每刷新一次就换一份新对象，所以「引用变了
 * 才重归一」既保证新鲜，也不会在热路径上反复建 Map。
 */
function declaredTable(): ReadonlyMap<string, ProviderFeatures> {
  const raw = shared().wbApp?.getState?.()?.accountFeatures
  if (raw !== declaredSource) {
    declaredSource = raw
    adoptDeclaredFeatures(raw)
  }
  return declaredFeatures
}

/** 当前收下的申报表（排障 / 测试用；界面请走 `providerFeatures`） */
export function declaredAccountFeatures(): ReadonlyMap<string, ProviderFeatures> {
  return declaredTable()
}

/** 把协议形态的申报归一成界面形态（缺省按 GENERIC 填）。 */
function normalizeDeclared(raw: Record<string, unknown>): ProviderFeatures {
  return {
    usage: raw.usage === true,
    edition: raw.edition === true,
    identifier: keyList(raw.identifier, GENERIC_FEATURES.identifier),
    expiry: keyList(raw.expiry, GENERIC_FEATURES.expiry),
    longLived: raw.long_lived === true,
    emailAsName: raw.email_as_name === true,
    claim: raw.claim === true,
    planChannel: raw.plan_channel === true,
    usageDetail: raw.usage_detail === true,
    creditDetail: raw.credit_detail === true,
    creditExpiry: raw.credit_expiry === true,
    concurrencyDefault: Number(raw.concurrency_default) > 0 ? Number(raw.concurrency_default) : undefined,
    defaultLimiters: declaredLimiters(raw.default_limiters),
  }
}

/** 键名候选列表：数组原样取字符串项，单个字符串包成一项，其余走兜底。 */
function keyList(value: unknown, fallback: readonly string[]): readonly string[] {
  if (typeof value === 'string') return value ? [value] : []
  if (!Array.isArray(value)) return fallback
  const keys = value.filter((item): item is string => typeof item === 'string' && item.length > 0)
  return keys.length ? keys : []
}

/** 插件申报的缺省限制规则（形态不对的条目整条丢弃）。 */
function declaredLimiters(value: unknown): readonly LimiterRule[] | undefined {
  if (!Array.isArray(value)) return undefined
  const rules: LimiterRule[] = []
  for (const item of value) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) continue
    const rule = item as Record<string, unknown>
    const threshold = Number(rule.threshold)
    if (!Number.isFinite(threshold) || threshold <= 0) continue
    const type = rule.kind === 'token' ? 'token' : rule.kind === 'balance' ? 'balance' : null
    if (!type) continue
    const action = rule.action === 'disable' ? 'disable' : 'skip'
    const period = Number(rule.period_seconds)
    const fixed = type === 'token' && Number.isFinite(period) && period > 0
    rules.push({
      type,
      action,
      threshold,
      enabled: true,
      ...(fixed ? { period, reset: 'fixed' as const } : {}),
      ...(type === 'token' && !fixed ? { reset: 'daily' as const } : {}),
    })
  }
  return rules
}

/**
 * provider id → 账号行参数。查询顺序：**插件申报 → 内置兜底表 → 通用兜底**。
 *
 * 插件申报优先，是因为它比网关更清楚自己那条账号该显示什么；兜底表只在
 * 插件没申报（旧插件）时生效，所以升级过程中两家都能画对。
 */
export function providerFeatures(providerId: string | undefined): ProviderFeatures {
  if (!providerId) return GENERIC_FEATURES
  const declared = declaredTable().get(providerId)
  if (declared) return declared
  return PROVIDER_FEATURES[providerId] || GENERIC_FEATURES
}

