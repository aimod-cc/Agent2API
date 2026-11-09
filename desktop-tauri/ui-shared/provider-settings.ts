import { documentText, type PluginDocumentText } from './provider-docs'

export type ProviderSettingValue = string | number | boolean | null
export type ProviderSettingField = {
  key: string
  label: PluginDocumentText
  description?: PluginDocumentText | null
  type: 'string' | 'number' | 'boolean' | 'select'
  default: ProviderSettingValue
  minimum?: number | null
  maximum?: number | null
  max_length?: number | null
  options: Array<{ value: string; label: PluginDocumentText }>
}
export type ProviderSettingsRow = {
  provider: string
  label: string
  group?: {
    id: string
    label?: PluginDocumentText | null
    providers: Array<{ id: string; label: string; enabled?: boolean }>
  }
  error?: string
  schema?: {
    title: PluginDocumentText
    description?: PluginDocumentText | null
    storage_provider?: string | null
    fields: ProviderSettingField[]
  }
  values?: Record<string, ProviderSettingValue>
  effective?: Record<string, ProviderSettingValue>
  invalid?: Record<string, boolean>
  overrides?: Record<string, { source: string; value: ProviderSettingValue }>
  revision?: string
  saved?: boolean
}
export type ProviderSettingsRequest = <T>(method: 'GET' | 'PATCH', path: string, body?: unknown) => Promise<T>
export type ProviderSettingsTranslate = (key: string, params?: Record<string, string | number>) => string

export function providerSettingsKey(row: ProviderSettingsRow): string {
  return row.group?.id || row.schema?.storage_provider || row.provider
}

export function providerSettingsMembers(row: ProviderSettingsRow): Array<{ id: string; label: string; enabled?: boolean }> {
  return row.group?.providers?.length ? row.group.providers : [{ id: row.provider, label: row.label }]
}

export function providerSettingsLabel(row: ProviderSettingsRow, locale: string, t: ProviderSettingsTranslate): string {
  return documentText(row.group?.label, locale) || providerSettingsMembers(row).map(provider => t(provider.label)).join(' / ')
}

export function matchesProviderSettings(row: ProviderSettingsRow, provider: string): boolean {
  return providerSettingsKey(row) === provider || row.provider === provider
    || providerSettingsMembers(row).some(member => member.id === provider)
}
