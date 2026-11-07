import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger } from '@ui'
import { t } from '../i18n'
import { RouteIcon, formatDuration } from './routing-ui'
import { errorText, formatTraceTokens, isTraceRunning, requestModel, traceCandidates, traceProvider } from './routing-trace-model'
import { execAccounts, execAttemptName, execTiming, type ExecAccount } from './routing-exec-model'
import type { ReplayRow } from './routing-types'

export function RoutingExecDuration({ row, now }: { row: ReplayRow; now: number }) {
  const { total, firstFrame, response, scale, timings } = execTiming(row, now)
  const attempts = row.attempts ?? timings.length
  const recorded = timings.filter(timing => !timing.attempt.inferred).length
  const multi = Math.max(attempts, timings.length) > 1
  const metrics = [
    `${isTraceRunning(row) ? t('已用时') : t('总耗时')} ${formatDuration(total)}`,
    `${t('首帧')} ${formatDuration(firstFrame)}`,
    `${t('输出')} ${row.completionTokens == null ? '—' : `${formatTraceTokens(row.completionTokens)} tokens`}`,
  ]
  if (row.completionTokens == null && row.totalTokens != null) metrics.push(t('总计 {n} tokens', { n: formatTraceTokens(row.totalTokens) }))
  return <section className='routing-exec-section'>
    <div className='routing-exec-section-head'><h3>{t('执行耗时')}</h3><span className='routing-exec-section-note'>{metrics.join(' · ')}</span></div>
    <div className='routing-exec-bars'>
      {timings.map(timing => {
        const { attempt, segments } = timing
        const info = [
          multi ? `${t('本次用时')} ${formatDuration(timing.duration)}` : '',
          `${t('响应到达')} ${timing.response == null ? '—' : `+${formatDuration(timing.response)}`}`,
          attempt.retries?.length ? `${t('同账号内部重试')} ${attempt.retries.length}` : '',
        ].filter(Boolean).join(' · ')
        return <div className='routing-exec-bar-row' data-multi={multi ? '' : undefined} key={attempt.index}>
          {multi ? <span className='routing-exec-bar-owner' title={[traceProvider(attempt.provider, attempt.providerLabel), execAttemptName(attempt), attempt.model].filter(Boolean).join(' · ')}>{execAttemptName(attempt)}</span> : null}
          <div className='routing-exec-bar-cell'>
            {segments.length ? <div className='routing-exec-bar-track' role='img' aria-label={segments.map(segment => segment.label).join(' · ')}>
              {segments.map((segment, index) => <span key={index} data-tone={segment.tone} title={segment.label}
                style={{ left: `${segment.start / scale * 100}%`, width: `${(segment.end - segment.start) / scale * 100}%` }} />)}
            </div> : null}
            <span className='routing-exec-bar-note'>{info}</span>
            {!multi && attempt.notice ? <span className='routing-exec-bar-note'>{errorText(attempt.notice)}</span> : null}
          </div>
          {multi ? <span className='routing-exec-bar-result' data-tone={timing.tone}>{timing.label}</span> : null}
        </div>
      })}
      {!timings.length ? <p className='routing-exec-note'>{t('响应到达')} {formatDuration(response)} · {t('没有保存调用明细')}</p> : null}
      {attempts > recorded ? <p className='routing-exec-note'>{t('共尝试 {total} 次，保存了 {saved} 条明细', { total: attempts, saved: recorded })}</p> : null}
    </div>
  </section>
}

function CandidateRow({ item, model }: { item: ExecAccount; model: string }) {
  return <div className='routing-exec-candidate' role='row'>
    <div className='routing-exec-candidate-account' role='cell'>
      <div><b>{item.name}</b><span>{item.provider}</span></div>
      {item.model && item.model !== model ? <code>{item.model}</code> : null}
    </div>
    <span className='routing-exec-candidate-state' role='cell' data-tone={item.tone}>{item.label}</span>
    <span className='routing-exec-candidate-reason' role='cell'>{item.reason}</span>
  </div>
}

export function RoutingExecCandidates({ row }: { row: ReplayRow }) {
  const [expanded, setExpanded] = React.useState(false)
  React.useEffect(() => { setExpanded(false) }, [row.id, row.ts])
  const entries = execAccounts(row)
  const other = entries.filter(item => !item.current)
  const tried = other.filter(item => item.kind === 'tried')
  const skipped = other.filter(item => item.kind === 'skipped')
  const shown = [...tried, ...skipped.slice(0, 2)]
  const rest = other.filter(item => !shown.includes(item))
  const route = row.route
  const saved = (route?.ordered?.length || 0) + (route?.blocked?.length || 0)
  const recordedTotal = (route?.orderedTotal ?? route?.ordered?.length ?? 0) + (route?.blockedTotal ?? route?.blocked?.length ?? 0)
  const partial = saved < recordedTotal
  const empty = !entries.length ? t('没有保存其他候选信息') : entries.some(item => item.current) ? t('没有其他候选账号') : t('没有保存调用明细')
  return <section className='routing-exec-section'>
    <div className='routing-exec-section-head'><h3>{t('选路情况')}</h3>
      <span className='routing-exec-section-note'>{t('已保存 {n} 个候选', { n: traceCandidates(row).length })}</span>
    </div>
    <Collapsible open={expanded} onOpenChange={setExpanded}>
      {other.length ? <div className='routing-exec-candidates' role='table' aria-label={t('选路情况')}>
        <div className='routing-exec-candidate is-heading' role='row'><span role='columnheader'>{t('账号')}</span><span role='columnheader'>{t('本次状态')}</span><span role='columnheader'>{t('原因')}</span></div>
        <div role='rowgroup'>{shown.map(item => <CandidateRow key={item.key} item={item} model={requestModel(row)} />)}</div>
        <CollapsibleContent role='rowgroup'>{rest.map(item => <CandidateRow key={item.key} item={item} model={requestModel(row)} />)}</CollapsibleContent>
      </div> : <p className='routing-exec-note'>{empty}</p>}
      {rest.length ? <CollapsibleTrigger render={<Button variant='link' size='xs' />} className='routing-exec-more'>
        <RouteIcon name={expanded ? 'chevronUp' : 'chevronDown'} />{expanded ? t('收起其他候选') : t('查看其他 {n} 个候选', { n: rest.length })}
      </CollapsibleTrigger> : null}
    </Collapsible>
    {partial ? <p className='routing-exec-note'>{t('候选快照仅保存部分记录，未保存的账号无法回放')}</p> : null}
  </section>
}
