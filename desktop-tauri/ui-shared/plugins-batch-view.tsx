import * as React from 'react'
import { Alert, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Label, Popover, PopoverContent, PopoverTitle, PopoverTrigger, Progress, Spinner, cn } from '@ui'
import { PluginArtwork } from './plugin-artwork'
import { applicablePluginItems, type PluginBatchAction, type PluginBatchItem } from './plugins-batch-items'
import type { PluginBatchTask } from './plugins-batch-runner'
import type { PluginTranslate } from './plugins-store'

export function pluginBatchVerb(action: PluginBatchAction, t: PluginTranslate): string {
  if (action === 'install') return t('安装')
  if (action === 'update') return t('更新')
  if (action === 'uninstall') return t('卸载')
  return action === 'enable' ? t('启用') : t('停用')
}

export function PluginBatchToolbar({ items, visibleKeys, selected, disabled, onSelection, onAction, onExit, t }: {
  items: PluginBatchItem[]; visibleKeys: string[]; selected: Set<string>; disabled: boolean
  onSelection(keys: string[], checked: boolean): void; onAction(action: PluginBatchAction): void; onExit(): void; t: PluginTranslate
}) {
  const [open, setOpen] = React.useState(false)
  const visibleSet = React.useMemo(() => new Set(visibleKeys), [visibleKeys])
  const picked = items.filter(item => selected.has(item.key))
  const count = visibleKeys.filter(key => selected.has(key)).length
  const installed = picked.filter(item => item.installed).length
  const updates = applicablePluginItems(picked, 'update').length
  return <div className='plugins-selection-bar' role='region' aria-label={t('批量管理')}>
    <Label className='flex shrink-0 items-center gap-3 text-xs font-normal'>
      <Checkbox className='size-4' disabled={disabled || !visibleKeys.length} checked={count > 0 && count === visibleKeys.length}
        indeterminate={count > 0 && count < visibleKeys.length} aria-label={t('全选当前结果')}
        onCheckedChange={value => onSelection(visibleKeys, value === true)} />
      {t('已选 {count} 个插件', { count: picked.length })}
    </Label>
    <span className='plugins-selection-summary'>{t('{installed} 已安装 · {missing} 未安装', { installed, missing: picked.length - installed })}</span>
    <div className='plugins-selection-actions'>
      {(['install', 'update', 'uninstall'] as const).map(action => {
        const targets = applicablePluginItems(picked, action)
        if (!targets.length) return null
        const title = action === 'install' ? t('安装所选（{count}）', { count: targets.length })
          : action === 'update' ? t('批量更新（{count}）', { count: targets.length }) : t('批量卸载（{count}）', { count: targets.length })
        return <Button key={action} size='sm' disabled={disabled} variant={action === 'update' ? 'default' : 'outline'}
          className={action === 'uninstall' ? 'text-destructive hover:text-destructive' : undefined} onClick={() => onAction(action)}>{title}</Button>
      })}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger render={<Button variant='outline' size='sm' disabled={disabled} />}>{t('更多')}</PopoverTrigger>
        <PopoverContent align='end' className='w-44 p-1.5'>
          <PopoverTitle className='sr-only'>{t('更多批量操作')}</PopoverTitle>
          {(['enable', 'disable'] as const).map(action => <Button key={action} variant='ghost' className='w-full justify-start' size='sm'
            disabled={!applicablePluginItems(picked, action).length} onClick={() => { setOpen(false); onAction(action) }}>
            {action === 'enable' ? t('批量启用') : t('批量停用')}
          </Button>)}
          <Button variant='ghost' size='sm' className='w-full justify-start' disabled={!updates && !items.some(item => visibleSet.has(item.key) && item.updatable)}
            onClick={() => { setOpen(false); onSelection([...selected], false); onSelection(items.filter(item => visibleSet.has(item.key) && item.updatable && !item.active).map(item => item.key), true) }}>{t('仅选可更新')}</Button>
          <Button variant='ghost' size='sm' className='w-full justify-start' onClick={() => { setOpen(false); onSelection([...selected], false) }}>{t('取消选择')}</Button>
        </PopoverContent>
      </Popover>
      <Button variant='ghost' size='sm' disabled={disabled} onClick={onExit}>{t('退出')}</Button>
    </div>
  </div>
}

export function PluginBatchConfirm({ action, items, disabled, onClose, onConfirm, t }: {
  action: PluginBatchAction; items: PluginBatchItem[]; disabled: boolean; onClose(): void; onConfirm(): void; t: PluginTranslate
}) {
  const [trusted, setTrusted] = React.useState(false)
  const scriptInstall = (action === 'install' || action === 'update') && items.some(item => item.kind === 'script')
  const destructive = action === 'uninstall' || action === 'disable'
  const verb = pluginBatchVerb(action, t)
  const accounts = items.reduce((sum, item) => sum + item.accounts, 0)
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='w-[min(680px,calc(100vw-48px))]'>
      <DialogHeader><DialogTitle>{t('{action}所选的 {count} 个插件', { action: verb, count: items.length })}</DialogTitle></DialogHeader>
      <DialogBody className='min-h-0 gap-4'>
        <ul className='plugins-confirm-list'>{items.map(item => <li key={item.key}>
          <PluginArtwork src={item.icon} />
          <div className='min-w-0 flex-1'><p className='m-0 text-sm'>{item.name}</p><p className='m-0 mt-1 text-xs text-muted-foreground'>{item.sourceLabel} · {action === 'update' ? item.latest || item.version : item.version || t('最新版本')}</p>{item.kind === 'script' && item.source ? <p className='m-0 mt-1 break-all text-xs text-muted-foreground'>{item.source}</p> : null}</div>
          {destructive && item.accounts ? <span className='shrink-0 text-xs text-warning'>{t('{count} 个账号受影响', { count: item.accounts })}</span> : null}
        </li>)}</ul>
        {destructive ? <Alert variant='warning'>{t('对应提供商将停止转发。账号、凭据和原有设置均保留，重新安装或启用并成功加载后恢复。')}</Alert> : null}
        {scriptInstall ? <>
          <Alert variant='warning'>{t('脚本不是安全沙箱，可以访问网关所在电脑的文件和网络。不要安装来历不明的插件。')}</Alert>
          <div className='space-y-2 text-xs text-subtle'>
            <p>{t('Magpie 官方和社区插件均通过兼容方式接入 Agent2API，不保证 100% 可用。')}</p>
            <p>{t('签到及附带任务默认允许，实际执行遵循签到中心的开关和计划。')}</p>
            <p className='text-muted-foreground'>{t('部分插件的附带任务可能消耗额度、发送对话或创建及解散群聊。')}</p>
          </div>
          <Label className='flex items-start gap-3 text-xs font-normal leading-5'><Checkbox checked={trusted} onCheckedChange={value => setTrusted(value === true)} />{t('我信任本次所选脚本插件的来源，并允许它们在网关电脑上运行。')}</Label>
        </> : null}
      </DialogBody>
      <DialogFooter className='flex-wrap'>
        <span className='mr-auto text-xs text-muted-foreground'>{destructive && accounts ? t('保留 {count} 个账号的数据', { count: accounts }) : t('单项失败不会中断其余插件')}</span>
        <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
        <Button variant={destructive ? 'destructive' : 'default'} disabled={disabled || scriptInstall && !trusted} onClick={onConfirm}>{t('确认{action}（{count}）', { action: verb, count: items.length })}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

export function PluginBatchProgress({ task, onRetry, onClose, t }: { task: PluginBatchTask; onRetry(items: PluginBatchItem[]): void; onClose(): void; t: PluginTranslate }) {
  const [open, setOpen] = React.useState(false)
  const completed = task.results.filter(item => !['waiting', 'working'].includes(item.state)).length
  const failed = task.results.filter(item => item.state === 'failed')
  const unresolved = task.results.some(item => item.state === 'unconfirmed')
  const verb = pluginBatchVerb(task.action, t)
  const current = task.results.find(item => item.state === 'working')
  const succeeded = task.results.filter(item => item.state === 'succeeded' || item.state === 'warning').length
  const warnings = task.results.filter(item => item.state === 'warning' || item.state === 'unconfirmed').length
  const processed = task.results.filter(item => item.state === 'succeeded')
  const restoredAccounts = task.action === 'install' ? processed.filter(item => item.item.required).reduce((total, item) => total + item.item.accounts, 0) : 0
  React.useEffect(() => { if (!task.running && task.results.some(item => item.state !== 'succeeded')) setOpen(true) }, [task.running, task.results])
  return <div className='plugins-task' role='status' aria-live='polite'>
    <div className='plugins-task-head'>
      {task.running ? <Spinner className='size-3.5' /> : null}
      <span className='text-xs'>{task.running ? t('正在批量{action}插件', { action: verb }) : unresolved ? t('本批操作已暂停，请先刷新插件状态') : t('批量{action}结束', { action: verb })}</span>
      <span className='text-xs tabular-nums text-muted-foreground'>{completed}/{task.results.length}</span>
      {current ? <span className='plugins-task-current'>{current.item.name} · {current.message}</span> : <span className='plugins-task-current'>{restoredAccounts ? t('{count} 个账号所需插件已就绪', { count: restoredAccounts }) : t('完成 {done} 个 · 失败 {failed} 个 · 需检查 {warnings} 个', { done: succeeded, failed: failed.length, warnings })}</span>}
      <div className='ml-auto flex items-center gap-2'>
        {!task.running && failed.length && !unresolved ? <Button variant='outline' size='xs' onClick={() => onRetry(failed.map(item => item.item))}>{t('重试失败项')}</Button> : null}
        <Button variant='link' size='xs' onClick={() => setOpen(value => !value)}>{open ? t('收起明细') : t('展开明细')}</Button>
        {!task.running ? <Button variant='ghost' size='xs' onClick={onClose}>{t('关闭')}</Button> : null}
      </div>
    </div>
    <Progress value={completed} max={task.results.length} aria-label={t('插件批量操作进度')} />
    {open ? <ul className='plugins-task-results'>{task.results.map(result => <li key={result.item.key} className={cn('text-xs', result.state === 'failed' ? 'text-destructive' : result.state === 'warning' || result.state === 'unconfirmed' ? 'text-warning' : 'text-subtle')}>
      <span>{result.item.name}</span><span className='ml-auto'>{result.state === 'succeeded' ? t('完成') : result.message || t('等待处理')}</span>
    </li>)}</ul> : null}
  </div>
}
