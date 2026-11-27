import * as React from 'react'
import { Alert, Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@ui'
import { PluginArtwork } from './plugin-artwork'
import type { HiddenAccountProvider } from './plugins'
import type { PluginAliasBinding, PluginBatchItem } from './plugins-batch-items'
import type { PluginTranslate } from './plugins-store'
import { ScriptTrustConfirmDialog } from './plugins-task-dialog'

export function PluginMissingBanner({ count, accounts, onOpen, t }: { count: number; accounts: number; onOpen(): void; t: PluginTranslate }) {
  return <div className='plugins-missing-banner'>
    <svg viewBox='0 0 24 24' className='size-5 shrink-0 text-warning' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'><path d='M9 3v5M15 3v5M6.5 8h11v3.5a5.5 5.5 0 0 1-11 0V8ZM12 17v4' /></svg>
    <p className='m-0 min-w-0 flex-1 text-xs'>{accounts ? t('{count} 个插件缺失，{accounts} 个账号暂不可用', { count, accounts }) : t('{count} 个插件文件缺失', { count })}</p>
    <Button size='sm' onClick={onOpen}>{t('补全缺失插件')}</Button>
  </div>
}

function PluginMissingRows({ items, selected, disabled, onToggle, t }: {
  items: PluginBatchItem[]; selected: Set<string>; disabled: boolean; onToggle(key: string, checked: boolean): void; t: PluginTranslate
}) {
  return <div className='plugins-repair-rows'>{items.map(item => <div key={item.key} className='plugins-repair-row'>
    <Checkbox className='size-4' checked={selected.has(item.key)} disabled={disabled || !item.installable || item.active}
      aria-label={t('选择插件 {name}', { name: item.name })} onCheckedChange={value => onToggle(item.key, value === true)} />
    <PluginArtwork src={item.icon} />
    <div className='min-w-0 flex-1'><p className='m-0 text-sm'>{item.name}</p><p className='m-0 mt-1 text-xs text-muted-foreground'>{item.sourceLabel} · {item.version || t('暂未发布')}</p>
      {item.kind === 'script' && item.source ? <p className='m-0 mt-1 break-all text-xs text-muted-foreground'>{item.source}</p> : null}
      {item.required ? <p className='m-0 mt-2 text-xs text-warning'>{item.accounts ? t('{count} 个账号等待插件恢复', { count: item.accounts }) : t('插件文件缺失，请重新安装')}</p> : null}
      {item.reason ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{item.reason}</p> : null}
    </div>
    {item.size ? <span className='shrink-0 text-xs tabular-nums text-muted-foreground'>{(item.size / 1048576).toFixed(1)} MiB</span> : null}
    {item.active ? <Badge variant='outline'>{t('正在处理…')}</Badge> : !item.installable ? <Badge variant='outline'>{t('暂不可安装')}</Badge> : null}
  </div>)}</div>
}

export function PluginMissingDialog({ required, bindable = [], unresolved, disabled, onClose, onInstall, onBindAlias, t }: {
  required: PluginBatchItem[]; bindable?: PluginAliasBinding[]; unresolved: HiddenAccountProvider[]; disabled: boolean
  onClose(): void; onInstall(items: PluginBatchItem[]): void
  onBindAlias?(binding: { packageId: string; original: string; alias: string }): Promise<void>
  t: PluginTranslate
}) {
  const [selected, setSelected] = React.useState(() => new Set(required.filter(item => item.installable && !item.active).map(item => item.key)))
  const [bindingKey, setBindingKey] = React.useState('')
  const [bindError, setBindError] = React.useState('')
  const [confirmOpen, setConfirmOpen] = React.useState(false)
  const knownKeys = React.useRef(new Set(required.map(item => item.key)))
  React.useEffect(() => {
    const valid = new Set(required.map(item => item.key))
    const newlyInstallable = required
      .filter(item => !knownKeys.current.has(item.key) && item.installable && !item.active)
      .map(item => item.key)
    knownKeys.current = valid
    setSelected(previous => {
      const next = new Set([...previous].filter(key => valid.has(key)))
      newlyInstallable.forEach(key => next.add(key))
      return next.size === previous.size && [...next].every(key => previous.has(key)) ? previous : next
    })
  }, [required])
  const picked = required.filter(item => selected.has(item.key) && item.installable && !item.active)
  const accounts = picked.reduce((sum, item) => sum + item.accounts, 0)
  const hasScripts = picked.some(item => item.kind === 'script')
  React.useEffect(() => {
    if (!hasScripts || disabled || !picked.length) setConfirmOpen(false)
  }, [hasScripts, disabled, picked.length])
  const toggle = (key: string, checked: boolean) => {
    setSelected(previous => {
      const next = new Set(previous)
      checked ? next.add(key) : next.delete(key)
      return next
    })
  }
  const handleBind = async (packageId: string, original: string, alias: string) => {
    if (!onBindAlias || bindingKey) return
    const key = `${packageId}:${alias}`
    setBindingKey(key)
    setBindError('')
    try { await onBindAlias({ packageId, original, alias }) }
    catch (cause) { setBindError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBindingKey('') }
  }
  const unresolvedAlert = unresolved.length
    ? <Alert variant='warning' className='shrink-0'><p>{t('以下提供商暂未找到对应的可安装插件：{names}', { names: unresolved.map(item => item.label).join(t('、')) })}</p></Alert>
    : null

  return <Dialog open onOpenChange={(open, details) => {
    if (!open) {
      if (bindingKey || confirmOpen) details.cancel()
      else onClose()
    }
  }}>
    <DialogContent className='w-[min(840px,calc(100vw-48px))] max-sm:w-[calc(100vw-24px)]'>
      <DialogHeader className='shrink-0'><div>
        <DialogTitle>{t('补全缺失的插件')}</DialogTitle>
        <p className='m-0 mt-1 text-xs text-muted-foreground'>{t('安装并成功加载后，现有账号和原有计划会自动恢复。')}</p>
      </div></DialogHeader>
      <DialogBody className='min-h-0 gap-4'>
        {unresolvedAlert}
        {bindError ? <Alert variant='destructive' className='shrink-0'>{bindError}</Alert> : null}
        {bindable.length ? <div className='space-y-2.5'>
          <p className='m-0 text-xs text-muted-foreground'>{t('以下同步账号的提供商标识与本机已安装插件不一致，确认属于同一插件后可直接关联恢复：')}</p>
          <div className='plugins-repair-rows'>{bindable.map(entry => entry.candidates.map(candidate => {
            const key = `${candidate.packageId}:${entry.provider.id}`
            const busy = bindingKey === key
            return <div key={key} className='plugins-repair-row'>
              <PluginArtwork src={candidate.icon} />
              <div className='min-w-0 flex-1'>
                <p className='m-0 text-sm'>{candidate.name}</p>
                <p className='m-0 mt-1 text-xs text-muted-foreground'>{candidate.sourceLabel} · {entry.provider.label}</p>
                <p className='m-0 mt-1.5 text-xs text-warning'>{t('{count} 个账号等待插件恢复', { count: entry.provider.count })}</p>
              </div>
              {onBindAlias ? <Button size='sm' variant='secondary' disabled={disabled || Boolean(bindingKey) || candidate.active}
                onClick={() => void handleBind(candidate.packageId, candidate.original, entry.provider.id)}>
                {busy ? t('正在关联…') : t('关联到此插件')}
              </Button> : null}
            </div>
          }))}</div>
        </div> : null}
        {required.length ? <PluginMissingRows items={required} selected={selected} disabled={disabled || Boolean(bindingKey)} onToggle={toggle} t={t} /> : null}
        {!required.length && !bindable.length && !unresolved.length ? <p className='text-sm text-muted-foreground'>{t('没有缺失的插件')}</p> : null}
      </DialogBody>
      <DialogFooter className='shrink-0 flex-wrap'>
        <div className='mr-auto text-xs text-muted-foreground'>
          <p>{t('已选 {count} 个插件', { count: picked.length })}</p>
          {accounts ? <p className='mt-1'>{t('{count} 个账号等待恢复', { count: accounts })}</p> : null}
        </div>
        <Button variant='outline' disabled={Boolean(bindingKey)} onClick={onClose}>{t('稍后')}</Button>
        <Button disabled={disabled || Boolean(bindingKey) || !picked.length} onClick={() => {
          if (hasScripts) setConfirmOpen(true)
          else onInstall(picked)
        }}>{t('补全所选（{count}）', { count: picked.length })}</Button>
      </DialogFooter>
      <ScriptTrustConfirmDialog
        open={confirmOpen}
        action='install'
        onCancel={() => setConfirmOpen(false)}
        onConfirm={() => { setConfirmOpen(false); onInstall(picked) }}
        t={t}
      />
    </DialogContent>
  </Dialog>
}
