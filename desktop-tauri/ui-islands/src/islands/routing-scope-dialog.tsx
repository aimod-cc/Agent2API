import * as React from 'react'
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogSection, DialogTitle, Empty, Field, FieldContent, FieldDescription, FieldLabel,
  Label, SegmentedControl,
} from '@ui'
import { t } from '../i18n'
import { saveProfile } from './routing-state'
import type { RouteProviderScope, RoutingWorkspace } from './routing-types'

export function GlobalRoutingScopeDialog({ workspace, onClose }: { workspace: RoutingWorkspace; onClose(): void }) {
  const [draft, setDraft] = React.useState(() => structuredClone(workspace.profile))
  const [busy, setBusy] = React.useState(false)
  const initialScope = workspace.profile.providerScope || 'selected'
  const scope = draft.providerScope || 'selected'
  const options = workspace.providers
  const setScope = (value: RouteProviderScope) => setDraft(current => ({
    ...current, providerScope: value,
    providers: [...new Set([...current.providers, ...current.disabledProviders,
      ...(value === 'all' ? options.map(provider => provider.id) : [])])],
  }))
  const toggleProvider = (id: string, on: boolean) => setDraft(current => ({
    ...current,
    providers: [...new Set([...current.providers, id])],
    disabledProviders: on ? current.disabledProviders.filter(provider => provider !== id)
      : [...new Set([...current.disabledProviders, id])],
  }))
  const save = async () => {
    setBusy(true)
    if (await saveProfile(draft, '')) onClose()
    setBusy(false)
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='w-[min(640px,calc(100vw-48px))]'>
      <DialogHeader><DialogTitle>{t('全局参与范围')}</DialogTitle></DialogHeader>
      <DialogBody>
        <Field><FieldLabel>{t('提供商参与方式')}</FieldLabel><FieldContent>
          <SegmentedControl aria-label={t('提供商参与方式')} value={scope} disabled={busy} options={[
            { value: 'all', label: t('自动参与（推荐）') },
            { value: 'selected', label: t('仅允许指定提供商') },
          ]} onValueChange={setScope} />
          <FieldDescription>{scope === 'all'
            ? t('所有可用提供商自动参与，新增账号或插件后无需再添加路由。已停用的提供商和已排除的账号保持不变。')
            : t('只有名单内的提供商可以参与；新增提供商不会自动加入。')}</FieldDescription>
        </FieldContent></Field>
        {initialScope === 'selected' && scope === 'all' ? <p className='text-xs text-warning' role='status'>
          {t('切换后，原本不在名单内的可用提供商也会参与转发。明确停用的提供商和排除的账号不会恢复。')}
        </p> : null}
        {scope === 'selected' ? <DialogSection>
          <h3 className='text-sm font-semibold'>{t('允许参与的提供商')}</h3>
          {options.length ? options.map(provider => <Label key={provider.id} className='flex items-center gap-3 rounded-md border border-border p-3'>
            <Checkbox checked={draft.providers.includes(provider.id) && !draft.disabledProviders.includes(provider.id)} disabled={busy}
              onCheckedChange={on => toggleProvider(provider.id, on === true)} />
            <span className='min-w-0 flex-1'>{provider.label}</span>
            <span className='text-xs text-muted-foreground'>{t('{n} 个账号', { n: provider.accountCount })}</span>
          </Label>) : <Empty>{t('没有可用提供商，请先添加账号或安装插件。')}</Empty>}
        </DialogSection> : null}
        <p className='text-xs text-muted-foreground'>{t('只修改全局参与范围，不改变账号排序、模型开关或模型独立路由；网关 Key 权限与限流仍然生效。')}</p>
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
