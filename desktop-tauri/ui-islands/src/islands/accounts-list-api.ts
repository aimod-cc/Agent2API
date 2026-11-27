import { t } from '../i18n'
import { shared, type AccountRecord } from './accounts-shared'
import type { AccountListPage, AccountListQuery } from '../../../ui-shared/accounts-list'

const details = new Map<string, { account: AccountRecord; signature: string }>()
const pending = new Map<string, Promise<AccountRecord>>()
let detailsRevision = 0
export const accountDetailsRevision = (): number => detailsRevision

export function invalidateAccountDetails(ids: readonly string[]): void {
  let changed = false
  for (const id of ids) {
    const saved = details.get(id)
    if (!saved || !saved.signature) continue
    details.set(id, { ...saved, signature: '' })
    changed = true
  }
  if (changed) detailsRevision++
}

const signatureOf = (account: AccountRecord): string => JSON.stringify([account.updatedAt, account.priority, account.enabled, account.name, account.nickname, account.displayName, account.email, account.available, account.rateLimits, account.routingCooldowns, account.needsRelogin, account.upstreamCooldownUntil, account.upstreamBlockedReason])

export function accountDetailsCurrent(index: AccountRecord): boolean {
  return index.detailsLoaded !== false || details.get(index.id)?.signature === signatureOf(index)
}

export function cachedAccount(index: AccountRecord): AccountRecord {
  if (index.detailsLoaded !== false) return index
  const saved = details.get(index.id)
  if (!saved) return index
  const cached = saved.account
  if (saved.signature === signatureOf(index)) {
    if (typeof index.listLimited === 'boolean' && cached.listLimited !== index.listLimited) {
      const account = { ...cached, listLimited: index.listLimited }
      details.set(index.id, { ...saved, account })
      return account
    }
    return cached
  }
  const updated: AccountRecord = { ...cached, ...index, balanceQuery: cached.balanceQuery, detailsLoaded: true }
  for (const key of ['rateLimits', 'routingCooldowns', 'upstreamCooldownUntil', 'upstreamBlockedReason']) {
    if (!Object.prototype.hasOwnProperty.call(index, key)) delete updated[key]
  }
  return updated
}

export function cacheAccountDetails(accounts: AccountRecord[]): AccountRecord[] {
  return accounts.map(account => {
    const signature = signatureOf(account)
    const saved = details.get(account.id)
    const fresh = saved && JSON.stringify(saved.account) === JSON.stringify(account) ? saved.account : { ...account, detailsLoaded: true }
    if (saved && typeof fresh.listLimited !== 'boolean' && typeof saved.account.listLimited === 'boolean') {
      fresh.listLimited = saved.account.listLimited
    }
    details.set(account.id, { account: fresh, signature })
    return fresh
  })
}

export function pruneAccountDetails(valid: Set<string>): void {
  for (const id of details.keys()) if (!valid.has(id)) details.delete(id)
}

export async function ensureAccountDetails(id: string): Promise<AccountRecord> {
  const index = shared().wbApp?.getState?.()?.accounts?.accounts?.find(account => account.id === id)
  const saved = details.get(id)
  if (saved && index && saved.signature === signatureOf(index)) return saved.account
  if (index && index.detailsLoaded !== false) return index
  let task = pending.get(id)
  if (!task) {
    task = Promise.resolve().then(async () => {
      const api = shared().workbuddyDesktop
      if (!api?.getAccount) throw new Error(t('账号详情读取不可用，请更新网关后重试'))
      const account = await api.getAccount(id)
      if (!account || account.id !== id) throw new Error(t('账号详情响应异常，请刷新后重试'))
      return cacheAccountDetails([account])[0]
    }).finally(() => pending.delete(id))
    pending.set(id, task)
  }
  return task
}

export async function queryAccountPage(query: AccountListQuery): Promise<AccountListPage<AccountRecord>> {
  const api = shared().workbuddyDesktop
  if (!api?.queryAccounts) throw new Error(t('账号分页接口不可用，请更新网关后重试'))
  const page = await api.queryAccounts({ ...query, utcOffsetMinutes: -new Date().getTimezoneOffset() })
  if (!Array.isArray(page?.accounts) || !Array.isArray(page.groups) || !Number.isFinite(page.total)) {
    throw new Error(t('账号分页响应异常，请刷新后重试'))
  }
  return { ...page, accounts: cacheAccountDetails(page.accounts) }
}

export async function ensureAccountsDetails(ids: string[]): Promise<void> {
  const indexes = new Map((shared().wbApp?.getState?.()?.accounts?.accounts || []).map(account => [account.id, account]))
  const missing = ids.filter(id => {
    const index = indexes.get(id)
    return index && !accountDetailsCurrent(index)
  })
  for (let offset = 0; offset < missing.length; offset += 200) {
    await queryAccountPage({ ids: missing.slice(offset, offset + 200), offset: 0, pageSize: 200 })
  }
}
