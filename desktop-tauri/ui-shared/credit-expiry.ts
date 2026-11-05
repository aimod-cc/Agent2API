import { creditPacksOf, isCreditUnit, usageNumberOrNull, usageUnitLabel } from './usage-readings'

type Translate = (key: string, params?: Record<string, string | number>) => string
type Format = (value: number) => string

// 这是展示窗口，不改变网关的账号消耗顺序。
export const CREDIT_EXPIRY_WINDOW_MS = 7 * 86_400_000

export type ExpiringCredit = {
  expireAt: number
  remain: number | null
  unit: string
}

export type CreditExpiryAmount = {
  unit: string
  sum: number | null
  count: number
  unknownCount: number
}

export type CreditExpirySummary = {
  count: number
  todayCount: number
  amounts: CreditExpiryAmount[]
  todayAmounts: CreditExpiryAmount[]
}

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}

function unitOf(value: unknown): string {
  const unit = typeof value === 'string' ? value.trim() : ''
  return isCreditUnit(unit) ? '积分' : unit
}

export function creditExpiryUnitOf(value: unknown): string {
  const data = object(value)
  if (!data) return ''
  // WorkBuddy 的 totalLeft 契约使用积分；其他缺单位的读数不作猜测。
  return unitOf(data.unit) || (Object.prototype.hasOwnProperty.call(data, 'totalLeft') ? '积分' : '')
}

export function expiringCreditOf(pack: Record<string, unknown>, defaultUnit: string, now = Date.now()): ExpiringCredit | null {
  if (['snapshot', 'record', 'daily', 'rolling', 'unlimited'].some(key => pack[key] === true)) return null
  const remain = usageNumberOrNull(pack.remain)
  const total = usageNumberOrNull(pack.total)
  const used = usageNumberOrNull(pack.used)
  const percent = usageNumberOrNull(pack.remainingPercent)
  if (total === -1 || (remain !== null && remain <= 0)) return null
  if (remain === null && ((used !== null && total !== null && total > 0 && used >= total)
    || (percent !== null && percent <= 0))) return null
  const startAt = usageNumberOrNull(pack.startAt)
  if (startAt !== null && startAt > now) return null
  const expireAt = usageNumberOrNull(pack.expireAt)
  if (expireAt === null || expireAt <= now || expireAt > now + CREDIT_EXPIRY_WINDOW_MS
    || !Number.isFinite(new Date(expireAt).getTime())) return null
  return { expireAt, remain, unit: unitOf(pack.unit) || defaultUnit }
}

export function expiringCreditsOf(value: unknown, now = Date.now()): ExpiringCredit[] {
  const unit = creditExpiryUnitOf(value)
  return creditPacksOf(value)
    .map(pack => expiringCreditOf(pack, unit, now))
    .filter((pack): pack is ExpiringCredit => pack !== null)
}

function amountsOf(packs: ExpiringCredit[]): CreditExpiryAmount[] {
  const amounts = new Map<string, CreditExpiryAmount>()
  for (const pack of packs) {
    let amount = amounts.get(pack.unit)
    if (!amount) {
      amount = { unit: pack.unit, sum: null, count: 0, unknownCount: 0 }
      amounts.set(pack.unit, amount)
    }
    amount.count++
    if (pack.remain === null) amount.unknownCount++
    // 不同单位分桶；单位缺失和比例读数都不能相加。
    if (pack.remain !== null && pack.unit && pack.unit !== '%') {
      amount.sum = (amount.sum ?? 0) + pack.remain
    }
  }
  return [...amounts.values()]
}

export function creditExpirySummaryOf(packs: ExpiringCredit[], now = Date.now()): CreditExpirySummary {
  const live = packs.filter(pack => pack.expireAt > now && pack.expireAt <= now + CREDIT_EXPIRY_WINDOW_MS)
  const tomorrow = new Date(now)
  tomorrow.setHours(24, 0, 0, 0)
  const today = live.filter(pack => pack.expireAt < tomorrow.getTime())
  return { count: live.length, todayCount: today.length, amounts: amountsOf(live), todayAmounts: amountsOf(today) }
}

function amountNumber(value: number, t: Translate, format: Format): string {
  return value > 0 && value < 0.01 ? t('不足 {n}', { n: format(0.01) }) : format(value)
}

function amountsText(amounts: CreditExpiryAmount[], t: Translate, format: Format): string {
  return amounts.map(amount => {
    if (!amount.unit) return t('{count} 份单位未知', { count: amount.count })
    if (amount.unit === '%') return t('{count} 份仅提供比例', { count: amount.count })
    const unit = usageUnitLabel(amount.unit, t)
    if (amount.sum === null) return t('{unit}余量未知（{count} 份）', { unit, count: amount.count })
    const value = `${amountNumber(amount.sum, t, format)} ${unit}`
    return amount.unknownCount > 0
      ? t('{amount}，另 {count} 份余量未知', { amount: value, count: amount.unknownCount })
      : value
  }).join('；')
}

export function creditExpiryText(summary: CreditExpirySummary, t: Translate, format: Format, displayUnit = ''): {
  badge: string
  group: string
  summary: string
  today: string
  detail: string
} {
  if (!summary.count) return { badge: '', group: '', summary: '', today: '', detail: '' }
  const single = summary.amounts.length === 1 ? summary.amounts[0] : null
  const compact = isCreditUnit(displayUnit) && single?.unit === '积分' && single.unknownCount === 0 && single.sum !== null
    ? amountNumber(single.sum, t, format) : null
  const main = t('7 天内到期 {amount}（{count} 份）', {
    amount: amountsText(summary.amounts, t, format), count: summary.count,
  })
  const today = summary.todayCount > 0 ? t('今天到期 {amount}（{count} 份）', {
    amount: amountsText(summary.todayAmounts, t, format), count: summary.todayCount,
  }) : ''
  return {
    badge: compact === null ? t('7天 {count}份', { count: summary.count }) : t('7天 {n}', { n: compact }),
    group: compact === null ? t('即将到期 {count} 份', { count: summary.count }) : t('即将过期 {n}', { n: compact }),
    summary: main,
    today,
    detail: [main, today].filter(Boolean).join('；'),
  }
}

export function creditExpiryLabel(expireAt: number, t: Translate, now = Date.now()): string {
  const target = new Date(expireAt)
  const left = expireAt - now
  if (!Number.isFinite(target.getTime()) || left <= 0) return ''
  const today = new Date(now)
  if (target.getFullYear() === today.getFullYear() && target.getMonth() === today.getMonth()
    && target.getDate() === today.getDate()) {
    if (left < 60_000) return t('不到 1 分钟到期')
    const hours = Math.floor(left / 3_600_000)
    return hours < 1 ? t('{minutes} 分钟后到期', { minutes: Math.floor(left / 60_000) })
      : t('{hours} 小时后到期', { hours })
  }
  const dayOf = (date: Date) => Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())
  const days = Math.round((dayOf(target) - dayOf(today)) / 86_400_000)
  return days === 1 ? t('明天到期') : t('{days} 天后到期', { days })
}
