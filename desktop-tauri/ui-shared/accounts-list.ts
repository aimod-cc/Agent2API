import type { AccountSorting } from './account-sorting'
import type { CreditExpirySummary } from './credit-expiry'
import type { HiddenAccountsSummary } from './plugins'

export type AccountListQuery = {
  q?: string
  providers?: string[]
  enabled?: string
  limit?: string
  status?: string
  expiry?: string
  usage?: string
  ids?: string[]
  collapsed?: string[]
  offset?: number
  pageSize?: number
  mobile?: boolean
  groupTotalFiltered?: boolean
  metadataOnly?: boolean
  utcOffsetMinutes?: number
}

export type AccountListSummary = {
  total: number
  enabled: number
  disabled: number
  limited: number
  available: number
}

export type AccountGroupBalance = {
  counted: number
  total: number
  sum: number | null
  unlimited: number
  unit: string
  failed: number
  uncountable: number
  pending: number
  partial: number
  scope: 'group' | 'filtered'
  additive: boolean
  ratio: {
    percent: number
    hasShortWindow: boolean
    adjusted: boolean
    windows: Array<{ label: string; percent: number | null; unlimited: boolean }>
  } | null
  expiring: CreditExpirySummary
}

export type AccountListGroup = {
  provider: string
  rank: number
  total: number
  enabled: number
  disabled: number
  limited: number
  matched: number
  balance?: AccountGroupBalance
}

export type AccountListPage<T> = {
  accounts: T[]
  matchedIds?: string[]
  displayIds?: string[]
  total: number
  displayTotal: number
  offset: number
  pageSize: number
  summary: AccountListSummary
  groups: AccountListGroup[]
  counts: Record<string, number> | {
    status: Record<string, number>
    expiry: Record<string, number>
    usage: Record<string, number>
  }
  providers?: Array<{ id?: string; label?: string; count?: number }>
  hiddenAccounts?: HiddenAccountsSummary
  sorting?: AccountSorting
  currentAccountId?: string | null
  currentAccountIds?: Record<string, string | null>
}

export type AccountPageSize = 20 | 50 | 100 | 200 | 'all'
export const ACCOUNT_PAGE_SIZES: AccountPageSize[] = [20, 50, 100, 200, 'all']

export function readAccountPageSize(key: string, fallback: AccountPageSize): AccountPageSize {
  try {
    const raw = localStorage.getItem(key)
    const value = raw === 'all' ? 'all' : Number(raw)
    return ACCOUNT_PAGE_SIZES.includes(value as AccountPageSize) ? value as AccountPageSize : fallback
  } catch { return fallback }
}
