import * as React from 'react'
import { getPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { shared } from './add-account-bridge'
import { useAdvertisements } from '../../../ui-shared/advertisements'

function availabilitySnapshot(): string {
  const startup = getPluginStartup().value
  return startup ? `${startup.bootId}:${startup.availabilityRevision}` : ''
}

function catalogSnapshot(): string {
  const app = shared().wbApp?.getState?.()
  const counts = new Map<string, number>()
  for (const account of app?.accounts?.accounts || []) {
    const provider = account.provider || 'workbuddy'
    counts.set(provider, (counts.get(provider) || 0) + 1)
  }
  return JSON.stringify([
    availabilitySnapshot(),
    shared().wbProviders?.all?.() || [],
    [...counts].sort(([left], [right]) => left.localeCompare(right)),
    (shared().wbProviders?.customList?.() || []).map(provider => ({
      id: provider.id, name: provider.name, protocol: provider.protocol, baseUrl: provider.baseUrl,
      presetKey: provider.presetKey, balanceScheme: provider.balanceScheme,
      clientEmulation: provider.clientEmulation, quirks: provider.quirks,
    })),
  ])
}

export function useAddAccountCatalog() {
  const ads = useAdvertisements()
  const availability = React.useSyncExternalStore(subscribePluginStartup, availabilitySnapshot)
  const [version, setVersion] = React.useState(0)
  const previous = React.useRef<string | null>(null)
  const mounted = React.useRef(false)
  if (previous.current === null) previous.current = catalogSnapshot()

  const sync = React.useCallback(() => {
    if (!mounted.current) return
    const next = catalogSnapshot()
    if (next === previous.current) return
    previous.current = next
    setVersion(value => value + 1)
  }, [])

  const refreshCustom = React.useCallback(async () => {
    await shared().wbProviders?.refreshCustom?.()
    sync()
  }, [sync])

  React.useEffect(() => {
    mounted.current = true
    const unsubscribe = shared().wbApp?.subscribeRefresh?.(sync)
    void shared().wbProviders?.load?.().then(sync)
    void refreshCustom()
    sync()
    return () => {
      mounted.current = false
      unsubscribe?.()
    }
  }, [sync, refreshCustom])

  React.useEffect(sync, [availability, sync])
  React.useEffect(() => { setVersion(value => value + 1) }, [ads])
  return { version, availability, refreshCustom }
}
