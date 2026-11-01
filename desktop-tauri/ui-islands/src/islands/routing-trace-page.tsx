import * as React from 'react'
import { Badge, Button, Checkbox, Empty, Input, Label, SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { t } from '../i18n'
import { RouteIcon, formatDuration } from './routing-ui'
import { RoutingTraceCanvas } from './routing-trace-canvas'
import { RoutingTraceDetails } from './routing-trace-details'
import { useRoutingTrace } from './routing-trace-state'
import { isTraceRunning, isTraceSuccess, requestKey, requestModel, traceAttempts, traceCandidates, tracePhase, traceProvider, type TraceNode } from './routing-trace-model'
import { host, type ReplayRow } from './routing-types'

type Props = { active: boolean; initialRow?: ReplayRow | null; onConfig(model: string): void; onWorkbench?(row: ReplayRow): void }
function Filter({ value, options, label, onChange }: { value: string; options: Array<[string, string]>; label: string; onChange(value: string): void }) {
  const text = options.find(option => option[0] === value)?.[1] || value
  return <Select value={value} onValueChange={next => onChange(String(next))}><SelectTrigger aria-label={label} title={text}><SelectValue>{text}</SelectValue></SelectTrigger><SelectContent>{options.map(([key, label]) => <SelectItem key={key} value={key}>{label}</SelectItem>)}</SelectContent></Select>
}
function RequestBadge({ row }: { row: ReplayRow }) {
  return <Badge shape='tag' variant={isTraceRunning(row) ? 'info' : isTraceSuccess(row) ? 'success' : 'destructive'}>{isTraceRunning(row) ? tracePhase(row.phase) : isTraceSuccess(row) ? t('成功') : t('失败')}</Badge>
}

export function RoutingTracePage({ active, initialRow, onConfig, onWorkbench }: Props) {
  const state = useRoutingTrace(active, initialRow)
  const row = state.selected
  const [view, setView] = React.useState<'canvas' | 'details'>('canvas')
  const [node, setNode] = React.useState<TraceNode>('overview')
  const [candidates, setCandidates] = React.useState(false)
  const [step, setStep] = React.useState<number | null>(null)
  const [playing, setPlaying] = React.useState(false)
  const content = React.useRef<HTMLDivElement>(null)
  const key = row ? requestKey(row) : ''
  const attempts = row ? traceAttempts(row) : []
  const others = row ? traceCandidates(row, attempts) : []
  const steps = attempts.length * 2 + 2
  React.useEffect(() => { setNode('overview'); setStep(null); setPlaying(false) }, [key])
  React.useEffect(() => { if (!active || view !== 'canvas' || !row || isTraceRunning(row)) setPlaying(false) }, [active, view, key])
  React.useLayoutEffect(() => { content.current?.scrollTo({ top: 0, left: 0 }) }, [view, key])
  React.useEffect(() => {
    if (!playing) return
    const timer = window.setInterval(() => {
      if (document.hidden || host().wbApp?.currentPage !== 'routing') return
      setStep(value => { const next = (value ?? 0) + 1; if (next >= steps - 1) setPlaying(false); return Math.min(next, steps - 1) })
    }, 950)
    return () => window.clearInterval(timer)
  }, [playing, steps])
  const selectNode = (next: TraceNode) => { setNode(next); setPlaying(false); setView('details') }
  const models = [...new Set([...state.models, state.filter.model].filter(Boolean))]
  return <div className='routing-workspace routing-trace-workspace'>
    <aside className='routing-trace-rail'>
      <div className='routing-model-head'><span>{t('请求记录')} <Badge variant='secondary' shape='tag'>{state.matched}</Badge></span><Button variant='ghost' size='icon' className='routing-icon-button' disabled={state.loading} title={t('刷新请求记录')} aria-label={t('刷新请求记录')} onClick={state.refresh}><RouteIcon name='refresh' size={14} /></Button></div>
      <div className='routing-trace-filters'><SegmentedControl aria-label={t('请求范围')} value={state.filter.range} onValueChange={range => state.change({ range })} options={[{ value: 'recent', label: t('最近请求') }, { value: 'history', label: t('历史记录') }]} />
        {state.filter.range === 'history' ? <Input aria-label={t('历史日期')} type='date' value={state.filter.date} onChange={event => state.change({ date: event.currentTarget.value })} /> : null}
        <Input aria-label={t('搜索模型或请求 ID')} placeholder={t('搜索模型或请求 ID')} value={state.filter.search} onChange={event => state.change({ search: event.currentTarget.value })} />
        <div className='routing-trace-filter-row'><Filter label={t('请求模型')} value={state.filter.model} options={[[ '', t('全部模型') ], ...models.map(model => [model, model] as [string, string])]} onChange={model => state.change({ model })} /><Filter label={t('请求状态')} value={state.filter.status} options={[[ '', t('全部状态') ], ['running', t('进行中')], ['switched', t('发生切换')], ['ok', t('成功')], ['error', t('失败')], ['sticky', t('粘性命中')]]} onChange={status => state.change({ status })} /></div>
      </div>
      <div className='routing-trace-request-list'>{state.entries.map(entry => <Button variant='ghost' key={requestKey(entry)} aria-pressed={requestKey(entry) === key} className={`routing-trace-request${requestKey(entry) === key ? ' is-selected' : ''}`} onClick={() => state.select(entry)}>
        <span className='routing-trace-request-title'><b title={requestModel(entry)}>{requestModel(entry)}</b><time>{new Date(entry.ts).toLocaleTimeString()}</time></span><span className='routing-trace-request-tags'><RequestBadge row={entry} />{(entry.attempts || 1) > 1 ? <Badge variant='warning' shape='tag'>{t('切换 {n} 次', { n: (entry.attempts || 1) - 1 })}</Badge> : entry.route?.stickyHit ? <Badge variant='brand' shape='tag'>{t('粘性命中')}</Badge> : null}<span>{isTraceRunning(entry) ? '—' : formatDuration(entry.durationMs)}</span></span><span className='routing-trace-request-target'>{entry.accountName || traceProvider(entry.provider, entry.providerLabel)}</span>
      </Button>)}{!state.entries.length ? <Empty>{state.loading ? t('正在读取…') : t('没有匹配的请求')}</Empty> : null}</div>
      <div className='routing-trace-pager'><Button variant='ghost' size='sm' disabled={state.loading || state.filter.offset === 0} onClick={() => state.change({ offset: Math.max(0, state.filter.offset - state.limit) })}>{t('上一页')}</Button><span>{Math.floor(state.filter.offset / state.limit) + 1} / {Math.max(1, Math.ceil(state.matched / state.limit))}</span><Button variant='ghost' size='sm' disabled={state.loading || state.filter.offset + state.limit >= state.matched} onClick={() => state.change({ offset: state.filter.offset + state.limit })}>{t('下一页')}</Button></div>
      <div className='routing-model-help'><RouteIcon name='info' /><p>{t('记录所有经过网关的请求，不要求绑定客户端。')}</p></div>
    </aside>
    <div className='routing-map-area'><div className='routing-map-head'><div className='routing-map-title'><b>{row ? requestModel(row) : t('请求跟踪')}</b>{row ? <RequestBadge row={row} /> : null}{row ? <code className='routing-trace-id'>{row.id}</code> : null}</div><div className='routing-map-actions'>{onWorkbench ? <Button variant='outline' size='sm' disabled={!row} onClick={() => row && onWorkbench(row)}>{t('在工作台查看')}</Button> : null}<Button variant='outline' size='sm' disabled={!row || isTraceRunning(row) || !attempts.length} onClick={() => { setView('canvas'); setStep(0); setPlaying(true); state.setFollowing(false) }}><RouteIcon name='refresh' />{t('逐步回放')}</Button><Button variant='ghost' size='sm' disabled={!row} onClick={() => row && onConfig(row.model)}>{t('查看配置')}</Button></div></div>
      {state.error ? <div className='routing-page-error' role='alert'><RouteIcon name='info' /><span>{state.error}</span><Button variant='ghost' size='sm' onClick={state.refresh}>{t('重试')}</Button></div> : null}
      {row ? <><div className='routing-trace-selection'><span><RouteIcon name={state.following ? 'traffic' : 'lock'} />{state.following ? t('跟随列表中的最新请求') : t('已固定这条请求，新请求不会打断当前查看')}</span><Button variant='link' size='sm' onClick={state.following ? () => state.setFollowing(false) : state.backToLive}>{state.following ? t('固定这条请求') : t('返回实时')}</Button></div>
        <div className='routing-trace-content-switch'><SegmentedControl aria-label={t('请求跟踪内容')} value={view} onValueChange={setView} options={[{ value: 'canvas', label: <><RouteIcon name='route' />{t('路由画布')}</> }, { value: 'details', label: <><RouteIcon name='requests' />{t('请求详情')}</> }]} />{view === 'canvas' ? <Label className='routing-trace-candidate-toggle'><Checkbox checked={candidates} disabled={!others.length} onCheckedChange={value => { setCandidates(value === true); if (value !== true && node.startsWith('candidate:')) setNode('overview') }} /><span>{t('显示其他候选')}</span><Badge variant='secondary' shape='tag'>{others.length}</Badge></Label> : <Button variant='link' size='sm' onClick={() => setView('canvas')}>{t('返回画布')}</Button>}</div>
        <div className='routing-trace-content' ref={content}>{view === 'canvas' ? <div className='routing-trace-canvas-view'><RoutingTraceCanvas row={row} selected={node} candidates={candidates} step={step} onSelect={selectNode} />{step != null ? <div className='routing-trace-replay-controls'><Button variant='outline' size='sm' onClick={() => { if (step >= steps - 1) setStep(0); setPlaying(!playing) }}>{playing ? t('暂停') : t('继续')}</Button><span>{t('按步骤回放，不重新发送请求')}</span><input aria-label={t('回放步骤')} type='range' min={0} max={steps - 1} value={step} onChange={event => { setStep(Number(event.currentTarget.value)); setPlaying(false) }} /><span>{step + 1} / {steps}</span><Button variant='ghost' size='sm' onClick={() => { setStep(null); setPlaying(false) }}>{t('完整过程')}</Button></div> : null}</div> : <RoutingTraceDetails row={row} selected={node} onSelect={selectNode} />}</div>
        <div className='routing-map-foot'><span><RouteIcon name='shield' size={13} />{t('历史只读，当前配置的修改不会改变这条请求的快照。')}</span></div>
      </> : <Empty>{state.loading ? t('正在读取…') : t('选择一条请求查看执行过程')}</Empty>}
    </div>
  </div>
}
