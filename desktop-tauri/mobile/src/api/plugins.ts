import { apiCall } from './client'
import { updateErrorMessage } from '../../../ui-shared/updates'
import { t } from '../i18n'
import type {
  PluginMutation, PluginOperation, PluginSettings, PluginsPayload, PluginUpdate, ReleaseState,
} from '../../../ui-shared/plugins'

export type {
  PackProvider, PackSummary, PluginMutation, PluginOperation, PluginSettings,
  PluginsPayload, PluginUpdate, StorePlugin,
} from '../../../ui-shared/plugins'

export function pluginErrorMessage(error: unknown): string {
  return updateErrorMessage(error, t)
}

export function getPlugins(signal?: AbortSignal): Promise<PluginsPayload> {
  return apiCall('/api/plugins', { signal })
}

export function refreshPluginCatalog(): Promise<ReleaseState> {
  return apiCall('/api/plugins/release')
}

export function checkPluginUpdates(): Promise<{ error?: boolean; reason?: string; updates?: PluginUpdate[] }> {
  return apiCall('/api/plugins/update')
}

export function startPluginInstall(id: string, target: string, update = false): Promise<{ operation: PluginOperation }> {
  return apiCall(update ? '/api/plugins/update' : '/api/plugins/install-from-release', {
    method: 'POST',
    body: { id, target },
  })
}

export function setPluginEnabled(id: string, enabled: boolean): Promise<PluginMutation> {
  return apiCall('/api/plugins/enable', { method: 'POST', body: { id, enabled } })
}

export function setPluginProvider(id: string, provider: string, enabled: boolean): Promise<PluginMutation> {
  return apiCall('/api/plugins/provider', { method: 'POST', body: { id, provider, enabled } })
}

export function uninstallPlugin(id: string): Promise<PluginMutation> {
  return apiCall('/api/plugins/uninstall', { method: 'POST', body: { id } })
}

export function getPluginSettings(signal?: AbortSignal): Promise<PluginSettings> {
  return apiCall('/api/plugins/settings', { signal })
}

export function savePluginSettings(body: Partial<PluginSettings>): Promise<PluginSettings> {
  return apiCall('/api/plugins/settings', { method: 'PATCH', body })
}
