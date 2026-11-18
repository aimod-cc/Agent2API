import * as React from 'react'
import {
  Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogSection, DialogTitle, Empty, Field, FieldContent, FieldDescription, FieldLabel,
  Input, Label, SegmentedControl, Switch, confirmDialog,
} from '@ui'
import { t } from '../i18n'
import { cloneProfile, independentProfile, loadWorkspace, saveProfile } from './routing-state'
import { bridge, errorMessage, host, notify, type RouteSettings, type RoutingWorkspace } from './routing-types'
import { RouteIcon, RouteStateBadge } from './routing-ui'
import { displayNameOf } from './accounts-domain'
import { groupSortSummary } from './accounts-group-settings'

export function ProviderRoutingDialog({ provider, workspace, onSort, onClose }: { provider: string; workspace: RoutingWorkspace; onSort(): void; onClose(): void }) {
  const label = workspace.providers.find(item => item.id === provider)?.label || provider
  const [draft, setDraft] = React.useState(() => workspace.model && !workspace.customized
    ? independentProfile(workspace.profile) : structuredClone(workspace.profile))
  const [busy, setBusy] = React.useState(false)
  const enabled = !draft.disabledProviders.includes(provider)
  const automatic = draft.providerScope === 'all'
  const accounts = workspace.accounts.filter(account => account.provider === provider)
  const statuses = new Map([...workspace.queue.ordered, ...workspace.queue.blocked].map(entry => [entry.id, entry]))
  const toggleAccount = (id: string, on: boolean) => setDraft(current => ({
    ...current, excludedAccounts: on ? current.excludedAccounts.filter(item => item !== id) : [...current.excludedAccounts.filter(item => item !== id), id],
  }))
  const save = async () => { setBusy(true); if (await saveProfile(draft)) onClose(); setBusy(false) }
  const remove = async () => {
    if (!await confirmDialog(t('只从当前路由移出，保留模型和账号配置；不会改变其他路由。'), {
      title: t('移出本路由'), confirmText: t('移出'),
    })) return
    const next = structuredClone(draft)
    if (!next.providers.includes(provider)) next.providers.push(provider)
    next.disabledProviders = [...new Set([...next.disabledProviders, provider])]
    setBusy(true)
    if (await saveProfile(next)) onClose()
    setBusy(false)
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='w-[min(640px,calc(100vw-48px))]'>
      <DialogHeader><RouteIcon name='plugins' /><DialogTitle>{t('{name} · 提供商路由设置', { name: label })}</DialogTitle></DialogHeader>
      <DialogBody>
        <Badge variant='brand' shape='tag' className='self-start'>{workspace.model || t('全局配置')}</Badge>
        <DialogSection>
          <div className='flex items-center justify-between gap-3'><h3 className='text-sm font-semibold'>{t('消耗顺序')}</h3>
            <Button variant='outline' size='sm' disabled={busy} onClick={onSort}>{t('调整全局消耗规则')}</Button></div>
          <p className='text-xs text-muted-foreground'>{workspace.model && workspace.customized
            ? t('当前模型使用独立顺序，不受全局自动消耗排序影响。删除独立路由后会重新跟随账号页。')
            : t('与账号页共用：{rules}。积分到期等规则直接影响网关实际选路。', { rules: groupSortSummary(workspace.sorting.rules[provider] || []) || t('仅按手动顺序') })}</p>
        </DialogSection>
        {workspace.providers.find(item => item.id === provider)?.available === false ? <p className='text-xs text-warning'>{t('此提供商当前没有可用的模型绑定，配置已保留；在模型管理中恢复绑定后会自动回来。')}</p> : null}
        <Field orientation='horizontal'><FieldContent><FieldLabel>{t('参与本路由')}</FieldLabel><FieldDescription>{t('这里只决定是否参与本路由，不改变账号启用状态。')}</FieldDescription></FieldContent>
          <Button variant='outline' disabled={busy} onClick={() => setDraft(current => ({ ...current,
            providers: [...new Set([...current.providers, provider])],
            disabledProviders: enabled ? [...new Set([...current.disabledProviders, provider])] : current.disabledProviders.filter(id => id !== provider),
          }))}>{enabled ? t('移出本路由') : t('加入路由')}</Button></Field>
        <DialogSection><h3 className='text-sm font-semibold'>{t('参与路由的账号')}</h3>
          {!accounts.length ? <Empty>{t('该提供商没有账号，请先到账号页添加。')}</Empty> : accounts.map(account => <div key={account.id} className='flex items-center gap-3 border-b border-hairline py-2 last:border-0'>
            <Checkbox checked={!draft.excludedAccounts.includes(account.id)} disabled={busy} aria-label={displayNameOf(account)} onCheckedChange={on => toggleAccount(account.id, on === true)} />
            <span className='min-w-0 flex-1 text-xs'>{displayNameOf(account)}</span><RouteStateBadge state={statuses.get(account.id)?.state || (account.enabled === false ? 'disabled' : 'usable')} />
          </div>)}
          <p className='text-xs text-muted-foreground'>{t('这里的参与范围只影响当前视图；账号启停、并发和限制器属于账号本身，所有模型共用。')}</p>
        </DialogSection>
        <div className='flex flex-wrap gap-2'><Button variant='outline' onClick={() => { onClose(); host().wbApp?.showPage('gateway') }}>{t('模型管理')}</Button>
          <Button variant='outline' onClick={() => { onClose(); host().wbApp?.showPage('settings'); host().wbSettingsPanel?.showCategory('gateway') }}>{t('系统提示词')}</Button>
          <Button variant='outline' onClick={() => { onClose(); host().wbApp?.showPage('settings'); host().wbSettingsPanel?.showCategory('security') }}>{t('指纹脱敏')}</Button></div>
      </DialogBody>
      <DialogFooter><Button variant='ghost' className='mr-auto text-destructive' disabled={busy || (automatic && !enabled)} onClick={() => void remove()}>{t('移出本路由')}</Button><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button><Button disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}

export function AddRouteProviderDialog({ workspace, onClose }: { workspace: RoutingWorkspace; onClose(): void }) {
  const [picked, setPicked] = React.useState<string[]>([])
  const automatic = workspace.profile.providerScope === 'all' && (!workspace.model || workspace.customized)
  const options = workspace.providers.filter(provider => !workspace.profile.providers.includes(provider.id)
    || automatic && workspace.profile.disabledProviders.includes(provider.id))
  const [busy, setBusy] = React.useState(false)
  const save = async () => {
    const profile = cloneProfile()
    for (const id of picked) {
      if (!profile.providers.includes(id)) profile.providers.push(id)
      if (automatic) profile.disabledProviders = profile.disabledProviders.filter(provider => provider !== id)
      profile.accountOrder[id] = workspace.accounts.filter(account => account.provider === id)
        .sort((left, right) => Number(left.priority) - Number(right.priority)).map(account => account.id)
    }
    setBusy(true)
    if (await saveProfile(profile)) onClose()
    setBusy(false)
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}><DialogContent>
    <DialogHeader><DialogTitle>{automatic ? t('恢复路由提供商') : t('添加路由提供商')}</DialogTitle></DialogHeader><DialogBody>
      <p className='text-xs text-muted-foreground'>{automatic ? t('新增可用提供商会自动参与；这里只需恢复曾明确停用的提供商。')
        : workspace.model ? t('只列出当前模型已有启用绑定的提供商；这里只设置路由，不新建模型绑定。') : t('添加到固定参与名单；以后新增的提供商仍需手动加入。')}</p>
      {options.length ? options.map(provider => <Label key={provider.id} className='flex items-center gap-3 rounded-md border border-border p-3'>
        <Checkbox checked={picked.includes(provider.id)} disabled={busy} onCheckedChange={on => setPicked(current => on ? [...current, provider.id] : current.filter(id => id !== provider.id))} />
        <span className='flex-1'>{provider.label}</span><Badge shape='tag' variant='secondary'>{t('{n} 个账号', { n: provider.accountCount })}</Badge>
      </Label>) : <Empty>{automatic ? t('没有需要恢复的提供商') : t('所有可用提供商都已在当前路由中')}</Empty>}
    </DialogBody><DialogFooter><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button><Button disabled={busy || !picked.length} onClick={() => void save()}>{automatic ? t('恢复') : t('添加')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
