import * as React from 'react'
import { Badge, Button, SegmentedControl, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, StatusDot, confirmDialog } from '@ui'
import { t } from '../i18n'
import { errorMessage, shared, toast, type HiddenAccountsSummary } from './accounts-shared'
import type { HiddenAccountProvider } from '../../../ui-shared/plugins'
import {
  allHiddenReplacedByTemplate, hiddenAction, hiddenDetail, hiddenFailed, hiddenFootText,
  hiddenInFilter, hiddenLabel, hiddenLeadText, isReplacedByTemplate, retiredRemoveConfirmText,
  retiredTemplateKey, type HiddenFilter,
} from '../../../ui-shared/plugin-hidden'
import { getPluginStartup, hiddenWithStartup, refreshPluginStartup, refreshPluginStartupIfIdle, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { requestPluginDiagnostics } from '../../../ui-shared/plugin-startup-view'

export type HiddenAccountsDrawerHandle = { open(): void }

function useHiddenAccounts(summary: HiddenAccountsSummary) {
  const state = React.useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  const hidden = React.useMemo(() => hiddenWithStartup(summary), [summary, state.value])
  return { hidden, state }
}

async function removeRetiredProviderAccounts(provider: string): Promise<void> {
  const invoke = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: (command: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__?.invoke
  if (!invoke) throw new Error(t('桌面运行时不可用（Tauri 未初始化）'))
  const result = await invoke('api_request', {
    request: { method: 'POST', path: '/api/accounts/retired/remove', body: { provider } },
  })
  if (result && typeof result === 'object' && (result as { success?: boolean }).success === false) {
    throw new Error((result as { error?: string }).error || t('请求失败'))
  }
}

export function brokenCountOf(hidden: HiddenAccountsSummary): number {
  return hidden.providers.filter(hiddenFailed).length
}

export function HiddenAccountsEntry({ hidden: summary, onOpen }: {
  hidden: HiddenAccountsSummary; onOpen(): void
}): React.ReactElement | null {
  const { hidden } = useHiddenAccounts(summary)
  if (hidden.total <= 0) return null
  return <Button id='btn-hidden-accounts' variant='outline' className='acct-hidden-entry'
    title={t('查看暂未在列表显示的账号')} onClick={onOpen}>
    <svg viewBox='0 0 24 24' width='14' height='14' fill='none' stroke='currentColor' strokeWidth={1.7}
      strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M2.5 12s3.4-6 9.5-6 9.5 6 9.5 6-3.4 6-9.5 6-9.5-6Z' /><path d='m4 20 16-16' />
    </svg>
    {t('未显示账号')}<Badge variant={brokenCountOf(hidden) ? 'warning' : 'outline'} shape='tag'>{hidden.providers.length}</Badge>
  </Button>
}

export const HiddenAccountsDrawer = React.forwardRef<HiddenAccountsDrawerHandle, {
  hidden: HiddenAccountsSummary
}>(function HiddenAccountsDrawer({ hidden: summary }, ref): React.ReactElement {
  const [open, setOpen] = React.useState(false)
  const [filter, setFilter] = React.useState<HiddenFilter>('all')
  const [removingId, setRemovingId] = React.useState('')
  const { hidden, state } = useHiddenAccounts(summary)
  React.useImperativeHandle(ref, () => ({ open: () => setOpen(true) }), [])
  React.useEffect(() => { if (open) refreshPluginStartupIfIdle() }, [open])
  const onlyRetired = allHiddenReplacedByTemplate(hidden.providers)
  const activeFilter: HiddenFilter = onlyRetired ? 'all' : filter
  const countOf = (value: HiddenFilter) => hidden.providers.filter(item => hiddenInFilter(item, value)).length
  const shown = hidden.providers.filter(item => hiddenInFilter(item, activeFilter))
  const loading = hidden.providers.filter(item => item.reason === 'loading').length
  const goPlugins = (item?: HiddenAccountProvider) => {
    if (item?.packId && !['not_installed', 'legacy_unsupported', 'pack_disabled', 'provider_disabled'].includes(item.reason)) requestPluginDiagnostics(item.packId)
    setOpen(false)
    shared().wbApp?.showPage?.('plugins')
  }
  const openTemplate = (item: HiddenAccountProvider) => {
    const key = retiredTemplateKey(item)
    setOpen(false)
    if (key && shared().wbAddAccountModal?.openProvider) shared().wbAddAccountModal?.openProvider?.(`preset:${key}`)
    else shared().wbAddAccountModal?.open?.()
  }
  const removeRetired = async (item: HiddenAccountProvider) => {
    if (removingId) return
    if (!await confirmDialog(retiredRemoveConfirmText(item, t), {
      title: t('移除旧账号'), confirmText: t('移除'), destructive: true,
    })) return
    setRemovingId(item.id)
    try {
      await removeRetiredProviderAccounts(item.id)
      toast(t('✅ 已移除 {name} 旧账号', { name: t(item.label) }), 'ok')
      await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('移除失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setRemovingId('')
    }
  }
  return <Sheet open={open} onOpenChange={next => { if (!next) setOpen(false) }}>
    <SheetContent side='right' showHandle={false} className='w-[min(480px,94vw)]'>
      <SheetHeader><SheetTitle>{t('未显示的账号')}</SheetTitle></SheetHeader>
      <SheetBody className='acct-hidden-body'>
        <p className='acct-hidden-lead'>{hiddenLeadText(hidden.providers, hidden.total, t)}</p>
        {loading ? <p className='m-0 text-xs text-muted-foreground' role='status'>
          {t('正在加载 · {n} 家提供商的账号暂未显示，就绪后自动恢复。', { n: loading })}
        </p> : null}
        {state.error && !state.unavailable && !onlyRetired ? <p className='m-0 text-xs text-muted-foreground' role='status'>
          {t('暂时无法获取插件状态，显示上次结果；这不代表插件启动失败。')}
          <Button variant='link' size='xs' onClick={refreshPluginStartup}>{t('刷新状态')}</Button>
        </p> : null}
        {!onlyRetired ? <SegmentedControl aria-label={t('按未显示原因筛选')} value={activeFilter} onValueChange={setFilter} options={[
          { value: 'all', label: t('全部'), count: countOf('all') },
          { value: 'loading', label: t('待就绪'), count: countOf('loading') },
          { value: 'missing', label: t('未安装'), count: countOf('missing') },
          { value: 'disabled', label: t('已停用'), count: countOf('disabled') },
          { value: 'broken', label: t('加载异常'), count: countOf('broken') },
        ]} /> : null}
        <div className='acct-hidden-rows'>{shown.map(item => <div key={item.id} className='acct-hidden-row'>
          <span className='acct-hidden-avatar' aria-hidden='true'>{t(item.label).trim().slice(0, 1)}</span>
          <div className='acct-hidden-info'>
            <div className='acct-hidden-name'><span className='truncate'>{t(item.label)}</span>
              <Badge variant={hiddenFailed(item) ? 'warning' : 'outline'} shape='tag'>{hiddenLabel(item, t)}</Badge>
            </div>
            <p className='acct-hidden-detail'>{t('保留 {n} 个账号', { n: item.count })} · {hiddenDetail(item, t, state.value?.now)}</p>
          </div>
          {isReplacedByTemplate(item) ? <div className='flex shrink-0 items-center gap-1.5'>
            <Button variant='outline' size='sm' disabled={Boolean(removingId)} onClick={() => openTemplate(item)}>{hiddenAction(item, t)}</Button>
            <Button variant='ghost' size='sm' disabled={Boolean(removingId)} onClick={() => void removeRetired(item)}>{removingId === item.id ? t('移除中…') : t('移除')}</Button>
          </div> : <Button variant='outline' size='sm' onClick={() => goPlugins(item)}>{hiddenAction(item, t)}</Button>}
        </div>)}{!shown.length ? <p className='acct-hidden-none'>{t('这一类现在没有账号')}</p> : null}</div>
        <p className='acct-hidden-foot'>{hiddenFootText(hidden.providers, t)}</p>
      </SheetBody>
      <SheetFooter><Button variant='outline' onClick={() => setOpen(false)}>{t('知道了')}</Button>
        {!onlyRetired ? <Button onClick={() => goPlugins()}>{t('去「插件」页')}</Button> : null}</SheetFooter>
    </SheetContent>
  </Sheet>
})

export function HiddenAccountsEmptyNote({ hidden: summary, onOpen }: { hidden: HiddenAccountsSummary; onOpen(): void }): React.ReactElement {
  const { hidden, state } = useHiddenAccounts(summary)
  const booting = state.value?.booting
  return <div className='acct-hidden-empty' role='status'>
    <span><StatusDot variant={booting ? 'muted' : brokenCountOf(hidden) ? 'warning' : 'muted'} className='size-1.5' />
      {booting ? t('正在准备插件，账号将在本轮启动完成后统一显示。') : t('共有 {n} 个账号暂未显示', { n: hidden.total })}</span>
    <Button variant='outline' size='sm' onClick={onOpen}>{t('查看')}</Button>
  </div>
}
