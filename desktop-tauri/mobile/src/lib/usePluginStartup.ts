import { useEffect } from 'react'
import { apiCall } from '../api/client'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { startPluginStartupPolling } from '../../../ui-shared/plugin-startup'

export function usePluginStartup(enabled: boolean, identity: string) {
  useEffect(() => {
    if (!enabled) return
    return startPluginStartupPolling({
      read: signal => apiCall('/api/plugins/startup', { signal }),
      refresh: async () => { if (serverIdentity() === identity) await store.refresh(true) },
      session: () => store.get().session?.pluginStartup,
    })
  }, [enabled, identity])
}
