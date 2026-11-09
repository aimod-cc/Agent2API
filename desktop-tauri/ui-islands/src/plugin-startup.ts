import { callJson } from './islands/assistant-api'
import { startPluginStartupPolling, type PluginStartupSnapshot } from '../../ui-shared/plugin-startup'

type App = {
  refresh(options?: { force?: boolean }): Promise<unknown>
  getState(): { pluginStartup?: PluginStartupSnapshot } | null
}
const app = () => (window as unknown as { wbApp?: App }).wbApp
let stop: (() => void) | undefined
function start() {
  if (stop || !app()) return
  stop = startPluginStartupPolling({
    read: signal => callJson('GET', '/api/plugins/startup', undefined, signal),
    refresh: () => app()?.refresh() ?? Promise.resolve(),
    session: () => app()?.getState()?.pluginStartup,
  })
}
window.addEventListener('wb:app-ready', start, { once: true })
start()
