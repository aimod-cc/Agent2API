import type { PanelChanges } from '../../../ui-shared/panel-events'
import { PANEL_EVENTS_PATH, subscribePanelEvents } from '../../../ui-shared/panel-events'
import { requestDirect } from './assistant-api'
import { bump, getStore } from './accounts-store'
import { refreshAccountIndex } from './accounts-index-refresh'
import { setAccountEventsConnected } from './accounts-live-state'
import { invalidateAccountDetails } from './accounts-list-api'
import { syncBalancesSnapshot } from './accounts-usage'
import { syncConnections } from './accounts-data'
import { refreshProxyPool } from './proxy-pool-cache'
import { shared, type AccountRecord } from './accounts-shared'

let started = false
let stop: (() => void) | undefined
let syncing = false
let generation = 0
let balanceRevision = 0
let pending: PanelChanges = { accounts: false, balances: false, usage: false, connections: false, proxies: false }

export const accountBalanceEventsRevision = (): number => balanceRevision

function visible(): boolean {
  return !document.hidden && shared().wbApp?.currentPage === 'accounts'
}

async function sync(changes: PanelChanges): Promise<void> {
  const tasks: Array<Promise<unknown>> = []
  if (changes.accounts || changes.proxies) tasks.push(refreshAccountIndex())
  if (changes.accounts) tasks.push(shared().wbProviders?.refreshCustom?.() || Promise.resolve())
  if (changes.balances || changes.usage) tasks.push(syncBalancesSnapshot())
  if (changes.connections) tasks.push(syncConnections())
  if (changes.proxies) {
    tasks.push(refreshProxyPool())
    invalidateAccountDetails(getStore().visibleUsageIds)
    bump()
  }
  await Promise.allSettled(tasks)
  if (changes.balances) { balanceRevision++; bump() }
}

function enqueue(changes: PanelChanges): void {
  for (const key of Object.keys(pending) as Array<keyof PanelChanges>) pending[key] ||= changes[key]
  if (syncing) return
  const run = generation
  syncing = true
  void (async () => {
    while (visible() && run === generation && Object.values(pending).some(Boolean)) {
      const changes = pending
      pending = { accounts: false, balances: false, usage: false, connections: false, proxies: false }
      await sync(changes)
    }
  })().finally(() => {
    syncing = false
    if (visible() && Object.values(pending).some(Boolean)) enqueue(pending)
  })
}

export function startAccountEvents(): void {
  if (started) return
  started = true
  const activate = () => {
    if (!visible()) {
      generation++
      stop?.()
      stop = undefined
      setAccountEventsConnected(false)
      return
    }
    if (stop) return
    stop = subscribePanelEvents({
      open: signal => requestDirect(PANEL_EVENTS_PATH, { method: 'GET', signal }),
      changed: enqueue,
      fallback: () => sync({ accounts: true, balances: true, usage: true, connections: true, proxies: true }),
      connection: status => { setAccountEventsConnected(status === 'connected') },
    })
  }
  window.addEventListener('wb:page', activate)
  document.addEventListener('visibilitychange', activate)
  activate()
}

export function accountListSignature(accounts: AccountRecord[]): string {
  return JSON.stringify(accounts.map(account => {
    const { detailsLoaded: _loaded, ...fields } = account
    return fields
  }))
}
