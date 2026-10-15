import { t } from '../i18n'
import { ACCOUNT_COLUMNS } from './accounts-columns'
import { nearestCreditOf } from './accounts-credit-detail'
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
 * ── 排序会写回队列（本文件自己仍只算值） ───────────────────
 * 全局账号表下点表头排序，页面会把排出来的先后**整队重编号**写回调度队列
 * （`accounts-page` 的 `reorderBySort` → `POST /api/accounts/reorder`），于是屏幕上
 * 那一列的先后就是转发会走的先后。本文件只负责「列 → 可比值」这一步，写回不在这里。
 * 另：「顺序」列不参与排序（见 `SORTABLE_COLUMNS` 上方的说明）。
 *
 * ── 缺失值恒排最后 ────────────────────────────────────────
 * 不随升降序翻转：把「没有这个读数」排到最前，会是一整批「未查询」的账号占满首屏 ——
 * 而它们恰恰是最不需要被先看到的那一批。所以方向在**两个都有值**时才参与比较。
 *
 * ── 并列有确定的次序 ──────────────────────────────────────
 * 主键相等时按 `(addedAt, id)` 定序（见 `tieBreak`），**不返回 0** —— 否则并列段
 * 的先后就由「传入数组的顺序」决定，而那个顺序每次渲染都可能变。详见比较器上方注释。
 */

/**
 * 可排序的列。
 *
 * 不给的三列：勾选列没有值（它那一格是个复选框）、操作列装的是按钮、「顺序」列见下。
 *
 * **「顺序」列不排序**：它显示的是**组内位次**（每一家从 `#1` 起，见 accounts-page 的
 * `rankMap`），而「组内第几位」不是一个能用来排**全表**的键 —— 组与组的先后由分组带的
 * 顺位表达、组内先后由拖拽把手改。早先它挂着按钮，但取值读的是另一份口径（全队列位次
 * `seatOf`），于是「点升序屏幕不动、点降序只部分反转」，且排序写回队列后这个矛盾会真的
 * 挪动队列 —— 故撤掉入口（`SORTABLE_COLUMNS` 这里同步移除，与表头保持一致）。历史见
 * docs/下游开发与同步方案.md §8.1.5。
 *
 * 「最近到期时间」在表——**谁先作废**是这一列存在的理由，能排才有用。
 * 「最近到期积分」也在表——**还剩多少**同样是个可比的量（剩得多 → 少），与时间列各管一个
 * 维度：一个管「什么时候作废」、一个管「作废前还剩多少」，两列点出来的次序互不打架。
 * 早先不给它按钮，理由是「跨天比大小无语义」（拿「3 天后到期的 200 分」比「明天作废的
 * 50 分」）；但用户实际要的就是按数值排，且缺值时那一格本来就是 `—`、恒排段末。
 * 只提示一点：这两列的读数取自**同一份**「最先作废的份」，所以「最近到期积分」排出来的
 * 次序只在该份之间可比 —— 它不是「账号总余额」的排序（那是「余额」列）。
 */
export const SORTABLE_COLUMNS: ReadonlySet<string> = new Set([
  'account', 'proxy', 'connections', 'status', 'limits', 'expiry', 'creditExpiry', 'creditLeft', 'usage',
])

/** 比较器要读的外部数据口（页面把当前作用域下最合适的那一份注进来） */
export type SortContext = {
  usageOf: (account: AccountRecord) => UsageEntry
  tokenUsageOf: (account: AccountRecord) => TokenReading[]
  connectionsOf: (id: string) => number
  /** 默认次序的位次：全局 = 全队列位次，模型作用域 = 该模型的队列位次。
   *  `null` = 不在这个作用域的队列里（模型作用域下的被跳过者）。
   *
   *  **注意**：全局域下「顺序」列**不再**用它（那一列显示组内位次、且已不可排序，
   *  见 `SORTABLE_COLUMNS`）；此口现在只服务模型作用域（那边位次即该模型队列里的
   *  真实排位，与 `状态` 列同源）。 */
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

/**
 * 「最近到期」那一份的到期时刻（毫秒；`nearestCreditOf` 的产物）。
 *
 * 与那一列**显示的文字**同一份数据 —— 显示的倒计时由同一个 `expireAt` 算出，
 * 于是「按这一列排序」出来的先后就是屏幕上那几个倒计时的先后（排完看不出来
 * 排过没有的那类问题，根子都在显示与排序各读一份数据）。
 *
 * 注意 `nearestCreditOf` 会在**没有一次性包时退到日额度包**（`expireAt` = 今天
 * 24 点）。那个值在这里照样可排：它就是「今天还剩多久」，与别的行的倒计时同一量纲。
 */
function creditExpiryOf(account: AccountRecord, ctx: SortContext): number | null {
  const nearest = nearestCreditOf(ctx.usageOf(account))
  return nearest ? nearest.expireAt : null
}

/**
 * 「最近到期积分」那一份的**剩余量**（`nearestCreditOf` 的 `remain`）。
 *
 * 与那一列**显示的数字**同一份数据（显示的就是 `remain`），所以「按这一列排序」出来的
 * 先后就是屏幕上那几个数字的先后。注意它是**那一份的剩余量**、不是账号总余额：
 * `remain` 缺失（无限量那类）时给 `null` → 恒排段末，与那一列的 `—` 一致。
 *
 * 日额度包兜底时也一样（那一份今天还剩多少）—— 与一次性包的 `remain` 是同一个字段、
 * 同一量纲，混排不会产生讲不通的次序。
 */
function creditLeftOf(account: AccountRecord, ctx: SortContext): number | null {
  const nearest = nearestCreditOf(ctx.usageOf(account))
  return nearest ? nearest.remain : null
}

/** 一列的原始可比值（`null` = 这一行没有这个读数） */
function valueOf(key: string, account: AccountRecord, ctx: SortContext): number | string | null {
  switch (key) {
    // 「顺序」列不在 SORTABLE_COLUMNS 里，页面不会把 'order' 送进来（保留这条是为了
    // 取值表完整、且模型作用域将来若要按位次排，这里就是落点）。见上方 SORTABLE_COLUMNS。
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
    case 'creditExpiry': return creditExpiryOf(account, ctx)
    case 'creditLeft': return creditLeftOf(account, ctx)
    case 'usage': return usageAmount(ctx.usageOf(account))
    default: return null
  }
}

/**
 * 生成一个比较器。
 *
 * `dir` 只作用在**两个值都存在**的那一半：缺失值（null）恒排最后，翻方向也不翻它。
 *
 * ── 并列时的稳定尾键（`(addedAt, id)`，**不随方向翻转**）────────
 * 主键相等时不再返回 0，而是按「加入时间 → 账号 id」定序。为什么必须有这一步：
 *
 *   · **可复现**：`Array.prototype.sort` 虽稳定，但稳定指的是「保**输入**序」——
 *     输入序从哪来？屏幕上这份数组每次渲染都可能是新对象。于是「主键相等的两行」
 *     在两次渲染里可能换个位置（实测：usage 未就绪时 16 行里 12 行键为 `null`，
 *     点一次排序、屏幕与写回各自的输入序不同 → 屏幕 ≠ 已落盘的队列序）。
 *   · **与后端默认序同源**：后端 `compare_by_priority`（`core/routing.rs`）的键是
 *     `(priority, addedAt)` 字典序，前端默认序 `byPriorityOrder`（`accounts-domain.ts`）
 *     与它同构。尾键取 `addedAt` 让「主键并列的那一段」与「取消排序后回到的默认序」
 *     次序一致 —— 取消排序不该把并列段悄悄换个队。
 *   · **`id` 兜底**：`addedAt` 可缺失（按 `0` 参与），两个都缺时再按 `id` 定序，
 *     保证**任何**输入都得到同一个结果（不是「大概率稳定」而是「确定」）。
 *
 * 尾键**不乘 `sign`**：它表达的是「同一批里的固有先后」，不是一个可翻的排序维度 ——
 * 与「缺失值恒排最后」同一个道理。
 */
export function accountComparator(sort: AccountSort, ctx: SortContext): (a: AccountRecord, b: AccountRecord) => number {
  const sign = sort.dir === 'desc' ? -1 : 1
  return (a, b) => {
    const left = valueOf(sort.key, a, ctx)
    const right = valueOf(sort.key, b, ctx)
    if (left === null && right === null) return tieBreak(a, b)
    if (left === null) return 1
    if (right === null) return -1
    const compared = typeof left === 'string' || typeof right === 'string'
      // 名字类按本地语言序列（带数字的自然序：「账号 2」排在「账号 10」前面）
      ? String(left).localeCompare(String(right), 'zh-Hans-CN', { numeric: true })
      : left - right
    return compared === 0 ? tieBreak(a, b) : compared * sign
  }
}

/**
 * 主键并列时的定序：加入时间升序，再按账号 id 升序（都是**固有次序**、不翻方向）。
 * 取值口径与 `byPriorityOrder` / 后端 `compare_by_priority` 一致：`addedAt` 缺失按 0。
 */
function tieBreak(a: AccountRecord, b: AccountRecord): number {
  const added = (Number(a?.addedAt) || 0) - (Number(b?.addedAt) || 0)
  if (added !== 0) return added
  return String(a?.id || '').localeCompare(String(b?.id || ''))
}

/** 列 key → 列名（批量栏那枚「已按 X 排序」的提示用它，认不出的列退回 key 本身） */
export function sortLabel(key: string): string {
  return ACCOUNT_COLUMNS.find(column => column.key === key)?.label || t(key)
}

/** 排序方向的短文案（表头的悬停提示与提示条共用） */
export function sortDirLabel(dir: AccountSort['dir']): string {
  return dir === 'desc' ? t('降序') : t('升序')
}
