import { MaintenancePanel } from '../../../ui-shared/maintenance-panel'
import { maintenanceApi } from '../api/maintenance'
import { store } from '../state/store'
import { apiCall } from '../api/client'

export function TasksPage() {
  return <div className='m-page'><MaintenancePanel api={maintenanceApi}
    pluginTaskRequest={(method, path, body) => apiCall(path, { method, body })}
    notify={(message, kind) => store.toast(message, kind)}
    onChanged={() => store.refresh(true)}
    onAccount={id => store.push('accounts', { id })} /></div>
}
