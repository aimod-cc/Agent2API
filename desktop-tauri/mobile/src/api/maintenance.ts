import { apiCall } from './client'
import type { MaintenanceApi, MaintenanceTask } from '../../../ui-shared/maintenance-types'

export const maintenanceApi: MaintenanceApi = {
  overview: query => {
    const params = new URLSearchParams(Object.entries(query).map(([key, value]) => [key, String(value)]))
    return apiCall(`/api/maintenance?${params}`)
  },
  detail: id => apiCall(`/api/maintenance/records/${encodeURIComponent(id)}`),
  run: id => apiCall(`/api/maintenance/run/${encodeURIComponent(id)}`, { method: 'POST' }),
  retry: (id, itemId) => apiCall(`/api/maintenance/records/${encodeURIComponent(id)}/retry`, { method: 'POST', body: itemId == null ? {} : { itemId } }),
  save: (id, patch) => apiCall<MaintenanceTask>(`/api/scheduled-tasks/${encodeURIComponent(id)}`, { method: 'PATCH', body: patch }),
}
