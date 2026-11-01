import * as React from 'react'
import { t } from '../i18n'
import { bridge, errorMessage, host, notify, type RouteProfile, type RoutingWorkspace } from './routing-types'
import { adoptAccountSorting } from './accounts-sort'

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
export function useRoutingEditor(onSaved: () => void) {
  const [scope, setScope] = React.useState('')
  const [drafts, setDrafts] = React.useState<Record<string, RoutingDraft>>({})
  const [loading, setLoading] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const sequence = React.useRef(0)
  const refs = React.useRef({ drafts, scope, busy, loading })
  refs.current = { drafts, scope, busy, loading }
  const load = React.useCallback(async (model: string, force = false) => {
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
      const independent = !model || workspace.customized
      setDrafts(current => ({ ...current, [model]: { workspace, mode: independent ? 'custom' : 'inherit', profile,
        baseline: independent ? clone(profile) : null, revision: workspace.revision! } }))
    } catch (failure) { if (id === sequence.current) setError(errorMessage(failure)) }
    finally { if (id === sequence.current) setLoading(false) }
  }, [])
  const update = (mutate: (draft: RoutingDraft) => RoutingDraft) => {
    if (refs.current.busy || refs.current.loading) return
    setDrafts(current => current[scope] ? { ...current, [scope]: mutate(current[scope]) } : current)
  }
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
  return { scope, load, draft: drafts[scope], drafts, update, save, discard, loading, busy, error,
    dirtyCount: Object.values(drafts).filter(draftDirty).length }
}
export type RoutingEditorState = ReturnType<typeof useRoutingEditor>
