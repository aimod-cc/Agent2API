import { apiCall } from './client'
import type { VisionCompatApi } from '../../../ui-shared/settings-vision-types'

export const visionCompatApi: VisionCompatApi = {
  load: () => apiCall('/api/vision-compatibility'),
  save: payload => apiCall('/api/vision-compatibility', { method: 'PUT', body: payload }),
}
