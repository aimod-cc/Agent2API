import * as React from 'react'
import { Badge, Button, Empty, Input, SegmentedControl, Spinner } from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow, type RoutingWorkspace, type StickyEntry } from './routing-types'
import type { FlowDetails, FlowPath, FlowQuery, FlowSelection, FlowSession } from './routing-flow-types'
import { clientLabel, matchesSelection, selectedRequest, sessionId } from './routing-flow-model'
import { isTraceRunning, isTraceSuccess, requestKey, requestModel, tracePhase } from './routing-trace-model'
import { RouteIcon, formatDuration, stickyLabel, strategyLabel } from './routing-ui'
import { RoutingTraceDetails } from './routing-trace-details'

export function RoutingExecution({ active, selection, query, paths, sticky, now, workspace, onSelect, onConfig }: {
  active: boolean; selection: FlowSelection; query: FlowQuery; paths: FlowPath[]; sticky: StickyEntry[]; now: number; workspace: RoutingWorkspace | null;
  onSelect(selection: FlowSelection): void; onConfig(model: string): void
}) {
  const [tab, setTab] = React.useState<'sessions' | 'requests'>('sessions')
  const [offset, setOffset] = React.useState(0)
  const [search, setSearch] = React.useState('')
  const [activity, setActivity] = React.useState('all')
  const [result, setResult] = React.useState<FlowDetails | null>(null)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [traceNode, setTraceNode] = React.useState<import('./routing-trace-model').TraceNode>('overview')
  const row = selectedRequest(selection)
  const selectionKey = JSON.stringify(selection?.kind === 'request' ? ['request', requestKey(selection.row)] : selection?.kind === 'session' ? ['session', selection.session.model, selection.session.id] : selection)
  const detailQuery: FlowQuery = { ...query, detail: tab, offset, limit: 30,
    path: selection?.kind === 'path' ? selection.id : undefined,
    model: selection?.kind === 'model' ? selection.id : selection?.kind === 'edge' ? selection.model : query.model,
    session: selection?.kind === 'session' ? selection.session.id : undefined,
    sessionSearch: tab === 'sessions' ? search.trim() || undefined : undefined,
    client: selection?.kind === 'client' ? selection.id || '__unknown' : selection?.kind === 'edge' && selection.client != null ? selection.client || '__unknown' : undefined,
    activity: tab === 'sessions' ? activity as 'all' | 'running' | 'idle' : undefined,
    provider: selection?.kind === 'provider' ? selection.id : selection?.kind === 'edge' ? selection.provider : undefined,
    account: selection?.kind === 'account' ? selection.id : selection?.kind === 'edge' ? selection.accountId : undefined,
  }
  const key = JSON.stringify(detailQuery)
  React.useEffect(() => { setOffset(0); setTraceNode('overview'); setResult(null); setError('') }, [selectionKey, JSON.stringify(query)])
  React.useEffect(() => {
    if (!active || row) return
    let alive = true, pending = false
    const load = async (silent = false) => {
      if (pending) return
      pending = true
      if (!silent) setLoading(true)
      try {
        const next = await bridge().getRoutingFlow(detailQuery)
        if (!alive) return
        if (!next.available) throw new Error(t('请求记录数据库不可用'))
        const lastOffset = Math.max(0, Math.ceil(next.matched / 30) - 1) * 30
        if (offset > lastOffset) { setOffset(lastOffset); setResult(null); return }
        setResult(next); setError('')
      } catch (failure) { if (alive) setError(errorMessage(failure)) }
      finally { pending = false; if (alive) setLoading(false) }
    }
    void load()
    const timer = window.setInterval(() => { if (!document.hidden && host().wbApp?.currentPage === 'routing') void load(true) }, 3000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [active, key, Boolean(row)])
  const scopePaths = paths.filter(path => matchesSelection(path, selection))
  if (row) {
    const binding = sticky.find(entry => entry.id === sessionId(row))
    const snapshot = row.route?.configSnapshot
    return <><div className='routing-inspector-head'><b><RouteIcon name='requests' />{t('本次执行')}</b><Button variant='ghost' size='sm' onClick={() => onSelect(null)}>{t('返回列表')}</Button></div><div className='routing-inspector-body'>
      <code>{row.id}</code><h3>{isTraceRunning(row) ? tracePhase(row.phase) : isTraceSuccess(row) ? t('本次请求已完成') : t('请求失败')}</h3>
      <div className='routing-execution-tags'><Badge variant={isTraceRunning(row) ? 'success' : isTraceSuccess(row) ? 'secondary' : 'destructive'} shape='tag'>{isTraceRunning(row) ? t('请求在途') : String(row.status)}</Badge>{row.route?.stickyHit ? <Badge variant='brand' shape='tag'>{t('粘滞命中')}</Badge> : null}</div>
      <dl className='routing-execution-facts'><div><dt>{t('客户端')}</dt><dd>{clientLabel(row.client || '')}</dd></div><div><dt>{t('请求模型')}</dt><dd><code>{requestModel(row)}</code></dd></div><div><dt>{isTraceRunning(row) ? t('当前承载') : t('最终承载')}</dt><dd>{row.providerLabel || row.provider || '—'} · {row.accountName || t('账号未记录')}</dd></div><div><dt>{t('开始时间')}</dt><dd>{new Date(row.ts).toLocaleTimeString()}</dd></div><div><dt>{isTraceRunning(row) ? t('在途时长') : t('总耗时')}</dt><dd>{formatDuration(isTraceRunning(row) ? now - row.ts : row.durationMs)}</dd></div></dl>
      <div className='routing-execution-snapshot'><b><RouteIcon name='lock' size={12} />{t('请求当时的配置')}</b><p>{snapshot || row.route ? `${strategyLabel(snapshot?.strategy || row.route?.strategy)} · ${t('会话粘滞')} ${stickyLabel(snapshot?.sticky || row.route?.sticky)}` : t('路由快照未记录')}</p>{snapshot ? <small>{new Date(snapshot.capturedAt).toLocaleString()} · {snapshot.scope ? t('模型独立配置') : t('继承全局')}</small> : null}<Button variant='link' size='sm' onClick={() => onConfig(row.model)}>{t('调整后续请求的配置')}</Button></div>
      {sessionId(row) ? <div className='routing-execution-binding'><b>{t('会话标识')}</b><code>{sessionId(row).slice(0, 12)}…</code><span>{row.route?.sessionSource === 'content-hash' ? t('内容哈希（推断）') : row.route?.sessionSource || t('来源未记录')}</span><p>{binding && binding.expiresAt > now ? t('当前绑定：{account}，剩余 {time}', { account: binding.accountName, time: formatDuration(binding.expiresAt - now) }) : t('当前没有可沿用的粘滞绑定')}</p></div> : <p className='routing-editor-hint'>{t('会话标识未记录，不将此请求推断为独立会话。')}</p>}
      <RoutingTraceDetails row={row} selected={traceNode} onSelect={setTraceNode} />
    </div><div className='routing-inspector-foot'>{t('请求完成不代表会话关闭；粘滞有效期不是在线状态。')}</div></>
  }
  const sessions = (result?.entries || []).filter((entry): entry is FlowSession => 'latest' in entry)
  const requests = (result?.entries || []).filter((entry): entry is ReplayRow => !('latest' in entry))
  const title = selection?.kind === 'client' ? clientLabel(selection.id) : selection?.kind === 'model' ? selection.id : selection?.kind === 'provider' ? workspace?.providers.find(provider => provider.id === selection.id)?.label || selection.id : t('关联请求路径')
  return <><div className='routing-inspector-head'><b><RouteIcon name='requests' />{t('执行记录')}</b><Badge shape='tag' variant='secondary'>{scopePaths.length}</Badge></div><div className='routing-inspector-body'>
    <h3>{title}</h3><p className='routing-editor-hint'>{t('按路径聚合，会话在列表中展开，不在画布逐条连线。')}</p>
    <SegmentedControl aria-label={t('选中流量详情')} value={tab} onValueChange={value => { setTab(value); setOffset(0); setResult(null) }} options={[{ value: 'sessions', label: t('会话') }, { value: 'requests', label: t('请求') }]} />
    {tab === 'sessions' ? <><Input aria-label={t('搜索会话或账号')} placeholder={t('搜索会话或账号')} value={search} onChange={event => { setSearch(event.currentTarget.value); setOffset(0); setResult(null) }} /><SegmentedControl aria-label={t('会话活动')} value={activity} onValueChange={value => { setActivity(value); setOffset(0); setResult(null) }} options={[{ value: 'all', label: t('全部') }, { value: 'running', label: t('有在途') }, { value: 'idle', label: t('当前空闲') }]} /></> : null}
    <div className='routing-activity-list'>{tab === 'sessions' ? sessions.map(session => {
      const binding = sticky.find(entry => entry.id === session.id && entry.expiresAt > now), current = session.latest
      return <Button variant='ghost' key={`${session.model}:${session.id}`} className='routing-activity-row' onClick={() => onSelect({ kind: 'session', session })}>
        <span><code>{session.id.slice(0, 12)}…</code><Badge variant={session.running ? 'success' : 'secondary'} shape='tag'>{session.running ? t('{n} 在途', { n: session.running }) : t('当前空闲')}</Badge></span><b>{current.providerLabel || current.provider} · {current.accountName || t('账号未记录')}</b><small>{session.running ? tracePhase(current.phase) : t('最近请求已完成')} · {new Date(session.lastSeen).toLocaleTimeString()}</small><small>{binding ? t('绑定：{account}', { account: binding.accountName }) : t('无有效绑定')}</small>
      </Button>
    }) : requests.map(request => <Button variant='ghost' className='routing-activity-row' key={requestKey(request)} onClick={() => onSelect({ kind: 'request', row: request })}><span><code>{request.id}</code><Badge variant={isTraceRunning(request) ? 'success' : isTraceSuccess(request) ? 'secondary' : 'destructive'} shape='tag'>{isTraceRunning(request) ? tracePhase(request.phase) : String(request.status)}</Badge></span><b>{requestModel(request)}</b><small>{request.accountName || request.providerLabel || t('账号未记录')} · {new Date(request.ts).toLocaleTimeString()}</small></Button>)}</div>
    {loading && !result ? <Empty><Spinner /></Empty> : !(tab === 'sessions' ? sessions : requests).length ? <Empty>{t('没有匹配的记录')}</Empty> : null}
    {result?.unknownRequests ? <p className='routing-editor-hint'>{t('{n} 条请求没有会话标识，未计入会话数量。', { n: result.unknownRequests })}</p> : null}
    {error ? <p className='routing-editor-error' role='alert'>{error}</p> : null}
  </div><div className='routing-inspector-foot routing-flow-pager'><Button variant='ghost' size='sm' disabled={offset === 0 || loading} onClick={() => setOffset(Math.max(0, offset - 30))}>{t('上一页')}</Button><span>{Math.floor(offset / 30) + 1} / {Math.max(1, Math.ceil((result?.matched || 0) / 30))}</span><Button variant='ghost' size='sm' disabled={loading || offset + 30 >= (result?.matched || 0)} onClick={() => setOffset(offset + 30)}>{t('下一页')}</Button></div></>
}
