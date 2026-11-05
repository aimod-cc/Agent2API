import { t } from '../i18n'
import { inactiveProvider } from './routing-display'
import { bridge, errorMessage, host, notify, type RouteOrderChange, type RouteProfile, type RoutingWorkspace } from './routing-types'
import { adoptAccountSorting, syncAccountSorting } from './accounts-sort'
import { activeGroupSortRules, manualOrderBlockedMessage } from './accounts-group-settings'
import type { GroupSortRule } from './accounts-shared'

export type RoutingState = {
  model: string
  workspace: RoutingWorkspace | null
  loading: boolean
  busy: boolean
  error: string
  search: string
  zoom: number
  selectedProvider: string
  expandedProviders: ReadonlySet<string>
}
let current: RoutingState = {
  model: '', workspace: null, loading: false, busy: false, error: '',
  search: '', zoom: 1, selectedProvider: '', expandedProviders: new Set<string>(),
}
const listeners = new Set<() => void>()
let loadSequence = 0
export const snapshot = () => current
export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
export function patch(next: Partial<RoutingState>): void {
  current = { ...current, ...next }
  for (const listener of listeners) listener()
}
export function setProviderExpanded(provider: string, expanded: boolean): void {
  const next = new Set(current.expandedProviders)
  if (expanded) next.add(provider)
  else next.delete(provider)
  patch({ expandedProviders: next })
}
export function setAllProvidersExpanded(expanded: boolean): void {
  if (!current.workspace) return
  const next = new Set(current.expandedProviders)
  for (const provider of current.workspace.profile.providers) {
    if (expanded && !inactiveProvider(current.workspace, provider)) next.add(provider)
    else next.delete(provider)
  }
  patch({ expandedProviders: next })
}
export async function loadWorkspace(silent = false): Promise<void> {
  if (current.busy) return
  const sequence = ++loadSequence
  const model = current.model
  if (!silent) patch({ loading: true, error: '' })
  try {
    const workspace = await bridge().getRoutingWorkspace(model || undefined)
    if (sequence !== loadSequence || model !== current.model) return
    if (!workspace?.profile || !Array.isArray(workspace.accounts)) throw new Error(t('路由接口返回了不完整的数据'))
    const expandedProviders = new Set(current.expandedProviders)
    for (const provider of workspace.profile.providers) {
      if (inactiveProvider(workspace, provider) && (!current.workspace || !inactiveProvider(current.workspace, provider))) expandedProviders.delete(provider)
    }
    adoptAccountSorting(workspace.sorting)
    syncAccountSorting()
    patch({ workspace, expandedProviders, error: '', loading: false })
  } catch (error) {
    if (sequence !== loadSequence) return
    patch({ error: errorMessage(error), loading: false })
  }
}
export async function selectModel(model: string): Promise<void> {
  if (current.busy) return
  patch({ model, workspace: null, selectedProvider: '', search: '' })
  await loadWorkspace()
}
export function cloneProfile(): RouteProfile {
  if (!current.workspace) throw new Error(t('路由配置尚未读取'))
  return structuredClone(current.workspace.profile)
}
export function independentProfile(profile = cloneProfile()): RouteProfile {
  const next = structuredClone(profile)
  if (!next.providerScope) next.providerScope = 'all'
  if (next.providerScope === 'selected') next.disabledProviders = next.disabledProviders.filter(id => next.providers.includes(id))
  return next
}

export async function saveProfile(profile: RouteProfile | null, model = current.model, orderChange?: RouteOrderChange): Promise<boolean> {
  if (current.busy) return false
  const createsIndependent = Boolean(model && current.model === model && current.workspace && !current.workspace.customized)
  const next = profile && createsIndependent ? independentProfile(profile) : profile
  loadSequence += 1
  patch({ busy: true, loading: false, error: '' })
  try {
    const workspace = await bridge().saveRoutingProfile({ model: model || null, profile: next, orderChange })
    adoptAccountSorting(workspace.sorting)
    if (!model) void host().wbApp?.refresh?.()
    if (current.model === model) {
      const expandedProviders = new Set(current.expandedProviders)
      for (const provider of workspace.profile.providers) {
        if (inactiveProvider(workspace, provider) && (!current.workspace || !inactiveProvider(current.workspace, provider))) expandedProviders.delete(provider)
      }
      patch({ workspace, expandedProviders })
    }
    notify(t('路由配置已保存'))
    return true
  } catch (error) {
    patch({ error: errorMessage(error) })
    notify(errorMessage(error), 'err')
    try {
      const workspace = await bridge().getRoutingWorkspace(current.model || undefined)
      adoptAccountSorting(workspace.sorting)
      patch({ workspace })
    } catch { /* 保留错误与最近一次成功快照 */ }
    return false
  } finally { patch({ busy: false }) }
}
export function providerLabel(id: string): string {
  return current.workspace?.providers.find(provider => provider.id === id)?.label || id
}
export function accountOrder(provider: string, profile = current.workspace?.profile): string[] {
  if (!profile) return []
  return (profile.accountOrder[provider] || []).filter(id => !profile.excludedAccounts.includes(id)
    && current.workspace?.accounts.some(account => account.id === id))
}
export function accountOrderRules(provider: string): GroupSortRule[] {
  if (current.model && current.workspace?.customized) return []
  return activeGroupSortRules(provider, current.workspace?.sorting?.rules[provider] || [])
}
export function accountOrderLocked(provider: string): boolean {
  return accountOrderRules(provider).length > 0
}
export async function reorderAccount(provider: string, id: string, before: string): Promise<void> {
  const orderRules = accountOrderRules(provider)
  if (orderRules.length > 0) {
    notify(manualOrderBlockedMessage(orderRules), 'err')
    return
  }
  if (id === before || !current.workspace || current.busy) return
  const profile = cloneProfile()
  const ids = profile.accountOrder[provider] || []
  const from = ids.indexOf(id)
  const to = ids.indexOf(before)
  if (from < 0 || to < 0) return
  ids.splice(from, 1)
  ids.splice(to, 0, id)
  await saveProfile(profile, current.model, { kind: 'account', provider })
}
export async function moveAccount(provider: string, id: string, direction: -1 | 1): Promise<void> {
  const all = accountOrder(provider)
  const account = current.workspace?.accounts.find(account => account.id === id)
  const ids = all.filter(candidate => (current.workspace?.accounts.find(account => account.id === candidate)?.enabled === false) === (account?.enabled === false))
  const index = ids.indexOf(id)
  const target = ids[index + direction]
  if (!target) return
  await reorderAccount(provider, id, target)
}
export async function moveProvider(id: string, direction: -1 | 1): Promise<void> {
  if (!current.workspace || current.busy) return
  const profile = cloneProfile()
  const inactive = inactiveProvider(current.workspace, id)
  const peers = profile.providers.filter(provider => inactiveProvider(current.workspace!, provider) === inactive)
  const index = profile.providers.indexOf(id)
  const peerIndex = peers.indexOf(id)
  const target = peers[peerIndex + direction]
  const to = target ? profile.providers.indexOf(target) : -1
  if (index < 0 || to < 0) return
  const [provider] = profile.providers.splice(index, 1)
  profile.providers.splice(to, 0, provider)
  await saveProfile(profile, current.model, { kind: 'provider' })
}
export async function addModel(model: string): Promise<boolean> {
  if (!model.trim()) return false
  const workspace = await bridge().getRoutingWorkspace(model)
  const profile = independentProfile(workspace.profile)
  profile.excludedAccounts = profile.excludedAccounts.filter(id => workspace.accounts.some(account => account.id === id))
  for (const provider of workspace.providers) {
    if (!profile.providers.includes(provider.id)) {
      profile.providers.push(provider.id)
      if (profile.providerScope === 'selected') profile.disabledProviders.push(provider.id)
    }
    if (!profile.accountOrder[provider.id]) {
      profile.accountOrder[provider.id] = workspace.accounts.filter(account => account.provider === provider.id)
        .sort((left, right) => Number(left.priority) - Number(right.priority)).map(account => account.id)
    }
  }
  if (!await saveProfile(profile, model)) return false
  await selectModel(model)
  return true
}
