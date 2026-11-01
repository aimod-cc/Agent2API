import * as React from 'react'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow, type RoutingWorkspace, type StickyEntry } from './routing-types'
import { isTraceRunning, requestKey } from './routing-trace-model'
import { requestModelId } from './routing-flow-model'
import type { FlowQuery, FlowSelection, FlowSnapshot } from './routing-flow-types'

export function useRoutingFlow(active: boolean, initialRow?: ReplayRow | null) {
  const [model, setModel] = React.useState('')
  const [minutes, setMinutes] = React.useState(10)
  const [scope, setScope] = React.useState<'window' | 'running'>('window')
  const [search, setSearch] = React.useState('')
  const [snapshot, setSnapshot] = React.useState<FlowSnapshot | null>(null)
  const [workspace, setWorkspace] = React.useState<RoutingWorkspace | null>(null)
  const [sticky, setSticky] = React.useState<StickyEntry[]>([])
  const [selection, setSelection] = React.useState<FlowSelection>(initialRow ? { kind: 'request', row: initialRow } : null)
  const [error, setError] = React.useState('')
  const [loading, setLoading] = React.useState(false)
  const [token, refresh] = React.useReducer(value => value + 1, 0)
  const [clock, setClock] = React.useState({ server: Date.now(), local: Date.now() })
  const [now, setNow] = React.useState(Date.now())
  const latest = React.useRef(selection)
  latest.current = selection
  const sequence = React.useRef(0)
  const query: FlowQuery = { model: model || undefined, minutes, scope, search: search || undefined }
  const queryKey = JSON.stringify(query)
  React.useEffect(() => {
    if (!initialRow) return
    setModel(requestModelId(initialRow))
    setSelection({ kind: 'request', row: initialRow })
  }, [initialRow])
  React.useEffect(() => {
    const row = selection?.kind === 'request' ? selection.row : selection?.kind === 'session' ? selection.session.latest : null
    const requestModel = row ? requestModelId(row) : ''
    if (row && requestModel.toLowerCase() !== model.toLowerCase()) {
      setModel(requestModel); setWorkspace(null); setSnapshot(null); setSearch('')
    }
  }, [selection])
  React.useEffect(() => {
    if (!active) return
    let alive = true, pending = false
    const load = async (silent: boolean) => {
      if (pending) return
      pending = true
      const id = ++sequence.current
      if (!silent) setLoading(true)
      try {
        const [flow, workspace, sessions] = await Promise.all([
          bridge().getRoutingFlow(query), bridge().getRoutingWorkspace(model || undefined), bridge().getRoutingSticky(model || undefined),
        ])
        if (!alive || id !== sequence.current) return
        if (flow.available === false) throw new Error(t('请求记录数据库不可用'))
        if (!workspace?.profile || !Array.isArray(workspace.accounts)) throw new Error(t('路由接口返回了不完整的数据'))
        setSnapshot(flow); setWorkspace(workspace); setSticky(sessions.entries || [])
        setClock({ server: flow.now, local: Date.now() }); setNow(flow.now); setError('')
        const selected = latest.current
        const row = selected?.kind === 'request' ? selected.row : selected?.kind === 'session' ? selected.session.latest : null
        if (row && isTraceRunning(row)) {
          try {
            const next = await bridge().getRoutingTraceRequest({ id: row.id, ts: row.ts })
            if (alive && id === sequence.current) setSelection(current => {
              if (current?.kind === 'request' && requestKey(current.row) === requestKey(row)) return { ...current, row: next }
              if (current?.kind === 'session' && requestKey(current.session.latest) === requestKey(row)) return { ...current, session: { ...current.session, latest: next } }
              return current
            })
          } catch (failure) { if (alive) setError(errorMessage(failure)) }
        }
      } catch (failure) { if (alive && id === sequence.current) setError(errorMessage(failure)) }
      finally { pending = false; if (alive && id === sequence.current) setLoading(false) }
    }
    void load(false)
    const timer = window.setInterval(() => { if (!document.hidden && host().wbApp?.currentPage === 'routing') void load(true) }, 2500)
    return () => { alive = false; sequence.current++; window.clearInterval(timer) }
  }, [active, queryKey, token])
  React.useEffect(() => {
    if (!active) return
    const timer = window.setInterval(() => { if (!document.hidden) setNow(clock.server + Date.now() - clock.local) }, 1000)
    return () => window.clearInterval(timer)
  }, [active, clock])
  const selectModel = (value: string) => { setModel(value); setSelection(null); setWorkspace(null); setSnapshot(null); setSearch('') }
  return { model, selectModel, minutes, setMinutes, scope, setScope, search, setSearch, snapshot, workspace, sticky,
    selection, setSelection, error, loading, refresh, query, now }
}
