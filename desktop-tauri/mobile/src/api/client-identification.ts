import { apiCall } from './client'
import type { ClientIdentificationApi } from '../../../ui-shared/client-identification'

export const clientIdentificationApi: ClientIdentificationApi = {
  load: () => apiCall('/api/client-identification'),
  save: payload => apiCall('/api/client-identification', { method: 'PUT', body: payload }),
  preview: payload => apiCall('/api/client-identification/preview', { method: 'POST', body: payload }),
}
