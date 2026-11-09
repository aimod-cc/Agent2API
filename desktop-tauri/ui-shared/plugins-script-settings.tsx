import * as React from 'react'
import { Badge, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter,
  DialogHeader, DialogSection, DialogTitle, Input, Label, Switch } from '@ui'
import type { ScriptPackage, ScriptRecommendation } from './plugins'
import { PluginArtwork, type PluginTranslate } from './plugins-store'
import { scriptPluginDescription, scriptPluginName, scriptSourceLabel } from './plugins-script-display'
import { PluginStartupDetails, requestPluginDiagnostics, usePluginLoadStatus } from './plugin-startup-view'
import { pluginFailed, pluginPhaseLabel } from './plugin-startup'
import { ScriptPluginOptions } from './plugins-script-options'
import { ScriptRepository, type PluginOpenExternal } from './plugins-script-catalog'

export function ScriptPluginSettings({ pack, recommended, disabled, error, onClose, onEnabled, onProvider,
  onReinstall, onRemove, icon, t, request, refresh, onOpenExternal }: {
  pack: ScriptPackage
  icon?: string | null
  recommended: ScriptRecommendation[]
  disabled: boolean
  error?: string
  onClose: () => void
  onEnabled: (enabled: boolean) => void
  onProvider: (id: string, enabled: boolean) => void
  onReinstall: () => void
  onRemove: () => void
  t: PluginTranslate
  request: (path: string, body: Record<string, unknown>) => Promise<unknown>
  refresh: () => Promise<void>
  onOpenExternal?: PluginOpenExternal
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const [search, setSearch] = React.useState('')
  const prefix = React.useId()
  const keyword = search.trim().toLocaleLowerCase()
  const providers = pack.providers.filter(provider => !keyword
    || `${t(provider.label)} ${provider.id}`.toLocaleLowerCase().includes(keyword))
  const name = scriptPluginName(pack, recommended, t)
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='w-[min(560px,calc(100vw-48px))] text-foreground'>
      <DialogHeader className='gap-3'>
        <PluginArtwork src={icon} />
        <div className='min-w-0 flex-1'>
          <DialogTitle className='m-0 break-words'>{name}</DialogTitle>
          <div className='mt-1 text-[11.5px] text-muted-foreground'>v{pack.version} · {scriptSourceLabel(pack, t)}</div>
        </div>
      </DialogHeader>
      <DialogBody className='min-h-0 gap-4'>
        <DialogDescription className='text-[12.5px] leading-relaxed text-subtle'>
          {scriptPluginDescription(pack, recommended, t)}
        </DialogDescription>
        <ScriptRepository item={pack} onOpenExternal={onOpenExternal} t={t} />
        <DialogSection>
          <Label htmlFor={`${prefix}-enabled`} className='justify-between text-[12.5px] text-foreground'>
            {t('启用')}<Switch id={`${prefix}-enabled`} checked={pack.enabled} disabled={disabled} onCheckedChange={onEnabled} />
          </Label>
          <p className='m-0'>{t('更新或启停会等待共享宿主内的请求结束；账号数据始终由网关统一保存。')}</p>
        </DialogSection>
        <DialogSection>
          <h3 className='m-0'>{t('加载状态')}</h3>
          <PluginStartupDetails id={pack.id} status={startup} t={t} />
          <Button variant='link' size='sm' onClick={() => { onClose(); requestPluginDiagnostics(pack.id) }}>{t('加载诊断')}</Button>
        </DialogSection>
        <DialogSection>
          <h3 className='m-0'>{t('版本')}</h3>
          <p className='m-0 break-all font-mono'>{pack.name}</p>
          <div className='flex flex-wrap items-center justify-between gap-3'>
            <div className='text-[11.5px] text-muted-foreground'>
              <span className='tabular-nums'>v{pack.version}</span>
              {pack.installedAt ? <span> · {t('安装于 {time}', { time: new Date(pack.installedAt).toLocaleString() })}</span> : null}
            </div>
            <Button variant='outline' size='sm' disabled={disabled} onClick={onReinstall}>{t('更新／重装')}</Button>
          </div>
          {pack.source?.kind === 'github' ? <p className='m-0 break-all'>
            GitHub · {pack.source.repository} · {pack.source.ref || t('默认分支')} · <span className='font-mono'>{pack.source.commit.slice(0, 12)}</span>
          </p> : null}
          {pack.source?.kind === 'magpie' ? <p className='m-0 break-all'>{t('Magpie 社区分发源')} · SHA-512</p> : null}
        </DialogSection>
        <DialogSection>
          <h3 className='m-0 flex items-baseline justify-between gap-3'>
            <span>{t('提供商启停')}</span>
            <span className='text-[11.5px] font-normal text-muted-foreground'>
              {pack.providers.filter(provider => provider.enabled).length}/{pack.providers.length} {t('已启用')}
            </span>
          </h3>
          <p className='m-0'>{t('这个包提供的提供商（可单独停用某一家）')}</p>
          {pack.providers.length > 6 ? <Input type='search' value={search} placeholder={t('搜索提供商')} aria-label={t('搜索提供商')}
            onChange={event => setSearch(event.currentTarget.value)} /> : null}
          <div className='max-h-[32vh] overflow-y-auto overscroll-contain'>
            {providers.map(provider => <div key={provider.id} className='flex items-center justify-between gap-3 border-t border-hairline py-2 first:border-t-0'>
              <Label htmlFor={`${prefix}-${provider.id}`} className='min-w-0 flex-1 flex-wrap text-[12.5px] text-foreground'>
                <span className='break-words'>{t(provider.label)}</span>
                {provider.enabled && !provider.loaded ? <Badge variant='secondary' shape='tag'>{pluginPhaseLabel(startup, t)}</Badge> : null}
              </Label>
              <Switch id={`${prefix}-${provider.id}`} checked={provider.enabled} disabled={disabled || !pack.enabled}
                onCheckedChange={enabled => onProvider(provider.id, enabled)} />
            </div>)}
            {!providers.length ? <p className='m-0 py-3 text-center'>{t('没有匹配「{keyword}」的提供商', { keyword: search.trim() })}</p> : null}
          </div>
        </DialogSection>
        <ScriptPluginOptions pack={pack} disabled={disabled} request={request} refresh={refresh} t={t} />
        {error || (startup ? pluginFailed(startup) && startup.error : pack.error) ? <p role='alert' className='m-0 break-words text-[12px] text-destructive'>{error || startup?.error || pack.error}</p> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='destructive' size='sm' className='mr-auto' disabled={disabled} onClick={onRemove}>{t('卸载插件')}</Button>
        <Button variant='outline' size='sm' onClick={onClose}>{t('完成')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
