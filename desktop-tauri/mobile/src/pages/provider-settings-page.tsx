import { apiCall } from '../api/client'
import { store, useStore } from '../state/store'
import { locale, t } from '../i18n'
import { serverIdentity } from '../lib/servers'
import { ProviderSettingsPanel } from '../../../ui-shared/provider-settings-panel'
import type { ProviderSettingsRequest } from '../../../ui-shared/provider-settings'

const request: ProviderSettingsRequest = (method, path, body) => apiCall(path, { method, body })

export function ProviderSettingsPage() {
  const { route } = useStore()
  return <div className='m-page'>
    <div className='m-section' style={{ marginTop: 0 }}>
      <ProviderSettingsPanel key={serverIdentity()} request={request} t={t} locale={locale()}
        provider={route.params?.id || ''} onPlugins={() => store.push('plugins')} />
    </div>
  </div>
}
