import { subscribePanelEvents, type PanelChanges } from '../../../ui-shared/panel-events'
import { mergeUsageQueryRow } from '../../../ui-shared/usage-query-state'
import { openPanelEvents } from '../api/client'
import { fetchUsageSnapshot, type UsageSnapshot } from '../api/gateway'
import { serverIdentity } from './servers'
import { store } from '../state/store'

let stop: (() => void) | undefined
let connected = false
let currentIdentity = ''
let generation = 0
let usagePending: Promise<void> | undefined
let accountsPending: Promise<void> | undefined
let accountsRequested = 0
let usageRequested = 0
let usageTimer: ReturnType<typeof setTimeout> | undefined
let usage: UsageSnapshot = {}
let balanceRevision = 0
const revisionListeners = new Set<() => void>()
const scopes = new Map<symbol, string[]>()
const listeners = new Set<(snapshot: UsageSnapshot) => void>()

export const accountEventsConnected = (): boolean => connected
export const accountUsageSnapshot = (): UsageSnapshot => usage
export const accountBalanceRevision = (): number => balanceRevision
export function subscribeBalanceRevision(listener: () => void): () => void {
  revisionListeners.add(listener)
  return () => revisionListeners.delete(listener)
}

export function subscribeAccountUsage(listener: (snapshot: UsageSnapshot) => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

async function refreshAccounts(): Promise<void> {
  accountsRequested++
  if (!accountsPending) accountsPending = (async () => {
    let completed: number
    do { completed = accountsRequested; await store.refreshAccounts() }
    while (completed !== accountsRequested)
  })().finally(() => { accountsPending = undefined })
  return accountsPending
}

export function refreshAccountUsage(): Promise<void> {
  usageRequested++
  if (!usagePending) usagePending = (async () => {
    let completed: number
    do { completed = usageRequested; await readAccountUsage() }
    while (completed !== usageRequested)
  })().finally(() => { usagePending = undefined })
  return usagePending
}

async function readAccountUsage(): Promise<void> {
  const identity = serverIdentity()
  const ids = [...new Set([...scopes.values()].flat())]
  if (!ids.length) return
  const next = await fetchUsageSnapshot(undefined, ids)
  if (serverIdentity() !== identity) return
  const rows = new Map((usage.results || []).map(row => [row.id, row]))
  for (const row of next.results || []) {
    if (Number(rows.get(row.id)?.at) > Number(row.at)) continue
    rows.set(row.id, mergeUsageQueryRow(row, rows.get(row.id)))
  }
  const queries = new Map((usage.queries || []).map(task => [task.id, task]))
  for (const task of next.queries || []) {
    const previous = queries.get(task.id)
    if (previous && (previous.startedAt > task.startedAt
      || previous.requestId === task.requestId && !['queued', 'running'].includes(previous.status) && ['queued', 'running'].includes(task.status)
      || previous.requestId === task.requestId && previous.status === 'running' && task.status === 'queued')) continue
    queries.set(task.id, task)
  }
  const result = { ...usage, ...next, results: [...rows.values()], queries: [...queries.values()],
    listLimited: { ...usage.listLimited, ...next.listLimited } }
  if (JSON.stringify(usage) !== JSON.stringify(result)) {
    usage = result
    listeners.forEach(listener => listener(usage))
  }
  const snapshot = store.get().session?.accounts
  if (snapshot && next.listLimited) {
    const accounts = (snapshot.accounts || []).map(account => {
      const value = next.listLimited?.[account.id]
      return typeof value === 'boolean' && value !== account.listLimited ? { ...account, listLimited: value } : account
    })
    store.applyAccounts({ ...snapshot, accounts })
  }
}

function ensure(): void {
  const identity = serverIdentity()
  if (stop && currentIdentity === identity) return
  stop?.()
  generation++
  if (currentIdentity !== identity) {
    currentIdentity = identity
    usage = {}
    listeners.forEach(listener => listener(usage))
  }
  const run = generation
  const sync = async (changes: PanelChanges) => {
    if (run !== generation || serverIdentity() !== identity) return
    await Promise.allSettled([
      changes.accounts || changes.proxies ? refreshAccounts() : Promise.resolve(),
      changes.balances || changes.usage ? refreshAccountUsage() : Promise.resolve(),
    ])
    if (run === generation && changes.balances) { balanceRevision++; revisionListeners.forEach(listener => listener()) }
  }
  stop = subscribePanelEvents({
    open: signal => openPanelEvents(signal),
    changed: changes => { void sync(changes) },
    fallback: () => sync({ accounts: true, balances: true, usage: true, connections: false, proxies: true }),
    connection: status => { connected = status === 'connected' },
  })
}

export function trackAccountUsage(ids: string[]): { update: (ids: string[]) => void; close: () => void } {
  const token = Symbol()
  scopes.set(token, ids)
  ensure()
  const update = (ids: string[]) => {
    if (JSON.stringify(scopes.get(token)) === JSON.stringify(ids)) return
    scopes.set(token, ids)
    clearTimeout(usageTimer)
    if (!document.hidden) usageTimer = setTimeout(() => { void refreshAccountUsage().catch(() => {}) }, 150)
  }
  return {
    update,
    close: () => {
      scopes.delete(token)
      if (scopes.size) return
      generation++
      clearTimeout(usageTimer)
      stop?.()
      stop = undefined
      connected = false
    },
  }
}

export async function refreshRealtimeAccounts(): Promise<void> {
  void refreshAccountUsage().catch(() => {})
  await refreshAccounts()
}
