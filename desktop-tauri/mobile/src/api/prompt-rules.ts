import { apiCall } from './client'
import type { PromptApi } from '../../../ui-shared/settings-prompt-types'

export const promptRulesApi: PromptApi = {
  load: () => apiCall('/api/prompt/rules'),
  save: payload => apiCall('/api/prompt/rules', { method: 'PUT', body: payload }),
  preview: payload => apiCall('/api/prompt/rules/preview', { method: 'POST', body: payload }),
  saveGateway: payload => apiCall('/api/prompt', { method: 'PUT', body: payload }),
}
