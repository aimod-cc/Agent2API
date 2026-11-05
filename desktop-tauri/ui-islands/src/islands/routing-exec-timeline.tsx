import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, StatusDot } from '@ui'
import { t } from '../i18n'
import { requestClientLabel } from '../../../ui-shared/model-api-key'
import { RoutingReplayCandidates } from './routing-replay-candidates'
import { RoutingReplayAttempt } from './routing-replay-attempt'
import { RouteIcon, formatDuration, stickyLabel, strategyLabel } from './routing-ui'
import { isTraceRunning, isTraceSuccess, requestModel, requestSummary, traceAttempts, traceCandidates, tracePhase, traceProvider } from './routing-trace-model'
import type { ReplayRow } from './routing-types'

/** 回放只读请求当时保存的事实，不以当前配置重算历史。 */
export function RoutingExecTimeline({ row, now, onDetail, onConfig }: {
  row: ReplayRow; now: number; onDetail(row: ReplayRow): void; onConfig(model: string): void
}) {
  const attempts = traceAttempts(row)
  const candidates = traceCandidates(row)
  const running = isTraceRunning(row), success = isTraceSuccess(row)
  const snapshot = row.route?.configSnapshot
  const strategy = snapshot?.strategy || row.route?.strategy
  const sticky = snapshot?.sticky || row.route?.sticky
  const provider = traceProvider(row.provider, row.providerLabel)
  const account = row.accountName ? `${provider} · ${row.accountName}` : provider
  const hasAccount = Boolean(row.accountName || row.accountId || row.provider)
  const status = running ? tracePhase(row.phase) : success ? String(row.status) : row.status ? `${row.status} · ${t('失败')}` : t('失败')
  const tone = running || success ? 'success' : 'destructive'
  const attemptCount = row.attempts ?? (row.attemptDetails ? row.attemptDetails.length : undefined)
  const blocked = row.route?.blockedTotal ?? (row.route?.blocked ? candidates.filter(candidate => candidate.kind === 'skipped').length : undefined)
  const duration = running ? Math.max(0, now - row.ts) : row.durationMs
  const scope = snapshot?.scope ?? row.route?.scope
  const scopeText = snapshot?.groupId ? `${t('自定义路由组')} · ${snapshot.groupName || scope}`
    : scope != null ? scope ? t('模型独立配置') : t('继承全局') : t('路由快照未记录')
  const session = row.route?.sessionId || row.route?.stickyId
  const source = row.route?.sessionSource === 'content-hash' ? t('内容哈希（推断）') : row.route?.sessionSource || t('未记录')

  return <div className='routing-exec'>
    <header className='routing-exec-result'>
      <div className='routing-exec-heading'>
        <div className='routing-exec-overline'><StatusDot variant={tone} pulse={running} />{status}
          {row.route?.stickyHit ? <span>{t('粘滞命中')}</span> : null}</div>
        <h2>{running ? hasAccount ? t('正在由 {account} 处理', { account }) : tracePhase(row.phase)
          : success ? t('最终由 {account} 承接', { account }) : t('请求失败')}</h2>
        <div className='routing-exec-meta'><code>{requestModel(row)}</code><span>{requestClientLabel(row, t)}</span>
          <time>{new Date(row.ts).toLocaleTimeString()}</time><code className='routing-exec-id' title={row.id}>{row.id}</code></div>
        {!success ? <p className={running ? 'routing-exec-summary' : 'routing-exec-error'}>{requestSummary(row)}</p> : null}
      </div>
      <Button variant='outline' size='sm' onClick={() => onDetail(row)}>{t('完整请求详情')}</Button>
    </header>
    <div className='routing-exec-scroll'>
      <div className='routing-exec-metrics'>
        <div><b>{formatDuration(duration)}</b><span>{running ? t('已用时') : t('总耗时')}</span></div>
        <div><b>{formatDuration(row.firstResponseMs)}</b><span>{t('首帧')}</span></div>
        <div><b>{attemptCount ?? '—'}</b><span>{t('尝试次数')}</span></div>
        <div><b>{blocked ?? '—'}</b><span>{t('跳过候选')}</span></div>
      </div>
      <section className='routing-exec-section'>
        <div className='routing-exec-section-head'><h3>{t('执行路径')}</h3><span>{t('执行路径按当时快照保留')}</span></div>
        <ol className='routing-timeline'>
          <li className='routing-timeline-step is-entry'><span className='routing-timeline-mark' aria-hidden='true'><StatusDot variant='muted' /></span>
            <div className='routing-timeline-body'><div className='routing-timeline-title'><b>{t('请求进入')}</b><time>{new Date(row.ts).toLocaleString()}</time></div>
              <p>{row.upstreamModel && requestModel(row) !== row.upstreamModel
                ? t('客户端模型 {client} → 实际模型 {model}', { client: requestModel(row), model: row.upstreamModel }) : requestModel(row)}</p>
              {session ? <p className='routing-exec-note' title={session}>{t('会话标识 {id} · 来源 {source}', { id: session.slice(0, 12), source })}</p> : null}
            </div>
          </li>
          {attempts.map(attempt => <RoutingReplayAttempt key={attempt.index} row={row} attempt={attempt} />)}
        </ol>
        {!attempts.length ? <p className='routing-exec-note'>{t('没有保存调用明细')}</p> : null}
        {row.completionTokens != null || row.totalTokens != null ? <p className='routing-exec-token-summary'>
          {row.completionTokens != null ? t('输出 {n} tokens', { n: row.completionTokens.toLocaleString() })
            : t('总计 {n} tokens', { n: row.totalTokens!.toLocaleString() })}
        </p> : null}
      </section>
      <RoutingReplayCandidates row={row} />
      <section className='routing-exec-section routing-exec-snapshot'>
        <div className='routing-exec-section-head'><h3>{t('请求当时的配置')}</h3>
          <Button variant='link' size='sm' onClick={() => onConfig(scope ?? (row.clientModel || row.model || ''))}>{t('按此范围查看配置')}</Button></div>
        <div className='routing-exec-kv'>
          <span>{strategy ? snapshot?.groupId && strategy === 'provider' ? t('成员内轮询') : strategyLabel(strategy) : t('路由快照未记录')}</span>
          {sticky ? <span>{t('会话粘滞 {mode}', { mode: stickyLabel(sticky) })}</span> : null}<span>{scopeText}</span>
          {snapshot?.providerCount != null ? <span>{t('{providers} 个提供商 / {accounts} 个账号', { providers: snapshot.providerCount, accounts: snapshot.accountCount ?? 0 })}</span> : null}
        </div>
        {snapshot ? <p className='routing-exec-note'>{new Date(snapshot.capturedAt).toLocaleString()}</p> : null}
        {snapshot?.members?.length ? <Collapsible className='routing-snapshot-members'>
          <CollapsibleTrigger className='routing-detail-toggle'><RouteIcon name='chevronDown' size={12} />{t('成员模型')} · {snapshot.members.length}</CollapsibleTrigger>
          <CollapsibleContent>{snapshot.members.map(member => <div key={member.id} className='routing-snapshot-member'><code>{member.model}</code>
            <span>{traceProvider(member.provider)} · {member.included ? t('参与本路由') : t('未参与本路由')} · {t('{n} 个账号', { n: member.accountCount })}</span></div>)}</CollapsibleContent>
        </Collapsible> : null}
        {row.error ? <Button variant='link' size='sm' onClick={() => onDetail(row)}>{t('查看上游错误')}</Button> : null}
      </section>
      <p className='routing-exec-foot'>{t('时间相对请求开始；历史只读，修改当前配置不会改变这条记录。')}</p>
    </div>
  </div>
}
