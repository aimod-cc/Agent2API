import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import { RouteIcon } from './routing-ui'
import type { RouteGroup, RouteMember, RouteModelChoice } from './routing-types'
import { createRouteMembers, routeGroupFieldsError, routeModelKey, RoutingGroupForm, ROUTING_MEMBER_LIMIT, type RoutingGroupFields } from './routing-group-form'
import { RoutingModelPicker } from './routing-model-picker'

export function RoutingGroupDialog({ group, reserved, onSave, onClose }: {
  group: RouteGroup; reserved: string[]; onSave(group: RouteGroup): void; onClose(): void
}) {
  const [fields, setFields] = React.useState<RoutingGroupFields>({ name: group.name, model: group.model, description: group.description })
  const [error, setError] = React.useState('')
  const save = () => {
    const message = routeGroupFieldsError(fields, reserved, group.model)
    if (message) { setError(message); return }
    onSave({ ...group, name: fields.name.trim(), model: fields.model.trim().toLowerCase(), description: fields.description.trim() })
    onClose()
  }
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='w-[min(600px,calc(100vw-40px))]'>
    <DialogHeader><RouteIcon name='route' /><DialogTitle>{t('编辑路由组')}</DialogTitle></DialogHeader>
    <DialogBody>
      <p className='routing-config-note'>{t('把不同提供商、不同模型组合成一个入口。客户端使用对外模型 ID 发起请求。')}</p>
      <RoutingGroupForm value={fields} onChange={setFields} />
      {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
    </DialogBody><DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button onClick={save}>{t('应用到草稿')}</Button></DialogFooter>
  </DialogContent></Dialog>
}

export function RoutingMemberDialog({ choices, members, accounts, onAdd, onClose }: {
  choices: RouteModelChoice[]; members: RouteMember[]; accounts: AccountRecord[]; onAdd(members: RouteMember[]): void; onClose(): void
}) {
  const [selected, setSelected] = React.useState<string[]>([])
  const options = choices.filter(choice => !members.some(member => member.provider === choice.provider && member.model.toLowerCase() === choice.model.toLowerCase()))
  const byKey = new Map(options.map(choice => [routeModelKey(choice), choice]))
  const picked = selected.flatMap(key => { const choice = byKey.get(key); return choice ? [choice] : [] })
  const limit = Math.max(0, ROUTING_MEMBER_LIMIT - members.length)
  const add = () => {
    if (!picked.length || picked.length > limit) return
    onAdd(createRouteMembers(picked, accounts))
    onClose()
  }
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='routing-create-dialog'>
    <DialogHeader><DialogTitle>{t('添加参与模型')}</DialogTitle></DialogHeader><DialogBody>
      <p className='routing-config-note'>{t('可以跨提供商、跨模型多选。同一提供商也可以加入不同模型。')}</p>
      <RoutingModelPicker choices={options} selected={selected} onChange={setSelected} limit={limit} />
      <p className='routing-config-note'>{t('仅列出真实目录中的对话模型，不自动更改模型映射。')}</p>
    </DialogBody><DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button disabled={!picked.length || picked.length > limit} onClick={add}>{t('添加')} · {picked.length}</Button></DialogFooter>
  </DialogContent></Dialog>
}

export function RoutingMemberAccounts({ member, accounts, onSave, onClose }: {
  member: RouteMember; accounts: AccountRecord[]; onSave(ids: string[]): void; onClose(): void
}) {
  const [selected, setSelected] = React.useState(member.accounts)
  const available = accounts.filter(account => providerOf(account) === member.provider)
  const order = [...new Set([...selected, ...available.map(account => account.id)])]
  const move = (id: string, direction: number) => setSelected(current => {
    const next = [...current], index = next.indexOf(id), to = index + direction
    if (index >= 0 && to >= 0 && to < next.length) [next[index], next[to]] = [next[to], next[index]]
    return next
  })
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='w-[min(680px,calc(100vw-40px))]'>
    <DialogHeader><DialogTitle>{t('参与账号')} · {member.model}</DialogTitle></DialogHeader><DialogBody>
      <p className='text-sm text-muted-foreground'>{t('只影响这个路由成员，不改变账号启用状态，也不影响其他模型。新增账号需要手动加入。')}</p>
      <div className='routing-model-picker'>{order.map(id => {
        const account = available.find(item => item.id === id), on = selected.includes(id), index = selected.indexOf(id)
        return <div className='routing-group-picker-row' key={id}>
          <div><b>{account ? displayNameOf(account) : t('账号已移除')}</b><small>{account?.enabled === false ? t('账号已禁用') : on ? t('参与本路由') : t('未参与本路由')}</small></div>
          <Button variant='ghost' size='icon' disabled={!on || index === 0} aria-label={t('提高优先级')} onClick={() => move(id, -1)}><RouteIcon name='arrowUp' /></Button>
          <Button variant='ghost' size='icon' disabled={!on || index === selected.length - 1} aria-label={t('降低优先级')} onClick={() => move(id, 1)}><RouteIcon name='arrowDown' /></Button>
          <Button variant={on ? 'ghost' : 'outline'} size='sm' disabled={!account && !on} onClick={() => setSelected(current => on ? current.filter(value => value !== id) : [...current, id])}>{on ? t('移出本路由') : t('加入路由')}</Button>
        </div>
      })}{!order.length ? <Empty>{t('该提供商没有账号，请先到账号页添加。')}</Empty> : null}</div>
    </DialogBody><DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button onClick={() => { onSave(selected); onClose() }}>{t('应用到草稿')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
