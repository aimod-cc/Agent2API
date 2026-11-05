import * as React from 'react'
import { Button, SegmentedControl, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { RoutingStateText } from './routing-state-text'
import { candidateReason, attemptState, isTraceRunning, isTraceSuccess, requestKey, traceAttempts, traceCandidates, traceProvider, type TraceCandidate } from './routing-trace-model'
import type { ReplayRow } from './routing-types'

export function RoutingReplayCandidates({ row }: { row: ReplayRow }) {
  const [expanded, setExpanded] = React.useState(false)
  const [filter, setFilter] = React.useState<'all' | 'available' | 'skipped'>('all')
  React.useEffect(() => { setExpanded(false); setFilter('all') }, [requestKey(row)])
  const candidates = traceCandidates(row)
  const attempts = traceAttempts(row)
  const running = isTraceRunning(row), success = isTraceSuccess(row)
  const count = candidates.filter(candidate => candidate.kind === 'available').length
  const skipped = candidates.length - count
  const filtered = candidates.filter(candidate => filter === 'all' || candidate.kind === filter)
  const shown = expanded ? filtered : candidates.slice(0, 3)
  const total = (row.route?.orderedTotal ?? count) + (row.route?.blockedTotal ?? skipped)
  const state = (candidate: TraceCandidate) => {
    const picked = candidate.candidateId && row.route?.pickedCandidate ? candidate.candidateId === row.route.pickedCandidate
      : candidate.id === row.accountId && (!candidate.model || !row.upstreamModel || candidate.model === row.upstreamModel)
    const tried = [...attempts].reverse().find(attempt => attempt.candidateId && candidate.candidateId ? attempt.candidateId === candidate.candidateId : attempt.accountId
      ? attempt.accountId === candidate.id && (!candidate.model || candidate.model === attempt.model)
      : Boolean(attempt.account) && attempt.account === candidate.name && attempt.provider === candidate.provider)
    const triedState = tried ? attemptState(row, tried) : null
    const label = picked && success ? t('最终承载') : triedState === 'failed' || (picked && !success && !running) ? t('已尝试 · 失败')
      : triedState === 'running' || (picked && running) ? t('尝试中') : triedState === 'success' ? t('上游已响应')
        : triedState === 'unknown' ? t('结果未记录') : candidate.kind === 'available' ? t('未轮到') : ''
    return { picked, label, tone: (picked && success || triedState === 'success' ? 'success' : triedState === 'failed' || (picked && !success && !running) ? 'destructive' : 'muted') as 'success' | 'destructive' | 'muted' }
  }
  return <section className='routing-exec-section'>
    <div className='routing-exec-section-head'><h3>{t('候选与跳过')}</h3><span>{t('按请求开始时的顺位')}</span>
      <Button variant='link' size='sm' aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? t('收起') : t('全部候选')} · {candidates.length}</Button>
    </div>
    {expanded ? <SegmentedControl className='routing-exec-filter' aria-label={t('候选筛选')} value={filter} onValueChange={setFilter} options={[
      { value: 'all', label: t('全部'), count: candidates.length },
      { value: 'available', label: t('候选'), count },
      { value: 'skipped', label: t('被跳过'), count: skipped },
    ]} /> : null}
    {shown.length ? <div className='routing-exec-list'>{shown.map((candidate, index) => {
      const presentation = state(candidate)
      const rank = candidate.kind === 'skipped' ? '—' : candidates.filter(item => item.kind === 'available').indexOf(candidate) + 1
      return <div className={cn('routing-exec-row', presentation.picked && success && 'is-picked')} key={candidate.candidateId || `${candidate.memberId || ''}:${candidate.id}:${index}`}>
        <span className='routing-exec-rank'>{rank}</span>
        <div className='routing-exec-row-main'><b>{candidate.name || candidate.id || t('账号未记录')}</b>
          <span className='routing-exec-provider'>{traceProvider(candidate.provider, candidate.providerLabel)}{candidate.model ? ` · ${candidate.model}` : ''}</span>
          <p>{candidate.reason || candidateReason(row, candidate)}</p>
          {candidate.resetAt ? <time className='routing-exec-note'>{new Date(candidate.resetAt).toLocaleString()}</time> : null}
        </div>
        <span className='routing-exec-row-state'>{presentation.label ? <><StatusDot variant={presentation.tone} />{presentation.label}</> : <RoutingStateText state={candidate.state} />}</span>
      </div>
    })}</div> : <p className='routing-exec-note'>{row.route ? t('没有匹配的候选') : t('路由快照未记录')}</p>}
    {!expanded && candidates.length > shown.length ? <Button variant='link' size='sm' className='routing-exec-expand' onClick={() => setExpanded(true)}>
      {t('展开其余 {n} 个候选', { n: candidates.length - shown.length })}</Button> : null}
    {total > candidates.length ? <p className='routing-exec-note'>{t('历史快照保留了 {shown} / {total} 个候选明细，其余未记录。', { shown: candidates.length, total })}</p> : null}
  </section>
}
