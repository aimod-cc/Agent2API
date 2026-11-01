import * as React from 'react'
import { Badge, Button, Empty, cn } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { PROVIDER_ICONS } from './add-provider-pick'
import { RouteIcon, IconButton, formatDuration } from './routing-ui'
import { accountFlowState, accountParticipation, clientIcon, clientLabel, matchesSelection, pathForRequest, selectedRequest, sumPaths } from './routing-flow-model'
import { RoutingFlowEdges } from './routing-flow-edges'
import { attemptLabel, attemptState, traceAttempts, traceCandidates } from './routing-trace-model'
import type { ConfigTarget, FlowPath, FlowSelection, FlowSnapshot } from './routing-flow-types'
import type { RoutingWorkspace } from './routing-types'

type Props = { snapshot: FlowSnapshot | null; workspace: RoutingWorkspace; selection: FlowSelection; now: number; focused: boolean; sticky: boolean;
  onSelect(selection: FlowSelection): void; onConfig(target: ConfigTarget, model?: string): void }
function Mark({ provider, label }: { provider: string; label: string }) {
  return PROVIDER_ICONS[provider] ? <img className='routing-provider-mark' src={PROVIDER_ICONS[provider]} alt='' />
    : <span className='routing-provider-mark routing-letter'>{label.slice(0, 1)}</span>
}
export function RoutingFlowCanvas({ snapshot, workspace, selection, now, focused, sticky, onSelect, onConfig }: Props) {
  const root = React.useRef<HTMLDivElement>(null)
  const [collapsed, setCollapsed] = React.useState<ReadonlySet<string>>(new Set())
  const request = selectedRequest(selection)
  const paths = React.useMemo(() => request ? [pathForRequest(request)] : snapshot?.paths || [], [request, snapshot])
  const clients = [...new Set(paths.map(path => path.client))]
  const models = [...new Set([...(workspace.model ? [workspace.model] : []), ...paths.map(path => path.model)])]
  const attempts = request ? traceAttempts(request) : []
  const candidates = request ? traceCandidates(request, attempts) : []
  const providers = [...new Set([...workspace.profile.providers, ...workspace.providers.map(provider => provider.id), ...paths.map(path => path.provider), ...attempts.map(attempt => attempt.provider || '')])].filter(Boolean)
  const active = (rows: FlowPath[]) => Boolean(selection && rows.some(path => matchesSelection(path, selection)))
  return <div className='routing-flow-canvas-scroll'><div className={cn('routing-flow-canvas', models.length <= 1 && 'is-single')} ref={root}>
    <RoutingFlowEdges root={root} paths={paths} selection={selection} collapsed={collapsed} focused={focused} onSelect={onSelect} />
    <div className='routing-flow-column'><div className='routing-column-label'><RouteIcon name='monitor' />{t('客户端')}</div><div className='routing-flow-nodes'>
      {!clients.length ? <Empty>{t('当前范围内没有请求')}<span>{t('没有请求也能配置路由')}</span></Empty> : clients.map(client => {
        const rows = paths.filter(path => path.client === client), running = sumPaths(rows, 'running'), asset = clientIcon(client)
        return <Button key={client} variant='ghost' data-flow-node={`client:${client}`} className={cn('routing-flow-node', active(rows) && 'is-selected')} onClick={() => onSelect({ kind: 'client', id: client })}>
          <span className='routing-flow-node-title'>{asset ? <img src={`assets/agents/${asset}`} alt='' /> : <RouteIcon name='monitor' />}<b>{clientLabel(client)}</b></span>
          <span className='routing-flow-node-note'>{t('按客户端类型聚合')}</span><span className='routing-flow-node-foot'><span>{t('{n} 次请求', { n: sumPaths(rows, 'requests') })}</span><Badge shape='tag' variant={running ? 'success' : 'secondary'}>{running ? t('{n} 在途', { n: running }) : t('仅历史')}</Badge></span>
        </Button>
      })}</div></div>
    <div className='routing-flow-column'><div className='routing-column-label'><RouteIcon name='gateway' />{t('请求模型')}</div><div className='routing-flow-nodes'>
      {models.map(model => {
        const rows = paths.filter(path => path.model === model)
        return <div className='routing-flow-node-wrap' key={model}><Button variant='ghost' className={cn('routing-flow-node', active(rows) && 'is-selected')} data-flow-node={`model:${model}`} onClick={() => onConfig({ kind: 'model', id: model }, model)}>
          <code title={model}>{model || t('模型未记录')}</code><span className='routing-flow-node-note'>{t('{n} 条账号路径', { n: rows.filter(path => path.accountId || path.provider).length })}</span><span className='routing-flow-node-foot'>{t('{n} 次请求', { n: sumPaths(rows, 'requests') })}{sumPaths(rows, 'running') ? <Badge variant='success' shape='tag'>{t('{n} 在途', { n: sumPaths(rows, 'running') })}</Badge> : null}</span>
        </Button><IconButton className='routing-flow-node-settings' name='settings' title={t('配置该模型')} onClick={() => onConfig({ kind: 'model', id: model }, model)} /></div>
      })}</div></div>
    <div className='routing-flow-column providers'><div className='routing-column-label'><RouteIcon name='plugins' />{t('提供商 / 账号')}<span>{t('当前状态')}</span></div><div className='routing-flow-providers'>
      {providers.map(provider => {
        const info = workspace.providers.find(item => item.id === provider), label = info?.label || provider
        const order = workspace.profile.accountOrder[provider] || []
        const accounts = workspace.accounts.filter(account => providerOf(account) === provider).sort((left, right) => {
          const a = order.indexOf(left.id), b = order.indexOf(right.id)
          return (a < 0 ? Infinity : a) - (b < 0 ? Infinity : b)
        })
        const rows = paths.filter(path => path.provider === provider)
        const missing = rows.filter(path => !accounts.some(account => account.id === path.accountId))
        const isCollapsed = collapsed.has(provider)
        return <section className='routing-flow-provider' key={provider}>
          <header data-flow-node={`provider:${provider}`}><Button variant='ghost' className='routing-flow-provider-name' onClick={() => onConfig({ kind: 'provider', id: provider })}><Mark provider={provider} label={label} /><span><b>{label}</b><small>{t('{n} 个账号', { n: accounts.length })}{info?.available === false ? ` · ${t('模型绑定不可用')}` : ''}</small></span></Button><IconButton name='settings' title={t('提供商路由设置')} onClick={() => onConfig({ kind: 'provider', id: provider })} /><IconButton name={isCollapsed ? 'chevronRight' : 'chevronDown'} title={isCollapsed ? t('展开这一组') : t('折叠这一组')} onClick={() => setCollapsed(current => { const next = new Set(current); if (next.has(provider)) next.delete(provider); else next.add(provider); return next })} /></header>
          {!isCollapsed ? <div className='routing-flow-accounts'>{accounts.map(account => {
            const state = accountFlowState(workspace, account, now), included = accountParticipation(workspace, account), name = displayNameOf(account)
            const history = rows.filter(path => path.accountId === account.id)
            const running = workspace.metrics.accountRunning?.[account.id] || 0
            const attempt = attempts.find(attempt => attempt.accountId === account.id)
            const failed = request && attempt && attemptState(request, attempt) === 'failed'
            const candidate = candidates.find(candidate => candidate.id === account.id)
            const note = request ? failed ? t('本次尝试失败') : attempt ? attemptLabel(request, attempt) : candidate?.kind === 'skipped' ? `${t('已跳过')} · ${candidate.reason}` : candidate ? t('未轮到') : t('保存的尝试明细中没有这次调用')
              : history.length ? t('近期 {n} 次请求', { n: sumPaths(history, 'requests') }) : t('本窗口未调用')
            const rank = order.indexOf(account.id)
            return <div key={account.id} className='routing-flow-account-wrap'><Button variant='ghost' data-flow-node={`account:${account.id}`} className={cn('routing-flow-account', active(history) && 'is-selected', state.state === 'cooldown' && 'is-limited', !included && 'is-excluded')} onClick={() => onConfig({ kind: 'account', id: account.id })}>
              <span className='routing-account-avatar'>{name.slice(0, 1)}</span><span className='routing-flow-account-info'><b>{name}</b><span>{state.label}{state.resetAt > now ? ` · ${formatDuration(state.resetAt - now)}` : ''}</span><small>{note}</small><small>{included ? rank >= 0 ? t('配置顺位 {n}', { n: rank + 1 }) : t('参与路由') : t('当前配置已排除')}</small></span><span className='routing-flow-account-tags'>{running ? <Badge variant='success' shape='tag'>{t('{n} 在途', { n: running })}</Badge> : null}{sticky && sumPaths(history, 'stickyHits') ? <Badge variant='brand' shape='tag'><RouteIcon name='lock' size={10} />{t('粘滞')}</Badge> : null}</span>
            </Button><IconButton name='settings' className='routing-flow-node-settings' title={t('账号路由设置')} onClick={() => onConfig({ kind: 'account', id: account.id })} /></div>
          })}{missing.map(path => <Button key={path.id} variant='ghost' className='routing-flow-account is-unrecorded' data-flow-node={`account:${path.accountId || path.id}`} onClick={() => onSelect({ kind: 'path', id: path.id })}><span><b>{path.accountName || t('账号未记录')}</b><small>{t('历史承载账号当前不可用')}</small></span></Button>)}</div> : <p className='routing-flow-collapsed'>{t('{n} 个账号已折叠', { n: accounts.length })}</p>}
        </section>
      })}</div></div>
  </div></div>
}
