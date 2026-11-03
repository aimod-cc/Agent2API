import * as React from 'react'
import { Button, Spinner, StatusDot } from '@ui'
import { t } from '../i18n'
import { formatDuration, RouteIcon, stickyLabel, strategyLabel } from './routing-ui'
import { RoutingStateText } from './routing-state-text'
import type { RouteEntry, RoutingWorkspace } from './routing-types'

/**
 * 配置页右栏的「此刻队列」。
 *
 * 只读：数据来自后端按**已保存配置**算出的队列预览，不推进轮转游标，也不改变
 * 会话绑定。草稿有未保存修改时只提示「预演仍按已保存配置」，不假装预演了草稿。
 */
export function RoutingLivePanel({ workspace, model, loading, error, dirty, globalScope, modelsAvailable, onOpenScope, onRefresh }: {
  workspace: RoutingWorkspace | null
  model: string
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
  const badge = dirty
    ? { dot: 'warning' as const, text: t('按已保存配置') }
    : customized ? { dot: 'muted' as const, text: t('独立配置') }
      : { dot: 'success' as const, text: t('继承全局') }

  const side = (entry: RouteEntry) => {
    const label = entry.providerLabel || entry.provider
    const reset = entry.resetAt && entry.resetAt > now
      ? t('{time} 后恢复', { time: formatDuration(entry.resetAt - now) })
      : ''
    return <div className='routing-live-side'><span>{label}</span>{reset ? <span className='routing-live-reset'>{reset}</span> : null}</div>
  }

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
          <div className='routing-live-eyebrow'>{t('下一条请求将走')}</div>
          <div className='routing-live-name'>{next.name}</div>
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
        <div className='routing-live-list'>{available.map((entry, index) => <div className='routing-live-row' key={entry.id}>
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
        <div className='routing-live-list'>{blocked.map(entry => <div className='routing-live-row is-blocked' key={entry.id}>
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
      </div>
    </> : null}
  </aside>
}
