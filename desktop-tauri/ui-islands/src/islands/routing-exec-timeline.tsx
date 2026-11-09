import { Button } from '@ui'
import { t } from '../i18n'
import { requestClientLabel } from '../../../ui-shared/model-api-key'
import { isTraceCancelled, isTraceRunning, isTraceSuccess, requestModel } from './routing-trace-model'
import { execCarrier } from './routing-exec-model'
import { RoutingRequestState } from './routing-state-text'
import { RoutingExecDuration, RoutingExecCandidates } from './routing-exec-sections'
import { RoutingExecRewrite } from './routing-exec-rewrite'
import { RoutingExecSnapshot } from './routing-exec-snapshot'
import type { ReplayRow } from './routing-types'

export function RoutingExecTimeline({ row, now, onDetail, onConfig }: {
  row: ReplayRow; now: number; onDetail(row: ReplayRow): void; onConfig(model: string): void
}) {
  const carrier = execCarrier(row)
  const failed = !isTraceRunning(row) && !isTraceSuccess(row) && !isTraceCancelled(row)
  return <div className='routing-exec'>
    <div className='routing-exec-inner'>
      <header className='routing-exec-head'>
        <div className='routing-exec-heading'>
          <h2>{requestModel(row)}</h2>
          <div className='routing-exec-meta'>
            <RoutingRequestState row={row} />
            <span>{requestClientLabel(row, t)}</span>
            <time title={new Date(row.ts).toLocaleString()}>{new Date(row.ts).toLocaleTimeString()}</time>
            {row.status ? <span>HTTP {row.status}</span> : null}
          </div>
        </div>
        <Button variant='outline' size='sm' onClick={() => onDetail(row)}>{t('完整详情')}</Button>
      </header>
      {/* 模型改写排在承载之前：请求真实经过的顺序就是「先改写模型、再选账号」 */}
      <RoutingExecRewrite row={row} />
      <section className='routing-exec-carrier' data-failed={failed ? '' : undefined}>
        <p className='routing-exec-label'>{carrier.label}</p>
        <p className='routing-exec-carrier-name'>{carrier.name}</p>
        <p className='routing-exec-carrier-reason'>{carrier.reason}</p>
      </section>
      <RoutingExecDuration row={row} now={now} />
      <RoutingExecCandidates row={row} />
      <RoutingExecSnapshot row={row} onConfig={onConfig} />
    </div>
  </div>
}
