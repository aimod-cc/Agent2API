import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, InputGroup, InputGroupAddon, InputGroupInput,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  SegmentedControl, Switch,
  Card, CardHeader, CardContent, Label } from '@ui'
import { SERVER_PAGE_SIZES, TableFooter, readPageSize, writePageSize, type PageSizeChoice } from './table-shell'
import { t } from '../i18n'
import {
  autoConfig, DEFAULT_AUTO, errorMessage, LEVEL_OPTIONS, logCategory, logGroups, persistFilters,
  queryFor, RANGE_OPTIONS, readFilters, shared, toast, VIEW_OPTIONS,
  type AutoConfig, type Filters, type IntervalTask, type LoadOptions, type LogGroup, type LogPage, type LogStats,
} from './logs-model'
import { LogDetail, LogsList } from './logs-list'
import { DateRangeFields } from './date-range-fields'

type PanelHandle = {
  load(options?: LoadOptions): Promise<void>
  render(data?: LogPage | null): void
  showCategory(category: string): Promise<void>
  applyAutoConfig(config: AutoConfig): void
}
let handle: PanelHandle | null = null
let pendingCategory: string | null = null
let lastStatsValue: LogStats = null
let automatic = DEFAULT_AUTO

function applyStats(stats: LogStats): void {
  lastStatsValue = stats
  shared().wbApp?.updateLogsBadge?.(stats)
}

function applyAutoRefresh(task: IntervalTask | null): void {
  automatic = autoConfig(task)
  handle?.applyAutoConfig(automatic)
}

function LogsPanel() {
  const [data, setData] = React.useState<LogPage>({ entries: [], groups: [], total: 0, matched: 0, grouped: 0 })
  const [loaded, setLoaded] = React.useState(false)
  const [filters, setFilters] = React.useState<Filters>(readFilters)
  const [page, setPage] = React.useState(1)
  const [size, setSize] = React.useState(() => Number(readPageSize('logs', SERVER_PAGE_SIZES)))
  const [auto, setAuto] = React.useState(automatic)
  const [paused, setPaused] = React.useState(false)
  const [reading, setReading] = React.useState(false)
  const [pending, setPending] = React.useState(0)
  const [detail, setDetail] = React.useState<LogGroup | null>(null)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState<'clear' | 'export' | null>(null)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  const current = React.useRef({ filters, page, size, data, paused, detail })
  current.current = { filters, page, size, data, paused, detail }
  const snapshotRef = React.useRef<number | undefined>(undefined)
  const seqRef = React.useRef(0)
  const pollingRef = React.useRef(false)
  const busyRef = React.useRef(false)
  const autoSyncedRef = React.useRef(false)
  const restoredRef = React.useRef(false)
  const touchedRef = React.useRef(false)
  const keywordTimerRef = React.useRef<number | undefined>(undefined)
  const scrollRef = React.useRef<number | null>(null)

  const syncAuto = React.useCallback(async () => {
    if (autoSyncedRef.current) return
    try {
      const result = await shared().workbuddyDesktop?.getScheduledTasks()
      if (Array.isArray(result?.tasks)) {
        autoSyncedRef.current = true
        applyAutoRefresh(result.tasks.find(task => task.id === 'logsAutoRefresh') || null)
      }
    } catch { /* 下次切回页面时再读取，当前会话沿用已知刷新间隔。 */ }
  }, [])

  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<void> => {
    void syncAuto()
    if (!restoredRef.current && shared().wbFilterMemory) {
      restoredRef.current = true
      if (!touchedRef.current) {
        const saved = readFilters()
        current.current.filters = saved
        setFilters(saved)
      }
    }
    const state = current.current
    if (options.resetPage || options.latest) {
      state.page = 1
      setPage(1)
      snapshotRef.current = undefined
    }
    if (options.latest) {
      state.paused = false
      setPaused(false)
      setReading(false)
      if (listRef.current) listRef.current.scrollTop = 0
    }
    const seq = ++seqRef.current
    const selectedPage = state.page
    const selectedSize = state.size
    const selectedFilters = state.filters
    if (!options.silent) setLoading(true)
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const locked = !options.latest && !options.resetPage && (selectedPage > 1 || state.paused || state.detail !== null || (listRef.current?.scrollTop || 0) > 20)
      const [result, stats] = await Promise.all([
        api.getLogs(queryFor(selectedFilters, {
          aggregate: selectedFilters.aggregate, offset: (selectedPage - 1) * selectedSize,
          limit: selectedSize, untilId: locked ? snapshotRef.current : undefined,
        })), api.getLogStats(),
      ])
      if (seq !== seqRef.current) return
      if (!result) throw new Error(t('读取日志失败'))
      const totalGroups = Number(result.grouped ?? result.matched ?? result.entries?.length) || 0
      const maxPage = Math.max(1, Math.ceil(totalGroups / selectedSize))
      if (selectedPage > maxPage) {
        current.current.page = maxPage
        setPage(maxPage)
        await loadPanel(options)
        return
      }
      const nowReading = current.current.paused || current.current.detail !== null || (listRef.current?.scrollTop || 0) > 20
      if (options.silent && !locked && nowReading && snapshotRef.current !== undefined) {
        const changed = await api.getLogs(queryFor(current.current.filters, { sinceId: snapshotRef.current, limit: 1 }))
        if (seq === seqRef.current) setPending(Number(changed?.matched) || 0)
        return
      }
      scrollRef.current = options.resetPage || options.latest || !options.silent ? 0 : (listRef.current?.scrollTop || 0)
      current.current.data = result
      setData(result)
      setLoaded(true)
      setError('')
      if (!locked || snapshotRef.current === undefined) {
        snapshotRef.current = Number(result.lastId ?? stats?.lastId) || 0
        setPending(0)
      }
      applyStats(stats)
      if (selectedFilters.category && result.categories && !Object.hasOwn(result.categories, selectedFilters.category)) {
        const next = { ...current.current.filters, category: '' }
        current.current.filters = next
        setFilters(next)
        persistFilters(next)
        await loadPanel({ resetPage: true })
      }
    } catch (failure) {
      if (seq === seqRef.current) setError(errorMessage(failure))
    } finally {
      if (seq === seqRef.current) setLoading(false)
    }
  }, [syncAuto])

  const poll = React.useCallback(async () => {
    if (pollingRef.current || busyRef.current || keywordTimerRef.current !== undefined) return
    pollingRef.current = true
    try {
      const state = current.current
      const locked = state.paused || state.detail !== null || state.page > 1 || (listRef.current?.scrollTop || 0) > 20
      if (locked && snapshotRef.current !== undefined) {
        const api = shared().workbuddyDesktop
        if (!api) return
        const sinceId = snapshotRef.current
        const filtersBefore = state.filters
        const [result, stats] = await Promise.all([
          api.getLogs(queryFor(filtersBefore, { sinceId, limit: 1 })), api.getLogStats(),
        ])
        if (filtersBefore !== current.current.filters || sinceId !== snapshotRef.current) return
        if (stats?.lastId !== undefined && stats.lastId < sinceId) {
          snapshotRef.current = undefined
          await loadPanel({ resetPage: true })
        } else {
          setPending(Number(result?.matched) || 0)
          applyStats(stats)
        }
      } else await loadPanel({ silent: true })
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pollingRef.current = false }
  }, [loadPanel])

  const changeFilters = React.useCallback((patch: Partial<Filters>, debounce = false) => {
    touchedRef.current = true
    const next = { ...current.current.filters, ...patch }
    current.current.filters = next
    setFilters(next)
    if (keywordTimerRef.current !== undefined) window.clearTimeout(keywordTimerRef.current)
    ++seqRef.current
    setLoading(true)
    const apply = () => {
      keywordTimerRef.current = undefined
      persistFilters(next)
      setPending(0)
      void loadPanel({ resetPage: true })
    }
    if (debounce) keywordTimerRef.current = window.setTimeout(apply, 300)
    else apply()
  }, [loadPanel])

  React.useEffect(() => {
    handle = {
      load: loadPanel,
      render: result => {
        if (!result) return
        current.current.data = result
        setData(result)
        setLoaded(true)
      },
      showCategory: async category => {
        changeFilters({ category, view: 'important' })
        shared().wbApp?.showPage?.('logs', { persist: true })
      },
      applyAutoConfig: config => { autoSyncedRef.current = true; setAuto(config) },
    }
    if (pendingCategory !== null) {
      const category = pendingCategory
      pendingCategory = null
      void handle.showCategory(category)
    } else void loadPanel({ silent: true })
    return () => {
      handle = null
      ++seqRef.current
      if (keywordTimerRef.current !== undefined) window.clearTimeout(keywordTimerRef.current)
    }
  }, [changeFilters, loadPanel])

  React.useEffect(() => {
    if (!auto.enabled) return
    const timer = window.setInterval(() => {
      if (!document.hidden && shared().wbApp?.currentPage === 'logs') void poll()
    }, auto.ms)
    return () => window.clearInterval(timer)
  }, [auto, poll])

  React.useLayoutEffect(() => {
    if (scrollRef.current === null) return
    if (listRef.current) listRef.current.scrollTop = scrollRef.current
    scrollRef.current = null
  })

  const groups = logGroups(data)
  const totalGroups = Number(data.grouped ?? data.matched ?? groups.length) || 0
  const matched = Number(data.matched) || 0
  const pageCount = Math.max(1, Math.ceil(totalGroups / size))
  const badgeText = t('{n} 条', { n: matched })
  React.useEffect(() => { shared().wbApp?.renderTopbarStatus?.() }, [badgeText])

  function gotoPage(target: number): void {
    current.current.page = Math.max(1, Math.min(target, pageCount))
    setPage(current.current.page)
    setReading(false)
    if (listRef.current) listRef.current.scrollTop = 0
    void loadPanel()
  }

  function changeSize(choice: PageSizeChoice): void {
    const next = Number(choice)
    const first = (current.current.page - 1) * current.current.size
    current.current.size = next
    current.current.page = Math.floor(first / next) + 1
    setSize(next)
    setPage(current.current.page)
    writePageSize('logs', next)
    void loadPanel()
  }

  async function runAction(action: 'clear' | 'export', run: () => Promise<void>): Promise<void> {
    if (busyRef.current) return
    busyRef.current = true
    setBusy(action)
    try { await run() } catch (failure) { toast(t('操作失败：{error}', { error: errorMessage(failure) }), 'err') }
    finally { busyRef.current = false; setBusy(null) }
  }

  async function clearLogs(): Promise<void> {
    if (busyRef.current || loading || error || !matched) return
    const query = queryFor(current.current.filters, { untilId: snapshotRef.current })
    const confirmed = await shared().wbConfirm?.ask?.({
      title: t('清空运行日志'),
      html: t('确定清空当前筛选出的 <strong>{count}</strong> 条日志？清空后无法恢复。', { count: matched }),
      okText: t('清空'), okClass: 'danger',
    })
    if (!confirmed) return
    await runAction('clear', async () => {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      await api.clearLogs(query)
      await loadPanel({ resetPage: true })
      toast(t('运行日志已清空'), 'ok')
    })
  }

  async function exportLogs(): Promise<void> {
    await runAction('export', async () => {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const result = await api.exportLogs(queryFor(current.current.filters, { untilId: snapshotRef.current }))
      if (!result || result.canceled) return
      if (result.count === 0) { toast(t('暂无日志可导出'), 'err'); return }
      toast(t('✅ 已导出 {count} 条日志到 {file}', { count: String(result.count), file: String(result.file) }), 'ok')
    })
  }

  const held = paused || reading || page > 1 || detail !== null
  const levelLabel = LEVEL_OPTIONS.find(option => option.value === filters.level)?.label || t('全部级别')
  const counts = data.byLevel || {}
  const empty = !loaded && !error ? t('正在加载日志…') : Number(data.total) ? t('没有符合筛选条件的日志') : t('暂无日志')

  return (
    <Card render={<section />} className='panel' id='logs-panel-events'>
      <CardHeader className='panel-head'>
        <h2>{t('系统事件')}</h2><Badge id='logs-badge' variant='outline' data-tone=''>{loaded ? badgeText : '—'}</Badge>
        <div className='head-actions'>
          <Button variant='ghost' disabled={busy !== null} onClick={() => {
            const next = !current.current.paused
            current.current.paused = next
            setPaused(next)
            if (!next && !reading && page === 1 && !detail) void loadPanel({ latest: true })
          }}>{paused ? t('恢复刷新') : t('暂停刷新')}</Button>
          <Button id='btn-logs-export' variant='outline' disabled={busy !== null || loading || Boolean(error)} onClick={() => void exportLogs()}>
            {busy === 'export' ? t('导出中…') : t('导出当前筛选')}</Button>
          <Button id='btn-logs-clear' variant='destructive' disabled={busy !== null || loading || Boolean(error) || !matched} onClick={() => void clearLogs()}>
            {busy === 'clear' ? t('清空中…') : t('清空当前筛选')}</Button>
        </div>
      </CardHeader>
      <CardContent className='panel-body'>
        <div className='event-log-toolbar'>
          <SegmentedControl options={VIEW_OPTIONS} value={filters.view} onValueChange={view => changeFilters({ view })} aria-label={t('日志视图')} />
          <span className='event-log-status'>{held ? t('阅读模式，列表保持不动') : auto.enabled ? t('自动刷新') : t('自动刷新已关闭')}</span>
          <Label className='event-log-aggregate'><Switch checked={filters.aggregate} onCheckedChange={aggregate => changeFilters({ aggregate })} aria-label={t('折叠重复日志')} />{t('折叠重复日志')}</Label>
        </div>
        <div className='log-filters'>
          <SegmentedControl options={RANGE_OPTIONS} value={filters.range} onValueChange={range => changeFilters({ range })} aria-label={t('事件日志时间范围')} className='shrink-0' />
          {filters.range === 'custom' ? <DateRangeFields dates={filters.dates}
            onChange={dates => changeFilters({ dates })} onClear={() => changeFilters({ range: 'all' })} /> : null}
          {filters.view !== 'debug' ? <Select value={filters.level} onValueChange={value => changeFilters({ level: String(value ?? '') })}>
            <SelectTrigger className='min-w-[120px]' aria-label={t('按最低级别筛选')}><SelectValue>{levelLabel}</SelectValue></SelectTrigger>
            <SelectContent>{LEVEL_OPTIONS.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select> : null}
          <Select value={filters.category} onValueChange={value => changeFilters({ category: String(value ?? '') })}>
            <SelectTrigger className='min-w-[120px]' aria-label={t('按分类筛选')}><SelectValue>{filters.category ? logCategory(filters.category, t) : t('全部分类')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value=''>{t('全部分类')}</SelectItem>{Object.keys(data.categories || {}).map(category =>
              <SelectItem key={category} value={category}>{logCategory(category, t)}</SelectItem>)}</SelectContent>
          </Select>
          <InputGroup className='w-auto flex-auto'><InputGroupInput type='search' placeholder={t('搜索消息、提供商或账号…')}
            aria-label={t('搜索消息、提供商或账号')} autoComplete='off' value={filters.keyword}
            onChange={event => changeFilters({ keyword: event.currentTarget.value }, true)} /><InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon></InputGroup>
        </div>
        <div className='event-log-overview'>
          <span>{t('显示 {groups} 组 · 原始记录 {entries} 条', { groups: totalGroups, entries: matched })}</span>
          <span className='event-log-errors'>{t('错误 {n}', { n: counts.error || 0 })}</span>
          <span className='event-log-warnings'>{t('警告 {n}', { n: counts.warn || 0 })}</span>
          {filters.view !== 'important' ? <span>{t('调试 {n}', { n: counts.debug || 0 })}</span> : null}
        </div>
        {filters.view === 'debug' && !data.debugEnabled ? <div className='event-log-note'>{t('调试采集未开启，仅显示已保留的记录。可在设置中开启调试模式。')}</div> : null}
        {error ? <div className='event-log-error' role='status'>{error}<Button variant='ghost' size='xs' onClick={() => void loadPanel()}>{t('重试')}</Button></div> : null}
        {pending > 0 || (held && !paused) ? <div className='event-log-new' role='status'>
          <span>{pending ? t('有 {n} 条新日志', { n: pending }) : t('阅读模式，列表保持不动')}</span>
          <Button variant='outline' size='sm' disabled={loading} onClick={() => void loadPanel({ latest: true })}>{t('查看最新')}</Button>
        </div> : null}
        <LogsList groups={groups} empty={empty} loading={loading} listRef={listRef}
          onScroll={() => setReading((listRef.current?.scrollTop || 0) > 20)} onDetail={group => { current.current.detail = group; setDetail(group) }} />
      </CardContent>
      <TableFooter total={totalGroups} page={page} pageCount={pageCount} size={size} sizes={SERVER_PAGE_SIZES}
        disabled={loading || busy !== null} range={{ start: groups.length ? (page - 1) * size + 1 : 0, end: (page - 1) * size + groups.length }}
        leading={<span className='event-log-footer-note'>{t('分页按折叠后的组数计算')}</span>} onSizeChange={changeSize} onPageChange={gotoPage} />
      <LogDetail group={detail} onClose={() => { current.current.detail = null; setDetail(null) }} />
    </Card>
  )
}

async function load(options: LoadOptions = {}): Promise<void> { await handle?.load(options) }
function render(data?: LogPage | null): void { handle?.render(data) }
function lastStats(): LogStats { return lastStatsValue }
async function showCategory(category: string): Promise<void> {
  if (!handle) { pendingCategory = category; return }
  await handle.showCategory(category)
}

function mount(): void {
  const section = document.querySelector<HTMLElement>('.page[data-page="logs"]')
  if (!section) return
  section.replaceChildren()
  createRoot(section).render(<LogsPanel />)
}
mountOnPage('logs', '.page[data-page="logs"]', mount)

declare global {
  interface Window {
    wbLogsPanel?: {
      load(options?: LoadOptions): Promise<void>
      render(data?: LogPage | null): void
      lastStats(): LogStats
      applyAutoRefresh(task: IntervalTask | null): void
      showCategory(category: string): Promise<void>
    }
  }
}
window.wbLogsPanel = { load, render, lastStats, applyAutoRefresh, showCategory }
