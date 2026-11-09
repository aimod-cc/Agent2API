import { t } from '../i18n'
import { logQueryString, type LogPage, type LogQuery, type LogView } from '../../../ui-shared/logs'
import { dateRangeQuery, readCustomRange, saveCustomRange, type CustomDates } from '../../../ui-shared/date-range'

export * from '../../../ui-shared/logs'
export type Filters = { view: LogView; level: string; category: string; keyword: string; range: string; aggregate: boolean; dates: CustomDates }
export type IntervalTask = { id?: string; enabled?: boolean; interval?: number; unit?: string }
export type LogStats = { lastId?: number; total?: number } | null
export type LoadOptions = { silent?: boolean; resetPage?: boolean; latest?: boolean; manual?: boolean }

const FILTERS_KEY = 'workbuddy-desktop-logs-filters'
const RANGE_KEY = 'workbuddy-desktop-logs-range'
const CUSTOM_RANGE_KEY = 'workbuddy-desktop-logs-custom-range'
const VIEW_KEY = 'workbuddy-desktop-logs-view'
const AGGREGATE_KEY = 'workbuddy-desktop-logs-aggregate'
export const RANGE_OPTIONS = [
  { value: 'today', label: t('今天') }, { value: 'yesterday', label: t('昨天') }, { value: '7', label: t('7 天') },
  { value: '30', label: t('30 天') }, { value: 'month', label: t('本月') }, { value: 'all', label: t('全部') },
  { value: 'custom', label: t('自定义') },
]
export const VIEW_OPTIONS = [
  { value: 'important' as LogView, label: t('重要事件') },
  { value: 'all' as LogView, label: t('全部事件') },
  { value: 'debug' as LogView, label: t('调试详情') },
]
export const LEVEL_OPTIONS = [
  { value: '', label: t('全部级别') }, { value: 'info', label: t('信息及以上') },
  { value: 'warn', label: t('警告及以上') }, { value: 'error', label: t('仅错误') },
]

type Shared = {
  workbuddyDesktop?: {
    getLogs(query: string): Promise<LogPage | null | undefined>
    getLogStats(): Promise<LogStats>
    clearLogs(query: string): Promise<unknown>
    exportLogs(query?: string): Promise<{ canceled?: boolean; count?: number; file?: string } | null | undefined>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    updateLogsBadge?: (stats: LogStats) => void
    renderTopbarStatus?: () => void
    showPage?: (page: string, options?: { persist?: boolean }) => void
    readonly currentPage?: string
  }
  wbConfirm?: { ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean> }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
  copyToClipboard?: (text: string) => Promise<boolean>
}

export function shared(): Shared { return window as unknown as Shared }
export function toast(message: string, kind?: 'err' | 'ok'): void { shared().wbApp?.toast?.(message, kind) }
export function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error) }

export function readFilters(): Filters {
  const saved = shared().wbFilterMemory?.load(FILTERS_KEY, { level: '', category: '', keyword: '' })
  let view: LogView = 'important'
  let range = 'all'
  let aggregate = true
  try {
    const storedView = localStorage.getItem(VIEW_KEY)
    if (storedView === 'all' || storedView === 'debug') view = storedView
    const storedRange = localStorage.getItem(RANGE_KEY)
    if (RANGE_OPTIONS.some(option => option.value === storedRange)) range = storedRange!
    aggregate = localStorage.getItem(AGGREGATE_KEY) !== '0'
  } catch { /* 存储不可用不影响当前会话。 */ }
  return {
    view, range, aggregate, dates: readCustomRange(CUSTOM_RANGE_KEY),
    level: LEVEL_OPTIONS.some(option => option.value === saved?.level) ? saved!.level : '',
    category: saved?.category || '', keyword: saved?.keyword || '',
  }
}

export function persistFilters(filters: Filters): void {
  saveCustomRange(CUSTOM_RANGE_KEY, filters.dates)
  shared().wbFilterMemory?.save(FILTERS_KEY, { level: filters.level, category: filters.category, keyword: filters.keyword.trim() })
  try {
    localStorage.setItem(VIEW_KEY, filters.view)
    localStorage.setItem(RANGE_KEY, filters.range)
    localStorage.setItem(AGGREGATE_KEY, filters.aggregate ? '1' : '0')
  } catch { /* 存储不可用不影响当前会话。 */ }
}

export function queryFor(filters: Filters, extra: Partial<LogQuery> = {}): string {
  return logQueryString({ view: filters.view, level: filters.view === 'debug' ? undefined : filters.level,
    category: filters.category, keyword: filters.keyword.trim(), ...dateRangeQuery(filters.range, filters.dates), ...extra })
}
