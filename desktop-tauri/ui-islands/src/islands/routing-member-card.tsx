import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Switch, cn } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import type { AccountRecord, AccountSorting } from './accounts-shared'
import { RoutingConsumptionOrder } from './routing-account-order'
import type { RouteMember } from './routing-types'
import type { useRoutingDrag } from './routing-drag'
import { RoutingAccountRow, RoutingActions, RoutingProviderMark, routingOrderKey } from './routing-canvas-controls'
import { RouteIcon } from './routing-ui'

export function RoutingMemberCard({ member, accounts, label, name, index, first, last, open, locked, query, drag, sorting, now,
  onOpen, onJoin, onMove, onAccounts, onToggleAccount, onMoveAccount, onAccount, onSort }: {
  member: RouteMember; accounts: AccountRecord[]; label: string; name: string; index: number; first: boolean; last: boolean; open: boolean; locked: boolean; query: string
  drag: ReturnType<typeof useRoutingDrag>; onOpen(open: boolean): void; onJoin(on: boolean): void; onMove(direction: number): void
  onAccounts(): void; onToggleAccount(id: string): void; onMoveAccount(id: string, direction: number): void; onAccount(id: string): void
  sorting?: AccountSorting | null; now: number; onSort(): void
}) {
  const available = accounts.filter(account => providerOf(account) === member.provider)
  const order = [...new Set([...member.accounts, ...available.map(account => account.id)])]
  const searchingMember = `${label} ${name} ${member.model}`.toLowerCase().includes(query)
  const orderLocked = locked || Boolean(query)
  const accountDrag = drag.state?.kind === 'account' && drag.state.provider === member.id ? drag.state : null
  return <Collapsible render={<section />} open={open} onOpenChange={onOpen} data-route-sort={member.included ? member.id : undefined}
    data-route-node={member.included ? member.id : undefined} className={cn('routing-config-provider', !member.included && 'is-off',
      drag.state?.id === member.id && 'is-dragging', drag.state?.target === member.id && (drag.state.after ? 'drop-after' : 'drop-before'))}>
    <header>
      <Button variant='ghost' size='icon-sm' className='routing-grip' disabled={orderLocked || !member.included}
        title={t('拖动或按 Alt + 方向键调整顺序')} aria-label={t('调整模型顺序')}
        onPointerDown={event => drag.start(event, 'member', member.id, member.id)} onKeyDown={event => routingOrderKey(event, onMove)}><RouteIcon name='gripVertical' size={15} /></Button>
      <span className='routing-config-rank'>{member.included ? String(index + 1).padStart(2, '0') : '—'}</span>
      <RoutingProviderMark id={member.provider} label={label} />
      <CollapsibleTrigger className='routing-provider-toggle'><span className='routing-provider-name'><b title={name}>{name}</b>
        <small title={`${label} · ${member.model}`}>{label} · {member.model}</small></span><RouteIcon name='chevronDown' size={13} /></CollapsibleTrigger>
      <Switch checked={member.included} disabled={locked} onCheckedChange={onJoin} aria-label={t('{model} 参与本路由', { model: name })}
        title={member.included ? t('移出本路由') : t('加入路由')} />
      <Button variant='ghost' size='icon-sm' className='routing-settings-button' disabled={locked}
        title={t('分组设置')} aria-label={t('{name} · 分组设置', { name: label })} onClick={onSort}><RouteIcon name='settings' size={14} /></Button>
      <RoutingActions label={t('路由成员操作')} actions={[
        { label: t('提高优先级'), icon: 'arrowUp', disabled: orderLocked || !member.included || first, onClick: () => onMove(-1) },
        { label: t('降低优先级'), icon: 'arrowDown', disabled: orderLocked || !member.included || last, onClick: () => onMove(1) },
        { label: t('参与账号'), icon: 'accounts', disabled: locked, onClick: onAccounts },
      ]} />
    </header>
    <RoutingConsumptionOrder provider={member.provider} sorting={sorting} independent disabled={locked} onSettings={onSort} />
    <CollapsibleContent className='routing-config-accounts'>
      <div className='routing-member-summary'><span>{t('{selected} / {total} 个账号参与', { selected: member.accounts.length, total: available.length })}</span>
        <Button variant='link' size='sm' disabled={locked} onClick={onAccounts}>{t('选择账号')}</Button></div>
      {[true, false].map(joined => <div key={String(joined)} data-route-list>
        {!joined && order.some(id => !member.accounts.includes(id)) ? <p className='routing-account-section'>{t('未参与本路由')}</p> : null}
        {order.filter(id => member.accounts.includes(id) === joined).filter(id => !query || searchingMember
          || displayNameOf(available.find(account => account.id === id)).toLowerCase().includes(query)).map(id => {
          const account = available.find(item => item.id === id)
          const rank = member.accounts.indexOf(id)
          const state = !account ? 'missing' : account.enabled === false ? 'disabled' : account.hasCredentials === false ? 'no-credentials' : 'enabled'
          return <RoutingAccountRow key={id} id={id} account={account} rank={rank + 1} joined={joined} state={state}
            sorting={sorting} now={now} settingsDisabled={locked}
            note={joined ? t('参与本路由') : t('未参与本路由')} locked={locked || !member.included} orderLocked={orderLocked || !member.included}
            first={rank === 0} last={rank === member.accounts.length - 1} dragging={accountDrag?.id === id}
            drop={accountDrag?.target === id ? accountDrag.after ? 'after' : 'before' : undefined}
            onDrag={event => drag.start(event, 'account', member.id, id)} onMove={direction => onMoveAccount(id, direction)}
            onToggle={() => onToggleAccount(id)} onAccount={account ? () => onAccount(id) : undefined} />
        })}
      </div>)}
      {!order.length ? <p className='routing-config-note'>{t('该提供商没有账号，请先到账号页添加。')}</p> : null}
    </CollapsibleContent>
  </Collapsible>
}
