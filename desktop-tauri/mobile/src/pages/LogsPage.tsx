import { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Input, SegmentedControl, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, Switch , Label, Card } from '@ui'
import { PageAutoRefresh, usePageAutoRefresh } from '../components/PageAutoRefresh'
import { usePageRefreshTimer } from '../../../ui-shared/page-auto-refresh-timer'
import { PageIntro, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { store } from '../state/store'
import { fetchLogs } from '../api/gateway'
import { usePullToRefresh, PullIndicator } from '../components/PullToRefresh'
import { t } from '../i18n'
import {
  formatLogTime, groupTime, logAdvice, logContext, logDetailText, logGroups, logLevel, logPlugins, logRawText,
  logSource, logSteps, logSummary, logViewNote, type LogGroup, type LogPage, type LogView,
} from '../../../ui-shared/logs'
import { Toggle as UiToggle } from '@ui'
import { DateRangeFilter } from '../components/DateRangeFilter'
import { dateRangeQuery, readCustomRange, saveCustomRange, type CustomDates, type DateRange } from '../../../ui-shared/date-range'

const LIMIT = 50
const CUSTOM_RANGE_KEY = 'agent2api-mobile-logs-custom-range'
const VIEWS = [
  { value: 'important' as LogView, label: t('重要事件') },
  { value: 'all' as LogView, label: t('全部事件') },
  { value: 'debug' as LogView, label: t('调试详情') },
]

export function LogsPage() {
  const auto = usePageAutoRefresh('logsAutoRefresh')
  const [view, setView] = useState<LogView>('important')
  const [range, setRange] = useState<DateRange>('all')
  const [dates, setDates] = useState<CustomDates>(() => readCustomRange(CUSTOM_RANGE_KEY))
  const [level, setLevel] = useState('')
  const [keyword, setKeyword] = useState('')
  const [search, setSearch] = useState('')
  const [aggregate, setAggregate] = useState(true)
  const [data, setData] = useState<LogPage | null>(null)
  const [page, setPage] = useState(1)
  const [paused, setPaused] = useState(false)
  const [reading, setReading] = useState(false)
  const [pending, setPending] = useState(0)
  const [detail, setDetail] = useState<LogGroup | null>(null)
  const [detailTab, setDetailTab] = useState<'detail' | 'raw'>('detail')
  useEffect(() => setDetailTab('detail'), [detail?.id])
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const bodyRef = useRef<HTMLDivElement | null>(null)
  const snapshotRef = useRef<number | undefined>(undefined)
  const seqRef = useRef(0)
  const stateRef = useRef({ view, level, search, aggregate, page, paused, detail, range, dates })
  stateRef.current = { view, level, search, aggregate, page, paused, detail, range, dates }
  const pollingRef = useRef(false)
  const loadingRef = useRef(0)

  useEffect(() => {
    const timer = window.setTimeout(() => setSearch(keyword.trim()), 300)
    return () => window.clearTimeout(timer)
  }, [keyword])

  const load = useCallback(async (latest = false, silent = false, manual = false) => {
    const state = stateRef.current
    if (latest) {
      state.page = 1
      setPage(1)
      snapshotRef.current = undefined
      setPaused(false)
      setReading(false)
      setDetail(null)
      state.paused = false
      state.detail = null
      if (bodyRef.current) bodyRef.current.scrollTop = 0
    }
    const seq = ++seqRef.current
    loadingRef.current += 1
    if (!silent) setLoading(true)
    try {
      const locked = !latest && (state.page > 1 || state.paused || state.detail !== null || (bodyRef.current?.scrollTop || 0) > 20)
      const result = await fetchLogs({
        view: state.view, level: state.view === 'debug' ? undefined : state.level,
        keyword: state.search, aggregate: state.aggregate, ...dateRangeQuery(state.range, state.dates),
        offset: (state.page - 1) * LIMIT, limit: LIMIT, untilId: locked ? snapshotRef.current : undefined,
      })
      if (seq !== seqRef.current || state.view !== stateRef.current.view || state.level !== stateRef.current.level || state.search !== stateRef.current.search || state.aggregate !== stateRef.current.aggregate || state.range !== stateRef.current.range || state.dates !== stateRef.current.dates) return
      const maxPage = Math.max(1, Math.ceil(Number(result.grouped ?? result.matched ?? result.entries?.length) / LIMIT))
      if (state.page > maxPage) {
        stateRef.current.page = maxPage
        setPage(maxPage)
        await load(false, silent, manual)
        return
      }
      if (silent && !manual && !locked && (stateRef.current.paused || stateRef.current.detail !== null || (bodyRef.current?.scrollTop || 0) > 20)) return
      setData(result)
      setError('')
      if (!locked || snapshotRef.current === undefined) { snapshotRef.current = result.lastId ?? 0; setPending(0) }
    } catch (failure) {
      if (seq === seqRef.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      loadingRef.current -= 1
      if (seq === seqRef.current) setLoading(false)
    }
  }, [])

  useEffect(() => {
    snapshotRef.current = undefined
    stateRef.current.page = 1
    setPage(1)
    setPending(0)
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    void load()
    return () => { ++seqRef.current }
  }, [view, level, search, aggregate, range, dates, load])

  usePageRefreshTimer(async () => {
    if (pollingRef.current || loadingRef.current > 0) return
    pollingRef.current = true
    try {
      const state = stateRef.current
      const locked = state.page > 1 || state.paused || state.detail !== null || (bodyRef.current?.scrollTop || 0) > 20
      if (locked && snapshotRef.current !== undefined) {
        const sinceId = snapshotRef.current
        const result = await fetchLogs({ view: state.view, level: state.view === 'debug' ? undefined : state.level,
          keyword: state.search, ...dateRangeQuery(state.range, state.dates), sinceId, limit: 1 })
        if (sinceId !== snapshotRef.current || state.view !== stateRef.current.view || state.search !== stateRef.current.search || state.level !== stateRef.current.level || state.aggregate !== stateRef.current.aggregate || state.range !== stateRef.current.range || state.dates !== stateRef.current.dates) return
        if (result.lastId !== undefined && result.lastId < sinceId) await load(true)
        else setPending(Number(result.matched) || 0)
      } else await load(false, true)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { pollingRef.current = false }
  }, auto.enabled, auto.ms)

  const pull = usePullToRefresh({ onRefresh: () => load(true), scrollRef: bodyRef })
  const groups = logGroups(data)
  const totalGroups = Number(data?.grouped ?? data?.matched ?? groups.length) || 0
  const pageCount = Math.max(1, Math.ceil(totalGroups / LIMIT))
  const held = paused || reading || page > 1 || detail !== null
  const nextPage = (target: number) => {
    stateRef.current.page = target
    setPage(target)
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    void load()
  }
  const copy = async (entries: LogGroup[]) => {
    try {
      const raw = entries.flatMap(group => group.entries)
      await navigator.clipboard.writeText(logRawText(raw))
      store.toast(t('已复制 {n} 条日志', { n: raw.length }))
    } catch { store.toast(t('复制失败'), 'err') }
  }

  return (
    <div className='m-page m-fixed' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <div className='m-sticky'>
        <PageAutoRefresh settings={auto} onRefresh={() => load(false, true, true)} disabled={loading} />
        <PageIntro subtitle={data ? t('显示 {groups} 组 · 原始记录 {entries} 条', { groups: totalGroups, entries: Number(data.matched) || 0 }) : undefined}
          actions={<Button variant='ghost' size='sm' aria-pressed={paused} onClick={() => {
            const next = !stateRef.current.paused
            stateRef.current.paused = next
            setPaused(next)
            if (!next && page === 1 && !detail && (bodyRef.current?.scrollTop || 0) <= 20) void load(true)
          }}>{paused ? t('结束阅读') : t('阅读模式')}</Button>} />
        <div className='m-section' style={{ marginTop: 0 }}>
          <SegmentedControl options={VIEWS} value={view} onValueChange={setView} aria-label={t('日志视图')} className='w-full' />
          <Input className='mt-2.5 w-full' type='search' value={keyword} onChange={event => setKeyword(event.currentTarget.value)}
            placeholder={t('搜索插件、消息、提供商或账号…')} aria-label={t('搜索插件、消息、提供商或账号')} />
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10, marginTop: 10, fontSize: 11.5, color: 'var(--m-text-3)' }}>
            <span>{held ? t('阅读模式，列表保持不动') : auto.enabled ? t('自动刷新') : t('自动刷新已关闭')}</span>
            <Label style={{ display: 'flex', alignItems: 'center', gap: 6 }}><Switch checked={aggregate} onCheckedChange={setAggregate} aria-label={t('折叠重复日志')} />{t('折叠重复日志')}</Label>
          </div>
        </div>
        <DateRangeFilter range={range} dates={dates} onRangeChange={setRange} label={t('事件日志时间范围')}
          onDatesChange={next => { setDates(next); saveCustomRange(CUSTOM_RANGE_KEY, next) }} />
        {view !== 'debug' ? <div className='m-chips' style={{ padding: '0 16px 10px' }}>
          {[{ value: '', label: t('全部级别') }, { value: 'warn', label: t('警告及以上') }, { value: 'error', label: t('仅错误') }].map(option =>
            <UiToggle size='default' variant='outline' key={option.value} type='button' className='m-chip rounded-pill' pressed={level === option.value} onClick={() => setLevel(option.value)}>{option.label}</UiToggle>)}
        </div> : null}
      </div>
      <div className='m-body' ref={bodyRef} onScroll={() => setReading((bodyRef.current?.scrollTop || 0) > 20)}>
        {error ? <ErrorNote text={error} /> : null}
        <div className='m-section' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.6 }}>{logViewNote(view, t)}</div>
        {view === 'debug' && data && !data.debugEnabled ? <div className='m-section' style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{t('调试采集未开启，仅显示已保留的记录。可在设置中开启调试模式。')}</div> : null}
        {pending || held ? <div className='m-section'><Button variant='outline' className='w-full' disabled={loading} onClick={() => void load(true)}>{pending ? t('有 {n} 条新日志', { n: pending }) : t('阅读模式，列表保持不动')} · {t('查看最新')}</Button></div> : null}
        {!data && !error ? <SkeletonList rows={5} height={68} /> : null}
        {data && !groups.length ? <div className='m-section'><Card className='m-card'><Empty text={t('没有符合筛选条件的日志')} /></Card></div> : null}
        {groups.length ? <div className='m-section' style={{ marginTop: 0 }}><Card className='m-card'>
          {groups.map(group => <div key={group.id} style={{ padding: '11px 14px', borderBottom: '1px solid var(--m-line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 11 }}>
              <span style={{ color: levelFg(group.entry.level) }}>{logLevel(group.entry.level, t)}</span>
              <span style={{ flex: 1, color: 'var(--m-text-3)', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{logSource(group.entry, t)}</span>
              {group.count > 1 ? <span style={{ color: 'var(--m-text-3)' }}>×{group.count}</span> : null}
              <Button variant='ghost' size='xs' onClick={() => { stateRef.current.detail = group; setDetail(group) }}>{t('详情')}</Button>
            </div>
            <div style={{ color: 'var(--m-text)', lineHeight: 1.55, fontSize: 12.5, marginTop: 3 }}>{logSummary(group.entry, t)}</div>
            {logContext(group.entry, t) ? <div style={{ color: 'var(--m-text-3)', fontSize: 11, lineHeight: 1.5, marginTop: 3, overflowWrap: 'anywhere' }}>{logContext(group.entry, t)}</div> : null}
            <div style={{ color: 'var(--m-text-3)', fontSize: 10.5, marginTop: 6, fontVariantNumeric: 'tabular-nums' }}>{groupTime(group)}</div>
          </div>)}
        </Card></div> : null}
        {data ? <div className='m-section' style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
          <Button variant='outline' size='sm' disabled={loading || page <= 1} onClick={() => nextPage(page - 1)}>{t('上一页')}</Button>
          <span style={{ color: 'var(--m-text-3)', fontSize: 11.5 }}>{page} / {pageCount}</span>
          <Button variant='outline' size='sm' disabled={loading || page >= pageCount} onClick={() => nextPage(page + 1)}>{t('下一页')}</Button>
        </div> : null}
        {groups.length ? <div className='m-section'><Button variant='ghost' className='w-full' onClick={() => void copy(groups)}>{t('复制本页原始记录')}</Button></div> : null}
      </div>
      <Sheet open={Boolean(detail)} onOpenChange={open => { if (!open) { stateRef.current.detail = null; setDetail(null) } }}>
        <SheetContent><SheetHeader><SheetTitle>{t('日志详情')}</SheetTitle></SheetHeader><SheetBody>
          {detail ? <>
            <div style={{ fontSize: 13, color: 'var(--m-text)', lineHeight: 1.6 }}>{logSummary(detail.entry, t)}</div>
            <div style={{ fontSize: 11, color: 'var(--m-text-3)' }}>{groupTime(detail)} · {t('原始记录 {n} 条', { n: detail.count })}</div>
            <div style={{ fontSize: 11.5, color: 'var(--m-text-2)', overflowWrap: 'anywhere' }}>{logContext(detail.entry, t)}</div>
            <SegmentedControl options={[{ value: 'detail' as const, label: t('事件详情') }, { value: 'raw' as const, label: t('原始记录') }]}
              value={detailTab} onValueChange={setDetailTab} aria-label={t('日志详情视图')} />
            {detailTab === 'detail' ? <>
              <div style={{ fontSize: 12 }}>{t('来源')}：{logSource(detail.entry, t)}</div>
              {logAdvice(detail.entry, t) ? <div style={{ fontSize: 12, lineHeight: 1.6, padding: 12, background: 'var(--m-surface)', borderRadius: 10 }}>
                <strong>{t('处理建议')}</strong><p style={{ margin: '6px 0 0' }}>{logAdvice(detail.entry, t)}</p>
              </div> : null}
              {logPlugins(detail.entry, t).length ? <section><h3 style={{ fontSize: 12 }}>{t('启动结果')}</h3>
                {logPlugins(detail.entry, t).map(plugin => <div key={plugin.id} style={{ display: 'flex', justifyContent: 'space-between', gap: 12, padding: '6px 0', fontSize: 12 }}>
                  <span style={{ minWidth: 0, overflowWrap: 'anywhere' }}>{plugin.name}</span>
                  <span style={{ flexShrink: 0, color: plugin.phase === 'ready' ? 'var(--m-text-3)' : 'var(--m-warn)' }}>{plugin.status}</span>
                </div>)}
              </section> : null}
              {logSteps(detail.entry, t).length ? <section><h3 style={{ fontSize: 12 }}>{t('各阶段耗时')}</h3>
                {logSteps(detail.entry, t).map((step, index) => <div key={index} style={{ display: 'flex', justifyContent: 'space-between', gap: 12, padding: '5px 0', fontSize: 12 }}>
                  <span>{step.label}</span><span>{step.duration}</span>
                </div>)}
              </section> : null}
              {logDetailText(detail.entry) ? <section><h3 style={{ fontSize: 12 }}>{t('详细信息')}</h3>
                <pre style={{ fontSize: 11, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6 }}>{logDetailText(detail.entry)}</pre>
              </section> : null}
            </> : detail.entries.map((entry, index) => <div key={entry.id ?? index}>
              <div style={{ fontSize: 10.5, color: 'var(--m-text-3)', marginBottom: 5 }}>{formatLogTime(entry.ts)}</div>
              <pre style={{ fontFamily: 'var(--font-mono)', fontSize: 11, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6, padding: 10, margin: 0, borderRadius: 10, background: 'var(--m-surface)' }}>{JSON.stringify(entry, null, 2)}</pre>
            </div>)}
          </> : null}
        </SheetBody><SheetFooter><Button variant='outline' className='w-full' onClick={() => { if (detail) void copy([detail]) }}>{t('复制原始记录')}</Button></SheetFooter></SheetContent>
      </Sheet>
    </div>
  )
}

function levelFg(level?: string): string {
  if (level === 'error') return 'var(--m-danger)'
  if (level === 'warn') return 'var(--m-warn)'
  return 'var(--m-text-3)'
}
