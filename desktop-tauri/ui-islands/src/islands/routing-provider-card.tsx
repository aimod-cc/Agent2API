import * as React from 'react'
import { Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Switch, cn } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { activeGroupSortRules, groupSortSummary, manualOrderBlockedMessage } from './accounts-group-settings'
import { accountFlowState } from './routing-flow-model'
import { RoutingAccountRow, RoutingActions, RoutingProviderMark, routingOrderKey } from './routing-canvas-controls'
import { formatDuration, RouteIcon } from './routing-ui'
import type { useRoutingDrag } from './routing-drag'
import type { RouteProfile, RoutingWorkspace } from './routing-types'

export function RoutingProviderCard({ id, profile, workspace, scope, now, index, first, last, included, open, locked, busy, query, drag,
  onOpen, onJoin, onMove, onMoveAccount, onToggleAccount, onAccount, onSort }: {
  id: string; profile: RouteProfile; workspace: RoutingWorkspace; scope: string; now: number
  index: number; first: boolean; last: boolean; included: boolean; open: boolean; locked: boolean; busy: boolean; query: string
  drag: ReturnType<typeof useRoutingDrag>; onOpen(open: boolean): void; onJoin(on: boolean): void; onMove(direction: number): void
  onMoveAccount(id: string, direction: number): void; onToggleAccount(id: string): void; onAccount(id: string): void; onSort(): void
}) {
  const label = workspace.providers.find(provider => provider.id === id)?.label || id
  const accounts = workspace.accounts.filter(account => providerOf(account) === id)
  const order = [...new Set([...(profile.accountOrder[id] || []), ...accounts.map(account => account.id)])].filter(key => accounts.some(account => account.id === key))
  const participants = order.filter(key => !profile.excludedAccounts.includes(key))
  const rules = !scope ? activeGroupSortRules(id, workspace.sorting?.rules?.[id] || []) : []
  const sorted = rules.length > 0
  const unavailable = workspace.providers.find(provider => provider.id === id)?.available === false
  const orderLocked = locked || Boolean(query)
  const summary = t('{selected} / {total} 个账号参与', { selected: participants.length, total: accounts.length })
  const bodyId = React.useId()
  return <Collapsible render={<section />} open={open} onOpenChange={onOpen} data-route-sort={included ? id : undefined}
    data-route-node={included ? id : undefined} className={cn('routing-config-provider', !included && 'is-off',
      drag.state?.id === id && 'is-dragging', drag.state?.target === id && (drag.state.after ? 'drop-after' : 'drop-before'))}>
    <header>
      <Button variant='ghost' size='icon-sm' className='routing-grip' disabled={orderLocked || !included}
        title={t('拖动或按 Alt + 方向键调整顺序')} aria-label={t('拖动调整提供商顺序')}
        onPointerDown={event => drag.start(event, 'provider', id, id)} onKeyDown={event => routingOrderKey(event, onMove)}>
        <RouteIcon name='gripVertical' size={15} />
      </Button>
      <span className='routing-config-rank'>{included ? String(index + 1).padStart(2, '0') : '—'}</span>
      <RoutingProviderMark id={id} label={label} />
      <CollapsibleTrigger className='routing-provider-toggle' aria-controls={bodyId}>
        <span className='routing-provider-name'><b title={label}>{label}</b><small>{summary}{unavailable ? ` · ${t('模型绑定不可用')}` : ''}</small></span>
        <RouteIcon name='chevronDown' size={13} />
      </CollapsibleTrigger>
      <Switch checked={included} disabled={locked} onCheckedChange={onJoin} aria-label={t('{provider} 参与本路由', { provider: label })}
        title={included ? t('移出本路由') : t('加入路由')} />
      <RoutingActions label={t('提供商路由设置')} actions={[
        { label: t('提高优先级'), icon: 'arrowUp', disabled: orderLocked || !included || first, onClick: () => onMove(-1) },
        { label: t('降低优先级'), icon: 'arrowDown', disabled: orderLocked || !included || last, onClick: () => onMove(1) },
        { label: t('消耗规则'), icon: 'settings', disabled: busy, onClick: onSort },
      ]} />
    </header>
    <CollapsibleContent className='routing-config-accounts' id={bodyId}>
      {sorted ? <div className='routing-config-lock'><RouteIcon name='lock' size={13} />
        <Badge variant='ghost' shape='tag' title={manualOrderBlockedMessage(rules)}>{groupSortSummary(rules)}</Badge>
        <Button variant='link' size='sm' onClick={onSort} disabled={busy}>{t('消耗规则')}</Button>
      </div> : null}
      {[true, false].map(joined => <div key={String(joined)} data-route-list>
        {!joined && order.some(key => profile.excludedAccounts.includes(key)) ? <p className='routing-account-section'>{t('未参与本路由')}</p> : null}
        {order.filter(key => !profile.excludedAccounts.includes(key) === joined).filter(key => !query || label.toLowerCase().includes(query)
          || displayNameOf(accounts.find(account => account.id === key)).toLowerCase().includes(query)).map(key => {
          const account = accounts.find(item => item.id === key)!
          const status = accountFlowState(workspace, account, now)
          const accountIndex = participants.indexOf(key)
          const connections = workspace.metrics.accountConnections ? workspace.metrics.accountConnections[key] || 0 : undefined
          const note = [
            connections == null ? '' : t('在途 {n} / {limit}', { n: connections, limit: Number(account.maxConcurrent) || '∞' }),
            status.resetAt > now ? t('{time} 后恢复', { time: formatDuration(status.resetAt - now) }) : status.reason,
          ].filter(Boolean).join(' · ')
          return <RoutingAccountRow key={key} id={key} account={account} rank={accountIndex + 1} joined={joined}
            state={account.enabled === false ? 'disabled' : account.hasCredentials === false ? 'no-credentials' : status.state}
            stateText={account.enabled !== false && account.hasCredentials !== false && status.state === 'partial' ? status.label : undefined}
            note={note} locked={locked || !included} orderLocked={orderLocked || sorted || !included} orderHint={sorted ? manualOrderBlockedMessage(rules) : undefined}
            first={accountIndex === 0} last={accountIndex === participants.length - 1} dragging={drag.state?.id === key}
            drop={drag.state?.target === key ? drag.state.after ? 'after' : 'before' : undefined}
            onDrag={event => drag.start(event, 'account', id, key)} onMove={direction => onMoveAccount(key, direction)}
            onToggle={() => onToggleAccount(key)} onAccount={busy ? undefined : () => onAccount(key)} />
        })}
      </div>)}
      {!accounts.length ? <p className='routing-config-note'>{t('该提供商没有账号，请先到账号页添加。')}</p> : null}
    </CollapsibleContent>
  </Collapsible>
}
