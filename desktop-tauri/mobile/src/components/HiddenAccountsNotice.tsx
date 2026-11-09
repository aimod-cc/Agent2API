import { useEffect, useId, useState, useSyncExternalStore } from 'react'
import { Badge, Button, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, SegmentedControl } from '@ui'
import type { HiddenAccountProvider, HiddenAccountsSummary } from '../../../ui-shared/plugins'
import { hiddenAction, hiddenDetail, hiddenFailed, hiddenInFilter, hiddenLabel, type HiddenFilter } from '../../../ui-shared/plugin-hidden'
import { getPluginStartup, hiddenWithStartup, refreshPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { store } from '../state/store'
import { t } from '../i18n'

export function HiddenAccountsNotice({ hidden }: { hidden?: HiddenAccountsSummary }) {
  const [open, setOpen] = useState(false)
  const [filter, setFilter] = useState<HiddenFilter>('all')
  const state = useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  const prefix = useId()
  useEffect(() => { if (open) refreshPluginStartup() }, [open])
  if (!hidden?.total) return null
  const providers = hiddenWithStartup(hidden).providers
  const broken = providers.filter(hiddenFailed).length
  const loading = providers.filter(item => item.reason === 'loading').length
  const countOf = (value: HiddenFilter) => providers.filter(item => hiddenInFilter(item, value)).length
  const shown = providers.filter(item => hiddenInFilter(item, filter))
  const goPlugins = (item?: HiddenAccountProvider) => {
    setOpen(false)
    const inspect = item?.packId && !['not_installed', 'pack_disabled', 'provider_disabled'].includes(item.reason)
    store.push('plugins', inspect ? { id: item.packId! } : undefined)
  }
  return <>
    <div className='m-section'>
      <Button variant='outline' size='sm' id={`${prefix}-entry`} onClick={() => setOpen(true)}>
        {t('未显示账号')} {providers.length}
        {broken ? <Badge variant='warning' shape='tag'>{t('加载异常')}</Badge> : null}
      </Button>
    </div>
    <Sheet open={open} onOpenChange={next => { if (!next) setOpen(false) }}>
      <SheetContent>
        <SheetHeader><SheetTitle>{t('未显示的账号')}</SheetTitle></SheetHeader>
        <SheetBody className='gap-3'>
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>
            {t('{providers} 家提供商、共 {total} 个账号。数据仍在，只是当前没有可用的插件来执行它们。', { providers: providers.length, total: hidden.total })}
          </p>
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
          {shown.map(item => <div key={item.id} className='flex items-center gap-2.5 border-t border-hairline pt-2.5'>
            <div className='min-w-0 flex-1'>
              <div className='flex items-center gap-1.5 text-[13px] font-medium'><span className='truncate'>{t(item.label)}</span>
                <Badge variant={hiddenFailed(item) ? 'warning' : 'secondary'} shape='tag'>{hiddenLabel(item, t)}</Badge></div>
              <p className='m-0 mt-0.5 text-xs text-muted-foreground'>
                {t('保留 {n} 个账号', { n: item.count })} · {hiddenDetail(item, t, state.value?.now)}
              </p>
            </div>
            <Button variant='outline' size='sm' onClick={() => goPlugins(item)}>{hiddenAction(item, t)}</Button>
          </div>)}
          {!shown.length ? <p className='m-0 text-center text-xs text-muted-foreground'>{t('这一类现在没有账号')}</p> : null}
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('这里只影响「显示」：卸载或停用插件不会删除账号；重新安装、启用并成功加载后，这些账号会自动回到列表。')}</p>
        </SheetBody>
        <SheetFooter><Button variant='outline' onClick={() => setOpen(false)}>{t('知道了')}</Button>
          <Button onClick={() => goPlugins()}>{t('去「插件」页')}</Button></SheetFooter>
      </SheetContent>
    </Sheet>
  </>
}
