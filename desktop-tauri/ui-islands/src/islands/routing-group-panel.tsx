import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty, InputGroup, InputGroupAddon, InputGroupInput, Label, confirmDialog } from '@ui'
import { t } from '../i18n'
import type { AccountRecord, AccountSorting } from './accounts-shared'
import { displayNameOf } from './accounts-domain'
import { RoutingDragOverlay, reorderRoute, useRoutingDrag, type RoutingDrop } from './routing-drag'
import { RoutingGroupDialog, RoutingMemberAccounts, RoutingMemberDialog } from './routing-group-dialogs'
import { groupDirty, type RouteGroupEditor } from './routing-group-state'
import { RoutingSelect } from './routing-select'
import { RoutingActions } from './routing-canvas-controls'
import { RoutingCanvasLayout, RoutingDraftFooter } from './routing-canvas-layout'
import { RoutingMemberCard } from './routing-member-card'
import { routingAccountSections, routingCollapsedSections, useRoutingExpansion } from './routing-account-sections'
import { IconButton, RouteIcon } from './routing-ui'
import { notify, type RouteGroup, type RouteMember } from './routing-types'

export function RoutingGroupPanel({ editor, accounts, reserved, baseUrl, sorting, now, onAccount, onSort }: {
  editor: RouteGroupEditor; accounts: AccountRecord[]; reserved: string[]; baseUrl: string; sorting?: AccountSorting | null; now: number
  onAccount(id: string): void; onSort(provider: string): void
}) {
  const [dialog, setDialog] = React.useState<'edit' | 'add' | 'example' | null>(null)
  const [accountMember, setAccountMember] = React.useState<string | null>(null)
  const { setOpened, sectionOpen, setSectionOpen, search, setSearch, query, isOpen } = useRoutingExpansion()
  const group = editor.draft?.group
  const locked = editor.busy || editor.loading
  const update = (mutate: (group: RouteGroup) => void) => editor.update(mutate)
  const drop = ({ kind, provider, id, target, after }: RoutingDrop) => update(next => {
    if (kind === 'account') {
      const member = next.members.find(member => member.id === provider)
      if (member) reorderRoute(member.accounts, id, target, after)
      return
    }
    const ids = next.members.filter(member => member.included).map(member => member.id)
    reorderRoute(ids, id, target, after)
    next.members = [...ids.map(id => next.members.find(member => member.id === id)!), ...next.members.filter(member => !member.included)]
  })
  const drag = useRoutingDrag(!locked && !query, `${group?.id || ''}:${search}`, drop)
  if (!group || !editor.draft) return <Empty>{t('选择路由入口')}</Empty>
  const active = group.members.filter(member => member.included)
  const sections = routingAccountSections(group.members, accounts, member => member.provider, member => member.included)
  const orderingPeers = (id: string) => (Object.values(sections).find(members => members.some(member => member.id === id)) || []).filter(member => member.included)
  const dirty = groupDirty(editor.draft)
  const label = (provider: string) => editor.data?.catalog.find(choice => choice.provider === provider)?.providerLabel || provider
  const name = (member: RouteMember) => editor.data?.catalog.find(choice => choice.provider === member.provider && choice.model.toLowerCase() === member.model.toLowerCase())?.name || member.model
  const editMember = (id: string, mutate: (member: RouteMember) => void) => update(next => { const member = next.members.find(item => item.id === id); if (member) mutate(member) })
  const move = (id: string, direction: number) => {
    const peers = orderingPeers(id), target = peers[peers.findIndex(member => member.id === id) + direction]
    if (target) drop({ kind: 'member', provider: '', id, target: target.id, after: direction > 0 })
  }
  const moveAccount = (member: RouteMember, id: string, direction: number) => {
    const target = member.accounts[member.accounts.indexOf(id) + direction]
    if (target) editMember(member.id, next => reorderRoute(next.accounts, id, target, direction > 0))
  }
  const removeAll = async () => {
    if (await confirmDialog(t('只移出本路由，账号状态和其他路由不变；保存后此入口将没有可用路由。'), {
      title: t('全部移出本路由'), confirmText: t('全部移出'),
    })) update(next => { next.members.forEach(member => { member.included = false }) })
  }
  const copy = async (value: string) => {
    try { await navigator.clipboard.writeText(value); notify(t('已复制')) }
    catch { notify(t('复制失败，请手动选择并复制'), 'error') }
  }
  const request = JSON.stringify({ model: group.model, messages: [{ role: 'user', content: 'Hello' }], stream: true }, null, 2)
  const snippet = `POST ${baseUrl.replace(/\/$/, '')}/v1/chat/completions\nAuthorization: Bearer <YOUR_GATEWAY_KEY>\nContent-Type: application/json\n\n${request}`
  const matches = (member: RouteMember) => !query || `${name(member)} ${label(member.provider)} ${member.model}`.toLowerCase().includes(query)
    || accounts.some(account => account.provider === member.provider && displayNameOf(account).toLowerCase().includes(query))
  const visible = sections.active.filter(matches)
  const expandable = [...sections.active, ...routingCollapsedSections.flatMap(({ key }) => sectionOpen[key] ? sections[key] : [])].filter(matches)
  const allOpen = expandable.length > 0 && expandable.every(member => isOpen(member.id))
  const renderMember = (member: RouteMember) => <RoutingMemberCard key={member.id} member={member} accounts={accounts} name={name(member)} label={label(member.provider)}
    index={active.indexOf(member)} first={orderingPeers(member.id)[0]?.id === member.id} last={orderingPeers(member.id).at(-1)?.id === member.id}
    open={isOpen(member.id)} locked={locked} query={query} drag={drag}
    onOpen={open => setOpened(current => ({ ...current, [member.id]: open }))} onJoin={on => editMember(member.id, next => { next.included = on })}
    onMove={direction => move(member.id, direction)} onAccounts={() => setAccountMember(member.id)} onMoveAccount={(id, direction) => moveAccount(member, id, direction)}
    onToggleAccount={id => editMember(member.id, next => { next.accounts = next.accounts.includes(id) ? next.accounts.filter(key => key !== id) : [...next.accounts, id] })}
    onAccount={onAccount} sorting={sorting} now={now} onSort={() => onSort(member.provider)} />
  const selectedAccountMember = group.members.find(member => member.id === accountMember)
  const members = <section className='routing-groups-column'>
    <div className='routing-column-head'><RouteIcon name='plugins' size={14} /><b>{t('成员模型')}</b><span>{t('{n} 项参与', { n: active.length })}</span>
      <div className='right'><Button variant='ghost' size='sm' disabled={!expandable.length || Boolean(query)} onClick={() => setOpened(current => ({ ...current, ...Object.fromEntries(expandable.map(member => [member.id, !allOpen])) }))}>
        {allOpen ? t('折叠全部') : t('展开全部')}</Button>
        <RoutingActions label={t('路由参与操作')} actions={[
          { label: t('添加模型'), icon: 'plus', disabled: locked, onClick: () => setDialog('add') },
          { label: t('全部移出'), disabled: locked || !active.length, onClick: () => void removeAll() },
        ]} />
      </div>
    </div>
    <InputGroup className='routing-config-search'><InputGroupInput value={search} onChange={event => setSearch(event.currentTarget.value)}
      placeholder={t('搜索提供商或模型')} aria-label={t('搜索提供商或模型')} /><InputGroupAddon><RouteIcon name='search' size={14} /></InputGroupAddon></InputGroup>
    <p className='routing-groups-hint'>{t('拖动把手或按 Alt + 方向键调整顺序，更多操作在右侧菜单。')}</p>
    <div className='routing-config-listcol' data-route-scroll>
      <div className='routing-config-participants' data-route-list>{visible.map(renderMember)}</div>
      {!active.length ? <Empty><b>{t('还没有参与模型')}</b><span>{t('可以保存空路由，但请求会明确失败，不会回退全局。')}</span></Empty> : null}
      {routingCollapsedSections.map(({ key, label }) => {
        const members = sections[key].filter(matches)
        return members.length ? <Collapsible key={key} className='routing-config-excluded' open={Boolean(sectionOpen[key]) || Boolean(query)}
          onOpenChange={open => setSectionOpen(current => ({ ...current, [key]: open }))}>
          <CollapsibleTrigger className='routing-excluded-trigger'><RouteIcon name='chevronDown' size={13} />{label} · {members.length}</CollapsibleTrigger>
          <CollapsibleContent data-route-list>{members.map(renderMember)}</CollapsibleContent>
        </Collapsible> : null
      })}
      {query && !group.members.some(matches) ? <Empty>{t('没有匹配的候选')}</Empty> : null}
      <Button variant='dashed' className='routing-add-member' disabled={locked} onClick={() => setDialog('add')}><RouteIcon name='plus' size={14} />{t('添加模型')}</Button>
    </div>
  </section>
  return <>
    <RoutingCanvasLayout title={t('自定义路由组')} groups={members} note={t('只影响这个路由成员，不改变账号启用状态，也不影响其他模型。新增账号需要手动加入。')}>
      <div className='routing-config-card' data-route-source>
        <div className='routing-config-card-scroll'>
          <header className='routing-config-head'><div><span className='routing-config-eyebrow'>{t('跨模型组合')}</span><h2>{group.name}</h2></div>
            <RoutingActions label={t('路由配置操作')} actions={[
              { label: t('编辑路由组'), icon: 'settings', disabled: locked, onClick: () => setDialog('edit') },
              { label: t('调用示例'), disabled: locked, onClick: () => setDialog('example') },
              { label: t('删除路由组'), destructive: true, disabled: locked, onClick: () => void editor.remove() },
            ]} />
          </header>
          {group.description ? <p className='routing-config-note'>{group.description}</p> : null}
          <div className='routing-config-alias'><span>{t('对外模型 ID')}</span><code title={group.model}>{group.model}</code>
            <IconButton name='copy' title={t('复制')} onClick={() => void copy(group.model)} /></div>
          <div className='routing-config-rules'>
            <div className='routing-rule-field'><Label>{t('路由策略')}</Label><RoutingSelect label={t('路由策略')} value={group.strategy} disabled={locked}
              options={[[ 'order', t('顺序优先') ], ['rotate', t('负载均衡')], ['provider', t('成员内轮询')]]} onChange={value => update(next => { next.strategy = value })} /></div>
            <div className='routing-rule-field'><Label>{t('会话粘滞')}</Label><RoutingSelect label={t('会话粘滞')} value={group.sticky} disabled={locked}
              options={[[ 'auto', t('自动') ], ['session', t('会话内')], ['turn', t('每轮')], ['off', t('关闭')]]} onChange={value => update(next => { next.sticky = value })} /></div>
          </div>
          <p className='routing-config-note'>{group.sticky === 'auto' || group.sticky === 'session' ? t('同一会话优先保持模型与账号，不可用时才顺延。') : t('每轮重新选路，不保留跨请求的账号绑定。')}</p>
          <Button variant='link' size='sm' className='routing-group-example' onClick={() => setDialog('example')}>{t('调用示例')}</Button>
        </div>
        <RoutingDraftFooter dirty={dirty} busy={editor.busy} disabled={locked} error={editor.error} onDiscard={editor.discard} onSave={() => void editor.save()} />
      </div>
    </RoutingCanvasLayout>
    <RoutingDragOverlay state={drag.state} />
    {dialog === 'edit' ? <RoutingGroupDialog group={group} reserved={reserved} onSave={group => update(next => Object.assign(next, group))} onClose={() => setDialog(null)} /> : null}
    {dialog === 'add' ? <RoutingMemberDialog choices={(editor.data?.catalog || []).filter(choice => choice.model.toLowerCase() !== group.model.toLowerCase()
      && !Object.values(editor.drafts).some(draft => draft.group.model.toLowerCase() === choice.model.toLowerCase()))} members={group.members} accounts={accounts} onAdd={members => update(next => { next.members.push(...members) })} onClose={() => setDialog(null)} /> : null}
    {selectedAccountMember ? <RoutingMemberAccounts member={selectedAccountMember} accounts={accounts} onSave={ids => editMember(selectedAccountMember.id, member => { member.accounts = ids })} onClose={() => setAccountMember(null)} /> : null}
    {dialog === 'example' ? <Dialog open onOpenChange={open => { if (!open) setDialog(null) }}><DialogContent>
      <DialogHeader><DialogTitle>{t('调用示例')}</DialogTitle></DialogHeader><DialogBody><p className='text-sm text-muted-foreground'>{t('保存路由后，在客户端填写此模型 ID。密钥使用你自己的网关 Key。')}</p><pre className='routing-group-code'>{snippet}</pre></DialogBody>
      <DialogFooter><Button variant='outline' onClick={() => setDialog(null)}>{t('关闭')}</Button><Button onClick={() => void copy(snippet)}>{t('复制')}</Button></DialogFooter>
    </DialogContent></Dialog> : null}
  </>
}
