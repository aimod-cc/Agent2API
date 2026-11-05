import * as React from 'react'
import { confirmDialog } from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, host, notify, type RouteGroup, type RouteGroups } from './routing-types'

type GroupDraft = { group: RouteGroup; baseline: RouteGroup | null; revision: string }
export const groupDirty = (draft: GroupDraft) => JSON.stringify(draft.group) !== JSON.stringify(draft.baseline)

/** 按组保留草稿。新组在保存前不发布到模型目录。 */
export function useRouteGroups(onSaved: () => void, onRemoved: () => void) {
  const [data, setData] = React.useState<RouteGroups | null>(null)
  const [drafts, setDrafts] = React.useState<Record<string, GroupDraft>>({})
  const [selected, setSelected] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const refs = React.useRef({ busy, loading, drafts, selected })
  refs.current = { busy, loading, drafts, selected }
  const apply = React.useCallback((next: RouteGroups, saved?: string) => {
    setData(next)
    setDrafts(current => {
      const result: Record<string, GroupDraft> = {}
      for (const row of next.groups) {
        const old = current[row.group.id]
        result[row.group.id] = old && groupDirty(old) && row.group.id !== saved ? old
          : { group: structuredClone(row.group), baseline: structuredClone(row.group), revision: row.revision }
      }
      for (const [id, old] of Object.entries(current)) {
        if (!result[id] && id !== saved && groupDirty(old)) result[id] = old
      }
      return result
    })
  }, [])
  const load = React.useCallback(async () => {
    if (refs.current.busy || refs.current.loading) return
    refs.current.loading = true; setLoading(true); setError('')
    try { apply(await bridge().getRoutingGroups()) }
    catch (failure) { setError(errorMessage(failure)) }
    finally { refs.current.loading = false; setLoading(false) }
  }, [apply])
  React.useEffect(() => { void load() }, [load])
  const reload = async () => {
    const id = refs.current.selected, draft = refs.current.drafts[id]
    if (refs.current.busy || refs.current.loading) return
    if (draft && groupDirty(draft) && !await confirmDialog(t('重新读取会丢弃当前范围的未保存草稿，其他范围的草稿保留。'), {
      title: t('重新读取配置'), confirmText: t('重新读取'),
    })) return
    refs.current.loading = true; setLoading(true); setError('')
    try {
      const next = await bridge().getRoutingGroups()
      apply(next, id)
      if (!next.groups.some(row => row.group.id === id)) setSelected('')
    } catch (failure) { setError(errorMessage(failure)) }
    finally { refs.current.loading = false; setLoading(false) }
  }
  const create = (group: RouteGroup): boolean => {
    if (!data || refs.current.busy || refs.current.loading) return false
    setDrafts(current => ({ ...current, [group.id]: { group, baseline: null, revision: data.newRevision } }))
    setSelected(group.id)
    return true
  }
  const update = (mutate: (group: RouteGroup) => void) => {
    if (refs.current.busy || refs.current.loading) return
    setDrafts(current => {
      const draft = current[refs.current.selected]
      if (!draft) return current
      const group = structuredClone(draft.group); mutate(group)
      return { ...current, [group.id]: { ...draft, group } }
    })
  }
  const save = async () => {
    const draft = refs.current.drafts[refs.current.selected]
    if (!draft || refs.current.busy || refs.current.loading || !groupDirty(draft)) return
    if (!draft.group.members.some(member => member.included && member.accounts.length)
      && !await confirmDialog(t('此入口没有参与路由的账号。保存后请求将明确失败，不会回退全局或自动恢复参与名单。'), {
        title: t('保存空路由'), confirmText: t('仍然保存'), destructive: true,
      })) return
    refs.current.busy = true; setBusy(true); setError('')
    try {
      apply(await bridge().saveRoutingGroup({ id: draft.group.id, group: draft.group, revision: draft.revision }), draft.group.id)
      notify(t('路由配置已保存，仅影响后续选路')); void host().wbApp?.refresh?.(); onSaved()
    } catch (failure) { setError(errorMessage(failure)) }
    finally { refs.current.busy = false; setBusy(false) }
  }
  const remove = async () => {
    const draft = refs.current.drafts[refs.current.selected]
    if (!draft || refs.current.busy || refs.current.loading || !await confirmDialog(t('删除后此入口将不再接受请求。不会删除成员模型、提供商或账号。'), {
      title: t('删除路由组'), confirmText: t('删除'), destructive: true,
    })) return
    refs.current.busy = true; setBusy(true); setError('')
    try {
      if (draft.baseline) apply(await bridge().saveRoutingGroup({ id: draft.group.id, group: null, revision: draft.revision }), draft.group.id)
      else setDrafts(current => { const next = { ...current }; delete next[draft.group.id]; return next })
      setSelected(''); onRemoved(); onSaved()
    } catch (failure) { setError(errorMessage(failure)) }
    finally { refs.current.busy = false; setBusy(false) }
  }
  const discard = () => {
    const draft = refs.current.drafts[refs.current.selected]
    if (!draft || refs.current.busy || refs.current.loading) return
    if (!draft.baseline) {
      setDrafts(current => { const next = { ...current }; delete next[draft.group.id]; return next }); setSelected(''); onRemoved()
    } else setDrafts(current => ({ ...current, [draft.group.id]: { ...draft, group: structuredClone(draft.baseline!) } }))
    setError('')
  }
  React.useEffect(() => {
    const listener = (event: BeforeUnloadEvent) => { if (Object.values(refs.current.drafts).some(groupDirty)) event.preventDefault() }
    window.addEventListener('beforeunload', listener)
    return () => window.removeEventListener('beforeunload', listener)
  }, [])
  return { data, drafts, selected, select: setSelected, draft: drafts[selected], create, update, save, remove, discard, load, reload, busy, loading, error,
    dirtyCount: Object.values(drafts).filter(groupDirty).length }
}
export type RouteGroupEditor = ReturnType<typeof useRouteGroups>
