import * as React from 'react'
import { t } from '../i18n'
import { bridge, errorMessage, host, notify, type RouteProfile, type RoutingWorkspace } from './routing-types'
import { adoptAccountSorting } from './accounts-sort'
import type { AccountRecord, AccountSorting } from './accounts-shared'
import { canSyncSortingRevision, syncRoutingSorting } from './routing-sorting-state'

export type RoutingDraft = { mode: 'inherit' | 'custom'; profile: RouteProfile; baseline: RouteProfile | null; revision: string; workspace: RoutingWorkspace }
const clone = (value: RouteProfile) => structuredClone(value)
export function editableProfile(workspace: RoutingWorkspace): RouteProfile {
  const profile = clone(workspace.profile)
  if (!workspace.model) { profile.strategy = workspace.settings.routingStrategy; profile.sticky = workspace.settings.routingSticky }
  return profile
}
export function draftDirty(draft: RoutingDraft) {
  return JSON.stringify(draft.mode === 'inherit' ? null : draft.profile) !== JSON.stringify(draft.baseline)
}
/**
 * 路由配置草稿。按作用范围（'' = 全局）各存一份，切换范围不会丢掉没保存的改动。
 *
 * `load` 的第三个参数用于「新增模型独立配置」：直接以独立模式打开某个模型，
 * 不必先打开它再看它继承全局。不传时按后端返回的 customized 决定。
 */
export function useRoutingEditor(onSaved: () => void) {
  const [scope, setScope] = React.useState('')
  const [drafts, setDrafts] = React.useState<Record<string, RoutingDraft>>({})
  const [loading, setLoading] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const sequence = React.useRef(0)
  const refs = React.useRef({ drafts, scope, busy, loading })
  refs.current = { drafts, scope, busy, loading }
  const load = React.useCallback(async (model: string, force = false, mode?: 'inherit' | 'custom') => {
    if (refs.current.busy) return
    model = model.trim().toLowerCase()
    setScope(model)
    const id = ++sequence.current
    if (!force && refs.current.drafts[model] && draftDirty(refs.current.drafts[model])) { setError(''); setLoading(false); return }
    setLoading(true); setError('')
    try {
      const workspace = await bridge().getRoutingWorkspace(model || undefined)
      if (id !== sequence.current) return
      if (!workspace.revision) throw new Error(t('路由配置版本不可用，请更新网关后重试'))
      const profile = editableProfile(workspace)
      const independent = mode === 'custom' ? true : mode === 'inherit' ? false : (!model || workspace.customized)
      setDrafts(current => ({ ...current, [model]: { workspace, mode: independent ? 'custom' : 'inherit', profile,
        baseline: independent ? clone(profile) : null, revision: workspace.revision! } }))
    } catch (failure) { if (id === sequence.current) setError(errorMessage(failure)) }
    finally { if (id === sequence.current) setLoading(false) }
  }, [])
  const update = (mutate: (draft: RoutingDraft) => RoutingDraft) => {
    if (refs.current.busy || refs.current.loading) return
    setDrafts(current => current[scope] ? { ...current, [scope]: mutate(current[scope]) } : current)
  }
  const syncAccounts = React.useCallback((accounts: AccountRecord[], updatedAt?: number) => {
    setDrafts(current => Object.fromEntries(Object.entries(current).map(([model, draft]) => [model,
      updatedAt != null && updatedAt < draft.workspace.updatedAt ? draft : { ...draft, workspace: { ...draft.workspace, accounts } },
    ])))
  }, [])
  const syncSorting = React.useCallback((sorting: AccountSorting) => {
    if (!sorting?.rules || !sorting.accountOrder) return
    setDrafts(current => Object.fromEntries(Object.entries(current).map(([model, draft]) => [model,
      model ? syncRoutingSorting(draft, sorting) : draft,
    ])))
  }, [])
  const syncWorkspace = React.useCallback((workspace: RoutingWorkspace, convertedGlobalAccounts = false) => {
    setDrafts(current => {
      const model = workspace.model?.toLowerCase() || '', draft = current[model]
      if (!draft) return current
      const dirty = draftDirty(draft), profile = editableProfile(workspace)
      if (!model && convertedGlobalAccounts) return { ...current, [model]: { ...draft, workspace,
        profile: dirty ? { ...draft.profile, providerScope: 'all', disabledProviders: [], excludedAccounts: [] } : profile,
        baseline: clone(profile), revision: workspace.revision || draft.revision } }
      if ((workspace.updatedAt || 0) < (draft.workspace.updatedAt || 0)) return current
      const synced = syncRoutingSorting(draft, workspace.sorting)
      const refreshOrder = workspace.sorting.updatedAt >= draft.workspace.sorting.updatedAt
        && canSyncSortingRevision(draft.workspace, workspace)
      return { ...current, [model]: { ...synced, revision: refreshOrder ? workspace.revision || draft.revision : draft.revision,
        workspace: { ...synced.workspace, accounts: workspace.accounts,
          ...(refreshOrder ? { profile: workspace.profile, inheritedProfile: workspace.inheritedProfile, revision: workspace.revision } : {}),
          accountStates: workspace.accountStates, metrics: workspace.metrics, queue: workspace.queue, updatedAt: workspace.updatedAt,
          globalAccountControl: workspace.globalAccountControl, legacyGlobalParticipation: workspace.legacyGlobalParticipation } } }
    })
  }, [])
  const save = async () => {
    const draft = refs.current.drafts[scope]
    if (!draft || refs.current.busy || refs.current.loading || !draftDirty(draft)) return
    refs.current.busy = true
    setBusy(true); setError('')
    try {
      const workspace = await bridge().saveRoutingProfile({ model: scope || null, profile: draft.mode === 'inherit' ? null : draft.profile, revision: draft.revision })
      if (!workspace?.revision) throw new Error(t('保存响应不完整，请刷新当前配置确认结果'))
      const profile = editableProfile(workspace), independent = !scope || workspace.customized
      setDrafts(current => ({ ...current, [scope]: { workspace, mode: independent ? 'custom' : 'inherit', profile,
        baseline: independent ? clone(profile) : null, revision: workspace.revision! } }))
      adoptAccountSorting(workspace.sorting)
      if (!scope) void host().wbApp?.refresh?.()
      notify(t('路由配置已保存，仅影响后续选路'))
      onSaved()
    } catch (failure) { setError(errorMessage(failure)) }
    finally { refs.current.busy = false; setBusy(false) }
  }
  // 删除模型独立配置：直接落库回继承全局，不再让用户先切「继承」再保存。
  const remove = async (): Promise<boolean> => {
    const target = scope
    const draft = refs.current.drafts[target]
    if (!target || !draft || refs.current.busy || refs.current.loading) return false
    refs.current.busy = true
    setBusy(true); setError('')
    try {
      const workspace = await bridge().saveRoutingProfile({ model: target, profile: null, revision: draft.revision })
      if (!workspace?.revision) throw new Error(t('保存响应不完整，请刷新当前配置确认结果'))
      setDrafts(current => {
        const next = { ...current }
        delete next[target]
        for (const [model, cached] of Object.entries(next)) {
          next[model] = { ...cached, workspace: { ...cached.workspace, configuredModels: workspace.configuredModels } }
        }
        return next
      })
      adoptAccountSorting(workspace.sorting)
      notify(t('已删除独立配置，改为跟随全局'))
      onSaved()
      return true
    } catch (failure) { setError(errorMessage(failure)); return false }
    finally { refs.current.busy = false; setBusy(false) }
  }
  // 新增模型独立配置：立即落库，而不是只打开一份未保存草稿，避免用户以为已经创建。
  const create = async (model: string): Promise<boolean> => {
    const target = model.trim().toLowerCase()
    if (!target || refs.current.busy || refs.current.loading) return false
    refs.current.busy = true
    setBusy(true); setError('')
    try {
      const workspace = await bridge().getRoutingWorkspace(target)
      if (!workspace.revision) throw new Error(t('路由配置版本不可用，请更新网关后重试'))
      const profile = editableProfile(workspace)
      // 新独立配置沿用当前生效的参与方式；没有显式配置时默认全部自动参与。
      if (!profile.providerScope) profile.providerScope = 'all'
      if (profile.providerScope === 'selected') profile.disabledProviders = profile.disabledProviders.filter(id => profile.providers.includes(id))
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
      const saved = await bridge().saveRoutingProfile({ model: target, profile, revision: workspace.revision })
      if (!saved?.revision) throw new Error(t('保存响应不完整，请刷新当前配置确认结果'))
      const nextProfile = editableProfile(saved)
      setDrafts(current => ({ ...current, [target]: { workspace: saved, mode: 'custom', profile: nextProfile,
        baseline: clone(nextProfile), revision: saved.revision! } }))
      setScope(target)
      adoptAccountSorting(saved.sorting)
      notify(t('路由配置已保存，仅影响后续选路'))
      onSaved()
      return true
    } catch (failure) { setError(errorMessage(failure)); return false }
    finally { refs.current.busy = false; setBusy(false) }
  }
  const discard = () => {
    const draft = drafts[scope]
    if (!draft || busy || loading) return
    update(current => ({ ...current, mode: current.baseline ? 'custom' : 'inherit', profile: current.baseline ? clone(current.baseline) : editableProfile(current.workspace) }))
    setError('')
  }
  React.useEffect(() => {
    const listener = (event: BeforeUnloadEvent) => {
      if (Object.values(refs.current.drafts).some(draftDirty)) event.preventDefault()
    }
    window.addEventListener('beforeunload', listener)
    return () => { sequence.current++; window.removeEventListener('beforeunload', listener) }
  }, [])
  return { scope, load, create, remove, draft: drafts[scope], drafts, update, syncAccounts, syncSorting, syncWorkspace, save, discard, loading, busy, error,
    dirtyCount: Object.values(drafts).filter(draftDirty).length }
}
export type RoutingEditorState = ReturnType<typeof useRoutingEditor>
