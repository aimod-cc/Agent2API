import * as React from 'react'
import { Badge, Button, SegmentedControl, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, StatusDot } from '@ui'
import { t } from '../i18n'
import { shared, type HiddenAccountsSummary } from './accounts-shared'
import type { HiddenAccountProvider } from '../../../ui-shared/plugins'
import { hiddenAction, hiddenDetail, hiddenFailed, hiddenInFilter, hiddenLabel, type HiddenFilter } from '../../../ui-shared/plugin-hidden'
import { getPluginStartup, refreshPluginStartup } from '../../../ui-shared/plugin-startup'
import { requestPluginDiagnostics } from '../../../ui-shared/plugin-startup-view'

export function brokenCountOf(hidden: HiddenAccountsSummary): number {
  return hidden.providers.filter(hiddenFailed).length
}

export function HiddenAccountsEntry({ hidden, onOpen }: {
  hidden: HiddenAccountsSummary; onOpen(): void
}): React.ReactElement | null {
  if (hidden.total <= 0) return null
  return <Button id='btn-hidden-accounts' variant='outline' className='acct-hidden-entry'
    title={t('查看因插件未就绪而暂未显示的账号')} onClick={() => { refreshPluginStartup(); onOpen() }}>
    <svg viewBox='0 0 24 24' width='14' height='14' fill='none' stroke='currentColor' strokeWidth={1.7}
      strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M2.5 12s3.4-6 9.5-6 9.5 6 9.5 6-3.4 6-9.5 6-9.5-6Z' /><path d='m4 20 16-16' />
    </svg>
    {t('未显示账号')}<Badge variant={brokenCountOf(hidden) ? 'warning' : 'outline'} shape='tag'>{hidden.providers.length}</Badge>
  </Button>
}

export function HiddenAccountsDrawer({ hidden, open, onOpenChange }: {
  hidden: HiddenAccountsSummary; open: boolean; onOpenChange(open: boolean): void
}): React.ReactElement {
  const [filter, setFilter] = React.useState<HiddenFilter>('all')
  React.useEffect(() => { if (open) refreshPluginStartup() }, [open])
  const state = getPluginStartup()
  const countOf = (value: HiddenFilter) => hidden.providers.filter(item => hiddenInFilter(item, value)).length
  const shown = hidden.providers.filter(item => hiddenInFilter(item, filter))
  const loading = hidden.providers.filter(item => item.reason === 'loading').length
  const goPlugins = (item?: HiddenAccountProvider) => {
    if (item?.packId && !['not_installed', 'pack_disabled', 'provider_disabled'].includes(item.reason)) requestPluginDiagnostics(item.packId)
    onOpenChange(false)
    shared().wbApp?.showPage?.('plugins')
  }
  return <Sheet open={open} onOpenChange={next => { if (!next) onOpenChange(false) }}>
    <SheetContent side='right' showHandle={false} className='w-[min(480px,94vw)]'>
      <SheetHeader><SheetTitle>{t('未显示的账号')}</SheetTitle></SheetHeader>
      <SheetBody className='acct-hidden-body'>
        <p className='acct-hidden-lead'>{t('{providers} 家提供商、共 {total} 个账号。数据仍在，只是当前没有可用的插件来执行它们。', {
          providers: hidden.providers.length, total: hidden.total,
        })}</p>
        {loading ? <p className='m-0 text-xs text-muted-foreground' role='status'>
          {t('正在加载 · {n} 家提供商的账号暂未显示，就绪后自动恢复。', { n: loading })}
        </p> : null}
        {state.error && !state.unavailable ? <p className='m-0 text-xs text-muted-foreground' role='status'>
          {t('暂时无法获取插件状态，显示上次结果；这不代表插件启动失败。')}
          <Button variant='link' size='xs' onClick={refreshPluginStartup}>{t('刷新状态')}</Button>
        </p> : null}
        <SegmentedControl aria-label={t('按未显示原因筛选')} value={filter} onValueChange={setFilter} options={[
          { value: 'all', label: t('全部'), count: countOf('all') },
          { value: 'loading', label: t('待就绪'), count: countOf('loading') },
          { value: 'missing', label: t('未安装'), count: countOf('missing') },
          { value: 'disabled', label: t('已停用'), count: countOf('disabled') },
          { value: 'broken', label: t('加载异常'), count: countOf('broken') },
        ]} />
        <div className='acct-hidden-rows'>{shown.map(item => <div key={item.id} className='acct-hidden-row'>
          <span className='acct-hidden-avatar' aria-hidden='true'>{t(item.label).trim().slice(0, 1)}</span>
          <div className='acct-hidden-info'>
            <div className='acct-hidden-name'><span className='truncate'>{t(item.label)}</span>
              <Badge variant={hiddenFailed(item) ? 'warning' : 'outline'} shape='tag'>{hiddenLabel(item, t)}</Badge>
            </div>
            <p className='acct-hidden-detail'>{t('保留 {n} 个账号', { n: item.count })} · {hiddenDetail(item, t, state.value?.now)}</p>
          </div>
          <Button variant='outline' size='sm' onClick={() => goPlugins(item)}>{hiddenAction(item, t)}</Button>
        </div>)}{!shown.length ? <p className='acct-hidden-none'>{t('这一类现在没有账号')}</p> : null}</div>
        <p className='acct-hidden-foot'>{t('这里只影响「显示」：卸载或停用插件不会删除账号；重新安装、启用并成功加载后，这些账号会自动回到列表。')}</p>
      </SheetBody>
      <SheetFooter><Button variant='outline' onClick={() => onOpenChange(false)}>{t('知道了')}</Button>
        <Button onClick={() => goPlugins()}>{t('去「插件」页')}</Button></SheetFooter>
    </SheetContent>
  </Sheet>
}

export function HiddenAccountsEmptyNote({ hidden, onOpen }: { hidden: HiddenAccountsSummary; onOpen(): void }): React.ReactElement {
  return <div className='acct-hidden-empty' role='status'>
    <span><StatusDot variant={brokenCountOf(hidden) ? 'warning' : 'muted'} className='size-1.5' />{t('共有 {n} 个账号暂未显示', { n: hidden.total })}</span>
    <Button variant='outline' size='sm' onClick={onOpen}>{t('查看')}</Button>
  </div>
}
