import * as React from 'react'
import {
  Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Dialog, DialogBody, DialogContent,
  DialogDescription, DialogFooter, DialogHeader, DialogTitle, Spinner,
} from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, type ReplayRow, type RouteGroup, type RoutingWorkspace } from './routing-types'
import { traceProvider } from './routing-trace-model'
import { execScopeLabel } from './routing-exec-model'
import { RouteIcon, stickyLabel, strategyLabel, timeText } from './routing-ui'

type CurrentConfig = { workspace: RoutingWorkspace; group?: RouteGroup; missing: boolean }
const routeStrategy = (strategy?: string, group = false) => strategy ? group && strategy === 'provider' ? t('成员内轮询') : strategyLabel(strategy) : t('未记录')
const providerScope = (scope?: string) => scope === 'all' ? t('全部提供商') : scope === 'selected' ? t('指定提供商') : t('未记录')

function SnapshotMembers({ row }: { row: ReplayRow }) {
  const members = row.route?.configSnapshot?.members
  if (!members?.length) return null
  return <section className='routing-config-detail-section'>
    <h3>{t('当时的成员模型')}</h3>
    <div className='routing-snapshot-members'>{members.map(member => <div key={member.id} className='routing-snapshot-member'>
      <code>{member.model}</code><span>{traceProvider(member.provider)} · {member.included ? t('参与本路由') : t('未参与本路由')} · {t('{n} 个账号', { n: member.accountCount })}</span>
    </div>)}</div>
  </section>
}

function RoutingSnapshotDialog({ row, onClose, onConfig }: { row: ReplayRow; onClose(): void; onConfig(model: string): void }) {
  const [current, setCurrent] = React.useState<CurrentConfig | null>(null)
  const [error, setError] = React.useState('')
  const [revision, reload] = React.useReducer(value => value + 1, 0)
  const [loading, setLoading] = React.useState(true)
  const snapshot = row.route?.configSnapshot
  const scope = snapshot?.scope ?? row.route?.scope
  const requestModel = row.clientModel || row.model || ''
  const model = scope ?? requestModel
  const groupModel = scope || requestModel
  const key = `${row.id}:${row.ts}`
  React.useEffect(() => {
    let alive = true
    setCurrent(null); setError(''); setLoading(true)
    void Promise.all([bridge().getRoutingWorkspace(model || undefined), bridge().getRoutingGroups()]).then(([workspace, groups]) => {
      if (!alive) return
      if (!workspace?.settings || !workspace?.profile || !Array.isArray(groups?.groups)) throw new Error(t('路由接口返回了不完整的数据'))
      const group = groups.groups.find(entry => snapshot?.groupId ? entry.group.id === snapshot.groupId
        : Boolean(model && entry.group.model.toLowerCase() === groupModel.toLowerCase()))?.group
      setCurrent({ workspace, group, missing: Boolean(snapshot?.groupId && !group) })
    }).catch(failure => { if (alive) setError(errorMessage(failure)) }).finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [key, model, groupModel, snapshot?.groupId, revision])

  const scopeText = !current ? '' : current.missing ? t('路由组已删除') : current.group
    ? `${t('自定义路由组')} · ${current.group.name || current.group.model}`
    : current.workspace.customized ? `${t('模型独立配置')} · ${current.workspace.model || model}` : t('继承全局配置')
  const available = current && !current.missing
  const currentText = (text?: string) => loading ? t('正在读取…') : available ? text || '—' : '—'
  const rows = [
    { label: t('配置范围'), before: execScopeLabel(row), after: loading ? t('正在读取…') : scopeText || '—' },
    { label: t('选路方式'), before: routeStrategy(snapshot?.strategy || row.route?.strategy, Boolean(snapshot?.groupId)),
      after: currentText(routeStrategy(current?.group?.strategy || current?.workspace.settings.routingStrategy, Boolean(current?.group))) },
    { label: t('会话粘滞'), before: snapshot?.sticky || row.route?.sticky ? stickyLabel(snapshot?.sticky || row.route?.sticky) : t('未记录'),
      after: currentText(stickyLabel(current?.group?.sticky || current?.workspace.settings.routingSticky)) },
    { label: t('提供商范围'), before: providerScope(snapshot?.providerScope), after: currentText(current?.group ? t('指定提供商') : providerScope(current?.workspace.profile.providerScope)) },
  ]
  const session = row.route?.sessionId || row.route?.stickyId
  const editScope = current?.group?.model ?? (current?.workspace.customized ? current.workspace.model || model : '')
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='routing-config-detail-dialog'>
      <DialogHeader><DialogTitle>{t('当时配置与当前配置')}</DialogTitle></DialogHeader>
      <DialogBody>
        <DialogDescription className='routing-exec-note'>{t('历史只读，当前配置的修改不会改变这条请求的快照。')}</DialogDescription>
        <div className='routing-config-compare' role='table' aria-label={t('当时配置与当前配置')}>
          <div className='routing-config-compare-row is-heading' role='row'><span role='columnheader'>{t('配置项')}</span><span role='columnheader'>{t('当时')}</span><span role='columnheader'>{t('当前配置')}</span></div>
          {rows.map(item => <div key={item.label} className='routing-config-compare-row' role='row' data-same={item.before === item.after ? '' : undefined}>
            <span role='rowheader'>{item.label}</span><span role='cell'>{item.before}</span><span role='cell'>{item.after}</span>
          </div>)}
        </div>
        {loading ? <div className='routing-config-detail-loading'><Spinner />{t('正在读取当前配置')}</div> : null}
        {error ? <p className='routing-config-detail-error' role='alert'>{error}<Button variant='link' size='xs' onClick={reload}>{t('重试')}</Button></p> : null}
        {current?.missing ? <p className='routing-exec-note'>{t('这条请求使用的路由组已删除，当时的快照仍可查看')}</p> : null}
        <section className='routing-config-detail-section'>
          <h3>{t('请求当时的配置')}</h3>
          <dl className='routing-config-detail-facts'>
            <dt>{t('记录时间')}</dt><dd>{snapshot?.capturedAt != null ? timeText(snapshot.capturedAt) : t('未记录')}</dd>
            <dt>{t('候选规模')}</dt><dd>{snapshot?.providerCount != null ? `${t('{n} 个提供商', { n: snapshot.providerCount })} · ` : ''}{snapshot?.accountCount != null ? t('{n} 个账号', { n: snapshot.accountCount }) : t('未记录')}</dd>
            {session ? <><dt>{t('会话')}</dt><dd className='routing-exec-mono'>{session}</dd></> : null}
            {row.route?.sessionSource ? <><dt>{t('会话来源')}</dt><dd>{row.route.sessionSource}</dd></> : null}
            {row.route?.stickyHit != null ? <><dt>{t('粘性命中')}</dt><dd>{row.route.stickyHit ? t('是') : t('否')}</dd></> : null}
          </dl>
        </section>
        <SnapshotMembers row={row} />
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' size='sm' disabled={!available} onClick={() => { onClose(); onConfig(editScope) }}>{t('查看当前配置')}</Button>
        <Button variant='outline' size='sm' onClick={onClose}>{t('关闭')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

export function RoutingExecSnapshot({ row, onConfig }: { row: ReplayRow; onConfig(model: string): void }) {
  const [open, setOpen] = React.useState(false)
  React.useEffect(() => { setOpen(false) }, [row.id, row.ts])
  const snapshot = row.route?.configSnapshot
  const strategy = snapshot?.strategy || row.route?.strategy
  const sticky = snapshot?.sticky || row.route?.sticky
  return <section className='routing-exec-section'>
    <div className='routing-exec-section-head'><h3>{t('当时配置')}</h3><Button variant='link' size='xs' onClick={() => setOpen(true)}>{t('查看配置详情')}</Button></div>
    <dl className='routing-exec-definitions'>
      <div><dt>{t('配置范围')}</dt><dd>{execScopeLabel(row)}</dd></div>
      <div><dt>{t('选路方式')}</dt><dd>{routeStrategy(strategy, Boolean(snapshot?.groupId))}</dd></div>
      <div><dt>{t('会话粘滞')}</dt><dd>{sticky ? stickyLabel(sticky) : t('未记录')}</dd></div>
      {row.upstreamModel ? <div className='is-wide'><dt>{t('上游模型')}</dt><dd className='routing-exec-mono'>{row.upstreamModel}</dd></div> : null}
    </dl>
    {snapshot?.members?.length ? <Collapsible className='routing-exec-member-toggle'>
      <CollapsibleTrigger className='routing-detail-toggle'><RouteIcon name='chevronDown' />{t('成员模型')} · {snapshot.members.length}</CollapsibleTrigger>
      <CollapsibleContent><SnapshotMembers row={row} /></CollapsibleContent>
    </Collapsible> : null}
    {open ? <RoutingSnapshotDialog row={row} onClose={() => setOpen(false)} onConfig={onConfig} /> : null}
  </section>
}
