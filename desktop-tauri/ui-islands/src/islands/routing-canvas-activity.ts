import * as React from 'react'
import { bridge, host, type ReplayRow } from './routing-types'
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

export function useRoutingCanvasActivity(active: boolean, model: string, groupId?: string) {
  model = model.trim().toLowerCase()
  const key = JSON.stringify([model, groupId || ''])
  const [snapshot, setSnapshot] = React.useState<{ key: string; activity: RoutingCanvasActivity } | null>(null)

  React.useEffect(() => {
    if (!active) return
    let alive = true
    let pending = false
    const load = async () => {
      if (pending || document.hidden || host().wbApp?.currentPage !== 'routing') return
      pending = true
      try {
        const rows: ReplayRow[] = []
        let offset = 0
        let matched = Infinity
        while (alive && offset < matched) {
          const next = await bridge().getRoutingFlow({ model: model || undefined, minutes: 10, scope: 'running', detail: 'requests', offset, limit: 100 })
          if (!alive) return
          if (next.available === false || !Array.isArray(next.entries)) { setSnapshot(null); return }
          matched = Math.min(matched, next.matched)
          if (!next.entries.length) break
          rows.push(...(next.entries as ReplayRow[]))
          offset += next.entries.length
        }
        if (alive) setSnapshot({ key, activity: summarize(rows, model, groupId) })
      } catch {
        if (alive) setSnapshot(null)
      } finally { pending = false }
    }
    const refresh = () => { void load() }
    const pageChanged = (event: Event) => {
      if ((event as CustomEvent<string>).detail === 'routing') refresh()
    }
    refresh()
    const timer = window.setInterval(refresh, 2500)
    document.addEventListener('visibilitychange', refresh)
    window.addEventListener('wb:page', pageChanged)
    return () => {
      alive = false
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refresh)
      window.removeEventListener('wb:page', pageChanged)
    }
  }, [active, model, groupId, key])

  return active && snapshot?.key === key ? snapshot.activity : emptyActivity
}
