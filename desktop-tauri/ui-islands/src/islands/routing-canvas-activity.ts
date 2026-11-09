import * as React from 'react'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow, type RoutingWorkspace } from './routing-types'
import { attemptState, isTraceRunning, requestKey, requestModel, traceAttempts } from './routing-trace-model'

export type RoutingCanvasActivity = {
  running: number
  accounts: Record<string, number>
  members: Record<string, Record<string, number>>
}

const emptyActivity: RoutingCanvasActivity = { running: 0, accounts: {}, members: {} }

function summarize(rows: ReplayRow[], model: string, groupId?: string): RoutingCanvasActivity {
  const activity: RoutingCanvasActivity = { running: 0, accounts: {}, members: {} }
  const seen = new Set<string>()
  for (const row of rows) {
    const key = requestKey(row)
    if (seen.has(key) || !isTraceRunning(row) || model && requestModel(row).toLowerCase() !== model) continue
    if (groupId && row.route?.configSnapshot?.groupId !== groupId) continue
    seen.add(key)
    activity.running++
    const attempt = traceAttempts(row).at(-1)
    if (!attempt || attemptState(row, attempt) !== 'running') continue
    const account = attempt.accountId || row.accountId
    if (!account) continue
    activity.accounts[account] = (activity.accounts[account] || 0) + 1
    // 同一账号可属于多个成员，流量只落在这次请求实际选中的成员上。
    if (attempt.memberId) {
      const accounts = activity.members[attempt.memberId] ||= {}
      accounts[account] = (accounts[account] || 0) + 1
    }
  }
  return activity
}

export type RoutingCanvasRefresh = {
  activity: RoutingCanvasActivity
  workspace: RoutingWorkspace | null
  error: string
  refresh(): void
}

type RefreshOptions = {
  groupId?: string
  workspaceModel?: string
  paused?: boolean
  refreshToken?: number
  onWorkspace?(workspace: RoutingWorkspace): void
}

export function useRoutingCanvasActivity(active: boolean, model: string, options: RefreshOptions = {}): RoutingCanvasRefresh {
  model = model.trim().toLowerCase()
  const { groupId, paused = false, refreshToken = 0 } = options
  const workspaceModel = (options.workspaceModel ?? model).trim().toLowerCase()
  const key = JSON.stringify([model, groupId || '', workspaceModel])
  const [snapshot, setSnapshot] = React.useState<{ key: string; activity: RoutingCanvasActivity; workspace: RoutingWorkspace } | null>(null)
  const [failure, setFailure] = React.useState<{ key: string; message: string } | null>(null)
  const reloadRef = React.useRef<() => void>(() => {})
  const refresh = React.useCallback(() => reloadRef.current(), [])
  const lastRefreshToken = React.useRef(refreshToken)
  const onWorkspace = React.useRef(options.onWorkspace)
  onWorkspace.current = options.onWorkspace
  const context = React.useRef({ active, paused, key })
  context.current = { active, paused, key }

  React.useEffect(() => {
    if (!active || paused) return
    let alive = true
    let pending = false
    let queued = false
    let sequence = 0
    const visible = () => alive && context.current.active && !context.current.paused && context.current.key === key
      && !document.hidden && host().wbApp?.currentPage === 'routing'
    const current = (id: number) => visible() && id === sequence
    const loadActivity = async (id: number) => {
      const rows: ReplayRow[] = []
      let offset = 0
      let matched = Infinity
      while (current(id) && offset < matched) {
        const next = await bridge().getRoutingFlow({ model: model || undefined, minutes: 10, scope: 'running', detail: 'requests', offset, limit: 100 })
        if (!current(id)) return null
        if (next.available === false || !Array.isArray(next.entries) || !Number.isFinite(next.matched) || next.matched < 0) {
          throw new Error(t('路由接口返回了不完整的数据'))
        }
        matched = Math.min(matched, next.matched)
        if (!next.entries.length) break
        rows.push(...(next.entries as ReplayRow[]))
        offset += next.entries.length
      }
      return current(id) ? summarize(rows, model, groupId) : null
    }
    const load = async () => {
      if (pending || !visible()) return
      pending = true
      const id = ++sequence
      try {
        // 两份读数成批发布，不能让较早返回的动画或列表单独推进。
        const [workspaceResult, activityResult] = await Promise.allSettled([
          bridge().getRoutingWorkspace(workspaceModel || undefined), loadActivity(id),
        ])
        if (!current(id)) return
        if (workspaceResult.status === 'rejected') throw workspaceResult.reason
        if (activityResult.status === 'rejected') throw activityResult.reason
        if (!activityResult.value) return
        const workspace = workspaceResult.value
        if ((workspace?.model || '').trim().toLowerCase() !== workspaceModel || !workspace?.profile
          || !Array.isArray(workspace.accounts) || !Array.isArray(workspace.queue?.ordered) || !Array.isArray(workspace.queue?.blocked)
          || !workspace.metrics?.accountConnections || !Number.isFinite(workspace.updatedAt)) {
          throw new Error(t('路由接口返回了不完整的数据'))
        }
        setSnapshot({ key, workspace, activity: activityResult.value })
        setFailure(null)
        onWorkspace.current?.(workspace)
      } catch (error) {
        if (current(id)) setFailure({ key, message: errorMessage(error) })
      } finally {
        pending = false
        if (queued && visible()) { queued = false; void load() }
      }
    }
    const reload = () => {
      sequence++
      queued = visible() && pending
      if (!pending) void load()
    }
    reloadRef.current = reload
    void load()
    const timer = window.setInterval(() => { void load() }, 2500)
    document.addEventListener('visibilitychange', reload)
    window.addEventListener('wb:page', reload)
    return () => {
      alive = false
      sequence++
      reloadRef.current = () => {}
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', reload)
      window.removeEventListener('wb:page', reload)
    }
  }, [active, paused, model, groupId, workspaceModel, key])

  React.useEffect(() => {
    if (lastRefreshToken.current === refreshToken) return
    lastRefreshToken.current = refreshToken
    refresh()
  }, [refreshToken, refresh])

  const value = active && snapshot?.key === key ? snapshot : null
  return { activity: value?.activity || emptyActivity, workspace: value?.workspace || null,
    error: active && failure?.key === key ? failure.message : '', refresh }
}
