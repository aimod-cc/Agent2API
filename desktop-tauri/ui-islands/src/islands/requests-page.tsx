import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, SegmentedControl, Toggle,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  type SegmentedControlOption,
  Card, CardHeader, CardContent } from '@ui'
import {
  DEFAULT_PAGE_SIZE,
  SERVER_PAGE_SIZES,
  TableFooter,
  readPageSize,
  writePageSize,
  type PageSizeChoice,
} from './table-shell'
import { t } from '../i18n'
import { DATE_RANGES, dateRangeQuery, readCustomRange, saveCustomRange, validCustomRange } from '../../../ui-shared/date-range'
import { Empty } from '@ui'
import { ALL_CLIENTS, clientFilterValue, LogDateFilters, SourceLogFilters, sourceParams,
  type LogDates, type LogDrilldown, type SourceOption } from './requests/source-filters'
import { RequestCell, headRow, isOk, isRunning, uniqueRowKeys, providerLabelOf, type RequestEntry, type VisibleColumn } from './requests/request-cells'
import { getStore as getAccountsStore, subscribe as subscribeAccounts } from './accounts-store'
import { phaseOf, phaseLabelOf, phaseElapsedText, phaseBadgeHtml, phaseElapsedLineHtml } from './requests/request-phase'

type RequestQueryResult = {
  entries?: RequestEntry[]; total?: number; matched?: number

  running?: number
}

type ProviderOption = { value: string; label: string }
type FilterOptionsResult = {
  providers?: { id?: string; label?: string }[]; models?: string[]
  clients?: SourceOption[]; apiKeys?: SourceOption[]
}
type IntervalTask = { id?: string; enabled?: boolean; interval?: number; unit?: string }

type RequestsBridge = {

  getStatsRequests(query: string): Promise<RequestQueryResult | null | undefined>
  getStatsRequestFilters(): Promise<FilterOptionsResult | null | undefined>
  getScheduledTasks(): Promise<{ tasks?: IntervalTask[] } | null | undefined>
}

type Align = 'left' | 'center' | 'right'
type Column = {
  key: string
  label: string
  sel: string
  track: string
  align?: Align
  /** 上一版的默认对齐，只给列设置的归一化分辨「旧存盘里那一档是用户挑的还是旧默认」 */
  legacyAlign?: Align
}

type ColSettingsApi = {
  register(spec: {
    id: string
    label?: string
    columns: { key: string; label?: string; align?: Align; legacyAlign?: Align }[]
    mount?: string | (() => Element | null)
    onChange?: (config: unknown) => void
  }): { apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[] }
}

type SharedWindow = {
  workbuddyDesktop?: RequestsBridge
  wbApp?: { readonly currentPage?: string; showPage?: (page: string) => void }

  wbProviders?: { labelOf?: (id: string) => string }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
  wbRequestHover?: {
    bind?: (options?: { host?: HTMLElement | null; entryOf?: (tag: HTMLElement) => unknown }) => void

    hasProcessFacts?: (entry: unknown) => boolean
    beforeListRedraw?: () => void
    afterListRedraw?: () => void
  }
  wbRequestDetail?: { open?: (id: string, row: RequestEntry | null) => unknown }
  wbRequestClearModal?: { open?: () => unknown }
  wbColSettings?: ColSettingsApi
  wbTableColumns?: { repaint?: (id: string) => void }

  wbUnits?: { formatTokens?: (value: unknown) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

const COLUMNS: Column[] = [
  { key: 'time', label: t('时间'), sel: '.req-time', track: '92px' },
  { key: 'target', label: t('提供商 / 账号'), sel: '.req-target', track: 'minmax(0, 1.1fr)' },
  { key: 'retry', label: t('重试'), sel: '.req-retry', track: '52px' },
  { key: 'status', label: t('状态'), sel: '.req-status', track: '96px' },
  { key: 'model', label: t('模型'), sel: '.req-model', track: 'minmax(0, 1.3fr)' },
  { key: 'client', label: t('客户端'), sel: '.req-client', track: '110px' },
  { key: 'apiKey', label: t('API Key'), sel: '.req-api-key', track: '140px' },
  { key: 'dur', label: t('用时'), sel: '.req-dur', track: '96px', align: 'right' },
  { key: 'speed', label: t('速度'), sel: '.req-speed', track: '96px', align: 'center', legacyAlign: 'right' },
  { key: 'credit', label: t('积分'), sel: '.req-credit', track: '72px', align: 'center' },
  { key: 'usage', label: t('用量'), sel: '.req-usage', track: 'minmax(0, 1.6fr)' },
  { key: 'error', label: t('错误'), sel: '.req-error-cell', track: 'minmax(0, 1.2fr)' },
  { key: 'detail', label: t('详情'), sel: '.req-detail', track: '60px', align: 'right' },
]

const DEFAULT_AUTO_REFRESH_MS = 1_000

const PAGE_SIZES = SERVER_PAGE_SIZES

function numericSize(choice: PageSizeChoice): number {
  return typeof choice === 'number' && choice > 0 ? choice : DEFAULT_PAGE_SIZE
}

const RANGE_KEY = 'workbuddy-desktop-logs-requests-range'
const CUSTOM_RANGE_KEY = 'workbuddy-desktop-requests-custom-range'

const FILTERS_KEY = 'workbuddy-desktop-requests-filters'

const FILTER_OPTIONS_TTL_MS = 30_000

const RANGES: readonly string[] = DATE_RANGES
const DEFAULT_RANGE = 'all'

const RANGE_LABEL: Record<string, string> = {
  today: t('今天'), yesterday: t('昨天'), 7: t('近 7 天'), 30: t('近 30 天'),
  month: t('本月'), all: t('全部'), custom: t('自定义区间'),
}
const RANGE_OPTION_LABEL: Record<string, string> = {
  today: t('今天'), yesterday: t('昨天'), 7: t('7 天'), 30: t('30 天'),
  month: t('本月'), all: t('全部'), custom: t('自定义'),
}

const RANGE_OPTIONS: readonly SegmentedControlOption<string>[] = RANGES.map(value => ({
  value, label: RANGE_OPTION_LABEL[value],
}))

const STATUS_OPTIONS: readonly { value: string; label: string }[] = [
  { value: '', label: t('全部状态') },
  { value: 'ok', label: t('成功') },
  { value: 'error', label: t('失败') },
]
const ALL_STATUS_LABEL = t('全部状态')
const ALL_PROVIDER_LABEL = t('全部提供商')
const ALL_MODEL_LABEL = t('全部模型')

type Filters = { status: string; provider: string; model: string; client: string; apiKey: string }
const DEFAULT_FILTERS: Filters = { status: '', provider: '', model: '', client: ALL_CLIENTS, apiKey: '' }

const initialRange = readRange()
const current = {
  filters: { ...DEFAULT_FILTERS } as Filters,
  range: initialRange,
  runningOnly: false,
  dates: (initialRange === 'custom' ? readCustomRange(CUSTOM_RANGE_KEY) : null) as LogDates | null,
  offset: 0,

  size: numericSize(readPageSize('requests', SERVER_PAGE_SIZES)),
}

let savedFilters: Filters = { ...DEFAULT_FILTERS }

let filtersRestored = false

let filtersTouched = false

let entriesValue: RequestEntry[] = []

let autoRefreshMs = DEFAULT_AUTO_REFRESH_MS

let autoEnabled = true

let autoSynced = false

let polling = false

let seq = 0

let filterOptionsAt = 0

let colHandle: { apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[] } | null = null
let colTried = false

type PanelHandle = {
  load(options?: LoadOptions): Promise<void>
  setAuto(next: { ms: number; enabled: boolean }): void
  setFilterOptions(providers: ProviderOption[], models: string[], clients: SourceOption[], apiKeys: SourceOption[]): void
  openFiltered(query: LogDrilldown): void

  repaint(): void
}
let handle: PanelHandle | null = null

type LoadOptions = { silent?: boolean; resetPage?: boolean }

function readRange(): string {
  try {
    const saved = localStorage.getItem(RANGE_KEY)
    return saved !== null && RANGES.includes(saved) ? saved : DEFAULT_RANGE
  } catch {
    return DEFAULT_RANGE
  }
}

function persistRange(value: string): void {
  try {
    localStorage.setItem(RANGE_KEY, value)
  } catch {
  }
}

function readSavedFilters(): Filters {
  const memory = shared().wbFilterMemory
  if (!memory) return { ...DEFAULT_FILTERS }
  filtersRestored = true
  savedFilters = memory.load(FILTERS_KEY, { ...DEFAULT_FILTERS })
  current.filters = { ...savedFilters }
  return { ...savedFilters }
}

function persistFilters(filters: Filters): void {
  savedFilters = { ...filters }
  shared().wbFilterMemory?.save(FILTERS_KEY, {
    status: filters.status || '',
    provider: filters.provider || '',
    model: filters.model || '',
    client: filters.client || ALL_CLIENTS,
    apiKey: filters.apiKey || '',
  })
}

function filterParams(): URLSearchParams {
  const dates = current.range === 'custom' ? current.dates || readCustomRange(CUSTOM_RANGE_KEY) : { start: '', end: '' }
  const params = new URLSearchParams(dateRangeQuery(current.range, dates))
  const status = current.runningOnly ? 'running' : current.filters.status
  if (status) params.set('status', status)
  if (current.filters.provider) params.set('provider', current.filters.provider)
  if (current.filters.model) params.set('model', current.filters.model)
  sourceParams(params, current.filters, current.dates)
  if (current.runningOnly) params.delete('completedOnly')
  return params
}

function queryParams(): string {
  const params = filterParams()
  params.set('offset', String(current.offset))
  params.set('limit', String(current.size))
  return params.toString()
}

async function refreshFilterOptions(): Promise<void> {
  if (Date.now() - filterOptionsAt < FILTER_OPTIONS_TTL_MS) return
  filterOptionsAt = Date.now()
  try {
    const filters = await shared().workbuddyDesktop?.getStatsRequestFilters()
    const providers: ProviderOption[] = (Array.isArray(filters?.providers) ? filters.providers : [])
      .map(item => ({ value: String(item?.id || ''), label: t(String(item?.label || item?.id || '')) }))
      .filter(item => item.value)
    const models: string[] = (Array.isArray(filters?.models) ? filters.models : [])
      .map(name => String(name || ''))
      .filter(Boolean)
    handle?.setFilterOptions(providers, models, filters?.clients || [], filters?.apiKeys || [])
  } catch (error) {
    console.warn('读取请求日志筛选清单失败，筛选项退化为「全部」:', errorMessage(error))
  }
}

async function syncAutoRefresh(): Promise<void> {
  if (autoSynced) return
  try {
    const list = await shared().workbuddyDesktop?.getScheduledTasks()
    const task = (list?.tasks || []).find(item => item.id === 'requestsAutoRefresh')
    applyAutoRefresh(task || null)
    autoSynced = Array.isArray(list?.tasks)
  } catch (error) {
    console.warn('读取请求日志自动刷新间隔失败，按默认 1 秒:', errorMessage(error))
    applyAutoRefresh(null)
  }
}

function applyAutoRefresh(task: IntervalTask | null): void {
  autoSynced = true
  const interval = Number(task?.interval)
  const unit = task?.unit
  const valid = !!task && typeof task === 'object'
    && Number.isFinite(interval) && interval > 0
    && (unit === 'seconds' || unit === 'minutes')
  if (!valid) {
    autoEnabled = true
    autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
  } else {
    autoEnabled = task.enabled !== false
    autoRefreshMs = unit === 'minutes' ? interval * 60_000 : interval * 1000
  }
  handle?.setAuto({ ms: autoRefreshMs, enabled: autoEnabled })
}

function ensureColumns(): void {
  if (colTried) return
  const api = shared().wbColSettings
  if (!api) return
  colTried = true
  colHandle = api.register({
    id: 'requests',
    label: t('请求日志表'),
    columns: COLUMNS.map(({ key, label, align, legacyAlign }) => ({ key, label, align, legacyAlign })),
    mount: () => document.querySelector('.page[data-page="requests"] .panel-head .head-actions'),
    onChange: () => {
      flushSync(() => handle?.repaint())
      shared().wbTableColumns?.repaint?.('requests')
    },
  })
}

function visibleColumns(): VisibleColumn[] {
  const applied = colHandle ? colHandle.apply(COLUMNS) : COLUMNS
  return applied.map(column => ({ ...column, align: column.align || 'left' }))
}

type PanelData = {
  entries: RequestEntry[]; total: number; matched: number; running: number

  loaded: boolean

  error: string
}

const EMPTY_DATA: PanelData = {
  entries: [], total: 0, matched: 0, running: 0, loaded: false, error: '',
}

function emptyText(data: PanelData): string {
  if (!data.loaded) return t('正在加载请求日志…')
  if (!data.total) return t('暂无请求日志，网关还没有转发过请求')
  return t('没有符合筛选条件的请求')
}

function RequestsPage() {
  const namesHidden = React.useSyncExternalStore(subscribeAccounts, () => getAccountsStore().namesHidden)
  const [data, setData] = React.useState<PanelData>(EMPTY_DATA)
  const [filters, setFilters] = React.useState<Filters>(() => readSavedFilters())
  const [range, setRange] = React.useState<string>(() => current.range)
  const [runningOnly, setRunningOnly] = React.useState(false)
  const [offset, setOffset] = React.useState(0)

  const [pageSize, setPageSize] = React.useState<number>(() => current.size)
  const [auto, setAuto] = React.useState(() => ({ ms: autoRefreshMs, enabled: autoEnabled }))
  const [providerOptions, setProviderOptions] = React.useState<ProviderOption[]>([])
  const [modelOptions, setModelOptions] = React.useState<string[]>([])
  const [clientOptions, setClientOptions] = React.useState<SourceOption[]>([])
  const [apiKeyOptions, setApiKeyOptions] = React.useState<SourceOption[]>([])
  const [dates, setDates] = React.useState<LogDates | null>(() => current.dates)

  const [, setColumnsVersion] = React.useState(0)

  const listRef = React.useRef<HTMLDivElement | null>(null)

  const pendingScrollRef = React.useRef<number | null>(null)

  const afterRedrawRef = React.useRef(false)

  const applyFilters = React.useCallback((patch: Partial<Filters>): Filters => {
    current.filters = { ...current.filters, ...patch }
    setFilters({ ...current.filters })
    return current.filters
  }, [])
  const applyRange = React.useCallback((next: string) => { current.range = next; setRange(next) }, [])
  const applyOffset = React.useCallback((next: number) => { current.offset = next; setOffset(next) }, [])
  const applyRunningOnly = React.useCallback((next: boolean) => { current.runningOnly = next; setRunningOnly(next) }, [])

  const restoreSavedFilters = React.useCallback((): void => {
    if (filtersRestored) return
    const memory = shared().wbFilterMemory
    if (!memory) return
    filtersRestored = true
    if (filtersTouched) return
    applyFilters(readSavedFilters())
  }, [applyFilters])

  const paint = React.useCallback((next: PanelData, entries: RequestEntry[] | null) => {
    shared().wbRequestHover?.beforeListRedraw?.()
    if (entries) entriesValue = entries
    afterRedrawRef.current = true
    setData(next)
  }, [])

  const paintError = React.useCallback((text: string) => {
    shared().wbRequestHover?.beforeListRedraw?.()
    afterRedrawRef.current = true
    setData(prev => ({ ...prev, error: text }))
  }, [])

  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<void> => {
    const { silent = false, resetPage = false } = options
    if (resetPage) applyOffset(0)
    void syncAutoRefresh()
    restoreSavedFilters()
    if (!silent) void refreshFilterOptions()
    const token = ++seq
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const result = await api.getStatsRequests(queryParams())
      if (token !== seq) return
      const next: PanelData = {
        entries: Array.isArray(result?.entries) ? result.entries : [],
        total: Number(result?.total) || 0,
        matched: Number(result?.matched) || 0,
        running: Number(result?.running) || 0,
        loaded: true,
        error: '',
      }
      const lastOffset = Math.max(0, (Math.ceil(next.matched / current.size) - 1) * current.size)
      if (current.offset > lastOffset) {
        applyOffset(lastOffset)
        return loadPanel({ silent })
      }
      if (resetPage) pendingScrollRef.current = 0
      paint(next, next.entries)
    } catch (error) {
      if (!silent) {
        console.warn('读取请求日志失败:', errorMessage(error))
        paintError(t('读取请求日志失败，详见控制台'))
      }
    }
  }, [applyOffset, paint, paintError, restoreSavedFilters])

  React.useEffect(() => {
    handle = {
      load: loadPanel,
      setAuto: next => setAuto(next),
      setFilterOptions: (providers, models, clients, apiKeys) => {
        setProviderOptions(providers); setModelOptions(models); setClientOptions(clients); setApiKeyOptions(apiKeys)
      },
      openFiltered: query => {
        filtersTouched = true
        const next = { ...DEFAULT_FILTERS, model: query.model || '',
          client: query.client === undefined ? ALL_CLIENTS : clientFilterValue(query.client), apiKey: query.apiKey || '' }
        persistFilters(applyFilters(next))
        const custom = validCustomRange({ start: query.startDate, end: query.endDate })
        current.dates = custom ? { ...custom, productionOnly: true, completedOnly: true } : null
        setDates(current.dates)
        const range = current.dates ? 'custom' : 'all'
        applyRange(range)
        persistRange(range)
        if (current.dates) saveCustomRange(CUSTOM_RANGE_KEY, current.dates)
        applyRunningOnly(false)
        shared().wbApp?.showPage?.('requests')
        void loadPanel({ resetPage: true })
      },
      repaint: () => setColumnsVersion(version => version + 1),
    }
    shared().wbRequestHover?.bind?.({
      host: listRef.current,
      entryOf: tag => {
        const key = tag?.dataset?.reqId || ''
        if (!key) return null
        return entriesValue.find(item => String(item?.id || '') === key)
          || entriesValue.find(item => String(item?.ts || '') === key)
          || null
      },
    })
    void loadPanel({ silent: true })
    void refreshFilterOptions()
    return () => { handle = null }
  }, [loadPanel, applyFilters, applyRange, applyRunningOnly])

  React.useLayoutEffect(() => {
    ensureColumns()
    setColumnsVersion(version => version + 1)
  }, [])

  const headSignature = data.error || !data.entries.length
    ? ''
    : visibleColumns().map(column => column.key).join('|')
  React.useLayoutEffect(() => {
    if (!headSignature) return
    shared().wbTableColumns?.repaint?.('requests')
  }, [headSignature])

  React.useEffect(() => {
    if (!auto.enabled || auto.ms <= 0) return
    const timer = window.setInterval(() => {
      if (document.hidden || shared().wbApp?.currentPage !== 'requests') return
      if (polling) return
      polling = true
      void loadPanel({ silent: true }).finally(() => { polling = false })
    }, auto.ms)
    return () => { window.clearInterval(timer) }
  }, [auto, loadPanel])

  React.useLayoutEffect(() => {
    if (afterRedrawRef.current) {
      afterRedrawRef.current = false
      shared().wbRequestHover?.afterListRedraw?.()
    }
    const top = pendingScrollRef.current
    if (top === null) return
    pendingScrollRef.current = null
    const list = listRef.current
    if (list) list.scrollTop = top
  })

  function gotoPage(target: number): void {
    const pageCount = Math.max(1, Math.ceil(data.matched / current.size))
    const next = Math.min(Math.max(1, target), pageCount)
    const nextOffset = (next - 1) * current.size
    if (nextOffset === current.offset) return
    applyOffset(nextOffset)
    pendingScrollRef.current = 0   // 新一页从顶部开始读
    void loadPanel()
  }

  function onPageSizeChange(choice: PageSizeChoice): void {
    const next = numericSize(choice)
    if (next === current.size) return
    current.size = next
    setPageSize(next)
    writePageSize('requests', next)
    const first = current.offset + 1
    applyOffset(Math.max(0, Math.floor((first - 1) / next) * next))
    pendingScrollRef.current = 0
    void loadPanel()
  }

  function onSelectChange(patch: Partial<Filters>): void {
    filtersTouched = true
    persistFilters(applyFilters(patch))
    void loadPanel({ resetPage: true })
  }

  function onRangeChange(next: string): void {
    const value = RANGES.includes(next) ? next : DEFAULT_RANGE
    if (value === current.range) return
    current.dates = value === 'custom' ? readCustomRange(CUSTOM_RANGE_KEY) : null
    setDates(current.dates)
    applyRange(value)
    persistRange(value)
    void loadPanel({ resetPage: true })
  }

  function onRunningOnlyChange(next: boolean): void {
    if (next === current.runningOnly) return
    applyRunningOnly(next)
    void loadPanel({ resetPage: true })
  }

  const columns = visibleColumns()
  const pageCount = Math.max(1, Math.ceil(data.matched / current.size))
  const currentPage = Math.min(pageCount, Math.floor(offset / current.size) + 1)

  const pageRangeStart = data.matched ? offset + 1 : 0
  const pageRangeEnd = data.matched ? offset + data.entries.length : 0
  const badgeText = data.error
    ? '—'
    : data.matched === data.total
      ? t('{n} 条', { n: data.total })
      : t('{matched} / {total} 条', { matched: data.matched, total: data.total })
  const liveText = !runningOnly && data.running > 0 ? t(' · {n} 进行中', { n: data.running }) : ''

  const summaryParts = [dates ? `${dates.start} ～ ${dates.end}` : RANGE_LABEL[range] || t('全部')]
  if (dates?.productionOnly) summaryParts.push(t('生产请求'))
  if (filters.client !== ALL_CLIENTS) summaryParts.push(clientOptions.find(item => clientFilterValue(item.value) === filters.client)?.label || t('未记录客户端'))
  if (filters.apiKey) summaryParts.push(apiKeyOptions.find(item => item.value === filters.apiKey)?.label || filters.apiKey)
  if (runningOnly) summaryParts.push(t('只看进行中'))
  else if (filters.status === 'ok') summaryParts.push(t('只看成功'))
  else if (filters.status === 'error') summaryParts.push(t('只看失败'))
  if (filters.provider) summaryParts.push(providerLabelOf(filters.provider))
  if (filters.model) summaryParts.push(filters.model)

  const providerSelectOptions = React.useMemo(() => {
    if (!filters.provider || providerOptions.some(item => item.value === filters.provider)) return providerOptions
    return [{ value: filters.provider, label: filters.provider }, ...providerOptions]
  }, [filters.provider, providerOptions])
  const modelSelectOptions = React.useMemo(() => {
    if (!filters.model || modelOptions.includes(filters.model)) return modelOptions
    return [filters.model, ...modelOptions]
  }, [filters.model, modelOptions])

  const rowKeys = uniqueRowKeys(data.entries)
  const rows = data.entries.map((entry, index) => (
    <div key={rowKeys[index]} className={`req-row${isRunning(entry) ? ' running' : isOk(entry) ? '' : ' failed'}`}>
      {columns.map(column => <RequestCell key={column.key} entry={entry} column={column} namesHidden={namesHidden} />)}
    </div>
  ))

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('请求日志列表')}</h2>
        {}
        <Badge id='req-badge' variant='outline' data-tone=''>{badgeText + liveText}</Badge>
        <span className='panel-sub'>{t('按时间倒序，全量保留')}</span>
        <div className='head-actions'>
          {}
          <Button id='btn-req-clear' variant='destructive'
            onClick={() => { void shared().wbRequestClearModal?.open?.() }}>{t('清理')}</Button>
        </div>
      </CardHeader>

      <CardContent className='panel-body'>
        <div className='log-filters'>
          {}
          <SegmentedControl options={RANGE_OPTIONS} value={range} onValueChange={onRangeChange}
            aria-label={t('请求日志时间范围')} className='shrink-0' />
          {range === 'custom' && dates ? <LogDateFilters dates={dates} onChange={next => {
            current.dates = next
            setDates(next)
            saveCustomRange(CUSTOM_RANGE_KEY, next)
            void loadPanel({ resetPage: true })
          }} onClear={() => onRangeChange('all')} /> : null}
          {}
          <Select value={filters.status} disabled={runningOnly}
            onValueChange={next => onSelectChange({ status: String(next ?? '') })}>
            <SelectTrigger id='req-status' className='min-w-[120px]'
              title={runningOnly ? t('「仅看进行中」开启时，状态固定为进行中') : t('按请求结果筛选')}
              aria-label={t('按请求结果筛选')}>
              <SelectValue>
                {STATUS_OPTIONS.find(item => item.value === filters.status)?.label ?? ALL_STATUS_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={filters.provider}
            onValueChange={next => onSelectChange({ provider: String(next ?? '') })}>
            <SelectTrigger id='req-provider' className='min-w-[120px]'
              title={t('按提供商筛选（只列日志里出现过的）')} aria-label={t('按提供商筛选')}>
              <SelectValue>
                {filters.provider
                  ? (providerSelectOptions.find(item => item.value === filters.provider)?.label || filters.provider)
                  : ALL_PROVIDER_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{ALL_PROVIDER_LABEL}</SelectItem>
              {providerSelectOptions.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={filters.model}
            onValueChange={next => onSelectChange({ model: String(next ?? '') })}>
            <SelectTrigger id='req-model' className='min-w-[120px]'
              title={t('按模型筛选（只列日志里出现过的；精确匹配模型名）')} aria-label={t('按模型筛选')}>
              <SelectValue>{filters.model || ALL_MODEL_LABEL}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{ALL_MODEL_LABEL}</SelectItem>
              {modelSelectOptions.map(value => (
                <SelectItem key={value} value={value}>{value}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <SourceLogFilters filters={filters} clients={clientOptions} apiKeys={apiKeyOptions} onChange={onSelectChange} />
          {}
          <Toggle id='btn-req-running' variant='outline' size='default' pressed={runningOnly}
            onPressedChange={onRunningOnlyChange}
            title={t('只显示正在转发中的请求（状态列按阶段显示：连接中 / 等待响应 / 响应中 / 重试中，用时列显示已用时）')}>{t('仅看进行中')}</Toggle>
          <div className='spacer' />
          <span className='panel-sub' id='req-summary'>{summaryParts.join(' · ')}</span>
        </div>

        {}
        <div className='log-list' id='req-list' ref={listRef}>
          {data.error
            ? <Empty className='log-empty'>{data.error}</Empty>
            : data.entries.length
              ? [headRow(columns, namesHidden), ...rows]
              : <Empty className='log-empty'>{emptyText(data)}</Empty>}
        </div>
      </CardContent>

      {}
      <TableFooter
        total={data.matched}
        range={{ start: pageRangeStart, end: pageRangeEnd }}
        page={currentPage}
        pageCount={pageCount}
        size={pageSize}
        sizes={PAGE_SIZES}
        onSizeChange={onPageSizeChange}
        onPageChange={gotoPage}
        disabled={!data.loaded && !data.error}
      />
    </Card>
  )
}

async function load(options: LoadOptions = {}): Promise<void> {
  await handle?.load(options)
}

function openFiltered(query: LogDrilldown): void {
  handle?.openFiltered(query)
}

function clearParams(): string {
  return filterParams().toString()
}

function notifyCleared(): Promise<void> {
  filterOptionsAt = 0
  return load({ resetPage: true })
}

const PAGE_SELECTOR = '.page[data-page="requests"]'

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<RequestsPage />)
}

mountOnPage('requests', PAGE_SELECTOR, mount)

declare global {
  interface Window {

    wbRequestsPanel?: {

      load(options?: LoadOptions): Promise<void>
      openFiltered(query: LogDrilldown): void

      applyAutoRefresh(task: IntervalTask | null): void

      visibleColumns(): VisibleColumn[]

      clearParams(): string

      notifyCleared(): Promise<void>
    }

    wbRequestPhase?: {
      labelOf(entry: unknown): string
      badgeHtml(entry: unknown): string
      elapsedLineHtml(entry: unknown): string
      elapsedText(ms: unknown): string
      phaseOf(entry: unknown): string
    }
  }
}

window.wbRequestsPanel = { load, openFiltered, applyAutoRefresh, visibleColumns, clearParams, notifyCleared }
window.wbRequestPhase = {
  labelOf: phaseLabelOf,
  badgeHtml: phaseBadgeHtml,
  elapsedLineHtml: phaseElapsedLineHtml,
  elapsedText: phaseElapsedText,
  phaseOf,
}
