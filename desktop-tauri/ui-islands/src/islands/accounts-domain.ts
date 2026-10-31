/**
 * 账号页的**纯逻辑层**：基础判定、筛选口径、全局队列位置、限流文案。
 *
 * 替换 ui/accounts-groups.js + ui/accounts-model.js 里的纯逻辑部分（那两个文件里的
 * 「标签 / 面板 HTML」字符串生成器随表格一起变成 React 组件，见 accounts-page.tsx）。
 * 本文件不碰 DOM、不读写模块状态，只依赖 window.wbProviders 的 label 与
 * window.wbUnits 的读数格式化（都是运行期读）。
 *
 * ── 能力表不在这里 ──────────────────────────────────────────
 * 「这一家的账号行怎么画」（标识读哪个键、有效期读哪个键、有没有余额入口）
 * 在 `accounts-features.ts`：那些参数由**插件握手时申报**，本文件只是转出
 * `providerFeatures` 给既有调用点用。分出去是因为它们回答的是另一个问题，
 * 而且事实来源已经从网关前端的常量表变成了插件。
 *
 * ── 对外契约（必须原样保留的调用点）────────────────────────────
 *   · app.js:236  `wbAccountsModel.isLimited`（顶栏「已限流」计数，含余额不足档）
 *   · app.js:663  `wbAccountsModel.isDesktopAccount`（删除确认框的补充说明）
 *   · report.js:307 `wbAccountsModel.editionSuffix`（报表里账号名后的版本后缀）
 *   · models-fetch-modal.tsx:245 `wbAccountsModel.providerFeatures(...).emailAsName`
 *   · models-fetch-modal.tsx:255 `wbAccountsModel.byPriorityOrder`
 * 其余成员只被账号页自己用（`wbAccountsTable` / `wbAccountsColumns` /
 * `wbAccountsFilters` / `wbAccountsGroups` 四个对象在页外无任何引用，已随本页合并
 * 进岛里不再挂 window —— 见最终报告）。
 *
 * ── 全局一条队列 + 按家分组（2026-10 账号页分组化）───────────────
 * 优先级在后端仍是**全局唯一**的一条队列：转发时按优先级从小到大逐个尝试，跳过
 * 禁用 / 不支持该模型 / 该模型限流中的账号（见 priority.rs 与 rotate.rs）。
 * 界面上按家分组显示（`accountGroups`），组顺位 = 各家最靠前的账号的先后 ——
 * 两者能对上，靠的是队列不变量「同一家的账号连续」（后端新增/导入落在本组末尾、
 * 拖拽由 `POST /api/accounts/reorder` 整队重编号，见 store_order.rs）。
 * 于是旧的「全局第 N 位」（`positionMap`）不再直接显示在行上：行上是**组内序号**，
 * 队列的先后由「组顺位 + 组内序号」共同表达。positionMap 保留给需要绝对位置的地方
 * （模型测试的候选清单、⋯ 菜单的「已在第一位」判定）。
 */

import { shared, formatTime, type AccountRecord, type AccountsSnapshot, type AccountSorting, type GroupSortRule, type LimiterRule, type RateLimitInfo, type TokenReading, type UsageEntry } from './accounts-shared'
import { providerFeatures, type ProviderFeatures } from './accounts-features'
import { t } from '../i18n'
import { limiterHit } from './accounts-limiter-model'
import { accountReloginText } from './accounts-authorization'

// 能力表与插件申报的读取在 accounts-features.ts（那边的模块头说明了查询顺序）。
// 这里转出去，是因为域层的既有调用点（`providerFeatures` 被 app.js / report.js /
// models-fetch-modal 当 `wbAccountsModel` 的成员用）都从本文件拿。
export { adoptDeclaredFeatures, declaredAccountFeatures, providerFeatures } from './accounts-features'

/** 缺省 provider id（后端注册表的默认项；旧账号记录没有该字段时的兜底） */
export const DEFAULT_PROVIDER_ID = 'workbuddy'
/** 小浣熊 provider id（只有它需要「桌面端实时登录态」这类专属标记） */
export const RACCOON_PROVIDER_ID = 'raccoon'

/** 账号所属 provider（字段缺失 / 非字符串按默认 provider 兜底，与后端 store 口径一致） */
export function providerOf(account: AccountRecord | null | undefined): string {
  const id = account?.provider
  return typeof id === 'string' && id ? id : DEFAULT_PROVIDER_ID
}

/* ─── OpenCode Zen 判定（出网「轮询」档的唯一开放对象）─────────── */

/**
 * 客户端形态伪装开关的合法值（与后端 `custom_providers::CLIENT_EMULATION_OPENCODE`
 * 逐字一致）。开着它就说明这一家按 OpenCode 官方 CLI 的形状补齐出站请求 ——
 * 也就是免费档那一套（预置卡「OpenCode Zen」默认开启）。
 */
const CLIENT_EMULATION_OPENCODE = 'opencode'

/** OpenCode **Go**（订阅制）的端点：与免费档同域不同档，见后端 `is_opencode_go_endpoint` */
const OPENCODE_GO_BASE_URL = 'https://opencode.ai/zen/go/v1'

/**
 * 这条（提供商, 账号）组合是不是 **OpenCode Zen 免费档** —— 出网「轮询」档在
 * 界面上唯一出现的对象。判据与后端 `custom_providers::is_opencode_zen_account`
 * 逐条对应（两处必须同口径，否则会出现「界面给了这个档、保存却被 400 挡下」）：
 *
 *   1. 提供商记录开着客户端形态伪装（`clientEmulation === 'opencode'`）——
 *      免费档按 IP 限流、429 只管这个 IP 的额度，轮换出口才有意义；
 *   2. 最终 baseUrl 不是 Go 面端点。账号的覆盖项优先于提供商记录（与转发侧
 *      解析最终地址同序），两层都要过。
 */
export function isOpencodeZenProvider(
  provider: { baseUrl?: string; clientEmulation?: string } | null | undefined,
  accountBaseUrl?: string,
): boolean {
  if (!provider || provider.clientEmulation !== CLIENT_EMULATION_OPENCODE) return false
  const override = typeof accountBaseUrl === 'string' ? accountBaseUrl.trim() : ''
  const base = (override || String(provider.baseUrl || '')).trim().replace(/\/+$/, '').toLowerCase()
  if (!base) return false
  return base !== OPENCODE_GO_BASE_URL
}

/**
 * 按账号查出它所属的**自定义提供商记录**（目录缓存是唯一同步数据源；
 * 内置家 / 目录里查不到时给 null）。
 *
 * 目录（`wbProviders.customList`）由 providers.js 统一拉取与缓存，条目就是
 * 后端记录本身（含 `clientEmulation` / `baseUrl`）—— 与账号设置弹窗用
 * `wbCustomProvidersUi.find` 取到的是同一份数据，只是这里同步读缓存。
 */
export function customProviderOf(
  account: AccountRecord | null | undefined,
): { baseUrl?: string; clientEmulation?: string } | null {
  const id = account ? providerOf(account) : ''
  if (!id.startsWith('custom-')) return null
  const list = shared().wbProviders?.customList?.() || []
  return list.find(item => item?.id === id) || null
}

/**
 * 这条账号能不能用出网「轮询」档（弹窗与批量弹窗共用的唯一判据）。
 *
 * 一次读两处：账号自己的 baseUrl 覆盖项 + 目录里那家提供商记录。
 * 缓存里还没有这一家（首屏还没拉到目录）时判定为 false —— 「档位晚一轮出现」
 * 比「点开一个保存必然 400 的档」轻。
 */
export function supportsProxyRotate(account: AccountRecord | null | undefined): boolean {
  if (!account) return false
  return isOpencodeZenProvider(customProviderOf(account), account.baseUrl)
}

/**
 * providers 摘要归一化：`{ providers: [{id,label,count}] }`。
 *
 * 两处兜底是刻意的（摘要缺一项就让整页空白，代价远大于一个小偏差）：
 *   · 后端没给摘要（旧版 / 首屏 state 尚未到达）→ 按现有账号派生；
 *   · 摘要里没有、但账号里出现的 provider → 补在末尾，计数按现有账号算。
 * label 优先问共享的 wbProviders 目录，都取不到时退化成 id 本身。
 */
export function providerSummaries(snapshot: AccountsSnapshot | null | undefined): Array<{ id: string; label: string; count: number }> {
  const list = Array.isArray(snapshot?.providers) ? snapshot.providers : []
  const accounts = Array.isArray(snapshot?.accounts) ? snapshot.accounts : []
  const counts = new Map<string, number>()
  accounts.forEach(account => {
    const id = providerOf(account)
    counts.set(id, (counts.get(id) || 0) + 1)
  })
  const known = new Map<string, { id: string; label: string; count: number }>()
  list.forEach(item => {
    if (!item?.id) return
    known.set(String(item.id), {
      id: String(item.id),
      label: String(item.label || item.id),
      count: Number(item.count) || 0,
    })
  })
  counts.forEach((count, id) => {
    if (known.has(id)) return
    known.set(id, { id, label: shared().wbProviders?.labelOf?.(id) || id, count })
  })
  // 首屏尚无摘要/账号时保持空表；不凭空创建未安装插件的提供商分组。
  return [...known.values()]
}

/**
 * 账号标识（workbuddy 是 uid，小浣熊是 userId）；取不到返回空串。
 *
 * 按申报的候选键**顺序取第一个有值的**：同一条账号可能来自插件新落的记录
 * （键名由插件定）或升级上来的存量记录（键名是网关旧投影的历史契约），
 * 两套键名都列在申报里，谁有值用谁。
 */
export function identifierOf(account: AccountRecord | null | undefined): string {
  return firstKeyOf(account, providerFeatures(providerOf(account)).identifier)
}

/**
 * 账号标识**实际取自哪个键**（悬停气泡上显示 `uid 12345` 里的那个 `uid`）。
 *
 * 与 `identifierOf` 走同一套候选键：候选里第一个有值的那个就是标签。
 * 全部没值时返回空串 —— 气泡那一行整条不渲染，不会留下「 uid」这种半截。
 */
export function identifierKeyOf(account: AccountRecord | null | undefined): string {
  for (const key of providerFeatures(providerOf(account)).identifier) {
    const value = account?.[key]
    if (value === null || value === undefined || value === '') continue
    return key
  }
  return ''
}

/** 按候选键顺序取第一个非空值（空串 / null / undefined 都算没有）。 */
function firstKeyOf(account: AccountRecord | null | undefined, keys: readonly string[]): string {
  for (const key of keys) {
    const value = account?.[key]
    if (value === null || value === undefined || value === '') continue
    return String(value)
  }
  return ''
}

/**
 * 到期时刻**归一到毫秒**：库里落盘的单位在秒与毫秒之间漂过 —— CPA 的 auth 文件与
 * 手工粘贴进来的是 10 位秒，别家与上游刷新响应都是 13 位毫秒。不归一的后果不是
 * "少三位精度"而是整条账号被判死：`1791009732`（秒）当毫秒读是 1970-01-21，
 * 有效期那一列直接显示「已过期」，而它的令牌其实还有几天。
 *
 * 1e11 这个分界与后端 `providers::trae::Credential::expires_at_ms` **同一个口径**
 * （1e11 秒 ≈ 公元 5138 年，真正的毫秒时间戳不可能小于它），两处注释互相指着对方，
 * 改数值时要一起改。0 与负数原样返回：那代表"记录里没给到期时刻"，不能捏成
 * 一个 1970 年的时间戳。
 */
export function expiryMillis(value: unknown): number {
  const raw = Number(value) || 0
  if (raw <= 0) return 0
  return raw < 1e11 ? raw * 1000 : raw
}

/**
 * token 过期时间戳（毫秒，0 表示记录里没有这个字段）。
 *
 * 按申报的候选键顺序取**第一个能解析出正数**的：插件新落的记录与存量旧投影
 * 的键名不同（小浣熊是 `expiresAt` / `tokenExpiresAt`），两套都列在申报里。
 * 注意判据是「能解析出正数」而不是「有值」—— 一个 `null` 或空串不该把后面
 * 那个有值的键挡掉。
 */
export function tokenExpiryOf(account: AccountRecord | null | undefined): number {
  const keys = providerFeatures(providerOf(account)).expiry
  for (const key of keys) {
    const value = expiryMillis(account?.[key])
    if (value > 0) return value
  }
  return 0
}

/**
 * 这家的主凭证是不是**长期有效**（没有固定到期时刻，见 `ProviderFeatures.longLived`）。
 *
 * 有效期列据此把「—」换成「长期有效」：两者在界面上是**两件事** ——
 * 「—」读作「这条记录里没有到期时间」（可能是数据缺失），而长期有效是
 * 「本来就没有到期这回事」。只按 `expiry` 是否为空串判断做不到这个区分：
 * KukuAI 也是空串，但它的 BDUSS 会被上游作废、只是读不出时刻。
 */
export function isLongLivedCredential(account: AccountRecord | null | undefined): boolean {
  return providerFeatures(providerOf(account)).longLived === true
}

/**
 * 该账号所属 provider 是否有余额概念（没有就不渲染余额按钮，也不参与批量查询）。
 *
 * 内置家走能力表（`usage` 位）；**自定义家（含「预置 API」卡片建的那些）不走表**
 * —— 它们的能力来自**提供商记录里的余额方案**，由后端注入成账号公开形态上的
 * `balanceQuery.scheme`（六种方案见 `providers::custom::balance` 与
 * `balance-query.tsx`）：有方案 = 能查余额。
 *
 * 「有方案但凭证没配」**照样算有**：后端那次查询回的是「未配置」的中性提示
 * （不是红色失败），按钮与「查询设置」段都得在，用户才有地方去补 —— 与
 * CatPaw / ZCode 的「未配置也给入口」同一取舍。
 *
 * ── 访客账号（`isGuest`）不参与（2026-10）─────────────────────
 * 智谱清言的访客没有积分体系：上游那几个接口对访客一律 401，查询的结果**恒定
 * 为失败**。它不是「这次没查到」而是「这条账号没有可查的东西」，与「这家没有
 * 余额概念」是同一档事实 —— 所以这里一并返回 false：余额列显示「—」、
 * 批量查询不把它算进分母、设置弹窗里也不出现「查询设置」段。后端
 * `usage_query::supports_usage` 是同一判据（两处必须同口径，否则界面算的目标
 * 集合与后端返回的行会对不上）。
 */
export function supportsUsage(account: AccountRecord | null | undefined): boolean {
  if (account?.isGuest === true) return false
  const provider = providerOf(account)
  if (provider.startsWith('custom-')) return Boolean(account?.balanceQuery?.scheme)
  return providerFeatures(provider).usage
}

/* ─── 每账号的余额查询设置（账号设置弹窗「查询设置」段共用）─── */

/** 自动查询间隔的边界（秒）—— 与后端 `usage_records` 的常量同一对数值 */
export const USAGE_QUERY_MIN_SECONDS = 30
export const USAGE_QUERY_MAX_SECONDS = 86_400

/**
 * 缺省口径（记录上没有 usageQuery 字段时）—— 与后端
 * `usage_records::DEFAULT_QUERY_INTERVAL_SECONDS` 同值：自动查询**开启**、
 * 1 分钟。缺省必须对齐全局任务时代「默认就在查」的行为，否则升级后所有人的
 * 余额列会静默停更。
 */
export const DEFAULT_USAGE_INTERVAL_SECONDS = 60

/* ─── 限制器（余额 / Token 规则列表）─────────────────────────
 *
 * 「余额不足处理」升级成了每账号可自定义的**限制规则列表**（账号记录的
 * `limiters` 键，后端 `core::limiter`）：余额规则沿用旧两档的语义（阈值 +
 * 严格小于，读数来自自动余额查询），Token 规则按**重置周期**内的累计消耗
 * 判定（窗口对齐自然时间的固定窗口，30 分钟 ~ 24 小时）。跳过 = 自动恢复
 * （余额回升 / 窗口重置），禁用 = 不自动恢复、需手动启用 —— 与旧两档一致。
 *
 * 判定与后端选路（`rotate::filter_limiter_blocked`）同一口径；后端公开形态
 * 恒给有效规则数组，这里的读取只做容错（旧版后端没有 limiters 字段时按旧
 * lowBalance 推导，行为与升级前逐字一致）。
 */

/** Token 规则重置周期的边界（秒）—— 与后端 `limiter::MIN/MAX_TOKEN_PERIOD_SECONDS` 同一对数值 */
export const TOKEN_PERIOD_MIN_SECONDS = 1_800
export const TOKEN_PERIOD_MAX_SECONDS = 86_400

/** 旧「余额不足处理」的缺省阈值（限制器上线后只作为旧记录的推导兜底） */
export const DEFAULT_LOW_BALANCE_THRESHOLD = 1

/**
 * 各 provider 的**缺省限制规则**。查询顺序：**插件申报 → 内置表**。
 *
 * 插件申报优先（`AccountFeatures::default_limiters`）：这是「这一家的额度形态
 * 长什么样」的结论，属于 provider 知识。`undefined` = 插件没申报，走下面这张
 * 内置表（与后端 `limiter::default_rules` 同一口径）：
 * - **Cline 免费池 → 无规则**（原「不处理」缺省的理由不变）。
 * - **其余 → 总余额 < 1 跳过**。
 */
export function defaultLimiters(provider: string | undefined | null): LimiterRule[] {
  const declared = provider ? providerFeatures(provider).defaultLimiters : undefined
  if (declared && (provider !== 'codearts' || declared.every(rule => rule.type === 'balance'))) return declared.slice()
  if (provider === 'cline-free') return []
  return [{ type: 'balance', action: 'skip', threshold: DEFAULT_LOW_BALANCE_THRESHOLD, enabled: true }]
}

/**
 * 自动余额查询设置的规范化读取。缺省（字段缺失）= 开启、1 分钟；显式
 * `{enabled:false}` 才是关；开着但间隔缺失 / 越界按缺省间隔跑（脏值不把
 * 自动查询停掉）。调用方拿到的一定是完整形状，不必判「键缺失」。
 */
export function usageQueryOf(account: AccountRecord | null | undefined): {
  enabled: boolean
  interval: number
} {
  const config = account?.usageQuery
  if (!config || typeof config !== 'object') {
    return { enabled: true, interval: DEFAULT_USAGE_INTERVAL_SECONDS }
  }
  const enabled = config.enabled === undefined ? true : config.enabled === true
  const interval = Number(config.interval) || 0
  const valid = interval >= USAGE_QUERY_MIN_SECONDS && interval <= USAGE_QUERY_MAX_SECONDS
  return { enabled, interval: enabled && !valid ? DEFAULT_USAGE_INTERVAL_SECONDS : interval }
}

/** 一条限制规则是否完整可用（容错读取的过滤判据；与后端解析同一套口径） */
function limiterRuleValid(rule: unknown): rule is LimiterRule {
  if (!rule || typeof rule !== 'object') return false
  const data = rule as Record<string, unknown>
  if (data.type !== 'balance' && data.type !== 'token') return false
  if (data.action !== 'skip' && data.action !== 'disable') return false
  const threshold = Number(data.threshold)
  if (!Number.isFinite(threshold) || threshold <= 0) return false
  if (data.type === 'token' && data.reset !== 'daily') {
    // 固定周期要求合法周期；自然日没有周期长度，免校验
    const period = Number(data.period)
    if (!Number.isInteger(period) || period < TOKEN_PERIOD_MIN_SECONDS || period > TOKEN_PERIOD_MAX_SECONDS) {
      return false
    }
  }
  return true
}

/**
 * 记录上没有 `limiters` 键（旧版后端 / 旧记录）时的推导：有显式 `lowBalance`
 * （用户 / 旧版配过）就原样尊重；两者都没有时按 provider 给缺省规则
 * （`defaultLimiters`）。
 * 与后端 `limiter::derive_from_legacy` 同一口径。
 */
function legacyRulesOf(account: AccountRecord | null | undefined): LimiterRule[] {
  const low = account?.lowBalance
  if (!low || typeof low !== 'object' || !low.mode) return defaultLimiters(providerOf(account))
  const thresholdRaw = Number(low.threshold)
  const threshold = Number.isFinite(thresholdRaw) && thresholdRaw > 0 ? thresholdRaw : DEFAULT_LOW_BALANCE_THRESHOLD
  if (low.mode !== 'skip' && low.mode !== 'disable') return []
  return [{ type: 'balance', action: low.mode, threshold, enabled: true }]
}

/**
 * 这条账号的**有效限制规则**：后端公开形态恒给数组（未显式配置 = 推导结果），
 * 这里只做容错过滤 —— 脏条目整条丢弃，不让一条坏数据把其余规则全废掉。
 * 后端没有 limiters 字段（岛比后端新的一瞬 / 旧快照）时按旧配置推导兜底。
 */
export function limitersOf(account: AccountRecord | null | undefined): LimiterRule[] {
  const raw = (account as Record<string, unknown> | null | undefined)?.limiters
  if (Array.isArray(raw)) return raw.filter(limiterRuleValid)
  return legacyRulesOf(account)
}

/** Token 规则子集（列表徽章 / 余额格第二行 / 弹窗展示共用） */
export function tokenRulesOf(account: AccountRecord | null | undefined): LimiterRule[] {
  return limitersOf(account).filter(rule => rule.type === 'token')
}

/** 当前时刻所在窗口的起点（毫秒）—— 与后端 `limiter::window_start` 同一口径 */
export function windowStartOf(nowMs: number, periodSeconds: number): number {
  const periodMs = periodSeconds * 1000
  return nowMs - (nowMs % periodMs)
}

/** 规则的窗口种类：固定周期 = 周期秒数；自然日 = 0（与后端 `rule_kind` 同一口径） */
export function ruleKindOf(rule: LimiterRule): number {
  return rule.reset === 'daily' ? 0 : (rule.period ?? 0)
}

/** 种类 → 当前窗口起点：0 = 本地时区当日 0 点（与签到中心「今天」同一口径），其余固定周期 */
export function kindWindowStart(kind: number, nowMs: number = Date.now()): number {
  if (kind === 0) {
    const day = new Date(nowMs)
    day.setHours(0, 0, 0, 0)
    return day.getTime()
  }
  return windowStartOf(nowMs, kind)
}

/**
 * 规则在**当前窗口**的消耗读数：`readings`（快照 `tokenUsage` 的行）里找
 * 同窗口种类的那条；窗口已翻页（读数的 windowStart 对不上现算值）按 0 算 ——
 * 拦是对「这个窗口已经用超」的断言，拿不出证据就不拦。
 */
export function tokenReadingForRule(
  rule: LimiterRule,
  readings: TokenReading[] | undefined,
  nowMs: number = Date.now(),
): TokenReading | null {
  if (rule.type !== 'token') return null
  const kind = ruleKindOf(rule)
  const reading = (readings || []).find(item => Number(item.kind) === kind)
  if (!reading) return null
  return Number(reading.windowStart) === kindWindowStart(kind, nowMs) ? reading : null
}

/**
 * 这条账号此刻是否应因「余额不足」被跳过（余额列徽章的判据）。
 *
 * 与后端选路过滤（`limiter::balance_skip_blocked`）同一口径：任一启用的余额
 * skip 规则满足「最近一次读数判得出数字且严格小于阈值（等于仍可用）」即拦。
 * `entry` 是余额缓存的读数（`usageEntryOf` 的结果）—— 判不出（未查询 / 失败行
 * / unlimited / 数字缺失）一律放行：跳过是对「这个账号没钱」的断言，拿不出
 * 证据就不亮徽章。
 */
export function balanceBlockedOf(
  account: AccountRecord | null | undefined,
  entry: UsageEntry,
): boolean {
  if (Array.isArray(account?.limiterGroups)) return limiterHit(account, 'balance')
  const rules = limitersOf(account).filter(rule =>
    rule.type === 'balance' && rule.action === 'skip' && rule.enabled !== false && rule.threshold > 0)
  if (!rules.length) return false
  if (!entry || typeof entry !== 'object') return false
  const data = entry as Record<string, unknown>
  if (data.unlimited) return false
  // 数字口径与余额列同源：workbuddy 既有形状 totalLeft，其余 available。
  // **null/undefined 必须显式拦下**：`Number(null)` 是 0（不是 NaN），照直转会把
  // 「没有积分类读数」（CodeArts 免费版账号的统计里没有积分类计量表、available
  // 为 null，额度全在福利 token 池）判成「余额为 0」—— 徽章亮起来、用户去查一个
  // 不存在的问题，而它恰恰违反了上面「拿不出证据就不亮徽章」的口径。
  const raw = data.totalLeft ?? data.available
  if (raw === null || raw === undefined || raw === '') return false
  const remaining = Number(raw)
  return Number.isFinite(remaining) && rules.some(rule => remaining < rule.threshold)
}

/**
 * 这条账号此刻是否应因「Token 限额」被跳过（状态列 / 余额列徽章的判据）。
 *
 * 与后端选路过滤（`limiter::token_skip_blocked`）同一口径：任一启用的 Token
 * skip 规则满足「当前窗口消耗 ≥ 阈值」即拦。`readings` 是用量快照带来的窗口
 * 读数（`tokenUsageOf` 的结果）—— 没有读数 / 窗口已翻页按 0 算（放行）。
 */
export function tokenBlockedOf(
  account: AccountRecord | null | undefined,
  readings: TokenReading[] | undefined,
): boolean {
  if (Array.isArray(account?.limiterGroups)) return limiterHit(account, 'token')
  const rules = limitersOf(account).filter(rule =>
    rule.type === 'token' && rule.action === 'skip' && rule.enabled !== false)
  if (!rules.length || !readings?.length) return false
  const now = Date.now()
  return rules.some(rule => {
    const reading = tokenReadingForRule(rule, readings, now)
    return reading !== null && Number(reading.used) >= rule.threshold
  })
}

/**
 * Token **禁用**档是否已命中（状态列「Token 限额 · 已禁用」徽章的判据）：
 * 任一启用的 Token disable 规则满足「当前窗口消耗 ≥ 阈值」。与后端自动禁用
 * 钩子同判据 —— 账号被禁用（enabled=false）时用它回答「是不是 Token 限额禁的」
 * （禁用动作本身不落「为什么」的标记，读数就是证据）。
 */
export function tokenDisableTriggeredOf(
  account: AccountRecord | null | undefined,
  readings: TokenReading[] | undefined,
): boolean {
  if (Array.isArray(account?.limiterGroups)) return limiterHit(account, 'token', 'disable')
  return limitersOf(account).some(rule => {
    if (rule.type !== 'token' || rule.action !== 'disable' || rule.enabled === false) return false
    const reading = tokenReadingForRule(rule, readings)
    return reading !== null && Number(reading.used) >= rule.threshold
  })
}

/**
 * Token 规则窗口的信息（显示层共用：列表徽章、余额格第二行、编辑器实况提示）。
 * `remainingMs` 是到窗口重置的剩余毫秒 —— 固定周期 = 窗口终点 - now；自然日 =
 * 下一个本地 0 点 - now（用「明日 0 点」现算而不是 +24h，DST 日也说得准）。
 */
export function tokenWindowInfo(
  rule: LimiterRule,
  nowMs: number = Date.now(),
): { kind: number; startMs: number; remainingMs: number; daily: boolean } {
  const kind = ruleKindOf(rule)
  const startMs = kindWindowStart(kind, nowMs)
  const remainingMs = kind === 0
    ? Math.max(0, kindWindowStart(0, startMs + 86_400_000 + 3_600_000) - nowMs)
    : Math.max(0, startMs + kind * 1000 - nowMs)
  return { kind, startMs, remainingMs, daily: kind === 0 }
}

/** 剩余毫秒 → 倒计时一句话：固定周期「剩 23 分钟」，自然日「今日剩 6 小时」。 */
export function tokenCountdownText(remainingMs: number, daily: boolean): string {
  if (remainingMs <= 0) return ''
  const minutes = Math.ceil(remainingMs / 60_000)
  if (daily) {
    const hours = Math.ceil(remainingMs / 3_600_000)
    return hours >= 1 ? t('今日剩 {n} 小时', { n: hours }) : t('今日剩 {n} 分钟', { n: minutes })
  }
  return minutes >= 1 ? t('剩 {n} 分钟', { n: minutes }) : t('剩不到 1 分钟')
}

/** units.js 的公开面（本文件只读 formatTokens）：量级词（万 / 亿 与 k / M）已按界面语言
 *  在那边一处处理，这里不再自己拼字面量。局部窄类型 + 转型而不是 declare global
 *  —— wbUnits 是多页共享的桥，各岛各 declare 一份会撞 TS2717（同 accounts-panels 的处理）。 */
type UnitsBridge = { formatTokens?: (value: unknown) => string }

/** Token 数的展示形态：量级词与分档全交给 units.js 的 formatTokens（简体「31万」、
 *  英文「31k」，设置页那个开关拨一下两处一起变）；拿不到桥时退回裸数字，非数字给「—」。 */
export function formatTokenCount(value: unknown): string {
  const number = Number(value)
  if (!Number.isFinite(number)) return '—'
  const api = (window as unknown as { wbUnits?: UnitsBridge }).wbUnits
  return api?.formatTokens ? api.formatTokens(number) : String(Math.round(number))
}

/** 秒数 → 人能读的间隔文案（`每 90 分钟` 这类），与后端变更提示同一口径：
 * 整小时 / 整分钟进位，其余按秒。 */
export function formatIntervalSeconds(seconds: number): string {
  if (seconds > 0 && seconds % 3600 === 0) return t('{n} 小时', { n: seconds / 3600 })
  if (seconds > 0 && seconds % 60 === 0) return t('{n} 分钟', { n: seconds / 60 })
  return t('{n} 秒', { n: seconds })
}

/** 是否为「桌面端实时登录态」账号（凭证实时读客户端文件；可禁用、也可删除） */
export function isDesktopAccount(account: AccountRecord | null | undefined): boolean {
  return account?.desktop === true
}

/**
 * 该账号所属的 provider 是否能承接推理转发（后端公开形态的 `chatSupported`）。
 * 字段缺失（旧版后端）按「能转发」处理：宁可让界面显示一个正常账号，也不要因为
 * 少了一个字段就把所有账号标成「仅账号管理」。
 */
export function supportsChat(account: AccountRecord | null | undefined): boolean {
  return account?.chatSupported !== false
}

/**
 * 自定义提供商的 id 判据（`custom-` 前缀）。
 *
 * 与后端 `custom_providers::is_custom_provider_id` 的**第一半**一致：那边还要求
 * 「能在配置里找到这家」（判「这个 id 是不是真家」），界面这里问的是另一件事 ——
 * 「这条账号是不是自定义形态」，提供商被删后残留的账号同样要按自定义形态展示
 * （与后端 `to_custom_public_account` 用前缀分派同一取向）。
 */
export function isCustomProviderId(provider: string | null | undefined): boolean {
  return typeof provider === 'string' && provider.startsWith('custom-')
}

export function typeLabel(type: string | undefined): string {
  if (type === 'enterprise') return t('企业')
  if (type === 'ultimate') return t('旗舰')
  return t('个人')
}

/**
 * 转发顺序排序键：优先级升序，并列时按加入时间。
 * 与后端 workbuddy-account-store.mjs 的 byPriorityOrder 保持一致（渲染层无法 import
 * 后端 ESM，只能同构实现；改一处必须同步另一处）。优先级在写入侧强制唯一，
 * 并列只会出现在手工编辑的账号文件里。
 */
export function byPriorityOrder(a: AccountRecord, b: AccountRecord): number {
  const diff = Number(a?.priority ?? 100) - Number(b?.priority ?? 100)
  if (diff !== 0) return diff
  return (Number(a?.addedAt) || 0) - (Number(b?.addedAt) || 0)
}

/** 账号是否启用（禁用账号不参与转发） */
export function isEnabled(account: AccountRecord | null | undefined): boolean {
  return account?.enabled !== false
}

/**
 * 账号是否处于限流状态（存在未到恢复时间的限额记录）。
 * 传入 model 时只判定该模型 —— 限额是按模型记的，一个账号可能对 A 模型限额、
 * 对 B 模型完全正常。
 */
export function isRateLimited(account: AccountRecord | null | undefined, model = ''): boolean {
  const limits = account?.rateLimits || {}
  const now = Date.now()
  if (model) return Number(limits[model]?.resetAt) > now
  return Object.values(limits).some(info => Number(info?.resetAt) > now)
}

/** 账号所属版本：cn=国内 / intl=国际（缺省视为国内，兼容旧账号记录） */
export function accountEdition(account: AccountRecord | null | undefined): 'cn' | 'intl' {
  return account?.edition === 'intl' ? 'intl' : 'cn'
}

/** ZCode 的两条上游通道取值（与后端 `providers::zcode` 的常量逐字一致） */
export const ZCODE_PLAN_CODING = 'coding-plan'
export const ZCODE_PLAN_START = 'start-plan'

/**
 * 这个账号能不能选「使用哪个套餐」（ZCode 独有的设置）。
 *
 * 判据只有能力位：**没有 jwt 也照样给这个设置** —— 那个账号只能选编码套餐，
 * 但用户看得到「有这回事」并在换账号后回来改，比让这个设置凭空消失好。
 * 「没有套餐登录态就别选活动套餐」由对话框里的禁用态说明（见 accounts-dialogs）。
 */
export function supportsPlanChannel(account: AccountRecord | null | undefined): boolean {
  return Boolean(providerFeatures(providerOf(account)).planChannel)
}

/**
 * 该账号当前走哪条通道（非 ZCode 账号返回空串）。
 *
 * 后端公开形态**总是**给这个字段（缺失时它自己就按默认给 `coding-plan`，
 * 见 `to_zcode_public_account`），所以这里只在字段真缺失时兜默认值 ——
 * 两处都兜同一个默认，是为了让「老版本后端 + 新版本界面」也不显示空白。
 */
export function zcodePlanOf(account: AccountRecord | null | undefined): string {
  if (!supportsPlanChannel(account)) return ''
  const raw = String(account?.zcodePlan || '').trim()
  return raw === ZCODE_PLAN_START ? ZCODE_PLAN_START : ZCODE_PLAN_CODING
}

/**
 * 通道的展示名（与后端 `zcode::plan_label` 同一套措辞）。
 *
 * 后端保存成功后会回一句「套餐通道 → 套餐池（Start Plan）」，两处措辞若
 * 不一致，用户会以为设置里选的与提示里说的不是同一件事。
 * Start Plan 叫「套餐池」而不是早期的「活动套餐」：池子里除了活动领取的额度，
 * 还有新注册用户的免费体验额度（issue #134 / #232 的用户都在这一档）。
 */
export function zcodePlanLabel(plan: string | undefined): string {
  return plan === ZCODE_PLAN_START ? t('套餐池（Start Plan）') : t('编码套餐（Coding Plan）')
}

/**
 * 套餐状态 → 展示文案。键由后端给（`providers::zcode::balance` 的 `PlanState::key`：
 * pending / active / expired / unknown），**文案只在前端拼** —— 后端返回中文的话，
 * 非中文界面里会露出一串看不懂的汉字。
 *
 * 三处共用同一份译法：套餐明细弹窗的行内徽章、余额列的「待生效」徽章及其悬停提示。
 * 认不出的键原样显示（上游将来多一个状态时，界面上看到的是那个键，不是空白）。
 */
export function zcodePlanStateLabel(state: unknown): string {
  switch (String(state ?? '').trim()) {
    case 'pending': return t('待生效')
    case 'active': return t('生效中')
    case 'expired': return t('已过期')
    case 'unknown': return t('状态未知')
    default: return String(state ?? '')
  }
}

/**
 * 今天**已经领过的套餐 id**（北京时间自然日）。
 *
 * 读数来自后端落盘的领取台账 `claimPlans`（`{planId: 毫秒}`，见
 * `AccountStore::mark_zcode_claim`）。**逐份**给状态是必需的：同一个账号可能
 * 同时挂着几份可领套餐（活动大额包 + 每日包），而上游的「已领取过」又是**按套餐**
 * 判的 —— 领了 A 之后 B 照样能领。只给一个「今天领过了」会把整颗按钮按住，
 * 用户就再也领不了剩下那几份。
 *
 * 日界用北京时间，与签到 / 福利同一口径：上游的活动按中国时间换期
 * （那期 Trust Build 的套餐 id 就带日期段，每天换一个）。
 */
export function claimedPlanIdsToday(account: AccountRecord | null | undefined): string[] {
  const ledger = account?.claimPlans
  if (!ledger || typeof ledger !== 'object' || Array.isArray(ledger)) return []
  const day = beijingDay()
  return Object.entries(ledger as Record<string, unknown>)
    .filter(([, at]) => Number(at) > 0 && beijingDay(Number(at)) === day)
    .map(([planId]) => planId)
}

/**
 * 余额列要不要走「套餐徽标 + 点击明细弹层」的两行形态（见 ProviderFeatures.usageDetail）。
 */
export function supportsUsageDetail(account: AccountRecord | null | undefined): boolean {
  return Boolean(providerFeatures(providerOf(account)).usageDetail)
}

/**
 * 余额格的读数旁边要不要挂「积分明细」入口（见 ProviderFeatures.creditDetail）。
 *
 * 与 `supportsUsageDetail` 是两件事：那个是**整套两行形态**（会替掉读数本来的
 * 排版），这个是**只加一个入口**（读数末尾的小箭头）。所以判据也不能共用。
 */
export function supportsCreditDetail(account: AccountRecord | null | undefined): boolean {
  return Boolean(providerFeatures(providerOf(account)).creditDetail)
}

/**
 * **账号最早一份「还能用」的积分包到期时刻**（毫秒）；读不到就是 `null`。
 *
 * 「还能用」= 未过期 + 剩余不为 0：已经作废的包不参与「快到期优先」，
 * 否则一个上个月就过期的账号会永远霸占组首。
 *
 * ── 数据从哪来 ────────────────────────────────────────────────
 * 余额读数（`UsageEntry`）里的 `credits[]`：后端逐份给出面额 / 已用 / 剩余 /
 * 领取 / 到期（见 `billing::usage` 的 `credit_packs_from`）。只有登记了
 * `creditDetail` 能力的家才会产出这个数组，其余家自然读到 `null`。
 *
 * ── 非到期积分包要跳过（与 creditRowsOf 的展示判据同源）────────
 *   · `snapshot: true` —— 钱包余额或计数器读数，不是一份有到期的积分包。
 *   · `daily: true` —— 每日回满而不是到期作废，它的 expireAt 是周期边界
 *     （今天 24 点）；拿它参与排序会让日额度账号天天霸占组首。
 *   · `record: true` —— 流水一笔（收支记录），不是一份还有余额的包。
 *   · `unlimited: true` —— 没有「花完」的概念。
 * 这些形态在 `creditRowsOf` 里也有对应判据（那边决定「要不要折进已用完」），
 * 两处问的是同一件事：这份包现在还值不值得关心。
 */
export function creditExpiryOf(entry: UsageEntry, now = Date.now()): number | null {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null
  const data = entry as Record<string, unknown>
  const packs = Array.isArray(data.credits) ? data.credits as Array<Record<string, unknown>> : []
  let best: number | null = null
  for (const pack of packs) {
    if (!pack || typeof pack !== 'object') continue
    if (pack.snapshot === true || pack.record === true || pack.daily === true || pack.unlimited === true) continue
    const expire = Number(pack.expireAt)
    if (!Number.isFinite(expire) || expire <= 0 || expire <= now) continue
    const remain = pack.remain
    // 剩余读到 0 就是花完了；读不到（缺键）时不猜 —— 与 creditRowsOf 的 exhausted
    // 判据同一取向：两侧都读不到就不下结论，宁可让它参与排序
    if (remain !== null && remain !== undefined && remain !== '' && Number(remain) <= 0) continue
    if (best === null || expire < best) best = expire
  }
  return best
}

/**
 * 这一家能不能按「积分到期时间」排序（静态能力位，插件申报优先）。
 *
 * ── 为什么不能直接用 `creditDetail` ───────────────────────────
 * `creditDetail` 只说「这一家有逐份积分明细」，**不保证明细里带到期时间**。
 * 所以协议里另有一位 `creditExpiry`（插件握手时申报），本函数读它。
 * 逐家核对后端产出（`credits[].expireAt`）之后的真实分布是：
 *
 *   ✅ 产出 `expireAt`（申报了这一位）：
 *     · workbuddy / workbuddy-intl —— `billing::usage` 的 `credit_packs_from`
 *       逐份给出 `DeductEndTime`（见 workbuddy/adapter.rs）
 *     · trae —— `trae/usage.rs` 把权益包的 `expire_time`（缺则 `end_time`）写成
 *       毫秒 `expireAt`，并按到期升序排好
 *     · officeace —— `officeace/subscription.rs` 的套餐配额 / 奖励活动行都带
 *       `expireAt`（毫秒，`expire_ms > 0` 才写）
 *     · qoder / qoder-intl —— 配置网页 Cookie（`webCookie`）后，
 *       `resource_package_quota.quota_detail` 的逐份 `expires_at` 写成 `expireAt`；
 *       未配置 Cookie 时只出聚合行、不参与到期排序
 *
 *   ❌ 有明细、但**明细里没有到期时间**（不进名单）：
 *     · raccoon —— 明细只有 `name` + `remain`（上游只给余额一个数）
 *     · zcode —— `credits_of` 只写 total/used/remain，纯比例窗口整行跳过
 *     · loomy —— 永久积分 / 每日赠送两行，都没有到期
 *     · modelscope —— 魔粒余额两行，没有到期
 *     · lobsterai —— 测试明确断言「未核实格式的字段一律不写」
 *     · chatglm —— 只有**流水记录**（`record: true`）带 `expireAt`，
 *       而流水不是「一份还有余额的包」，排序时被 creditExpiryOf 跳过
 *     · autoclaw —— `credits[]` 里没有 `expireAt`；它的 `expireAt` 在
 *       `subscription` 上（订阅到期，不是积分包到期），而「即将过期」那行只有
 *       相对文案（“7 天”），拿它推时刻等于替上游编一个时间
 *
 * 这份分布是**契约**：后端哪天给某一家补上了逐份到期，插件那边要同步申报
 * 这一位，否则那一家的设置项会一直显示「不可用」。
 *
 * 运行时还有第二道闸（方案 B，见 GroupSettingsDialog 的「暂无数据」）：
 * 申报了的家如果这一组所有账号都还没查到到期（没点过「查询余额」），
 * 设置项会标注「暂无数据」而不是假装可用。
 */
export function supportsCreditExpiry(provider: string | null | undefined): boolean {
  if (!provider) return false
  return providerFeatures(provider).creditExpiry === true
}

/**
 * 上游套餐名是否为免费版（`^free$`，不区分大小写）。
 *
 * 判定必须比**上游原始名**（后端 `plan_name` 取 `package_name_en`，现网给过 `Free`），
 * 不能比 `planBadgeLabel` 的输出 —— 那是随界面语言变的展示文案（余额格的
 * `plan-chip.free` 样式类就按这个判）。单独导出，与 `planBadgeLabel` 共用同一口径。
 */
export function isFreePlan(planName: unknown): boolean {
  return /^free$/i.test(String(planName ?? '').trim())
}

/**
 * 上游套餐名 → 界面徽标文案。
 *
 * 后端 `plan_name` 取 `package_name_en` 优先（见 balance.rs），本家现网给过
 * `Trial` / `Free`；中文文案按账号页的措辞习惯映射，认不出的名字原样显示
 * （宁显原名不编中文 —— 与 `quota_meter_label` 的回落同一取向）。
 */
export function planBadgeLabel(planName: unknown): string {
  const name = String(planName ?? '').trim()
  if (/^trial$/i.test(name)) return t('试用版')
  if (isFreePlan(name)) return t('免费版')
  return name
}

/**
 * 今天的**北京时间**自然日（`YYYY-MM-DD`）。
 *
 * ⚠️ 不能按浏览器本地日算：后端那条自然日界是 `welfare::today`（UTC+8，中国无夏令时），
 * 台账里的 `day` 就是它写进去的字符串。界面若在别的时区按本地日判会出现两种错：
 * 北京 0 点前本地已是新一天 → 把昨天的「已领」显示成今天的（按钮被误置灰）；
 * 反过来则今天的读数被当成昨天（该灰不灰）。所以这里比的是**同一个字符串**，
 * 判据只有一份定义。
 */
export function beijingDay(at: number = Date.now()): string {
  return new Date(at + 8 * 3600 * 1000).toISOString().slice(0, 10)
}

/**
 * 领取台账 → 「已领取了吗」的读数（后端写在 `account.welfare` 上；签到中心的
 * 福利行从快照里拿到同一份台账后，用 `{ welfare: row.welfare }` 复用本函数）。
 */
export type WelfareState = { known: boolean; today: boolean; day: string; accepted: boolean; attempts: number; confirmed: number }

/**
 * 台账读数（`{known, today, day, accepted, attempts, confirmed}`）。
 *
 * ── 读不懂的台账一律按「今天没有读数」──────────────────────
 * 缺字段 / 日期不是今天 / 压根没领过，三种情况在这里都是 `today: false` 且
 * `attempts`/`accepted`/`confirmed` 归零：按钮照常可点，由后端如实报错（它那份
 * 校验比这里严，见 `ledger_of`）。这里**不复制**那份校验逻辑，否则同一件事有两处
 * 判据、改一处漏一处。**昨天的台账与今天无关**（后端也是整份重来），所以它只留下
 * `known: true`（「这台机器上有过台账」）而不带昨天的数字。
 *
 * `confirmed` 是**条数**而不是台账上的某个字段：后端把每条活动的进度存在
 * `campaigns` 字典里（`{idempotentKey, claimed, confirmed}`），台账顶层没有
 * `confirmed` 这个键 —— 悬停里那句「已确认到账 N 项」数的就是这里的项。
 */
export function welfareStateOf(account: AccountRecord | null | undefined): WelfareState {
  const day = beijingDay()
  const empty: WelfareState = { known: false, today: false, day, accepted: false, attempts: 0, confirmed: 0 }
  const ledger = account?.welfare
  if (!ledger || typeof ledger !== 'object' || Array.isArray(ledger)) return empty
  const row = ledger as Record<string, unknown>
  const campaigns = (row.campaigns && typeof row.campaigns === 'object' ? row.campaigns : {}) as Record<string, { confirmed?: unknown }>
  const state: WelfareState = {
    known: true,
    today: row.day === day,
    day,
    accepted: row.accepted === true,
    attempts: Number(row.attempts) || 0,
    confirmed: Object.values(campaigns).filter(item => item?.confirmed === true).length,
  }
  return state.today ? state : { ...empty, known: true }
}

/** 「已领取」的悬停说明：说清哪一天、领到哪一份额度、什么时候能再领。（签到中心的福利行用它） */
export function welfareDoneTitle(state: WelfareState): string {
  // 「不增加福利模型的 token 池」是**故意留在这里**的：这一家有两份账，领到的积分进的
  // 是套餐赠送积分，而用户点完最可能问的下一句就是「那我的福利模型怎么还是没额度」——
  // 答案放在这行说明里，不必再去余额明细里猜（后端 usage 文档的 note 同口径）。
  return t('今天（北京时间 {day}）已由官方确认到账 {n} 项；领到的是套餐赠送积分，不增加福利模型的 token 池；按自然日重置，明天可再领', {
    day: state.day, n: state.confirmed,
  })
}

/** 「去领取」的悬停说明：把台账里已有的读数带上，回答「今天第几次了」。 */
export function welfareTodoTitle(state: WelfareState): string {
  const tried = state.today && state.attempts > 0
    ? t('今天（北京时间 {day}）已试过 {n} 次但官方尚未确认到账，', { day: state.day, n: state.attempts })
    : ''
  return t('{tried}探测并领取官方每日登录赠送的套餐积分（到账进套餐积分，不增加福利模型 token 池）', { tried })
}

/* ─── 筛选维度（provider / enabled / limit / q 四维各自独立）───
   `provider` 是工具条最前那颗下拉（'all' = 不限）：分组之后「哪一家」虽然由页面
   结构本身表达，但「只看某一家」仍是常用动作 —— 分组带收起得掉一家，却收不掉
   其余那几家；账号多了之后，把别家整体请出视野正是筛选该干的事。
   `q` 是搜索框（账号名 / 备注 / 邮箱 / 标识），与另外三维共用同一条可见性口径。 */

export type AccountFilter = { provider: string; enabled: string; limit: string; q: string }

/** 余额读数的查找口（账号 → `usageEntryOf` 的结果）；不传视作「都没有读数」 */
export type UsageLookup = (account: AccountRecord) => UsageEntry | undefined

/** Token 周期消耗读数的查找口（账号 → `tokenUsageOf` 的结果）；不传视作「都没有读数」 */
export type TokenUsageLookup = (account: AccountRecord) => TokenReading[]

/**
 * 「已限流」的**完整口径**：模型限流中，**或**限制器把账号拦在了选路外 ——
 * 余额不足已到跳过档（`entry` 是余额缓存的读数）**或** Token 限额跳过档
 * （`tokenReadings` 是用量快照带来的窗口读数）。后两类在 `rateLimits` 里没有
 * 任何记录，但同样让账号此刻接不了转发 —— 用户在筛选分段里问的是「哪些账号
 * 不接请求」，所以一起归进「已限流」，筛选、分段计数、限流列与顶栏徽标共用
 * 这一份判定，不会各算各的。
 * 两条限制**只有拿得出读数才参与**（判据与徽章同一条，见 balanceBlockedOf /
 * tokenBlockedOf）：没查过 / 查失败的账号不因限制归入已限流 —— 拿不出证据就
 * 不下断言。
 */
export function isLimitedNow(
  account: AccountRecord | null | undefined,
  entry?: UsageEntry,
  tokenReadings?: TokenReading[],
): boolean {
  return isRateLimited(account)
    || balanceBlockedOf(account, entry)
    || tokenBlockedOf(account, tokenReadings)
}

/** 搜索词是否命中这条账号：账号名 / 备注名 / 邮箱 / 厂商标识 / id 任一含它就留下。
 *  大小写不敏感（英文邮箱与 id 常见大小写混写）。空词命中一切。 */
export function matchQuery(account: AccountRecord, filter: AccountFilter): boolean {
  const q = String(filter.q || '').trim().toLowerCase()
  if (!q) return true
  const haystack = [
    displayNameOf(account),
    account?.name,
    account?.nickname,
    account?.email,
    identifierOf(account),
    account?.id,
  ]
  return haystack.some(value => String(value || '').toLowerCase().includes(q))
}

/** 提供商维度：'all' 不限；其余按 provider id 精确相等（与 `providerOf` 同一口径 ——
 *  字段缺失的老记录归默认家，不会因为筛选而漏掉） */
export function matchProvider(account: AccountRecord, filter: AccountFilter): boolean {
  return filter.provider === 'all' || providerOf(account) === filter.provider
}

export function matchEnabled(account: AccountRecord, filter: AccountFilter): boolean {
  if (filter.enabled === 'all') return true
  return filter.enabled === 'enabled' ? isEnabled(account) : !isEnabled(account)
}

/** 已禁用账号既不算「正常」也不算「已限流」：限流状态只对参与转发的账号有意义 */
export function matchLimit(
  account: AccountRecord,
  filter: AccountFilter,
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
): boolean {
  if (filter.limit === 'all') return true
  if (!isEnabled(account)) return false
  const limited = isLimitedNow(account, usageOf?.(account), tokenUsageOf?.(account))
  return filter.limit === 'limited' ? limited : !limited
}

/** 单条账号是否通过当前筛选（四个维度的合取）。`visibleAccounts` 就是它在列表上的 filter */
export function matchesFilter(
  account: AccountRecord,
  filter: AccountFilter,
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
): boolean {
  return matchProvider(account, filter)
    && matchEnabled(account, filter)
    && matchLimit(account, filter, usageOf, tokenUsageOf)
    && matchQuery(account, filter)
}

/** 当前筛选条件下的可见账号（四个维度同时生效） */
export function visibleAccounts(
  all: AccountRecord[] | null | undefined,
  filter: AccountFilter,
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
): AccountRecord[] {
  return (all || []).filter(account => matchesFilter(account, filter, usageOf, tokenUsageOf))
}

/**
 * 分段计数：状态与限额按「两级筛选」各管一段 ——
 *   · 状态段的数字 = 全量（提供商 + 搜索取景后），不受限额档位影响：状态是
 *     一级筛选，它要回答「现在一共有多少账号」，数字必须恒定；
 *   · 限额段的数字 = 状态收窄后的范围（提供商 + 搜索 + 状态档），再按
 *     正常 / 已限流分档：限额是二级筛选，它数的是「这一级里还有多少」。
 * 徽标与点进去看到的行数仍然一致（状态段在限额=全部时所见即徽标；限额段
 * 在当前状态档下所见即徽标），而「全部」恒等于家底总数，切限额不再让状态
 * 的数字跟着变。
 */
export function filterCounts(
  all: AccountRecord[] | null | undefined,
  filter: AccountFilter,
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
): Record<string, number> {
  const list = all || []
  const scope = (except: 'enabled' | 'limit') => list.filter(account =>
    matchProvider(account, filter)
    && (except === 'enabled' || matchEnabled(account, filter))
    && matchQuery(account, filter))

  const forEnabled = scope('enabled')
  const forLimit = scope('limit')
  return {
    enabledAll: forEnabled.length,
    enabled: forEnabled.filter(isEnabled).length,
    disabled: forEnabled.filter(a => !isEnabled(a)).length,
    limitAll: forLimit.length,
    normal: forLimit.filter(a => isEnabled(a) && !isLimitedNow(a, usageOf?.(a), tokenUsageOf?.(a))).length,
    limited: forLimit.filter(a => isEnabled(a) && isLimitedNow(a, usageOf?.(a), tokenUsageOf?.(a))).length,
  }
}

/**
 * 账号**当前生效**的限流记录：`[{model, status, code, resetAt, message}]`，
 * 按恢复时间升序（最早恢复的排最前）。过期记录视为不存在（冷却自然结束）。
 * 「限流」列与展开的明细面板共用这一份口径 —— 列上的数字与点开看到的条数不会对不上。
 */
export function activeLimits(account: AccountRecord | null | undefined): Array<RateLimitInfo & { model: string }> {
  const limits = account?.rateLimits
  if (!limits || typeof limits !== 'object') return []
  const now = Date.now()
  return Object.entries(limits)
    .map(([model, info]) => ({ model, ...(info && typeof info === 'object' ? info : {}) }))
    .filter(item => Number(item.resetAt) > now)
    .sort((a, b) => Number(a.resetAt) - Number(b.resetAt))
}

/**
 * 转发顺序位置表：accountId → `{ position, total }`（1 起）。
 *
 * 有网关快照时使用有效消耗顺序；旧客户端回落到账号库的手动优先级。
 */
export function positionMap(all: AccountRecord[] | null | undefined, sorting?: AccountSorting | null): Map<string, { position: number; total: number }> {
  const positions = new Map((sorting?.providerOrder || []).flatMap(provider => sorting?.accountOrder[provider] || [])
    .map((id, index) => [id, index]))
  const ordered = (all || []).slice().sort(positions.size
    ? (left, right) => (positions.get(left.id) ?? Infinity) - (positions.get(right.id) ?? Infinity)
    : byPriorityOrder)
  const map = new Map<string, { position: number; total: number }>()
  ordered.forEach((account, index) => map.set(account.id, { position: index + 1, total: ordered.length }))
  return map
}

/* ─── 分组（账号页的显示口径）────────────────── */

/**
 * 组内排序规则的一项。**按数组顺序比较**（第一项相同才看第二项），
 * 与设置弹窗里从上到下的优先级是同一个东西。
 *
 * 只登记「可比较的字段」这两项：`manual` 不是一条规则，它是所有规则都相等时的
 * 兜底（保持拖出来的顺序），所以不在这里出现。
 */
export type { GroupSortRule } from './accounts-shared'

/**
 * 默认规则。`creditAvailable` 由调用方按「这一家此刻有没有可用到期依据」给：
 * Qoder 需要账号配置网页 Cookie，其余家沿用原有能力位。
 */
export function defaultGroupSortRules(
  provider: string,
  creditAvailable = supportsCreditExpiry(provider),
): GroupSortRule[] {
  return [
    { key: 'credit', on: creditAvailable, dir: 'soon' },
    { key: 'added', on: false, dir: 'new' },
  ]
}

export type AccountGroup = {
  /** 这一组的家（provider id） */
  provider: string
  /** 家名（后端注册表的 label，取不到回落 id） */
  label: string
  /** 组顺位（1 起）：拖动整组改变的就是它 */
  rank: number
  /** 组内全部账号，按网关的全局消耗规则排序；筛选不改变序号。 */
  members: AccountRecord[]
  /** 落库的手动顺序；自动规则只投影有效顺序，不覆盖手动兜底。 */
  queueMembers: AccountRecord[]
  /** 组内**可见**的账号（members 里通过筛选的那些，顺序同上）：渲染用它 */
  accounts: AccountRecord[]
  enabled: number
  disabled: number
  limited: number
}

/**
 * 一组账号按规则排序（不改变入参数组）。
 *
 * ── 为什么「没有值」恒排最后 ──────────────────────────────────
 * 排序方向只决定**有值的那批**谁先谁后。把「读不到到期时间」当成 0 或者当成
 * 无穷大，都会让方向一翻就把它甩到最前 —— 而「读不到」不是一个时间，它不该
 * 因为用户点了「最晚到期优先」就跑到第一位。
 *
 * ── 全相等时保持手动顺序 ──────────────────────────────────────
 * `Array.prototype.sort` 在 ES2019 起是稳定排序，这里不依赖它：显式带上原始下标，
 * 让「所有规则都相等 → 保持拖出来的顺序」成为代码里看得见的一条，而不是引擎的实现细节。
 */
export function sortGroupMembers(
  members: AccountRecord[],
  rules: GroupSortRule[] | null | undefined,
  usageOf?: UsageLookup,
  now = Date.now(),
): AccountRecord[] {
  const active = (rules || []).filter(rule => rule.on)
  if (!active.length) return members.slice()
  const indexed = members.map((account, index) => ({ account, index }))
  const expiry = new Map<string, number | null>()
  for (const { account } of indexed) {
    expiry.set(account.id, creditExpiryOf(usageOf?.(account), now))
  }
  indexed.sort((left, right) => {
    for (const rule of active) {
      const diff = rule.key === 'added'
        ? compareAdded(rule.dir, left.account, right.account)
        : compareExpiry(rule.dir, expiry.get(left.account.id) ?? null, expiry.get(right.account.id) ?? null)
      if (diff !== 0) return diff
    }
    return left.index - right.index
  })
  return indexed.map(item => item.account)
}

function compareAdded(dir: 'new' | 'old', left: AccountRecord, right: AccountRecord): number {
  const a = Number(left.addedAt) || 0
  const b = Number(right.addedAt) || 0
  // 缺 addedAt 的旧记录按 0 处理（= 最早），两侧都缺时相等
  return dir === 'new' ? b - a : a - b
}

function compareExpiry(dir: 'soon' | 'late', a: number | null, b: number | null): number {
  if (a === null && b === null) return 0
  if (a === null) return 1
  if (b === null) return -1
  return dir === 'soon' ? a - b : b - a
}

/**
 * 按家分组：组序 = 各家**最靠前**那个账号的先后（组内按优先级升序，并列按加入时间）。
 *
 * 与后端 `priority::grouped_queue_order` 是同一份规则的同构实现（渲染层拿不到 Rust，
 * 只能两处各写一遍 —— 改一处必须同步另一处）。两边的顺序之所以总是一致，靠的是
 * 队列不变量「同一家的账号连续」：组序因此就等于真实队列里的先后。
 * 拖完一次由后端按拖出来的顺序整队重编号，不变量继续成立。
 *
 * `keep` 是可见性谓词（筛选 / 搜索），**只过滤组内的行、不参与定组序**：筛选时
 * 组序不跳动，被筛掉的行也不会让整组消失（组头还在，只是组内空了）。
 *
 * 自动消耗规则由网关执行，`sorting` 提供实际生效的组内顺序。
 * 没有该字段的旧网关才在前端按同一口径降级排序。
 */
export function accountGroups(
  all: AccountRecord[] | null | undefined,
  keep?: ((account: AccountRecord) => boolean) | null,
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
  sortOf?: ((provider: string) => GroupSortRule[] | null | undefined) | null,
  sorting?: AccountSorting | null,
): AccountGroup[] {
  const ordered = (all || []).slice().sort(byPriorityOrder)
  const providers: string[] = []
  const members = new Map<string, AccountRecord[]>()
  for (const account of ordered) {
    const provider = providerOf(account)
    if (!members.has(provider)) {
      members.set(provider, [])
      providers.push(provider)
    }
    members.get(provider)!.push(account)
  }
  if (sorting) {
    const ranks = new Map(sorting.providerOrder.map((provider, index) => [provider, index]))
    providers.sort((left, right) => (ranks.get(left) ?? Infinity) - (ranks.get(right) ?? Infinity))
  }
  return providers.map((provider, index) => {
    const list = members.get(provider) || []
    const ids = sorting?.accountOrder[provider]
    const positions = new Map((ids || []).map((id, position) => [id, position]))
    const shown = ids ? list.slice().sort((left, right) => (positions.get(left.id) ?? Infinity) - (positions.get(right.id) ?? Infinity))
      : sortOf ? sortGroupMembers(list, sortOf(provider), usageOf) : list
    return {
      provider,
      label: shared().wbProviders?.labelOf?.(provider) || provider,
      rank: sorting?.providerOrder.includes(provider) ? sorting.providerOrder.indexOf(provider) + 1 : index + 1,
      members: shown,
      queueMembers: list,
      accounts: keep ? shown.filter(keep) : shown,
      enabled: shown.filter(isEnabled).length,
      disabled: shown.filter(account => !isEnabled(account)).length,
      limited: shown.filter(account =>
        isEnabled(account) && isLimitedNow(account, usageOf?.(account), tokenUsageOf?.(account))).length,
    }
  })
}

/* ─── 展示派生 ─────────────────────────────── */

/**
 * 账号展示名，纯 nameCustom 分流（两条线，不按家再分叉）：
 *   · 用户显式设置过备注名（打标，见后端 apply_patch / mark_name_custom）—— 备注名恒为主名；
 *   · 未打标 —— 维持历史口径：「以邮箱报名字」的三家（Qoder / AutoClaw 国际版 / Accio）
 *     邮箱优先，其余昵称优先，再退备注名种子 / 标识 / id。
 * 备注名种子与用户改的名在记录里无法区分，全靠 nameCustom 分流 —— 没有它，
 * 「备注名优先」会让未设备注的账号顶掉邮箱 / 昵称，显示成建号时的种子值。
 * （中间版本试过「非邮箱系家名字优先」的放宽，好处是更新前的旧备注免重存生效，
 *  代价是取名规则按家分叉、解释成本高 —— 已按用户决定回归纯标记这一条线。）
 */
export function displayNameOf(account: AccountRecord | null | undefined): string {
  if (!account) return ''
  const name = String(account.name || '').trim()
  if (account.nameCustom === true && name) return name
  const email = String(account.email || '').trim()
  if (providerFeatures(providerOf(account)).emailAsName && email) return email
  return account?.nickname || name || identifierOf(account) || account?.id || ''
}

/** 无有效恢复时间时的退化文案：它本身就是完整一句，调用方据此不再拼「，恢复时间：」 */
export const RESET_UNKNOWN = '已限流'

/** 某时刻所在自然日的零点（本地时区），用于按「日历天」计算今天 / 明天 */
const startOfDay = (value: Date): number => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime()

/**
 * 限流恢复时间文案：今天 HH:mm / 明天 HH:mm / M月d日 HH:mm。
 *
 * 为什么带「今天 / 明天」而不是相对毫秒数或完整时间戳：限流是自动解除的，用户扫过
 * 列表时最关心「到点了没、还要等多久」——「明天 01:04」比「09-19 01:04」少一步换算，
 * 也不会像「6 小时后」那样一过夜就说不清是哪天。
 * 无有效时间戳（缺失 / 非法 / 已过）时返回 RESET_UNKNOWN，由调用方退化成只输出这一句。
 */
export function formatResetText(resetAt: unknown): string {
  const time = Number(resetAt)
  if (!Number.isFinite(time) || time <= 0 || time <= Date.now()) return RESET_UNKNOWN
  const date = new Date(time)
  const clock = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false })
  // 按自然日求差而不是按 24 小时：今晚 23:50 到明天 00:10 只差 20 分钟，但用户嘴里
  // 它就是「明天」，按毫秒差算会显示成「今天」，与直觉相反
  const days = Math.round((startOfDay(date) - startOfDay(new Date())) / 86400e3)
  if (days === 0) return t('今天 {clock}', { clock })
  if (days === 1) return t('明天 {clock}', { clock })
  return t('{month}月{day}日 {clock}', { month: date.getMonth() + 1, day: date.getDate(), clock })
}

/**
 * 版本后缀：国内 / 国际。只返回文字，由调用方拼进提供商徽章 ——「WorkBuddy 国际版」
 * 是**一枚**徽章，与 AutoClaw 那种「名字自带版本」的家看起来是同一种标签。
 *
 * 名字里已经带地区的不再拼一遍：`zcode` / `zcode-intl` / `accio-cn` 这几个 provider 的
 * 注册名本身就以地区结尾（「ZCode 国内版」），拼出来是「ZCode 国内版 国内版」。判据取
 * 「注册名是否已含这个后缀串」，而不是再列一张名单 —— 名单会随新增地区漏项。
 * 别用 `edition` 能力位去关：它还兼着「有效期列读哪个字段」的判据，置 false 会把
 * 有效期列改读 tokenExpiresAt，而那对这几家是错的字段。
 */
export function editionSuffix(account: AccountRecord | null | undefined): string {
  const provider = providerOf(account)
  if (!providerFeatures(provider).edition) return ''
  const edition = accountEdition(account)
  const custom = account?.editionLabel
  // 判重比的是注册名里的中文地区词（后端数据，不随界面语言变）；只有最终展示的后缀走 t()
  const suffix = custom || (edition === 'intl' ? '国际版' : '国内版')
  const label = shared().wbProviders?.labelOf?.(provider) || ''
  if (label.includes(suffix)) return ''
  return custom || (edition === 'intl' ? t('国际版') : t('国内版'))
}

/** 健康标签（结构化；原来由 accountTags 直接拼 HTML，现在交给 React 渲染成 Badge） */
export type AccountTag = { text: string; kind: 'plain' | 'bad'; title: string }

/**
 * 状态标签集合：这一区只表达**健康状态**。
 *
 * 「限流」不在这里 —— 限额按模型记，它有自己的一列。**只标「需要关注的状态」，
 * 一切正常时返回空数组**：启用 / 禁用由开关自身表达，再补一枚「启用」徽章是在同一格
 * 里说第二遍同一件事。「不可用」也不再渲染（`available` 把手动禁用也算进去，
 * 禁用的账号开关明明是关着的，再标一枚是把同一件事说两遍）。
 */
export function accountTags(account: AccountRecord): AccountTag[] {
  return [
    account.needsRelogin
      ? {
        text: t('需要重新登录'),
        kind: 'bad' as const,
        title: accountReloginText(account),
      }
      : null,
    // 没有转发能力的家：它的启用开关对转发没有意义，这里如实说明，而不是留一片空白
    // 让人以为「没标记就是好的」。判据是后端的 chatSupported，正常配置下不会出现 ——
    // 留着是为了「将来某家处于只有账号管理的过渡期」时界面能自己说清楚
    supportsChat(account)
      ? null : {
        text: t('仅账号管理'),
        kind: 'plain' as const,
        title: t('该提供商的推理转发尚未接入，账号不参与转发'),
      },
    // 自定义账号没有凭证：既没填 API Key、也没勾「无需鉴权」时，它在选路里会被
    // **静默跳过**（后端 hasCredentials = false，目录也不广告它家的模型）——原样
    // 展示成一条普通账号会让用户完全看不出「为什么加了账号却发不出去请求」。
    // 判据用后端注入的 hasCredentials（所有家都有这个字段，但只有自定义家会为
    // false —— 其它家的凭证各有各的链路），再限定 id 前缀避免误报。
    isCustomProviderId(providerOf(account)) && account.hasCredentials === false
      ? {
        text: t('未配置凭证'),
        kind: 'bad' as const,
        title: t('这条自定义账号既没有 API Key，也没有勾选「无需鉴权」：转发时会被跳过，该提供商下的模型也不会出现在模型列表里。去账号「设置」里补上 Key，或勾选「该上游无需鉴权」'),
      }
      : null,
    // 代理配了解析不出来时明确标出：转发会回退直连，属于需要留意的情况
    account.proxy?.error
      ? {
        text: t('代理异常'),
        kind: 'bad' as const,
        title: t('{error}（转发时会回退直连）', { error: account.proxy.error }),
      }
      : null,
    // 走套餐池（Start Plan）通道时标出来：它不是默认值，而「这条请求到底花的
    // 是哪份额度」恰恰是用户在这个页面上要回答的问题（行上的余额列也可能同时
    // 挂着两份）
    zcodePlanOf(account) === ZCODE_PLAN_START
      ? {
        text: t('套餐池'),
        kind: 'plain' as const,
        title: t('转发走套餐池（Start Plan）通道（zcode.z.ai 的 Anthropic 端点，用账号里的体验额度 / 活动领取额度）；在账号设置里可切回编码套餐'),
      }
      : null,
  ].filter((tag): tag is AccountTag => tag !== null)
}

/** 更新时刻文案（时间戳非法时返回空串，调用处据此省略那半句） */
export const formatUpdatedAt = formatTime
