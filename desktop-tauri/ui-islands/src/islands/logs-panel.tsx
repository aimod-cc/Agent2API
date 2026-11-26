import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { Button, Card, CardContent, TooltipProvider } from '@ui'
import { SERVER_PAGE_SIZES, TableFooter, readPageSize, writePageSize, type PageSizeChoice } from './table-shell'
import { t } from '../i18n'
import {
  errorMessage, logGroups, persistFilters,
  queryFor, readFilters, shared, toast,
  type Filters, type IntervalTask, type LoadOptions, type LogGroup, type LogPage, type LogStats,
} from './logs-model'
import { LogDetail, LogsList } from './logs-list'
import { LogsToolbar } from './logs-toolbar'
import { PageAutoRefresh, pageRefreshStore, usePageAutoRefresh } from './page-auto-refresh'
import { requestDirect } from './assistant-api'
import { subscribeScopedLiveRefresh, type ScopedRefreshController } from '../../../ui-shared/page-live-refresh'

type PanelHandle = {
  load(options?: LoadOptions): Promise<void>
  render(data?: LogPage | null): void
  showCategory(category: string): Promise<void>
}
let handle: PanelHandle | null = null
let pendingCategory: string | null = null
let lastStatsValue: LogStats = null

function applyStats(stats: LogStats): void {
  lastStatsValue = stats
  shared().wbApp?.updateLogsBadge?.(stats)
}

function applyAutoRefresh(task: IntervalTask | null): void {
  pageRefreshStore.apply('logsAutoRefresh', task)
}

function LogsPanel() {
  const [data, setData] = React.useState<LogPage>({ entries: [], groups: [], total: 0, matched: 0, grouped: 0 })
  const [loaded, setLoaded] = React.useState(false)
  const [filters, setFilters] = React.useState<Filters>(readFilters)
  const [page, setPage] = React.useState(1)
  const [size, setSize] = React.useState(() => Number(readPageSize('logs', SERVER_PAGE_SIZES)))
  const auto = usePageAutoRefresh('logsAutoRefresh')
  const [detail, setDetail] = React.useState<LogGroup | null>(null)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState<'clear' | 'export' | null>(null)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  const current = React.useRef({ filters, page, size, data })
  current.current = { filters, page, size, data }
  const lastIdRef = React.useRef<number | undefined>(undefined)
  const seqRef = React.useRef(0)
  const pollingRef = React.useRef(false)
  const busyRef = React.useRef(false)
  const loadingRef = React.useRef(0)
  const restoredRef = React.useRef(false)
  const touchedRef = React.useRef(false)
  const keywordTimerRef = React.useRef<number | undefined>(undefined)
  const scrollRef = React.useRef<number | null>(null)
  const loadedOnceRef = React.useRef(false)
  const liveRef = React.useRef<ScopedRefreshController | null>(null)

  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<void> => {
    if (!loadedOnceRef.current && loadingRef.current > 0 && !options.resetPage) return
    if (!restoredRef.current && shared().wbFilterMemory) {
      restoredRef.current = true
      if (!touchedRef.current) {
        const saved = readFilters()
        current.current.filters = saved
        setFilters(saved)
      }
    }
    const state = current.current
    if (options.resetPage) {
      state.page = 1
      setPage(1)
      lastIdRef.current = undefined
    }
    const seq = ++seqRef.current
    loadingRef.current += 1
    const selectedPage = state.page
    const selectedSize = state.size
    const selectedFilters = state.filters
    liveRef.current?.markLoaded()
    if (!options.silent) setLoading(true)
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const result = await api.getLogs(queryFor(selectedFilters, {
        aggregate: selectedFilters.aggregate, offset: (selectedPage - 1) * selectedSize,
        limit: selectedSize,
      }))
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
      scrollRef.current = options.resetPage || !options.silent ? 0 : (listRef.current?.scrollTop || 0)
      current.current.data = result
      setData(result)
      loadedOnceRef.current = true
      setLoaded(true)
      setError('')
      lastIdRef.current = Number(result.lastId) || 0
      applyStats(result.lastId === undefined ? null : { total: result.total, lastId: result.lastId })
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
      loadingRef.current -= 1
      liveRef.current?.markLoaded()
      if (seq === seqRef.current) setLoading(false)
    }
  }, [])

  const poll = React.useCallback(async () => {
    if (pollingRef.current || loadingRef.current > 0 || busyRef.current || keywordTimerRef.current !== undefined) return
    pollingRef.current = true
    try {
      await loadPanel({ silent: true })
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
    const controller = subscribeScopedLiveRefresh({
      scope: 'logs',
      enabled: auto.enabled,
      intervalMs: auto.ms,
      isActive: () => !document.hidden && shared().wbApp?.currentPage === 'logs',
      open: (path, signal) => requestDirect(path, { method: 'GET', signal }),
      onRefresh: poll,
      windowEvents: ['wb:page'],
    })
    liveRef.current = controller
    return () => {
      liveRef.current = null
      controller.stop()
    }
  }, [auto.enabled, auto.ms, poll])

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
    const query = queryFor(current.current.filters, { untilId: lastIdRef.current })
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
      const result = await api.exportLogs(queryFor(current.current.filters, { untilId: lastIdRef.current }))
      if (!result || result.canceled) return
      if (result.count === 0) { toast(t('暂无日志可导出'), 'err'); return }
      toast(t('✅ 已导出 {count} 条日志到 {file}', { count: String(result.count), file: String(result.file) }), 'ok')
    })
  }

  const counts = data.byLevel || {}
  const empty = !loaded && !error ? t('正在加载日志…') : Number(data.total) ? t('没有符合筛选条件的日志') : t('暂无日志')

  return (
    <TooltipProvider delay={200}>
      <Card render={<section />} className='panel event-log-panel' id='logs-panel-events'>
        <PageAutoRefresh id='logsAutoRefresh' settings={auto} onRefresh={() => loadPanel({ silent: true })} disabled={busy !== null || loading} />
        {/* 供顶栏状态镜像读取当前筛选条数，不额外占卡片顶部一行 */}
        <span id='logs-badge' hidden data-tone=''>{loaded ? badgeText : '—'}</span>
        <LogsToolbar
          filters={filters}
          categories={Object.keys(data.categories || {})}
          counts={counts}
          matched={matched}
          busy={busy}
          loading={loading}
          hasError={Boolean(error)}
          onChange={changeFilters}
          onExport={() => void exportLogs()}
          onClear={() => void clearLogs()}
        />
        <CardContent className='panel-body event-log-body'>
          {filters.view === 'debug' && !data.debugEnabled ? <div className='event-log-note'>{t('调试采集未开启，仅显示已保留的记录。可在设置中开启调试模式。')}</div> : null}
          {error ? <div className='event-log-error' role='status'>{error}<Button variant='ghost' size='xs' onClick={() => void loadPanel()}>{t('重试')}</Button></div> : null}
          <LogsList groups={groups} empty={empty} loading={loading} listRef={listRef} onDetail={setDetail} />
        </CardContent>
        <TableFooter total={totalGroups} page={page} pageCount={pageCount} size={size} sizes={SERVER_PAGE_SIZES}
          disabled={loading || busy !== null} range={{ start: groups.length ? (page - 1) * size + 1 : 0, end: (page - 1) * size + groups.length }}
          leading={loaded ? <span className='event-log-footer-note'>{t('显示 {groups} 组 · 原始记录 {entries} 条', { groups: totalGroups, entries: matched })}</span> : null}
          onSizeChange={changeSize} onPageChange={gotoPage} />
        <LogDetail group={detail} onClose={() => setDetail(null)} />
      </Card>
    </TooltipProvider>
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
