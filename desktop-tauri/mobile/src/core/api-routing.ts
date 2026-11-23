import { apiCall } from './client'
import type { ErrorPolicyView, ErrorSettings } from '../../../ui-shared/routing-errors-types'

export type {
  CooldownMode,
  CooldownScope,
  ErrorAction,
  ErrorKind,
  ErrorPolicyView,
  ErrorRule,
  ErrorSettings,
  PluginErrorRule,
  ProviderRuleEntry,
} from '../../../ui-shared/routing-errors-types'

export {
  actionLabels,
  cooldownSummary,
  errorPolicyValidation,
  kindLabels,
  mergeProviderRules,
  ruleLabel,
  scopeLabels,
} from '../../../ui-shared/routing-errors-types'

export type StickyEntry = {
  id: string
  model: string
  accountId: string
  accountName: string
  provider: string
  providerLabel: string
  source: string
  keyPreview: string
  hits: number
  lastSeen: number
  expiresAt: number
}

export type StickyPayload = {
  entries: StickyEntry[]
  now: number
  sticky: string
}

export function fetchRoutingSticky(model?: string, signal?: AbortSignal): Promise<StickyPayload> {
  const query = model?.trim() ? `?model=${encodeURIComponent(model.trim())}` : ''
  return apiCall<StickyPayload>(`/api/routing/sticky${query}`, { signal })
}

export function clearRoutingSticky(body: {
  id?: string
  model?: string
  all?: boolean
}): Promise<{ cleared: number; success: boolean }> {
  return apiCall('/api/routing/sticky/clear', { method: 'POST', body })
}

export function fetchRoutingErrors(signal?: AbortSignal): Promise<ErrorPolicyView> {
  return apiCall<ErrorPolicyView>('/api/routing/errors', { signal })
}

export function saveRoutingErrors(settings: ErrorSettings, revision: string): Promise<ErrorPolicyView> {
  return apiCall<ErrorPolicyView>('/api/routing/errors', {
    method: 'PUT',
    body: { settings, revision },
  })
}
