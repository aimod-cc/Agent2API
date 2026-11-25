export type AccountSortRule =
  | { key: 'credit'; on: boolean; dir: 'soon' | 'late' }
  | { key: 'balance'; on: boolean; dir: 'high' | 'low' }
  | { key: 'added'; on: boolean; dir: 'new' | 'old' }

export type AccountSortBalance = {
  remaining: number | null
  unit: string
  unlimited: boolean
}

export type BalanceSortState = 'ready' | 'noData' | 'mixedUnits' | 'unsupported'
export type CreditSortState = 'ready' | 'noData' | 'unsupported'

export type AccountSorting = {
  updatedAt: number
  configured: Record<string, AccountSortRule[]>
  rules: Record<string, AccountSortRule[]>
  providerOrder: string[]
  accountOrder: Record<string, string[]>
  creditExpiresAt: Record<string, number | null>
  creditSortStates?: Record<string, CreditSortState>
  balances?: Record<string, AccountSortBalance>
  balanceSortStates?: Record<string, BalanceSortState>
}

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function numberOrNull(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value.trim())
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function usageObject(value: unknown): Record<string, unknown> | null {
  const entry = object(value)
  if (!entry || entry.stale) return null
  return entry.error ? object(entry.lastUsage) : entry
}

function creditExpiryFactsOf(value: unknown, now = Date.now()): { earliest: number | null; declared: boolean } {
  const data = usageObject(value)
  if (!data) return { earliest: null, declared: false }
  let best: number | null = null
  let declared = false
  const consider = (expiry: number | null, exhausted: boolean) => {
    if (expiry === null || expiry <= 0) return
    declared = true
    if (exhausted || expiry <= now) return
    if (best === null || expiry < best) best = expiry
  }

  const packs = Array.isArray(data.credits) ? data.credits : []
  for (const item of packs) {
    const pack = object(item)
    if (!pack || pack.snapshot === true || pack.record === true || pack.daily === true || pack.unlimited === true) continue
    const remain = numberOrNull(pack.remain)
    consider(numberOrNull(pack.expireAt), remain !== null && remain <= 0)
  }

  const sources = Array.isArray(data.quotaSources) ? data.quotaSources : []
  for (const item of sources) {
    const source = object(item)
    if (!source || source.unlimited === true || source.localOnly === true) continue
    const expiresAmount = numberOrNull(source.expiresAmount)
    const remaining = numberOrNull(source.remaining)
    const remainingPercent = numberOrNull(source.remainingPercent)
    const used = numberOrNull(source.used)
    const total = numberOrNull(source.total)
    const exhausted = (expiresAmount !== null && expiresAmount <= 0)
      || (remaining !== null && remaining <= 0)
      || (remaining === null && ((remainingPercent !== null && remainingPercent <= 0)
        || (used !== null && total !== null && total > 0 && used >= total)))
    consider(numberOrNull(source.expiresAt), exhausted)
  }

  const windows = Array.isArray(data.windows) ? data.windows : []
  for (const item of windows) {
    const row = object(item)
    if (!row || row.unlimited === true) continue
    const expiresAmount = numberOrNull(row.expiresAmount)
    const remaining = numberOrNull(row.remaining)
    const balance = numberOrNull(row.balance)
    const remainingPercent = numberOrNull(row.remainingPercent)
    const exhausted = (expiresAmount !== null && expiresAmount <= 0)
      || (remaining !== null && remaining <= 0)
      || (balance !== null && balance <= 0)
      || (remaining === null && balance === null && remainingPercent !== null && remainingPercent <= 0)
    consider(numberOrNull(row.expiresAt), exhausted)
  }

  return { earliest: best, declared }
}

export function creditExpiryReadingOf(value: unknown, now = Date.now()): number | null {
  return creditExpiryFactsOf(value, now).earliest
}

export function hasCreditExpiryReading(value: unknown): boolean {
  return creditExpiryFactsOf(value, 0).declared
}

export function accountSortBalanceOf(value: unknown): AccountSortBalance {
  const entry = object(value)
  if (entry?.stale) return { remaining: null, unit: '', unlimited: false }
  const usage = entry?.error ? object(entry.lastUsage) : entry
  const hasTotalLeft = Boolean(usage && Object.prototype.hasOwnProperty.call(usage, 'totalLeft'))
  const sources = Array.isArray(usage?.quotaSources) ? usage.quotaSources : []
  const account = sources.map(object).find(source => source?.id === 'account')
  const rawUnit = text(usage?.unit) || text(account?.unit) || (hasTotalLeft ? '积分' : '')
  const unit = /^(credits?|积分)$/i.test(rawUnit) ? '积分' : rawUnit
  const valueLeft = hasTotalLeft ? usage?.totalLeft : usage?.available
  const unlimited = Boolean(usage?.unlimited)
  const remaining = !unlimited && typeof valueLeft === 'number' && Number.isFinite(valueLeft) ? valueLeft : null
  return { remaining, unit, unlimited }
}

export function comparableBalance(balance: AccountSortBalance | null | undefined): number | null {
  if (!balance || balance.unlimited || !balance.unit || balance.unit === '%') return null
  return typeof balance.remaining === 'number' && Number.isFinite(balance.remaining) ? balance.remaining : null
}

export function balanceSortStateOf(balances: AccountSortBalance[]): BalanceSortState {
  const units = new Set(balances.filter(balance => comparableBalance(balance) !== null).map(balance => balance.unit))
  return units.size > 1 ? 'mixedUnits' : units.size ? 'ready' : 'noData'
}
