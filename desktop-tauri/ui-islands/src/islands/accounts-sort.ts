import { t } from '../i18n'
import { ACCOUNT_COLUMNS } from './accounts-columns'
import {
  activeLimits, balanceBlockedOf, displayNameOf, expiryMillis, isEnabled, providerFeatures, providerOf,
  tokenBlockedOf,
} from './accounts-domain'
import type { AccountRecord, TokenReading, UsageEntry } from './accounts-shared'
import { stateRank } from './accounts-schedule'
import type { AccountSort } from './accounts-store'

/**
 * 账号表 · **表头排序**（「列 → 可比值」的唯一一份实现）。
 *
 * ── 为什么单独一个文件 ─────────────────────────────────────
 * 比较器要读四份不同的数据（账号记录本身、余额读数、Token 读数、连接数，模型作用域下
 * 还要多读一份调度状态），而「怎么读」在 accounts-page / accounts-panels / accounts-domain
 * 三处各有各的入口。把它收在这里，页面只负责「哪一列、升还是降、排哪些行」。
 *
 * ── 只排序，不改任何数据 ───────────────────────────────────
 * 排序是**视图行为**：行序变了，但「顺序」列里那个 `#N` 仍是它在队列里的真实位次
 * （全局 = 优先级队列，模型作用域 = 调度队列）。两者分得清，才不会出现「排了一下序，
 * 界面显示的先后就与转发实际走的先后对不上、而用户看不出来」。
 *
 * ── 缺失值恒排最后 ────────────────────────────────────────
 * 不随升降序翻转：把「没有这个读数」排到最前，会是一整批「未查询」的账号占满首屏 ——
 * 而它们恰恰是最不需要被先看到的那一批。所以方向在**两个都有值**时才参与比较。
 */

/**
 * 可排序的列。
 *
 * 不给的两列：勾选列没有值（它那一格是个复选框）、操作列装的是按钮。除此之外每一列
 * 都有一个可比的读数 —— 包括「代理」（没有代理 = 直连，排最前）。
 */
export const SORTABLE_COLUMNS: ReadonlySet<string> = new Set([
  'order', 'account', 'proxy', 'connections', 'status', 'limits', 'expiry', 'usage',
])

/** 比较器要读的外部数据口（页面把当前作用域下最合适的那一份注进来） */
export type SortContext = {
  usageOf: (account: AccountRecord) => UsageEntry
  tokenUsageOf: (account: AccountRecord) => TokenReading[]
  connectionsOf: (id: string) => number
  /** 默认次序（顺序列的读数）：全局 = 全队列位次，模型作用域 = 该模型的队列位次。
   *  `null` = 不在这个作用域的队列里（模型作用域下的被跳过者） */
  seatOf: (account: AccountRecord) => number | null
  /** 模型作用域：该账号在这个模型上的调度状态（`状态`列显示的就是它，排序必须同源） */
  stateOf?: (id: string) => string | undefined
}

/** 数值字段 → number 或 null（判据与 accounts-panels 的 numberOrNull 逐字相同：
 *  `Number(null)` 是 0 而不是 NaN，照直转换会把「上游没给」变成「就是 0」） */
function numberOrNull(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

/**
 * 余额读数的**数值形式**（排序用；显示串由 accounts-panels 的 `usageSummary` 出）。
 *
 * 取值顺序照抄 `usageSummary` 读同一个账号时的优先级，逐条对应，改那边时要回来看一眼：
 *   ① `totalLeft`（workbuddy 两家的总剩余）
 *   ② 额度桶里 `total` 最大的那个桶的 `balance`（与 `usagePool` ① 同一条选桶规则）
 *   ③ `available`（CodeArts / Trae 那几家的可用量）
 *   ④ 只剩比例时取**剩余最少**那一档的百分比（监控通道）
 * ④ 与前三者的量纲不同（比例 vs 积分）：它只在真数一个都取不到时兜底 —— 同账号之间
 * 仍可排，跨家比出来的次序没有意义，但那也比「这一列全排不动」强。
 */
function usageAmount(entry: UsageEntry): number | null {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null
  const data = entry as Record<string, unknown>
  if (Object.prototype.hasOwnProperty.call(data, 'totalLeft')) {
    const total = numberOrNull(data.totalLeft)
    if (total !== null) return total
  }
  const wallets = Array.isArray(data.wallets) ? data.wallets as Array<Record<string, unknown>> : []
  let lead: Record<string, unknown> | null = null
  for (const wallet of wallets) {
    const total = Number(wallet?.total)
    if (!Number.isFinite(total) || total <= 0) continue
    if (!lead || total > Number(lead.total)) lead = wallet
  }
  if (lead) {
    const balance = numberOrNull(lead.balance)
    if (balance !== null) return balance
  }
  const available = numberOrNull(data.available)
  if (available !== null) return available
  let tightest: number | null = null
  for (const wallet of wallets) {
    const percent = numberOrNull(wallet?.remainingPercent)
    if (percent === null) continue
    if (tightest === null || percent < tightest) tightest = percent
  }
  return tightest
}

/** 代理列的可比值：池条目 / Clash 出口用它的标签，直连是空串（排最前） */
function proxyTextOf(account: AccountRecord): string {
  const proxy = account.proxy
  if (!proxy) return ''
  const source = proxy.config?.source || proxy.source || ''
  const label = String(proxy.label || '')
  return `${source} ${label}`.trim()
}

/** 有效期：与 `ExpiryCell` 同一个归一入口（`expiryMillis` 先归一到毫秒），取不到给 null */
function expiryOf(account: AccountRecord): number | null {
  const features = providerFeatures(providerOf(account))
  const raw = features.edition ? account.expiresAt : account[features.expiry]
  const value = expiryMillis(raw)
  return value > 0 ? value : null
}

/** 限流列的读数：限流中的模型数；被余额 / Token 限制器跳过时另算一档
 *  （那一档在界面上也显示成「余额不足 / Token 限额」，见 LimitsCell） */
function limitedCountOf(account: AccountRecord, ctx: SortContext): number {
  const models = activeLimits(account).length
  const blocked = balanceBlockedOf(account, ctx.usageOf(account))
    || tokenBlockedOf(account, ctx.tokenUsageOf(account))
  return models + (blocked ? 1 : 0)
}

/** 一列的原始可比值（`null` = 这一行没有这个读数） */
function valueOf(key: string, account: AccountRecord, ctx: SortContext): number | string | null {
  switch (key) {
    case 'order': return ctx.seatOf(account)
    case 'account': return displayNameOf(account).toLowerCase()
    case 'proxy': return proxyTextOf(account)
    case 'connections': return ctx.connectionsOf(account.id)
    case 'status':
      // 模型作用域下「状态」列画的是调度状态，排序就必须按同一件事排 —— 否则
      // 「按状态排」出来的次序与那一列显示的次序互相打脸
      return ctx.stateOf ? stateRank(ctx.stateOf(account.id)) : (isEnabled(account) ? 1 : 0)
    case 'limits': return limitedCountOf(account, ctx)
    case 'expiry': return expiryOf(account)
    case 'usage': return usageAmount(ctx.usageOf(account))
    default: return null
  }
}

/**
 * 生成一个比较器。
 *
 * `dir` 只作用在**两个值都存在**的那一半：缺失值（null）恒排最后，翻方向也不翻它。
 */
export function accountComparator(sort: AccountSort, ctx: SortContext): (a: AccountRecord, b: AccountRecord) => number {
  const sign = sort.dir === 'desc' ? -1 : 1
  return (a, b) => {
    const left = valueOf(sort.key, a, ctx)
    const right = valueOf(sort.key, b, ctx)
    if (left === null && right === null) return 0
    if (left === null) return 1
    if (right === null) return -1
    const compared = typeof left === 'string' || typeof right === 'string'
      // 名字类按本地语言序列（带数字的自然序：「账号 2」排在「账号 10」前面）
      ? String(left).localeCompare(String(right), 'zh-Hans-CN', { numeric: true })
      : left - right
    return compared * sign
  }
}

/** 列 key → 列名（批量栏那枚「已按 X 排序」的提示用它，认不出的列退回 key 本身） */
export function sortLabel(key: string): string {
  return ACCOUNT_COLUMNS.find(column => column.key === key)?.label || t(key)
}

/** 排序方向的短文案（表头的悬停提示与提示条共用） */
export function sortDirLabel(dir: AccountSort['dir']): string {
  return dir === 'desc' ? t('降序') : t('升序')
}
