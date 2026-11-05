import { t } from '../i18n'
import { shared, toast, errorMessage, type AccountSorting, type GroupSortRule } from './accounts-shared'
import { getStore, patch, snapshot } from './accounts-store'

const LEGACY_SORT_KEY = 'workbuddy-desktop-accounts-group-sort'
let migration: Promise<void> | null = null
let migrationAttempt = ''
let migrationRetryAt = 0
let lastSnapshot: AccountSorting | undefined
let lastUpdatedAt = 0
let lastContent = ''

export function adoptAccountSorting(sorting: AccountSorting | null | undefined): boolean {
  if (!sorting?.rules || !sorting.configured || !sorting.accountOrder || !Array.isArray(sorting.providerOrder)) return false
  if (sorting.updatedAt < lastUpdatedAt) return false
  lastUpdatedAt = sorting.updatedAt
  const { updatedAt: _updatedAt, ...content } = sorting
  const signature = JSON.stringify(content)
  if (signature === lastContent) return false
  lastContent = signature
  patch({ sorting, groupSort: sorting.configured })
  return true
}

async function migrateLocalSorting(): Promise<boolean> {
  const api = shared().workbuddyDesktop
  if (!api?.saveAccountSorting || !getStore().sorting) return false
  const raw = localStorage.getItem(LEGACY_SORT_KEY)
  if (raw === null) return true
  const configured = getStore().sorting?.configured || {}
  let legacy: Record<string, GroupSortRule[]>
  try { legacy = JSON.parse(raw) as Record<string, GroupSortRule[]> }
  catch { return false }
  if (!legacy || typeof legacy !== 'object' || Array.isArray(legacy)) return false
  const remaining: Record<string, GroupSortRule[]> = {}
  for (const [provider, rules] of Object.entries(legacy)) {
    if (!Array.isArray(rules) || Object.prototype.hasOwnProperty.call(configured, provider)) continue
    const valid = rules.filter(rule => rule && typeof rule.on === 'boolean'
      && (rule.key === 'credit' && (rule.dir === 'soon' || rule.dir === 'late')
        || rule.key === 'added' && (rule.dir === 'new' || rule.dir === 'old')))
    if (valid.length !== rules.length) continue
    if (!getStore().sorting?.rules[provider]) {
      remaining[provider] = rules
      continue
    }
    adoptAccountSorting(await api.saveAccountSorting({ provider, rules, onlyIfUnset: true }))
  }
  const complete = Object.keys(remaining).length === 0
  if (!complete) localStorage.setItem(LEGACY_SORT_KEY, JSON.stringify(remaining))
  else localStorage.removeItem(LEGACY_SORT_KEY)
  return complete
}

export function syncAccountSorting(): void {
  const sorting = snapshot()?.sorting
  if (sorting && sorting !== lastSnapshot && !getStore().sortBusy) {
    lastSnapshot = sorting
    adoptAccountSorting(sorting)
  }
  const migrationKey = JSON.stringify(getStore().sorting?.providerOrder || [])
  if (getStore().sortBusy) return
  if (!migration && getStore().sorting && migrationAttempt !== migrationKey && Date.now() >= migrationRetryAt) {
    migrationAttempt = migrationKey
    migration = migrateLocalSorting().then(complete => { if (!complete) migration = null }).catch(error => {
      migration = null
      migrationAttempt = ''
      migrationRetryAt = Date.now() + 60_000
      toast(t('排序设置迁移失败：{message}', { message: errorMessage(error) }), 'err')
    })
  }
}

export async function refreshAccountSorting(): Promise<void> {
  const api = shared().workbuddyDesktop
  if (!api?.getAccountSorting || getStore().sortBusy) return
  try { adoptAccountSorting(await api.getAccountSorting()) }
  catch (error) { toast(t('读取消耗排序失败：{message}', { message: errorMessage(error) }), 'err') }
}

export async function saveGroupSortRules(provider: string, rules: GroupSortRule[] | null): Promise<void> {
  if (!provider || getStore().sortBusy) return
  const api = shared().workbuddyDesktop
  if (!api?.saveAccountSorting) {
    toast(t('账号排序接口不可用，请确认网关与界面已更新'), 'err')
    return
  }
  patch({ sortBusy: true })
  try {
    const sorting = await api.saveAccountSorting({ provider, rules })
    if (!sorting?.rules || !sorting.accountOrder || !Array.isArray(sorting.providerOrder)) throw new Error(t('保存响应不完整，请刷新当前配置确认结果'))
    adoptAccountSorting(sorting)
    window.dispatchEvent(new CustomEvent('wb:account-sorting', { detail: sorting }))
    try { await shared().wbApp?.refresh?.() }
    catch (error) { toast(t('排序已保存，但列表刷新失败：{message}', { message: errorMessage(error) }), 'err') }
  } catch (error) {
    toast(t('排序未保存：{message}', { message: errorMessage(error) }), 'err')
  } finally { patch({ sortBusy: false }) }
}
