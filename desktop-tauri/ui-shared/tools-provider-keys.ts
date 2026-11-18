export type ProviderKeyStatus = {
  status: 'ready' | 'quota_exhausted' | 'rate_limited' | 'invalid_provider_key'
  scope: 'all' | 'search' | 'fetch' | null
  reset_at: number | null
  message: string | null
}

export type ProviderKeyRow = {
  id: string
  priority: number
  key_hint: string
  updated_at: number | null
  search: ProviderKeyStatus
  fetch: ProviderKeyStatus
}

export type SearchProviderRow = {
  provider: string
  display_name: string
  auth_type?: 'api_key' | 'account'
  supports_search?: boolean
  supports_fetch?: boolean
  available?: boolean
  configured: boolean
  enabled: boolean
  key_hint: string | null
  updated_at: number | null
  usable?: boolean
  fetch_usable?: boolean
  active_key_id?: string | null
  active_fetch_key_id?: string | null
  available_key_count?: number
  available_fetch_key_count?: number
  max_keys?: number
  keys?: ProviderKeyRow[]
  account_id?: string
  account_name?: string
  accounts?: Array<{ id: string; name: string; enabled: boolean }>
  max_query_chars?: number | null
  unavailable_reason?: string | null
}

export type ProviderKeyDraft = {
  localId: string
  id?: string
  apiKey: string
  row?: ProviderKeyRow
}

export type ProviderKeyUpdate = { id?: string; api_key?: string }

export function providerKeyDrafts(provider: SearchProviderRow): ProviderKeyDraft[] {
  return (provider.keys || []).map(row => ({ localId: row.id, id: row.id, apiKey: '', row }))
}

export function providerKeysPayload(drafts: ProviderKeyDraft[]): ProviderKeyUpdate[] {
  return drafts.map(draft => ({ id: draft.id, api_key: draft.apiKey.trim() || undefined }))
}

export function providerReady(provider: SearchProviderRow, operation: 'search' | 'fetch' = 'search'): boolean {
  return provider.enabled && provider.configured && (operation === 'search' ? provider.usable : provider.fetch_usable) !== false
}
