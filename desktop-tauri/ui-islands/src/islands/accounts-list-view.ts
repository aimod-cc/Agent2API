import type { AccountListPage, AccountListQuery } from '../../../ui-shared/accounts-list'
import type { AccountRecord, AccountsSnapshot, AccountSorting } from './accounts-shared'
import { cachedAccount } from './accounts-list-api'
import { isEnabled, providerOf, type AccountGroup } from './accounts-domain'

export type AccountListView = {
  snapshot: AccountsSnapshot | null | undefined
  groups: AccountGroup[]
  counts: Record<string, number>
  sorting?: AccountSorting | null
}

function pageAccounts(ids: string[], indexes: Map<string, AccountRecord>, saved: Map<string, AccountRecord>): AccountRecord[] {
  return ids.flatMap(id => {
    const index = indexes.get(id)
    const account = index ? cachedAccount(index) : saved.get(id)
    const old = saved.get(id)
    return account ? [old && JSON.stringify(old) === JSON.stringify(account) ? old : account] : []
  })
}

function pageRange(displayIds: string[], query: AccountListQuery, continuous: boolean, saved: Map<string, AccountRecord>) {
  const pageSize = Math.min(200, Math.max(1, query.pageSize || 50))
  const offset = continuous || !displayIds.length ? 0
    : Math.min(query.offset || 0, Math.floor((displayIds.length - 1) / pageSize) * pageSize)
  const wanted = new Set(displayIds.slice(offset, offset + pageSize))
  if (continuous) for (const id of saved.keys()) wanted.add(id)
  return { pageSize, offset, ids: displayIds.filter(id => wanted.has(id)) }
}

function nextOffset(displayIds: string[], accounts: AccountRecord[]): number {
  const loaded = new Set(accounts.map(account => account.id))
  const missing = displayIds.findIndex(id => !loaded.has(id))
  return missing === -1 ? displayIds.length : missing
}

export function reflowAccountPage(page: AccountListPage<AccountRecord>, query: AccountListQuery, continuous: boolean, snapshot: AccountsSnapshot | null | undefined): AccountListPage<AccountRecord> {
  const indexes = new Map((snapshot?.accounts || []).map(account => [account.id, account]))
  const saved = new Map(page.accounts.map(account => [account.id, account]))
  const collapsed = new Set(query.collapsed || [])
  const matched = page.matchedIds || page.displayIds || [...saved.keys()]
  const positions = new Map((page.sorting?.providerOrder || snapshot?.sorting?.providerOrder || []).flatMap(provider =>
    (page.sorting || snapshot?.sorting)?.accountOrder[provider] || []).map((id, index) => [id, index]))
  const displayIds = matched.filter(id => {
    const account = indexes.get(id) || saved.get(id)
    return account && !collapsed.has(providerOf(account))
  }).sort((left, right) => (positions.get(left) ?? Infinity) - (positions.get(right) ?? Infinity))
  const range = pageRange(displayIds, query, continuous, saved)
  const accounts = pageAccounts(range.ids, indexes, saved)
  return { ...page, accounts, displayIds, displayTotal: displayIds.length, pageSize: range.pageSize,
    sorting: page.sorting || snapshot?.sorting,
    offset: continuous ? nextOffset(displayIds, accounts) : range.offset }
}

export function previewAccountPage(query: AccountListQuery, continuous: boolean, view: AccountListView, previous: AccountListPage<AccountRecord> | null, reuseBalance: boolean): AccountListPage<AccountRecord> | null {
  const { snapshot, groups, counts, sorting } = view
  if (!snapshot) return null
  const indexes = new Map((snapshot.accounts || []).map(account => [account.id, account]))
  const saved = new Map((previous?.accounts || []).map(account => [account.id, account]))
  const matchedIds = groups.flatMap(group => group.accounts.map(account => account.id))
  const collapsed = new Set(query.collapsed || [])
  const displayIds = groups.filter(group => !collapsed.has(group.provider))
    .flatMap(group => group.accounts.map(account => account.id))
  const range = pageRange(displayIds, query, continuous, saved)
  const accounts = pageAccounts(range.ids, indexes, saved)
  const previousGroups = new Map((previous?.groups || []).map(group => [group.provider, group]))
  const previousIds = new Set(previous?.matchedIds || [])
  const summary = snapshot.summary || {
    total: indexes.size,
    enabled: groups.reduce((total, group) => total + group.enabled, 0),
    disabled: groups.reduce((total, group) => total + group.disabled, 0),
    limited: groups.reduce((total, group) => total + group.limited, 0),
    available: [...indexes.values()].filter(account => isEnabled(account) && account.available !== false).length,
  }
  return {
    accounts, matchedIds, displayIds, total: matchedIds.length, displayTotal: displayIds.length,
    offset: continuous ? nextOffset(displayIds, accounts) : range.offset, pageSize: range.pageSize,
    summary, counts, sorting: sorting || snapshot.sorting, providers: snapshot.providers,
    groups: groups.map(group => {
      const previousGroup = previousGroups.get(group.provider)
      const sameMembers = group.accounts.length === previousGroup?.matched
        && group.accounts.every(account => previousIds.has(account.id))
      const balance = previousGroup?.balance
      return { provider: group.provider, rank: group.rank, total: group.members.length,
        enabled: group.enabled, disabled: group.disabled, limited: group.limited, matched: group.accounts.length,
        balance: reuseBalance && balance && (query.groupTotalFiltered
          ? balance.scope === 'filtered' && sameMembers : balance.scope === 'group') ? balance : undefined }
    }),
  }
}

export function mergeAccountPages(first: AccountListPage<AccountRecord>, previous: AccountListPage<AccountRecord> | null, continuous: boolean, snapshot: AccountsSnapshot | null | undefined): AccountListPage<AccountRecord> {
  if (!continuous) return first
  if (!previous) {
    const displayIds = first.displayIds || first.accounts.map(account => account.id)
    return { ...first, offset: nextOffset(displayIds, first.accounts) }
  }
  const fresh = new Map(first.accounts.map(account => [account.id, account]))
  const indexes = new Map((snapshot?.accounts || []).map(account => [account.id, account]))
  const saved = new Map(previous.accounts.map(account => [account.id, account]))
  const displayIds = first.displayIds || [...fresh.keys(), ...saved.keys()]
  const accounts = displayIds.flatMap(id => {
    const account = fresh.get(id) || (saved.has(id) ? indexes.has(id) ? cachedAccount(indexes.get(id)!) : saved.get(id) : undefined)
    return account ? [account] : []
  })
  return { ...first, accounts, offset: nextOffset(displayIds, accounts) }
}
