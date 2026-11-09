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

export type AccountSorting = {
  updatedAt: number
  configured: Record<string, AccountSortRule[]>
  rules: Record<string, AccountSortRule[]>
  providerOrder: string[]
  accountOrder: Record<string, string[]>
  creditExpiresAt: Record<string, number | null>
  balances?: Record<string, AccountSortBalance>
  balanceSortStates?: Record<string, BalanceSortState>
}

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
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
