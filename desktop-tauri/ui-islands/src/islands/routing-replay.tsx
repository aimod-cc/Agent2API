import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Empty, SegmentedControl, cn,
} from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow, type RouteDecision } from './routing-types'
import { formatDuration, RouteIcon, RouteStateBadge, stickyLabel, strategyLabel, timeText } from './routing-ui'
import { ReplayStickyBinding } from './routing-replay-sticky'
import { requestClientLabel } from '../../../ui-shared/model-api-key'

export function RouteReplayFacts({ route, accountId, accountName, compact = false }: {
  route: RouteDecision
  accountId?: string
  accountName?: string
  compact?: boolean
}) {
  const ordered = Array.isArray(route.ordered) ? route.ordered : []
  const blocked = Array.isArray(route.blocked) ? route.blocked : []
  const picked = accountId || route.picked || ''
  const pickedName = accountName || [...ordered, ...blocked].find(entry => entry.id === picked)?.name || picked
  return <div className={cn('routing-replay-facts', compact && 'is-compact')}>
    <div className='routing-replay-chips'><Badge variant='brand' shape='tag'>{strategyLabel(route.strategy)}</Badge><Badge variant='outline' shape='tag'>{t('粘性：{mode}', { mode: stickyLabel(route.sticky) })}</Badge>
      {route.stickyHit ? <Badge variant='success' shape='tag'>{t('命中会话粘滞')}</Badge> : null}
      {route.scope ? <Badge variant='secondary' shape='tag'>{t('模型独立配置')}</Badge> : null}</div>
    <div className='routing-replay-note'>{t('这是请求当时保存的选路快照，修改当前路由不会改变这份记录。')}</div>
    {ordered.length ? <section><h4>{t('候选顺序')}</h4><ol className='routing-replay-queue'>
      {ordered.map((entry, index) => <li key={`${entry.id}-${index}`} className={entry.id === picked ? 'is-picked' : ''}>
        <span className='routing-replay-rank'>{index + 1}</span><div className='routing-replay-entry'><b>{entry.name || entry.id}</b><span>{host().wbProviders?.labelOf(entry.provider) || entry.provider}</span><p>{entry.reason}</p></div>
        {entry.id === picked ? <Badge variant='brand' shape='tag'>{t('选中')}</Badge> : null}
      </li>)}
    </ol></section> : null}
    {blocked.length ? <section><h4>{t('被跳过的账号')}</h4><div className='routing-replay-blocked'>
      {blocked.map((entry, index) => <div key={`${entry.id}-${index}`}><RouteStateBadge state={entry.state} /><b>{entry.name || entry.id}</b><p>{entry.reason}</p></div>)}
    </div></section> : null}
    {picked ? <div className='routing-replay-picked'><RouteIcon name='route' /><span>{t('最终承载：{picked}', { picked: pickedName })}</span></div> : null}
    <p className='routing-replay-note'>{t('候选与跳过清单各最多保存十二项；重试和跨账号切换见尝试明细。')}</p>
  </div>
}

function ReplayDetail({ row, onDetails }: { row: ReplayRow; onDetails(): void }) {
  const success = row.status >= 200 && row.status < 300 && !row.error
  return <div className='routing-replay-detail'>
    <div className='routing-replay-detail-title'><h3>{row.clientModel || row.model}</h3><Badge variant={row.status === 0 ? 'info' : success ? 'success' : 'destructive'} shape='tag'>{row.status === 0 ? t('进行中') : success ? t('成功') : t('失败')}</Badge></div>
    <div className='routing-replay-metadata'><span>{timeText(row.ts)}</span><span>{formatDuration(row.durationMs)}</span><span>{requestClientLabel(row, t)}</span><code>{row.id}</code></div>
    {row.error ? <p className='routing-replay-error'>{row.error}</p> : null}
    <ReplayStickyBinding row={row} />
    {row.route ? <RouteReplayFacts route={row.route} accountId={row.accountId} accountName={row.accountName} /> : <Empty>{t('这条请求没有保存路由决策')}</Empty>}
    {row.attemptDetails?.length ? <section className='routing-replay-attempts'><h4>{t('尝试明细')}</h4>{row.attemptDetails.map((attempt, index) => <div key={index}>
      <span className='routing-replay-rank'>{index + 1}</span><div><b>{attempt.account || attempt.accountName || attempt.accountId || host().wbProviders?.labelOf(attempt.provider || '') || attempt.provider || '—'}</b><p>{attempt.error || attempt.notice || t('上游请求已发出')}</p></div><Badge shape='tag' variant={attempt.error ? 'destructive' : 'outline'}>{attempt.status ?? '—'}</Badge>
    </div>)}</section> : null}
    <Button variant='outline' onClick={onDetails}>{t('打开完整请求详情')}</Button>
  </div>
}

export function RoutingReplayDialog({ model, initialRow, onClose }: { model: string; initialRow?: ReplayRow | null; onClose(): void }) {
  const [entries, setEntries] = React.useState<ReplayRow[]>(initialRow ? [initialRow] : [])
  const [selected, setSelected] = React.useState<ReplayRow | null>(initialRow || null)
  const [offset, setOffset] = React.useState(0)
  const [matched, setMatched] = React.useState(0)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [status, setStatus] = React.useState('all')
  const [revision, refresh] = React.useReducer(value => value + 1, 0)
  const limit = 30
  React.useEffect(() => {
    let alive = true
    setLoading(true)
    void bridge().getRoutingReplays({ model: model || undefined, offset, limit, status: status === 'all' ? undefined : status }).then(result => {
      if (!alive) return
      if (result.available === false) throw new Error(t('请求记录数据库不可用'))
      setEntries(result.entries)
      setMatched(result.matched)
      setSelected(current => result.entries.find(row => row.id === current?.id) || (offset === 0 && initialRow && status === 'all' ? initialRow : result.entries[0] || null))
      setError('')
    }).catch(error => { if (alive) setError(errorMessage(error)) }).finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [model, offset, revision, status, initialRow])
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='routing-replay-dialog'>
    <DialogHeader><RouteIcon name='route' /><DialogTitle>{t('路由回放')} · {model || t('全部模型')}</DialogTitle><Button variant='ghost' size='sm' disabled={loading} onClick={refresh}>{t('刷新')}</Button></DialogHeader>
    <DialogBody className='routing-replay-body'>
      <aside className='routing-replay-sidebar'><SegmentedControl className='mx-3 mt-3' aria-label={t('状态')} value={status} options={[
        { value: 'all', label: t('全部') }, { value: 'error', label: t('失败') }, { value: 'sticky', label: t('粘性命中') },
      ]} onValueChange={value => { setStatus(value); setOffset(0) }} />
        <div className='routing-replay-list'>{entries.map((row, index) => {
          const success = row.status >= 200 && row.status < 300 && !row.error
          return <Button key={`${row.id}-${index}`} variant='ghost' className={cn('routing-replay-row', row.id === selected?.id && 'is-selected')} onClick={() => setSelected(row)}>
            <div><b>{row.clientModel || row.model}</b><Badge shape='tag' variant={row.status === 0 ? 'info' : success ? 'success' : 'destructive'}>{row.status === 0 ? t('进行中') : row.status}</Badge></div>
            <span>{row.accountName || row.providerLabel || '—'}</span><span>{new Date(row.ts).toLocaleTimeString()} · {formatDuration(row.durationMs)}{row.route?.stickyHit ? ` · ${t('会话粘住')}` : ''}</span>
          </Button>
        })}
          {!entries.length ? <Empty>{loading ? t('正在读取…') : t('当前视图没有路由回放记录')}</Empty> : null}
        </div>
      </aside>
      <div className='routing-replay-main'>{error ? <p role='alert' className='routing-replay-error'>{error}</p> : null}{selected ? <ReplayDetail row={selected} onDetails={() => { const row = selected; onClose(); host().wbRequestDetail?.open(row.id, row) }} /> : <Empty>{t('选择一条请求查看选路过程')}</Empty>}</div>
    </DialogBody>
    <DialogFooter><span className='mr-auto text-xs text-muted-foreground'>{t('共 {n} 条路由回放', { n: matched })}</span><Button variant='outline' size='sm' disabled={loading || offset === 0} onClick={() => setOffset(Math.max(0, offset - limit))}>{t('上一页')}</Button><span className='text-xs'>{Math.floor(offset / limit) + 1} / {Math.max(1, Math.ceil(matched / limit))}</span><Button variant='outline' size='sm' disabled={loading || offset + limit >= matched} onClick={() => setOffset(offset + limit)}>{t('下一页')}</Button><Button onClick={onClose}>{t('关闭')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
