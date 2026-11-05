import * as React from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { type CustomDates } from '../../../../ui-shared/date-range'
export { DateRangeFields as LogDateFilters } from '../date-range-fields'
import { t } from '../../i18n'
import { requestApiKeyLabel, requestClientLabel, type RequestIdentity } from '../../../../ui-shared/model-api-key'

export type SourceFilters = { client: string; apiKey: string }
export type SourceOption = { value: string; label: string }
export type LogDates = CustomDates & { productionOnly?: boolean; completedOnly?: boolean }
export type LogDrilldown = { client?: string; apiKey?: string; model?: string; startDate?: string; endDate?: string }

export const ALL_CLIENTS = '__all_clients__'
export const UNKNOWN_CLIENT = '__unknown_client__'

export function clientFilterValue(value: string): string {
  return value === '' ? UNKNOWN_CLIENT : value
}

export function sourceParams(params: URLSearchParams, filters: SourceFilters, dates: LogDates | null): void {
  if (filters.client !== ALL_CLIENTS) params.set('client', filters.client === UNKNOWN_CLIENT ? '' : filters.client)
  if (filters.apiKey) params.set('apiKey', filters.apiKey)
  if (dates) {
    params.set('startDate', dates.start)
    params.set('endDate', dates.end)
    params.delete('start')
    params.delete('end')
    if (dates.productionOnly) params.set('productionOnly', '1')
    if (dates.completedOnly) params.set('completedOnly', '1')
  }
}

export function SourceLogFilters({ filters, clients, apiKeys, onChange }: {
  filters: SourceFilters; clients: SourceOption[]; apiKeys: SourceOption[]
  onChange: (patch: Partial<SourceFilters>) => void
}) {
  const clientOptions = React.useMemo(() => [
    { value: ALL_CLIENTS, label: t('全部客户端') },
    ...clients.map(item => ({ value: clientFilterValue(item.value), label: requestClientLabel({ client: item.value, clientLabel: item.label }, t) })),
  ], [clients])
  const keyOptions = React.useMemo(() => [{ value: '', label: t('全部 API Key') },
    ...apiKeys.map(item => ({ ...item, label: t(item.label) })),
  ], [apiKeys])
  const options = (list: SourceOption[], value: string) => list.some(item => item.value === value)
    ? list : [{ value, label: value === UNKNOWN_CLIENT ? t('未记录客户端') : value }, ...list]
  return (
    <>
      {(['client', 'apiKey'] as const).map(key => {
        const list = options(key === 'client' ? clientOptions : keyOptions, filters[key])
        return <Select value={filters[key]} key={key} onValueChange={value => onChange({ [key]: String(value ?? '') })}>
          <SelectTrigger id={`req-${key}`} className='min-w-[120px] max-w-[200px]'
            aria-label={key === 'client' ? t('按客户端筛选') : t('按 API Key 筛选')}>
            <SelectValue>{list.find(item => item.value === filters[key])?.label}</SelectValue>
          </SelectTrigger>
          <SelectContent>{list.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
        </Select>
      })}
    </>
  )
}

export function apiKeyRequestLabel(entry: RequestIdentity): string {
  return requestApiKeyLabel(entry, t)
}

export function clientRequestLabel(entry: RequestIdentity): string {
  return requestClientLabel(entry, t)
}
