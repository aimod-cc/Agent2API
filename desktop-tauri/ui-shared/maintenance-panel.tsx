import * as React from 'react'
import { Alert, Button, Empty, Spinner } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { MaintenanceSettings } from './maintenance-settings'
import { MaintenanceRecords } from './maintenance-records'
import { MaintenanceSummary } from './maintenance-summary'
import { hasFailures } from './maintenance-format'
import { MAINTENANCE_TASKS, type MaintenanceApi, type MaintenanceDetail, type MaintenanceOverview, type MaintenanceQuery, type MaintenanceRun, type MaintenanceTaskId } from './maintenance-types'
import './maintenance.css'

const initialQuery: MaintenanceQuery = { taskId: '', source: '', status: '', keyword: '', offset: 0, limit: 10 }

/**
 * 自动维护页。
 *
 * ── 版面（这次改动的重点）──────────────────────────────────────
 * 一屏三段，只有中间那段滚：页头固定 → 概览条（两类维护各一行，约 90px）→
 * 执行记录占满剩余高度、表格在自己卡内滚动。原来两张概览卡就要吃掉首屏一半高度，
 * 记录只剩三四行可见；这一页真正要看的是记录，所以概览只保留「是否正常、下次什么时候跑」。
 *
 * ── 明细 ─────────────────────────────────────────────────────
 * 点「查看明细」开弹窗（高度固定，不随条数变化），不再在行内展开。
 *
 * 其余数据流一律不动：20 秒静默轮询 + 页面可见性、搜索 300ms 防抖、明细按 id 缓存
 * （保留最近 20 条）、执行中的记录跑完自动重取、翻页越界回退、busy 互斥。
 */
export function MaintenancePanel({ api, active = true, revision = 0, onAccount, onChanged, notify }: {
  api: MaintenanceApi
  active?: boolean
  revision?: number
  onAccount?(id: string): void
  onChanged?(): void | Promise<unknown>
  notify?(message: string, kind?: 'ok' | 'err'): void
}) {
  const [overview, setOverview] = React.useState<MaintenanceOverview | null>(null)
  const [query, setQuery] = React.useState(initialQuery)
  const [search, setSearch] = React.useState('')
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [actionError, setActionError] = React.useState('')
  const [compact, setCompact] = React.useState(() => window.matchMedia('(max-width: 700px)').matches)
  const [busy, setBusy] = React.useState<MaintenanceTaskId | ''>('')
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [expanded, setExpanded] = React.useState('')
  const [details, setDetails] = React.useState<Record<string, MaintenanceDetail>>({})
  const [detailLoading, setDetailLoading] = React.useState('')
  const [detailError, setDetailError] = React.useState<Record<string, string>>({})
  const detailInFlight = React.useRef(new Set<string>())
  const operation = React.useRef(false)
  const sequence = React.useRef(0)
  const mounted = React.useRef(true)
  const currentQuery = React.useRef(query)
  currentQuery.current = query
  React.useEffect(() => { mounted.current = true; return () => { mounted.current = false; sequence.current += 1 } }, [])
  React.useEffect(() => {
    const media = window.matchMedia('(max-width: 700px)')
    const update = () => setCompact(media.matches)
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  const load = React.useCallback(async (quiet = false) => {
    const ticket = ++sequence.current
    if (!quiet) setLoading(true)
    try {
      const data = await api.overview(currentQuery.current)
      if (!mounted.current || ticket !== sequence.current) return
      setOverview(data)
      setError('')
    } catch (failure) {
      if (mounted.current && ticket === sequence.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      if (mounted.current && ticket === sequence.current) setLoading(false)
    }
  }, [api])
  React.useEffect(() => {
    if (!active) return
    void load()
    const timer = window.setInterval(() => { if (!document.hidden) void load(true) }, 20_000)
    const visibility = () => { if (!document.hidden) void load(true) }
    document.addEventListener('visibilitychange', visibility)
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', visibility); sequence.current += 1 }
  }, [active, query, revision, load])
  React.useEffect(() => {
    const timer = setTimeout(() => setQuery(previous => previous.keyword === search.trim() ? previous : { ...previous, keyword: search.trim(), offset: 0 }), 300)
    return () => clearTimeout(timer)
  }, [search])

  const fetchDetail = React.useCallback(async (id: string) => {
    if (detailInFlight.current.has(id)) return
    detailInFlight.current.add(id)
    setDetailLoading(id)
    try {
      const detail = await api.detail(id)
      if (!mounted.current) return
      setDetails(previous => ({ ...Object.fromEntries(Object.entries(previous).slice(-19)), [id]: detail }))
      setDetailError(previous => ({ ...Object.fromEntries(Object.entries(previous).slice(-19)), [id]: '' }))
    } catch (failure) {
      if (mounted.current) setDetailError(previous => ({ ...Object.fromEntries(Object.entries(previous).slice(-19)), [id]: failure instanceof Error ? failure.message : String(failure) }))
    } finally {
      detailInFlight.current.delete(id)
      if (mounted.current) setDetailLoading(previous => previous === id ? '' : previous)
    }
  }, [api])
  React.useEffect(() => {
    if (!expanded || !active) return
    const run = overview?.records.items.find(item => item.id === expanded)
    if (details[expanded]?.status === 'running' && run && run.status !== 'running') void fetchDetail(expanded)
  }, [overview, expanded, active, details, fetchDetail])
  // 明细弹窗的关闭与打开共用这一个入口：同一条再点一次就是收起
  const toggleDetail = (id: string) => {
    if (id === expanded) { setExpanded(''); return }
    setExpanded(id)
    if (!details[id] || details[id].status === 'running') void fetchDetail(id)
  }
  const changeQuery = (patch: Partial<MaintenanceQuery>) => { setQuery(previous => ({ ...previous, ...patch, offset: 0 })); setExpanded('') }
  const execute = async (taskId: MaintenanceTaskId, retryId?: string, itemId?: string) => {
    if (operation.current) return
    operation.current = true
    setBusy(taskId)
    setActionError('')
    try {
      const detail = retryId ? await api.retry(retryId, itemId) : await api.run(taskId)
      if (!mounted.current) return
      setDetails(previous => ({ ...Object.fromEntries(Object.entries(previous).slice(-19)), [detail.id]: detail }))
      setExpanded(detail.id)
      setQuery(initialQuery)
      currentQuery.current = initialQuery
      setSearch('')
      notify?.(detail.status === 'running' ? t('维护已开始，结果会自动更新，可离开此页') : hasFailures(detail) ? t('维护已结束，有失败项，请查看明细') : t('维护已完成，结果已保存'), hasFailures(detail) ? 'err' : 'ok')
      await load(true)
      try { await onChanged?.() } catch { /* 执行结果已经保存，页面同步失败不重新执行维护。 */ }
    } catch (failure) {
      if (!mounted.current) return
      setActionError(failure instanceof Error ? failure.message : String(failure))
      await load(true)
    } finally { operation.current = false; if (mounted.current) setBusy('') }
  }
  const showLatest = (taskId: MaintenanceTaskId) => {
    const latest = overview?.latest[taskId]
    if (!latest) return
    changeQuery({ taskId, source: '', status: '', keyword: '' })
    setSearch('')
    setExpanded(latest.id)
    void fetchDetail(latest.id)
  }
  const total = overview?.records.total ?? 0
  React.useEffect(() => {
    if (!overview || loading) return
    const lastOffset = Math.max(0, (Math.ceil(overview.records.total / query.limit) - 1) * query.limit)
    if (query.offset > lastOffset) setQuery(previous => ({ ...previous, offset: lastOffset }))
  }, [overview, loading, query.offset, query.limit])
  const page = Math.floor(query.offset / query.limit) + 1
  const pages = Math.max(1, Math.ceil(total / query.limit))
  const runs: MaintenanceRun[] = overview?.records.items ?? []

  return <div className='maintenance-page'>
    <div className='maintenance-heading'>
      <div>
        <h2>{t('自动维护')}</h2>
        <p>{t('查看凭证与模型目录的每次维护结果，失败和跳过都有原因')}</p>
      </div>
      <div className='maintenance-heading-actions'>
        <Button variant='ghost' disabled={loading} onClick={() => void load()}>{loading ? <Spinner /> : null}{loading ? t('加载中…') : t('刷新记录')}</Button>
        <Button variant='ghost' disabled={!overview?.tasks.length} onClick={() => setSettingsOpen(true)}>{t('维护设置')}</Button>
      </div>
    </div>

    {error || actionError ? <Alert variant='destructive'>{actionError || error}</Alert> : null}
    {!overview && loading ? <Empty><Spinner />{t('正在读取维护记录…')}</Empty> : null}

    {overview ? <>
      <MaintenanceSummary tasks={overview.tasks} latest={overview.latest} busy={busy} compact={compact}
        onRun={id => void execute(id)} onShowLatest={showLatest} />
      <MaintenanceRecords
        compact={compact}
        loading={loading}
        busy={Boolean(busy)}
        runs={runs}
        total={total}
        page={page}
        pages={pages}
        query={query}
        search={search}
        onSearch={setSearch}
        onQuery={changeQuery}
        onPage={delta => setQuery(previous => ({ ...previous, offset: Math.max(0, previous.offset + delta * previous.limit) }))}
        openId={expanded}
        onToggle={toggleDetail}
        detail={details[expanded]}
        detailLoading={detailLoading === expanded}
        detailError={detailError[expanded] || ''}
        onReloadDetail={() => void fetchDetail(expanded)}
        retentionDays={overview.retentionDays}
        maxRecords={overview.maxRecords}
        onRetry={(runId, itemId) => { const run = runs.find(item => item.id === runId); if (run) void execute(run.taskId, runId, itemId) }}
        onAccount={onAccount}
      />
    </> : null}

    {settingsOpen && overview ? <MaintenanceSettings tasks={overview.tasks} api={api} onClose={() => setSettingsOpen(false)} onSaved={() => void load(true)} /> : null}
  </div>
}
