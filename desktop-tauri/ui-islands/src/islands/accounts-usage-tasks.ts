import { t } from '../i18n'
import { errorMessage, shared, toast } from './accounts-shared'
import { getStore, patch } from './accounts-store'
import { markUsageStale, usageRowOf } from './accounts-usage-cache'
import {
  USAGE_QUERY_POLL_MS, usageQueryDeadline,
  type UsageQueryTask, type UsageQueryStart,
} from '../../../ui-shared/usage-query-task'
import type { UsageQueryRow } from '../../../ui-shared/usage-query-state'

let sync: (() => Promise<boolean>) | undefined
let timer: ReturnType<typeof setTimeout> | undefined
let pollBusy = false
let batch: Map<string, string> | null = null
let sequence = 0
let revision = 0
export const usageTaskRevision = (): number => revision
const starts = new Map<string, number>()
const waits = new Map<string, Array<{ resolve: (row: UsageQueryRow | null) => void; timer: ReturnType<typeof setTimeout> }>>()

export function installUsageTaskSync(callback: () => Promise<boolean>): void { sync = callback }

function schedule(): void {
  if (timer || pollBusy || !sync || !getStore().usageInflight.size) return
  timer = setTimeout(async () => {
    timer = undefined
    pollBusy = true
    try { await usageQueryDeadline(sync!(), 5000) } catch { /* 下一轮继续读状态，整轮有独立时限。 */ }
    const tasks = new Map(getStore().usageTasks)
    let changed = false
    for (const [id, task] of tasks) {
      if (task.status === 'running' && Date.now() > task.deadlineAt + 15_000) {
        tasks.set(id, { ...task, status: 'timeout' })
        changed = true
        void shared().workbuddyDesktop?.cancelBalanceQuery?.(id, task.requestId).catch(() => {})
      }
    }
    if (changed) { revision++; publish(tasks) }
    pollBusy = false
    schedule()
  }, USAGE_QUERY_POLL_MS)
}

function settle(requestId: string, row: UsageQueryRow | null): void {
  const waiters = waits.get(requestId)
  if (!waiters) return
  waits.delete(requestId)
  for (const waiter of waiters) { clearTimeout(waiter.timer); waiter.resolve(row) }
}

function publish(tasks: Map<string, UsageQueryTask>): void {
  const previous = getStore().usageTasks
  for (const [id, task] of tasks) {
    const before = previous.get(id)
    if (before && before.requestId !== task.requestId) settle(before.requestId, null)
    if (task.status !== 'running') {
      const row = usageRowOf(id)
      settle(task.requestId, row?.requestId === task.requestId ? row : null)
    }
  }
  const inflight = new Set([...tasks].filter(([, task]) => task.status === 'running').map(([id]) => id))
  const usageBusy = Boolean(batch && [...batch].some(([id, requestId]) => {
    const task = tasks.get(id)
    return task?.requestId === requestId && task.status === 'running'
  }))
  patch({ usageTasks: tasks, usageInflight: inflight, usageBusy })
  schedule()
}

export function applyUsageTasks(incoming?: UsageQueryTask[]): void {
  if (!Array.isArray(incoming)) return
  const tasks = new Map(getStore().usageTasks)
  let changed = false
  for (const task of incoming) {
    if (!task?.id || !task.requestId || !Number.isFinite(task.startedAt) || !Number.isFinite(task.deadlineAt)) continue
    const current = tasks.get(task.id)
    if (current && (current.startedAt > task.startedAt
      || current.requestId === task.requestId && current.status !== 'running' && task.status === 'running'
      || current.requestId === task.requestId && current.status === 'config_changed')) continue
    if (current?.requestId === task.requestId && current.status === task.status) {
      if (task.status !== 'running') {
        const row = usageRowOf(task.id)
        settle(task.requestId, row?.requestId === task.requestId ? row : null)
      }
      continue
    }
    tasks.set(task.id, task)
    changed = true
  }
  if (changed) publish(tasks)
}

export async function startUsageTasks(id?: string, replace = true): Promise<UsageQueryStart | null> {
  const key = id || '*'
  if (starts.has(key)) return null
  const api = shared().workbuddyDesktop
  if (!api?.startBalanceQuery) throw new Error(t('余额查询接口不可用，请更新网关后重试'))
  const start = ++sequence
  starts.set(key, start)
  patch({ usageStarting: new Set(starts.keys()) })
  try {
    const data = await usageQueryDeadline(api.startBalanceQuery(id, replace))
    if (starts.get(key) !== start) return null
    if (!Array.isArray(data?.queries) || id && !data.queries.some(task => task.id === id)) throw new Error(t('余额查询接口返回异常'))
    revision++
    if (!id) batch = new Map(data.queries.map(task => [task.id, task.requestId]))
    applyUsageTasks(data.queries)
    if (!id) publish(new Map(getStore().usageTasks))
    if (sync) void sync()
    return data
  } finally {
    if (starts.get(key) === start) starts.delete(key)
    patch({ usageStarting: new Set(starts.keys()) })
  }
}

export function waitUsageTask(task: UsageQueryTask): Promise<UsageQueryRow | null> {
  const current = getStore().usageTasks.get(task.id)
  if (!current || current.requestId !== task.requestId) return Promise.resolve(null)
  if (current.status !== 'running') {
    const row = usageRowOf(task.id)
    if (row?.requestId === task.requestId) return Promise.resolve(row)
    if (!['success', 'failed', 'timeout'].includes(current.status) || !sync) return Promise.resolve(null)
    return sync().then(async () => {
      if (usageRowOf(task.id)?.requestId !== task.requestId) await sync!()
      const latest = usageRowOf(task.id)
      return getStore().usageTasks.get(task.id)?.requestId === task.requestId && latest?.requestId === task.requestId ? latest : null
    })
  }
  return new Promise(resolve => {
    const budget = Math.max(1000, Math.min(195_000, task.deadlineAt - Date.now() + 15_000))
    const timer = setTimeout(() => settle(task.requestId, null), budget)
    waits.set(task.requestId, [...(waits.get(task.requestId) || []), { resolve, timer }])
  })
}

export async function cancelUsageQuery(id: string): Promise<void> {
  const task = getStore().usageTasks.get(id)
  if (!task || task.status !== 'running') return
  try {
    const result = await usageQueryDeadline(shared().workbuddyDesktop!.cancelBalanceQuery(id, task.requestId))
    if (getStore().usageTasks.get(id)?.requestId !== task.requestId) return
    if (result.cancelled) { revision++; applyUsageTasks([{ ...task, status: 'cancelled' }]) }
    else if (sync) void sync()
  } catch (error) {
    toast(t('取消查询失败：{message}', { message: errorMessage(error) }), 'err')
  }
}

export async function cancelAllUsage(): Promise<void> {
  if (!batch) return
  await Promise.all([...batch].map(([id, requestId]) => getStore().usageTasks.get(id)?.requestId === requestId
    ? cancelUsageQuery(id) : Promise.resolve()))
}

export function invalidateUsageTasks(ids: string[]): void {
  revision++
  const tasks = new Map(getStore().usageTasks)
  for (const id of ids) {
    markUsageStale(id)
    starts.delete(id)
    const task = tasks.get(id)
    if (!task) continue
    settle(task.requestId, null)
    tasks.set(id, { ...task, status: 'config_changed' })
    if (task.status === 'running') void shared().workbuddyDesktop?.cancelBalanceQuery?.(id, task.requestId).catch(() => {})
  }
  patch({ usageStarting: new Set(starts.keys()) })
  publish(tasks)
}
