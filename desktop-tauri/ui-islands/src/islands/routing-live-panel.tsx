import * as React from 'react'
import { Badge, Button, Spinner, StatusDot } from '@ui'
import { t } from '../i18n'
import { formatDuration, RouteIcon, stickyLabel, strategyLabel } from './routing-ui'
import { RoutingStateText } from './routing-state-text'
import type { RouteEntry, RoutePreview } from './routing-types'

/** 服务端只读预演；紧凑摘要与完整候选共用同一结果。 */
export function RoutingLivePanel({ workspace, model, loading, error, dirty, globalScope, modelsAvailable, onOpenScope, onRefresh, preview = false, compact = false }: {
  workspace: Pick<RoutePreview, 'customized' | 'queue'> | null
  model: string
  preview?: boolean
  compact?: boolean
  loading: boolean
  error: string
  dirty: boolean
  globalScope: boolean
  modelsAvailable: boolean
  onOpenScope(model: string): void
  onRefresh(): void
}) {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  const customized = Boolean(workspace?.customized)
  const queue = workspace?.queue
  const available = queue?.ordered || []
  const blocked = queue?.blocked || []
  const next = available[0]
  const badge = preview ? { dot: 'warning' as const, text: t('草稿预演') } : dirty
    ? { dot: 'warning' as const, text: t('按已保存配置') }
    : customized ? { dot: 'muted' as const, text: t('独立配置') }
      : { dot: 'success' as const, text: t('继承全局') }

  const side = (entry: RouteEntry) => {
    const label = entry.providerLabel || entry.provider
    const reset = entry.resetAt && entry.resetAt > now
      ? t('{time} 后恢复', { time: formatDuration(entry.resetAt - now) })
      : ''
    return <div className='routing-live-side'><span>{label}</span>{entry.model ? <code>{entry.model}</code> : null}{reset ? <span className='routing-live-reset'>{reset}</span> : null}</div>
  }

  if (compact) return <div className='routing-preview-summary' aria-label={t('草稿预演')}>
    {globalScope && customized && workspace ? <div className='routing-live-banner'>
      <b>{t('{model} 有独立配置', { model })}</b><span>{t('左侧修改的是全局默认，不会影响它。')}</span>
      <Button variant='link' size='sm' onClick={() => onOpenScope(model)}>{t('打开该模型配置')}</Button>
    </div> : null}
    {loading ? <div className='routing-live-loading'><Spinner />{t('计算中…')}</div>
      : error ? <p className='routing-live-error' role='alert'>{error}<Button variant='link' size='sm' onClick={onRefresh}>{t('重试')}</Button></p>
        : workspace ? <>
          {available.slice(0, 3).map((entry, index) => <div className={`routing-preview-row${index === 0 ? ' is-first' : ''}`} key={entry.candidateId || `${entry.memberId || ''}:${entry.id}`}>
            <span className='routing-live-rank'>{String(index + 1).padStart(2, '0')}</span>
            <div className='routing-preview-row-main'><b title={entry.name}>{entry.name}</b><span title={entry.reason}>{entry.providerLabel || entry.provider}{entry.model ? ` · ${entry.model}` : ''}</span></div>
            <Badge shape='tag' variant={index === 0 ? 'brand' : 'ghost'}>{index === 0 ? t('预演首选') : t('候选')}</Badge>
          </div>)}
          {!available.length ? <p className='routing-live-empty'>{t('没有可用账号')}</p> : null}
          <p className='routing-preview-counts'>{t('{available} 可用 · {blocked} 跳过', { available: available.length, blocked: blocked.length })}</p>
        </> : <p className='routing-live-empty'>{modelsAvailable ? t('选择预览模型后查看队列') : t('还没有可用模型，先在账号页添加账号')}</p>}
  </div>

  return <aside className='routing-live-panel' aria-label={t('此刻队列')}>
    <div className='routing-live-head'>
      <b>{t('此刻队列')}</b>
      <span className='routing-live-badge'><StatusDot variant={badge.dot} />{badge.text}</span>
      <span className='routing-live-model'>{model}</span>
      <Button variant='ghost' size='sm' className='routing-live-refresh' onClick={onRefresh} title={t('刷新')} aria-label={t('刷新')}>
        <RouteIcon name='refresh' size={13} />
      </Button>
    </div>

    {globalScope && customized && workspace ? <div className='routing-live-banner'>
      <b>{t('{model} 有独立配置', { model })}</b>
      <span>{t('左侧修改的是全局默认，不会影响它。')}</span>
      <Button variant='outline' size='sm' onClick={() => onOpenScope(model)}>{t('打开该模型配置')}</Button>
    </div> : null}

    {loading && !workspace ? <div className='routing-live-loading'><Spinner /></div> : null}
    {error ? <p className='routing-live-error' role='alert'>{error}</p> : null}
    {!loading && !error && !workspace ? <div className='routing-live-empty'>
      {modelsAvailable ? t('选择预览模型后查看队列') : t('还没有可用模型，先在账号页添加账号')}
    </div> : null}

    {workspace ? <>
      <div className='routing-live-summary'>
        {next ? <>
          <div className='routing-live-eyebrow'>{preview ? t('预演首选') : t('当前可用队首')}</div>
          <div className='routing-live-name'>{next.model || next.name}</div>
          {next.model ? <p className='routing-live-reason'>{next.providerLabel || next.provider} · {next.name}</p> : null}
          <p className='routing-live-reason'>{next.reason}</p>
        </> : <>
          <div className='routing-live-eyebrow'>{t('下一条请求')}</div>
          <div className='routing-live-name is-empty'>{t('没有可用账号')}</div>
          <p className='routing-live-reason'>{t('先处理凭证、限额或参与范围。')}</p>
        </>}
        <div className='routing-live-counts'>
          <span>{t('{available} 可用 · {blocked} 跳过', { available: available.length, blocked: blocked.length })}</span>
          <span>{strategyLabel(queue?.strategy)} · {t('粘滞 {mode}', { mode: stickyLabel(queue?.sticky) })}</span>
        </div>
      </div>

      {available.length ? <>
        <div className='routing-live-subhead'><span>{t('可用候选')}</span><span>{available.length}</span></div>
        <div className='routing-live-list'>{available.map((entry, index) => <div className='routing-live-row' key={entry.candidateId || `${entry.memberId || ''}:${entry.id}`}>
          <span className='routing-live-rank'>{index + 1}</span>
          <div className='routing-live-main'>
            <div className='routing-live-title'><b>{entry.name}</b><RoutingStateText state={entry.state} /></div>
            <p className='routing-live-row-reason'>{entry.reason}</p>
          </div>
          {side(entry)}
        </div>)}</div>
      </> : null}

      {blocked.length ? <>
        <div className='routing-live-subhead'><span>{t('被跳过')}</span><span>{blocked.length}</span></div>
        <div className='routing-live-list'>{blocked.map(entry => <div className='routing-live-row is-blocked' key={entry.candidateId || `${entry.memberId || ''}:${entry.id}`}>
          <span className='routing-live-rank'>–</span>
          <div className='routing-live-main'>
            <div className='routing-live-title'><b>{entry.name}</b><RoutingStateText state={entry.state} /></div>
            <p className='routing-live-row-reason'>{entry.reason}</p>
          </div>
          {side(entry)}
        </div>)}</div>
      </> : null}

      <div className='routing-live-foot'>
        {dirty ? <p>{t('有未保存修改，预演仍按已保存配置。')}</p> : null}
        <p>{t('只读预演，不会推进轮转；保存后才会影响真实请求。')}</p>
        <p>{t('真实请求还会考虑当时的会话绑定、并发与限额状态。')}</p>
      </div>
    </> : null}
  </aside>
}
