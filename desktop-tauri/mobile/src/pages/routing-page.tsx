import { apiCall } from '../api/client'
import { t } from '../i18n'
import { serverIdentity } from '../lib/servers'
import { RoutingErrorsPanel } from '../../../ui-shared/routing-errors-panel'
import type { ErrorPolicyApi } from '../../../ui-shared/routing-errors-types'

const api: ErrorPolicyApi = {
  load: () => apiCall('/api/routing/errors'),
  save: payload => apiCall('/api/routing/errors', { method: 'PUT', body: payload }),
}

export function RoutingPage() {
  return <div className='m-page' style={{ padding: '10px 8px' }}>
    <RoutingErrorsPanel api={api} t={t} identity={serverIdentity()} />
  </div>
}
