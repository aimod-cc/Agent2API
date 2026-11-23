import { apiCall } from './client'

/* ─── 1. 系统通知 ────────────────────────────────────────── */

export type NotificationSettings = {
  enabled: boolean
  events: {
    loginSuccess: boolean
    loginFailure: boolean
    checkinFailure: boolean
  }
  channels: {
    webhook: {
      enabled: boolean
      url: string
      headersSet: boolean
      headerNames: string[]
    }
    telegram: {
      enabled: boolean
      botTokenSet: boolean
      chatId: string
      apiBase: string
      proxyUrl?: string
    }
    bark: {
      enabled: boolean
      deviceKeySet: boolean
      serverUrl: string
      group: string
      sound: string
    }
  }
}

export function fetchNotifications(signal?: AbortSignal): Promise<NotificationSettings> {
  return apiCall<NotificationSettings>('/api/notifications', { signal })
}

export function saveNotifications(patch: Record<string, unknown>): Promise<NotificationSettings> {
  return apiCall<NotificationSettings>('/api/notifications', {
    method: 'PUT',
    body: patch,
  })
}

export function testNotificationChannel(
  payload: Record<string, unknown>,
): Promise<{ sent: boolean; channel: string }> {
  return apiCall<{ sent: boolean; channel: string }>('/api/notifications/test', {
    method: 'POST',
    body: payload,
  })
}

/* ─── 2. OAuth 第三方登录 ─────────────────────────────────── */

export type BoundOauthAccount = {
  id: string
  login?: string
  email?: string
  name?: string
  boundAt?: number
}

export type OauthSettingsData = {
  publicBaseUrl: string
  callbackPath: string
  google: {
    enabled: boolean
    clientId: string
    clientSecretSet: boolean
    allowedEmails: string[]
    boundAccounts: BoundOauthAccount[]
  }
  github: {
    enabled: boolean
    clientId: string
    clientSecretSet: boolean
    allowedUsers: string[]
    boundAccounts: BoundOauthAccount[]
  }
}

export function fetchOauthSettings(signal?: AbortSignal): Promise<OauthSettingsData> {
  return apiCall<OauthSettingsData>('/api/panel/oauth/settings', { signal })
}

export function saveOauthSettings(patch: Record<string, unknown>): Promise<OauthSettingsData> {
  return apiCall<OauthSettingsData>('/api/panel/oauth/settings', {
    method: 'PUT',
    body: patch,
  })
}

export function startOauthBind(payload: {
  provider: 'google' | 'github'
  origin: string
  patch?: Record<string, unknown>
}): Promise<{ state: string; authorizeUrl: string }> {
  return apiCall('/api/panel/oauth/bind-url', {
    method: 'POST',
    body: payload,
  })
}

export function pollOauthBindState(state: string): Promise<{
  status: 'pending' | 'done' | 'error'
  identity?: string
  provider?: string
  error?: string
}> {
  return apiCall(`/api/panel/oauth/poll?state=${encodeURIComponent(state)}`)
}

export type ModelPriceTier = {
  above: number
  input: number
  output: number
  cacheRead?: number
  cacheWrite?: number
  cacheWrite1h?: number
}

export type ModelPrice = {
  input: number
  output: number
  cacheRead?: number
  cacheWrite?: number
  cacheWrite1h?: number
  tiers?: ModelPriceTier[]
  peak?: string
  currency?: 'usd' | 'cny'
}

export type RateWindow = {
  days?: number[]
  start: string
  end: string
  rate: number
}

export type ProviderPricingRule = {
  priceRate?: number | null
  rateWindows?: RateWindow[]
}

export type ModelPricingSettingsData = {
  currency: 'usd' | 'cny'
  modelPrices: Record<string, ModelPrice>
  modelSameAs: Record<string, string>
  providerPricing: Record<string, ProviderPricingRule>
  fx: { rate: number; updatedAt: number; stale: boolean; fallback: boolean }
  referenceModels: string[]
}

export function fetchModelPricing(signal?: AbortSignal): Promise<ModelPricingSettingsData> {
  return apiCall<ModelPricingSettingsData>('/api/model-pricing', { signal })
}

export function saveModelPricing(payload: Record<string, unknown>): Promise<ModelPricingSettingsData> {
  return apiCall<ModelPricingSettingsData>('/api/model-pricing', {
    method: 'PUT',
    body: payload,
  })
}

export function refreshFxRate(): Promise<{ fx: ModelPricingSettingsData['fx'] }> {
  return apiCall('/api/model-pricing/fx/refresh', {
    method: 'POST',
    body: {},
  })
}
