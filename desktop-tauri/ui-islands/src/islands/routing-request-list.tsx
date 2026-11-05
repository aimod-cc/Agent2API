import * as React from 'react'
import { Button, Empty, InputGroup, InputGroupAddon, InputGroupInput, Spinner, StatusDot, Switch, cn } from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow } from './routing-types'
import type { FlowDetails, FlowModels, FlowQuery } from './routing-flow-types'
import { requestClientLabel } from '../../../ui-shared/model-api-key'
import { isTraceRunning, isTraceSuccess, requestKey, requestModel, tracePhase, traceProvider } from './routing-trace-model'
import { formatDuration, RouteIcon } from './routing-ui'
import { RoutingSelect } from './routing-select'

type Props = {
  active: boolean
  query: { model?: string; minutes: number; scope: 'window' | 'running'; search?: string }
  selectedKey?: string
  refreshToken: number
  onOpenRequest(row: ReplayRow): void
  onModel(model: string): void
  onMinutes(minutes: number): void
  onScope(scope: 'window' | 'running'): void
  onSearch(search: string): void
}

/** 请求回放左栏：最近请求列表 + 模型 / 时间 / 范围 / 搜索。 */
export function RoutingRequestList({ active, query, selectedKey, refreshToken, onOpenRequest,
  onModel, onMinutes, onScope, onSearch }: Props) {
  const [entries, setEntries] = React.useState<ReplayRow[]>([])
  const [matched, setMatched] = React.useState(0)
  const [offset, setOffset] = React.useState(0)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const queryKey = JSON.stringify(query)
  const sequence = React.useRef(0)
  const [models, setModels] = React.useState<string[]>([])
  const tableQuery: FlowQuery = { ...query, detail: 'requests', offset, limit: 30 }
  // 模型候选只看时间窗与活动范围，与列表同一口径：窗口内没请求过的模型不列出。
  const modelWindow = `${query.minutes}:${query.scope}`

  React.useEffect(() => { setOffset(0); setEntries([]); setError('') }, [queryKey])

  React.useEffect(() => {
    if (!active) return
    let alive = true
    const load = async () => {
      try {
        const next = await bridge().getRoutingFlow({ minutes: query.minutes, scope: query.scope, detail: 'models' }) as unknown as FlowModels
        if (!alive || next.available === false) return
        setModels(next.models || [])
      } catch { /* 候选清单失败不影响列表，保留上一次结果 */ }
    }
    void load()
    const timer = window.setInterval(() => {
      if (!document.hidden && host().wbApp?.currentPage === 'routing') void load()
    }, 5000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [active, modelWindow, refreshToken])

  React.useEffect(() => {
    if (!active) return
    let alive = true
    let pending = false
    const load = async (silent: boolean) => {
      if (pending) return
      pending = true
      const id = ++sequence.current
      if (!silent) setLoading(true)
      try {
        const next: FlowDetails & { available?: boolean } = await bridge().getRoutingFlow(tableQuery)
        if (!alive || id !== sequence.current) return
        if (next.available === false) throw new Error(t('请求记录数据库不可用'))
        const rows = (next.entries || []).filter((entry): entry is ReplayRow => !('latest' in entry))
        const lastOffset = Math.max(0, Math.ceil(next.matched / 30) - 1) * 30
        if (offset > lastOffset) { setOffset(lastOffset); return }
        setEntries(rows)
        setMatched(next.matched)
        setError('')
      } catch (failure) {
        if (alive && id === sequence.current) setError(errorMessage(failure))
      } finally {
        pending = false
        if (alive && id === sequence.current) setLoading(false)
      }
    }
    void load(false)
    const timer = window.setInterval(() => {
      if (!document.hidden && host().wbApp?.currentPage === 'routing') void load(true)
    }, 3000)
    return () => { alive = false; sequence.current++; window.clearInterval(timer) }
  }, [active, queryKey, offset, refreshToken])

  React.useEffect(() => {
    if (!selectedKey && entries.length) onOpenRequest(entries[0])
  }, [entries, selectedKey])

  const renderRow = (row: ReplayRow) => {
    const running = isTraceRunning(row), success = isTraceSuccess(row)
    const status = running ? tracePhase(row.phase) : success ? String(row.status) : row.status ? `${row.status} · ${t('失败')}` : t('失败')
    const note = (row.attempts || 1) > 1 ? t('切换 {n} 次', { n: (row.attempts || 1) - 1 }) : row.route?.stickyHit ? t('粘滞命中') : t('未切换')
    return <Button key={requestKey(row)} variant='ghost' aria-pressed={selectedKey === requestKey(row)}
      title={`${row.id} · ${note}`} className={cn('routing-request-item', selectedKey === requestKey(row) && 'is-selected')} onClick={() => onOpenRequest(row)}>
      <span className='routing-request-dot'><StatusDot variant={running || success ? 'success' : 'destructive'} pulse={running} /></span>
      <span className='routing-request-content'>
        <span className='routing-request-line'><b className='routing-request-model' title={requestModel(row)}>{requestModel(row)}</b><time className='routing-request-time'>{new Date(row.ts).toLocaleTimeString()}</time></span>
        <span className='routing-request-line'><span className='routing-request-meta'>{requestClientLabel(row, t)} · {row.accountName || traceProvider(row.provider, row.providerLabel)}</span>
          <span className='routing-request-note'>{status}{!running ? ` · ${formatDuration(row.durationMs)}` : ''}</span></span>
      </span>
    </Button>
  }
  const running = entries.filter(isTraceRunning)
  const completed = entries.filter(row => !isTraceRunning(row))
  return <>
    <div className='routing-request-head'><b>{t('最近请求')}</b><span>{matched}</span>{loading ? <Spinner /> : null}</div>
    <div className='routing-request-tools'>
      <InputGroup><InputGroupInput placeholder={t('搜索请求、模型或账号')} aria-label={t('搜索请求、模型或账号')}
        value={query.search || ''} onChange={event => onSearch(event.currentTarget.value)} /><InputGroupAddon><RouteIcon name='search' size={14} /></InputGroupAddon></InputGroup>
      <div className='routing-request-filters'>
        <RoutingSelect label={t('请求模型')} value={query.model || '__all'}
          options={[['__all', models.length ? t('全部模型') : t('全部模型（窗口内没有请求）')],
            ...(query.model && !models.includes(query.model) ? [[query.model, `${query.model} · ${t('窗口内无请求')}`]] as Array<[string, string]> : []),
            ...models.map(model => [model, model] as [string, string])]}
          onChange={value => onModel(value === '__all' ? '' : value)} />
        <RoutingSelect label={t('已完成请求时间范围')} value={String(query.minutes)}
          options={[5, 10, 30, 60].map(minutes => [String(minutes), t('最近 {n} 分钟', { n: minutes })])}
          onChange={value => onMinutes(Number(value))} />
        <label className='routing-request-only'><Switch size='sm' checked={query.scope === 'running'} onCheckedChange={on => onScope(on ? 'running' : 'window')} />{t('仅在途')}</label>
      </div>
    </div>
    <div className='routing-request-list'>
      {running.length ? <><div className='routing-request-section'>{t('正在处理')}<span>{running.length}</span></div>{running.map(renderRow)}</> : null}
      {completed.length ? <><div className='routing-request-section'>{t('最近完成')}<span>{completed.length}</span></div>{completed.map(renderRow)}</> : null}
      {loading && !entries.length ? <Empty><Spinner /></Empty> : null}
      {!loading && !entries.length ? <Empty>{t('没有匹配的请求')}</Empty> : null}
      {error ? <p className='routing-request-error' role='alert'>{error}</p> : null}
    </div>
    <div className='routing-request-foot'>
      <Button variant='ghost' size='icon-sm' title={t('上一页')} aria-label={t('上一页')} disabled={offset === 0 || loading} onClick={() => setOffset(Math.max(0, offset - 30))}><RouteIcon name='chevronLeft' size={14} /></Button>
      <span>{Math.floor(offset / 30) + 1} / {Math.max(1, Math.ceil(matched / 30))}</span>
      <Button variant='ghost' size='icon-sm' title={t('下一页')} aria-label={t('下一页')} disabled={loading || offset + 30 >= matched} onClick={() => setOffset(offset + 30)}><RouteIcon name='chevronRight' size={14} /></Button>
    </div>
  </>
}
