import * as React from 'react'
import { Button, Popover, PopoverContent, PopoverTrigger, cn } from '@ui'
import { SearchableSelect } from '../../../../ui-shared/searchable-select'
import { type CustomDates } from '../../../../ui-shared/date-range'
export { DateRangeFields as LogDateFilters } from '../date-range-fields'
import { t } from '../../i18n'
import { requestApiKeyLabel, requestClientLabel, type RequestIdentity } from '../../../../ui-shared/model-api-key'

export type SourceFilters = { client: string; apiKey: string }
export type SourceOption = { value: string; label: string }
export type ProviderOption = { value: string; label: string }
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

function FilterSlidersIcon() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' aria-hidden='true'>
      <path d='M2.5 4h11M4.5 8h7M6.5 12h3' />
    </svg>
  )
}

export type AdvancedFiltersState = {
  provider: string
  model: string
  client: string
  apiKey: string
}

export function AdvancedFiltersPopover({
  filters,
  providers,
  models,
  clients,
  apiKeys,
  onChange,
  onClearAll,
}: {
  filters: AdvancedFiltersState
  providers: ProviderOption[]
  models: string[]
  clients: SourceOption[]
  apiKeys: SourceOption[]
  onChange: (patch: Partial<AdvancedFiltersState>) => void
  onClearAll: () => void
}) {
  const clientOptions = React.useMemo(() => [
    { value: ALL_CLIENTS, label: t('全部来源') },
    ...clients.map(item => ({
      value: clientFilterValue(item.value),
      label: requestClientLabel({ client: item.value, clientLabel: item.label }, t),
    })),
  ], [clients])

  const keyOptions = React.useMemo(() => [
    { value: '', label: t('全部 API Key') },
    ...apiKeys.map(item => ({ ...item, label: t(item.label) })),
  ], [apiKeys])

  const ensureCurrent = (list: SourceOption[], value: string, fallback: string) =>
    list.some(item => item.value === value) ? list : [{ value, label: fallback }, ...list]

  const finalClients = ensureCurrent(
    clientOptions,
    filters.client,
    filters.client === UNKNOWN_CLIENT ? t('未记录客户端') : filters.client,
  )
  const finalKeys = ensureCurrent(keyOptions, filters.apiKey, filters.apiKey)
  const finalProviders = [{ value: '', label: t('全部提供商') }, ...providers]
  const finalModels = [{ value: '', label: t('全部模型') }, ...models.map(value => ({ value, label: value }))]

  const activeCount = [
    filters.client !== ALL_CLIENTS,
    Boolean(filters.provider),
    Boolean(filters.model),
    Boolean(filters.apiKey),
  ].filter(Boolean).length

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button
            variant='outline'
            size='default'
            type='button'
            className={cn(
              'req-filter-trigger',
              activeCount > 0 && 'border-primary-bd bg-primary-soft text-primary-fg',
            )}
            aria-label={t('筛选条件')}
          >
            <FilterSlidersIcon />
            <span>{activeCount > 0 ? t('筛选 ({n})', { n: activeCount }) : t('筛选')}</span>
          </Button>
        }
      />
      <PopoverContent align='start' side='bottom' className='w-[310px] p-3'>
        <div className='mb-2.5 flex items-center justify-between'>
          <span className='text-xs font-semibold text-foreground'>{t('筛选条件')}</span>
          {activeCount > 0 ? (
            <Button variant='ghost' size='xs' type='button' onClick={onClearAll}>{t('清空')}</Button>
          ) : null}
        </div>
        <div className='flex flex-col gap-2.5'>
          <div className='flex flex-col gap-1'>
            <span className='text-[11px] text-muted-foreground'>{t('来源软件')}</span>
            <SearchableSelect
              id='req-client'
              value={filters.client}
              options={finalClients}
              onValueChange={client => onChange({ client })}
              searchPlaceholder={t('搜索来源')}
              className='w-full'
              aria-label={t('按客户端筛选')}
            />
          </div>
          <div className='flex flex-col gap-1'>
            <span className='text-[11px] text-muted-foreground'>{t('提供商')}</span>
            <SearchableSelect
              id='req-provider'
              value={filters.provider}
              options={finalProviders}
              onValueChange={provider => onChange({ provider })}
              searchPlaceholder={t('搜索提供商')}
              className='w-full'
              aria-label={t('按提供商筛选')}
            />
          </div>
          <div className='flex flex-col gap-1'>
            <span className='text-[11px] text-muted-foreground'>{t('模型')}</span>
            <SearchableSelect
              id='req-model'
              value={filters.model}
              options={finalModels}
              onValueChange={model => onChange({ model })}
              searchPlaceholder={t('搜索模型')}
              className='w-full'
              aria-label={t('按模型筛选')}
            />
          </div>
          <div className='flex flex-col gap-1'>
            <span className='text-[11px] text-muted-foreground'>API Key</span>
            <SearchableSelect
              id='req-apiKey'
              value={filters.apiKey}
              options={finalKeys}
              onValueChange={apiKey => onChange({ apiKey })}
              searchPlaceholder={t('搜索 API Key')}
              className='w-full'
              aria-label={t('按 API Key 筛选')}
            />
          </div>
        </div>
      </PopoverContent>
    </Popover>
  )
}

export function apiKeyRequestLabel(entry: RequestIdentity): string {
  return requestApiKeyLabel(entry, t)
}

export function clientRequestLabel(entry: RequestIdentity): string {
  return requestClientLabel(entry, t)
}
