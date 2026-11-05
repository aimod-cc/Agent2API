import * as React from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { RouteIcon, formatDuration } from './routing-ui'
import { attemptDuration, attemptLabel, attemptState, errorText, relativeTime, traceProvider, type TraceAttempt } from './routing-trace-model'
import type { ReplayRow } from './routing-types'

export function RoutingReplayAttempt({ row, attempt }: { row: ReplayRow; attempt: TraceAttempt }) {
  const state = attemptState(row, attempt)
  const name = attempt.account || attempt.accountName || attempt.accountId || t('账号未记录')
  const provider = traceProvider(attempt.provider, attempt.providerLabel)
  const detail = errorText(attempt.error || attempt.notice)
  return <li className={cn('routing-timeline-step', `is-${state}`)}>
    <span className='routing-timeline-mark' aria-hidden='true'>{state === 'success' ? <RouteIcon name='check' size={10} /> : state === 'failed' ? '!' : <StatusDot variant='muted' pulse={state === 'running'} />}</span>
    <div className='routing-timeline-body'>
      <div className='routing-timeline-title'><b>{t('尝试 {n}', { n: attempt.index + 1 })} · {provider} · {name}</b>
        <span>{attempt.status != null ? `${attempt.status} · ` : ''}{attemptDuration(row, attempt)}</span></div>
      <div className='routing-timeline-meta'><span>{attemptLabel(row, attempt)}</span>
        {attempt.model ? <code>{attempt.model}</code> : null}
        {attempt.stickyHit ? <span>{t('粘滞命中')}</span> : null}
        {attempt.responseAt != null ? <span>{t('响应到达')} +{relativeTime(row, attempt.responseAt)}</span> : null}
        {attempt.startedAt != null ? <span>{t('开始于 +{time}', { time: relativeTime(row, attempt.startedAt) })}</span> : null}
      </div>
      {attempt.inferred ? <p className='routing-exec-note'>{t('未保存完整尝试明细，仅显示请求记录中的承载信息。')}</p> : null}
      {detail ? <Collapsible className='routing-attempt-detail'>
        <p>{errorText(detail, 180)}</p>
        {detail.length > 180 ? <><CollapsibleTrigger className='routing-detail-toggle'>{t('展开完整原因')}<RouteIcon name='chevronDown' size={12} /></CollapsibleTrigger>
          <CollapsibleContent><p className='routing-attempt-full'>{detail}</p></CollapsibleContent></> : null}
      </Collapsible> : null}
      {attempt.reason ? <p className='routing-exec-note'>{attempt.reason}</p> : null}
      {attempt.retries?.length ? <Collapsible className='routing-exec-retries'>
        <CollapsibleTrigger className='routing-detail-toggle'><RouteIcon name='chevronDown' size={12} />{t('同账号内部重试')} · {attempt.retries.length}</CollapsibleTrigger>
        <CollapsibleContent>{attempt.retries.map((retry, index) => <p key={index}>{retry.status ?? '—'} · {retry.reason || t('未知原因')} · {t('退避 {time}', { time: formatDuration(retry.delayMs) })}</p>)}</CollapsibleContent>
      </Collapsible> : null}
    </div>
  </li>
}
