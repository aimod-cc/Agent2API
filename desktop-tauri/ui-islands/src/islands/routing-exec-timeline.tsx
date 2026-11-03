import * as React from 'react'
import { Button, SegmentedControl, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { clientLabel } from './routing-flow-model'
import { RoutingStateText } from './routing-state-text'
import { RouteIcon, formatDuration, stickyLabel, strategyLabel } from './routing-ui'
import {
  attemptDuration, attemptLabel, attemptState, errorText, isTraceRunning, isTraceSuccess, requestKey, requestModel,
  requestSummary, traceAttempts, traceCandidates, tracePhase, traceProvider,
} from './routing-trace-model'
import type { ReplayRow } from './routing-types'

/**
 * 单条请求的回放：结果 → 请求进入 → 作用范围 → 候选队列 → 尝试记录 → 结果 → 配置快照。
 *
 * 候选顺序、跳过原因、尝试明细全部来自请求当时保存的快照，不用当前队列重算。
 */
export function RoutingExecTimeline({ row, now, onDetail, onConfig }: {
  row: ReplayRow
  now: number
  onDetail(row: ReplayRow): void
  onConfig(model: string): void
}) {
  const [filter, setFilter] = React.useState<'all' | 'available' | 'skipped'>('all')
  React.useEffect(() => setFilter('all'), [requestKey(row)])

  const attempts = traceAttempts(row)
  const candidates = traceCandidates(row)
  const running = isTraceRunning(row)
  const success = isTraceSuccess(row)
  const snapshot = row.route?.configSnapshot
  const strategy = snapshot?.strategy || row.route?.strategy
  const sticky = snapshot?.sticky || row.route?.sticky
  const account = row.accountName || traceProvider(row.provider, row.providerLabel)
  const status = running ? tracePhase(row.phase) : success ? String(row.status) : t('失败')
  const tone = running || success ? 'success' : 'destructive'
  const availableCount = candidates.filter(candidate => candidate.kind === 'available').length
  const skippedCount = candidates.filter(candidate => candidate.kind === 'skipped').length
  const shown = candidates.filter(candidate => filter === 'all' || candidate.kind === filter)

  return <div className='routing-exec'>
    <header className='routing-exec-result'>
      <div className='routing-exec-overline'>{t('请求结论')}</div>
      <h2>{running ? t('正在由 {account} 处理', { account }) : success ? t('最终由 {account} 承接', { account }) : t('请求失败')}</h2>
      <p title={row.error || undefined}>{requestSummary(row)}</p>
      <div className='routing-exec-meta'>
        <span className='routing-state-text'><StatusDot variant={tone} />{status}</span>
        <span>{running ? t('进行中') : formatDuration(row.durationMs)}</span>
        <span>{requestModel(row)}</span>
        <span>{clientLabel(row.client || '')}</span>
      </div>
    </header>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'><span>{t('请求进入')}</span></div>
      <div className='routing-exec-kv'>
        <span>{new Date(row.ts).toLocaleString()}</span>
        <span>{row.clientModel && row.model && row.clientModel !== row.model
          ? t('客户端模型 {client} → 实际模型 {model}', { client: row.clientModel, model: row.model })
          : requestModel(row)}</span>
      </div>
      {row.route?.sessionId || row.route?.stickyId ? <div className='routing-exec-kv'>
        <span>{t('会话标识 {id} · 来源 {source}', {
          id: (row.route.sessionId || row.route.stickyId || '').slice(0, 12),
          source: row.route.sessionSource === 'content-hash' ? t('内容哈希（推断）') : row.route.sessionSource || t('未记录'),
        })}</span>
      </div> : null}
    </section>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'>
        <span>{t('作用范围')}</span>
        {strategy ? <span className='routing-exec-chip'>{strategyLabel(strategy)}</span> : null}
        {sticky ? <span className='routing-exec-chip'>{t('粘滞 {mode}', { mode: stickyLabel(sticky) })}</span> : null}
        <span className='routing-exec-chip'>{snapshot?.scope ? t('模型独立配置') : t('继承全局')}</span>
      </div>
      <div className='routing-exec-kv'>
        <span>{t('{n} 个候选', { n: row.route?.orderedTotal ?? availableCount })}</span>
        <span>{t('{n} 个跳过', { n: row.route?.blockedTotal ?? skippedCount })}</span>
        {snapshot?.providerCount != null ? <span>{t('{providers} 个提供商 / {accounts} 个账号', { providers: snapshot.providerCount, accounts: snapshot.accountCount ?? 0 })}</span> : null}
      </div>
    </section>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'>
        <span>{t('候选队列')}</span><span className='routing-exec-note'>{t('按请求开始时的顺位')}</span>
        <SegmentedControl className='routing-exec-filter' aria-label={t('候选筛选')} value={filter}
          onValueChange={value => setFilter(value as 'all' | 'available' | 'skipped')} options={[
            { value: 'all', label: t('全部') },
            { value: 'available', label: t('候选 {n}', { n: availableCount }) },
            { value: 'skipped', label: t('跳过 {n}', { n: skippedCount }) },
          ]} />
      </div>
      {shown.length ? <div className='routing-exec-list'>{shown.map((candidate, index) => {
        const picked = candidate.id === row.accountId
        const tried = attempts.find(attempt => attempt.accountId
          ? attempt.accountId === candidate.id
          : Boolean(attempt.account) && attempt.account === candidate.name && attempt.provider === candidate.provider)
        const triedState = tried ? attemptState(row, tried) : null
        return <div className={cn('routing-exec-row', picked && 'is-picked')} key={`${candidate.id}:${index}`}>
          <span className='routing-exec-rank'>{candidate.kind === 'skipped' ? '–' : index + 1}</span>
          <div className='routing-exec-row-main'>
            <div className='routing-exec-row-title'>
              <b>{candidate.name || candidate.id}</b>
              {picked && success ? <span className='routing-exec-final'><RouteIcon name='check' size={11} />{t('最终承载')}</span>
                : triedState === 'failed' || (picked && !success && !running)
                  ? <RoutingStateText state='tried'>{t('已尝试 · 失败')}</RoutingStateText>
                  : triedState === 'running' || (picked && running)
                    ? <RoutingStateText state='usable'>{t('尝试中')}</RoutingStateText>
                    : triedState === 'success' ? <RoutingStateText state='usable'>{t('上游已响应')}</RoutingStateText>
                      : candidate.kind === 'skipped' ? <RoutingStateText state={candidate.state} />
                        : <RoutingStateText state='usable' />}
            </div>
            <p>{candidate.reason || (candidate.kind === 'skipped' ? t('当时被候选过滤规则跳过') : '')}</p>
          </div>
          <div className='routing-exec-row-side'>
            <span>{traceProvider(candidate.provider, candidate.providerLabel)}</span>
            {candidate.resetAt ? <span>{new Date(candidate.resetAt).toLocaleTimeString()}</span> : null}
          </div>
        </div>
      })}</div> : <p className='routing-exec-note'>{t('没有匹配的候选')}</p>}
    </section>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'><span>{t('尝试记录')}</span><span className='routing-exec-note'>{t('{n} 次', { n: attempts.length })}</span></div>
      {attempts.length ? <div className='routing-exec-list'>{attempts.map(attempt => {
        const state = attemptState(row, attempt)
        const failed = state === 'failed'
        return <div className={cn('routing-exec-row', failed && 'is-failed')} key={attempt.index}>
          <span className='routing-exec-rank'>{attempt.index + 1}</span>
          <div className='routing-exec-row-main'>
            <div className='routing-exec-row-title'>
              <b>{attempt.account || attempt.accountName || attempt.accountId || t('账号未记录')}</b>
              {attempt.status != null ? <span className='routing-exec-chip'>{attempt.status}</span> : null}
              <span className='routing-exec-note'>{attemptLabel(row, attempt)}</span>
            </div>
            <p title={attempt.error || attempt.notice || undefined}>{errorText(attempt.error || attempt.notice, 180) || t('上游请求已发出')}</p>
            {attempt.retries?.length ? <div className='routing-exec-retries'>
              <b>{t('同账号内部重试')}</b>
              {attempt.retries.map((retry, index) => <p key={index}>{retry.status ?? '—'} · {retry.reason || t('未知原因')} · {t('退避 {time}', { time: formatDuration(retry.delayMs) })}</p>)}
            </div> : null}
          </div>
          <div className='routing-exec-row-side'>
            <span>{attemptDuration(row, attempt)}</span>
            {attempt.model ? <span>{attempt.model}</span> : null}
          </div>
        </div>
      })}</div> : <p className='routing-exec-note'>{t('没有保存调用明细')}</p>}
    </section>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'><span>{t('结果')}</span><span className='routing-state-text'><StatusDot variant={tone} />{status}</span></div>
      <div className='routing-exec-result-line'>
        <b>{running ? t('进行中') : formatDuration(row.durationMs)}</b>
        <span>{running ? t('阶段 {phase} · 已进行 {time}', { phase: tracePhase(row.phase), time: formatDuration(now - row.ts) })
          : `${t('首帧 {time}', { time: formatDuration(row.firstResponseMs) })}${row.totalTokens != null ? ` · ${t('输出 {n} tokens', { n: row.totalTokens.toLocaleString() })}` : ''}`}</span>
      </div>
    </section>

    <section className='routing-exec-section'>
      <div className='routing-exec-section-head'><span>{t('请求当时的配置')}</span></div>
      <div className='routing-exec-kv'>
        <span>{strategy ? strategyLabel(strategy) : t('路由快照未记录')}</span>
        {sticky ? <span>{t('会话粘滞 {mode}', { mode: stickyLabel(sticky) })}</span> : null}
        <span>{snapshot?.scope ? t('模型独立配置') : t('继承全局')}</span>
      </div>
      {snapshot ? <p className='routing-exec-note'>{new Date(snapshot.capturedAt).toLocaleString()}{snapshot.providerCount != null
        ? ` · ${t('{providers} 个提供商 / {accounts} 个账号', { providers: snapshot.providerCount, accounts: snapshot.accountCount ?? 0 })}` : ''}</p> : null}
      <div className='routing-exec-actions'>
        <Button variant='outline' size='sm' onClick={() => onConfig(row.model || row.clientModel || '')}>{t('按此范围查看配置')}</Button>
        {row.error ? <Button variant='ghost' size='sm' onClick={() => onDetail(row)}>{t('查看上游错误')}</Button> : null}
      </div>
    </section>

    <div className='routing-exec-foot'>
      <span>{t('时间相对请求开始；历史只读，修改当前配置不会改变这条记录。')}</span>
      <Button variant='outline' size='sm' onClick={() => onDetail(row)}>{t('完整请求详情')}</Button>
    </div>
  </div>
}
