import * as React from 'react'
import { Alert, Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Label } from '@ui'
import { PluginArtwork } from './plugin-artwork'
import type { HiddenAccountProvider } from './plugins'
import type { PluginBatchItem } from './plugins-batch-items'
import type { PluginTranslate } from './plugins-store'

export function PluginMissingBanner({ count, accounts, onOpen, t }: { count: number; accounts: number; onOpen(): void; t: PluginTranslate }) {
  return <div className='plugins-missing-banner'>
    <svg viewBox='0 0 24 24' className='size-5 shrink-0 text-warning' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'><path d='M9 3v5M15 3v5M6.5 8h11v3.5a5.5 5.5 0 0 1-11 0V8ZM12 17v4' /></svg>
    <p className='m-0 min-w-0 flex-1 text-xs'>{accounts ? t('{count} 个插件缺失，{accounts} 个账号暂不可用', { count, accounts }) : t('{count} 个插件文件缺失', { count })}</p>
    <Button size='sm' onClick={onOpen}>{t('补全缺失插件')}</Button>
  </div>
}

export function PluginMissingDialog({ required, optional, unresolved, disabled, onClose, onInstall, t }: {
  required: PluginBatchItem[]; optional: PluginBatchItem[]; unresolved: HiddenAccountProvider[]; disabled: boolean
  onClose(): void; onInstall(items: PluginBatchItem[]): void; t: PluginTranslate
}) {
  const [selected, setSelected] = React.useState(() => new Set(required.filter(item => item.installable && !item.active).map(item => item.key)))
  const [query, setQuery] = React.useState('')
  const [trusted, setTrusted] = React.useState(false)
  const [trustedSource, setTrustedSource] = React.useState('')
  const all = [...required, ...optional]
  const picked = all.filter(item => selected.has(item.key) && item.installable && !item.active)
  const accounts = picked.filter(item => item.required).reduce((sum, item) => sum + item.accounts, 0)
  const scripts = picked.some(item => item.kind === 'script')
  const sourceKey = JSON.stringify(picked.filter(item => item.kind === 'script').map(item => [item.key, item.source, item.selector, item.version]))
  const sourcesTrusted = trusted && trustedSource === sourceKey
  const keyword = query.trim().toLocaleLowerCase()
  const optionalRows = optional.filter(item => !keyword || `${item.name} ${item.sourceLabel}`.toLocaleLowerCase().includes(keyword))
  const toggle = (key: string, checked: boolean) => { setTrusted(false); setSelected(previous => { const next = new Set(previous); checked ? next.add(key) : next.delete(key); return next }) }
  const rows = (items: PluginBatchItem[]) => <div className='plugins-repair-rows'>{items.map(item => <div key={item.key} className='plugins-repair-row'>
    <Checkbox className='size-4' checked={selected.has(item.key)} disabled={disabled || !item.installable || item.active}
      aria-label={t('选择插件 {name}', { name: item.name })} onCheckedChange={value => toggle(item.key, value === true)} />
    <PluginArtwork src={item.icon} />
    <div className='min-w-0 flex-1'><p className='m-0 text-sm'>{item.name}</p><p className='m-0 mt-1 text-xs text-muted-foreground'>{item.sourceLabel} · {item.version || t('暂未发布')}</p>
      {item.kind === 'script' && item.source ? <p className='m-0 mt-1 break-all text-xs text-muted-foreground'>{item.source}</p> : null}
      {item.required ? <p className='m-0 mt-2 text-xs text-warning'>{item.accounts ? t('{count} 个账号等待插件恢复', { count: item.accounts }) : t('插件文件缺失，请重新安装')}</p> : null}
      {item.reason ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{item.reason}</p> : null}
    </div>
    {item.size ? <span className='shrink-0 text-xs tabular-nums text-muted-foreground'>{(item.size / 1048576).toFixed(1)} MiB</span> : null}
    {item.active ? <Badge variant='outline'>{t('正在处理…')}</Badge> : !item.installable ? <Badge variant='outline'>{t('暂不可安装')}</Badge> : null}
  </div>)}</div>
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='w-[min(840px,calc(100vw-48px))]'>
      <DialogHeader><div><DialogTitle>{t('补全缺失的插件')}</DialogTitle><p className='m-0 mt-1 text-xs text-muted-foreground'>{t('安装并成功加载后，现有账号和原有计划会自动恢复。')}</p></div></DialogHeader>
      <DialogBody className='min-h-0 gap-5'>
        {required.length ? <section><h3 className='mb-3 text-sm font-medium'>{t('恢复现有账号')}</h3>{rows(required)}</section> : null}
        {unresolved.length ? <Alert variant='warning'><p>{t('以下提供商暂未找到对应的可安装插件：{names}', { names: unresolved.map(item => item.label).join(t('、')) })}</p></Alert> : null}
        {optional.length ? <section className='space-y-3'>
          <div className='flex flex-wrap items-center gap-3'><h3 className='text-sm font-medium'>{t('可选安装')}</h3><span className='text-xs text-muted-foreground'>{t('尚未使用的插件，默认不选')}</span>
            <Button variant='link' size='xs' className='ml-auto' disabled={disabled} onClick={() => { const keys = optionalRows.filter(item => item.installable && !item.active).map(item => item.key); const checked = !keys.every(key => selected.has(key)); setTrusted(false); setSelected(previous => { const next = new Set(previous); keys.forEach(key => checked ? next.add(key) : next.delete(key)); return next }) }}>{t('全选本组')}</Button>
          </div>
          <Input type='search' value={query} placeholder={t('搜索插件')} aria-label={t('搜索可选插件')} onChange={event => setQuery(event.currentTarget.value)} />
          <div className='max-h-[280px] overflow-y-auto'>{rows(optionalRows)}</div>
        </section> : null}
        {!all.length && !unresolved.length ? <p className='text-sm text-muted-foreground'>{t('没有缺失的插件')}</p> : null}
        {scripts ? <>
          <Alert variant='warning'>{t('脚本不是安全沙箱，可以访问网关所在电脑的文件和网络。不要安装来历不明的插件。')}</Alert>
          <p className='text-xs text-subtle'>{t('Magpie 官方和社区插件均通过兼容方式接入 Agent2API，不保证 100% 可用。')}</p>
          <p className='text-xs text-subtle'>{t('签到及附带任务默认允许，实际执行遵循签到中心的开关和计划。')}</p>
          <p className='text-xs text-muted-foreground'>{t('部分插件的附带任务可能消耗额度、发送对话或创建及解散群聊。')}</p>
          <Label className='flex items-start gap-3 text-xs font-normal leading-5'><Checkbox checked={sourcesTrusted} onCheckedChange={value => { setTrusted(value === true); setTrustedSource(sourceKey) }} />{t('我信任本次所选脚本插件的来源，并允许它们在网关电脑上运行。')}</Label>
        </> : null}
      </DialogBody>
      <DialogFooter className='flex-wrap'><div className='mr-auto text-xs text-muted-foreground'><p>{t('已选 {count} 个插件', { count: picked.length })}</p>{accounts ? <p className='mt-1'>{t('{count} 个账号等待恢复', { count: accounts })}</p> : null}</div>
        <Button variant='outline' onClick={onClose}>{t('稍后')}</Button>
        <Button disabled={disabled || !picked.length || scripts && !sourcesTrusted} onClick={() => onInstall(picked)}>{t('补全所选（{count}）', { count: picked.length })}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
