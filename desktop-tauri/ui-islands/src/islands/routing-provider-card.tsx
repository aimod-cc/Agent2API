import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Switch, cn } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { activeGroupSortRules, manualOrderBlockedMessage } from './accounts-group-settings'
import { RoutingConsumptionOrder } from './routing-account-order'
import { accountFlowState } from './routing-flow-model'
import { RoutingAccountRow, RoutingActions, RoutingProviderMark, routingOrderKey } from './routing-canvas-controls'
import { formatDuration, RouteIcon } from './routing-ui'
import type { useRoutingDrag } from './routing-drag'
import type { RouteProfile, RoutingWorkspace } from './routing-types'

export function RoutingProviderCard({ id, profile, workspace, scope, now, index, first, last, included, open, locked, busy, query, drag,
  onOpen, onJoin, onMove, onMoveAccount, onToggleAccount, onAccount, onSort, onEnableGroup }: {
  id: string; profile: RouteProfile; workspace: RoutingWorkspace; scope: string; now: number
  index: number; first: boolean; last: boolean; included: boolean; open: boolean; locked: boolean; busy: boolean; query: string
  drag: ReturnType<typeof useRoutingDrag>; onOpen(open: boolean): void; onJoin(on: boolean): void; onMove(direction: number): void
  onMoveAccount(id: string, direction: number): void; onToggleAccount(id: string): void; onAccount(id: string): void; onSort(): void
  onEnableGroup?(enabled: boolean): void
}) {
  const label = workspace.providers.find(provider => provider.id === id)?.label || id
  const accounts = workspace.accounts.filter(account => providerOf(account) === id)
  const order = [...new Set([...(profile.accountOrder[id] || []), ...accounts.map(account => account.id)])].filter(key => accounts.some(account => account.id === key))
  const globalAccounts = !scope
  const joined = (key: string) => globalAccounts ? accounts.find(account => account.id === key)?.enabled !== false : !profile.excludedAccounts.includes(key)
  const participants = order.filter(joined)
  const rules = !scope || !workspace.customized ? activeGroupSortRules(id, workspace.sorting?.rules?.[id] || []) : []
  const sorted = rules.length > 0
  const unavailable = workspace.providers.find(provider => provider.id === id)?.available === false
  const orderLocked = locked || Boolean(query)
  const summary = globalAccounts ? t('{selected} / {total} 个账号启用', { selected: participants.length, total: accounts.length })
    : t('{selected} / {total} 个账号参与', { selected: participants.length, total: accounts.length })
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
      <Switch checked={globalAccounts ? participants.length > 0 : included} disabled={locked} onCheckedChange={onJoin}
        aria-label={globalAccounts ? t('启用 {provider} 的账号', { provider: label }) : t('{provider} 参与本路由', { provider: label })}
        title={globalAccounts ? participants.length ? t('禁用全部账号') : t('启用全部账号') : included ? t('移出本路由') : t('加入路由')} />
      <Button variant='ghost' size='icon-sm' className='routing-settings-button' disabled={busy}
        title={t('分组设置')} aria-label={t('{name} · 分组设置', { name: label })} onClick={onSort}><RouteIcon name='settings' size={14} /></Button>
      <RoutingActions label={globalAccounts ? t('账号组操作') : t('提供商路由设置')} actions={[
        ...(globalAccounts && onEnableGroup ? [
          { label: t('启用全部账号'), disabled: locked || participants.length === accounts.length, onClick: () => onEnableGroup(true) },
          { label: t('禁用全部账号'), disabled: locked || !participants.length, onClick: () => onEnableGroup(false) },
        ] : []),
        { label: t('提高优先级'), icon: 'arrowUp', disabled: orderLocked || !included || first, onClick: () => onMove(-1) },
        { label: t('降低优先级'), icon: 'arrowDown', disabled: orderLocked || !included || last, onClick: () => onMove(1) },
      ]} />
    </header>
    <RoutingConsumptionOrder provider={id} sorting={workspace.sorting} independent={Boolean(scope) && workspace.customized} disabled={busy} onSettings={onSort} />
    <CollapsibleContent className='routing-config-accounts' id={bodyId}>
      {[true, false].map(active => <div key={String(active)} data-route-list>
        {!active && order.some(key => !joined(key)) ? <p className='routing-account-section'>{globalAccounts ? t('已禁用账号') : t('未参与本路由')}</p> : null}
        {order.filter(key => joined(key) === active).filter(key => !query || label.toLowerCase().includes(query)
          || displayNameOf(accounts.find(account => account.id === key)).toLowerCase().includes(query)).map(key => {
          const account = accounts.find(item => item.id === key)!
          const status = accountFlowState(workspace, account, now)
          const accountIndex = participants.indexOf(key)
          const connections = workspace.metrics.accountConnections ? workspace.metrics.accountConnections[key] || 0 : undefined
          const note = [
            connections == null ? '' : t('在途 {n} / {limit}', { n: connections, limit: Number(account.maxConcurrent) || '∞' }),
            status.resetAt > now ? t('{time} 后恢复', { time: formatDuration(status.resetAt - now) }) : status.reason,
          ].filter(Boolean).join(' · ')
          return <RoutingAccountRow key={key} id={key} account={account} rank={accountIndex + 1} joined={active} globalAccounts={globalAccounts}
            sorting={workspace.sorting} now={now} settingsDisabled={busy}
            state={account.enabled === false ? 'disabled' : account.hasCredentials === false ? 'no-credentials' : status.state}
            stateText={account.enabled !== false && account.hasCredentials !== false && status.state === 'partial' ? status.label : undefined}
            note={note} locked={locked || !globalAccounts && !included} orderLocked={orderLocked || sorted || !included} orderHint={sorted ? manualOrderBlockedMessage(rules) : undefined}
            first={accountIndex === 0} last={accountIndex === participants.length - 1} dragging={drag.state?.id === key}
            drop={drag.state?.target === key ? drag.state.after ? 'after' : 'before' : undefined}
            onDrag={event => drag.start(event, 'account', id, key)} onMove={direction => onMoveAccount(key, direction)}
            onToggle={() => onToggleAccount(key)} onAccount={() => onAccount(key)} />
        })}
      </div>)}
      {!accounts.length ? <p className='routing-config-note'>{t('该提供商没有账号，请先到账号页添加。')}</p> : null}
    </CollapsibleContent>
  </Collapsible>
}
