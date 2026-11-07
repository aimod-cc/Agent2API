import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter,
  DialogHeader, DialogSection, DialogTitle, Input, Label, Switch,
} from '@ui'
import { formatTime, pluginDescription, pluginName } from './plugins-shared'
import type { PackProvider, PackSummary, PluginOperation, PluginUpdate, StorePlugin } from './plugins-types'
import { PluginArtwork } from '../../../ui-shared/plugins-store'
import { PluginUpdateDetails } from './plugins-cards'
import { t } from '../i18n'

export function PluginPackSettingsDialog({ pack, store, update, operation, disabled, onClose, onEnabled, onProvider,
  onRemove, onUpdate, onNotes }: {
  pack: PackSummary
  store?: StorePlugin
  update?: PluginUpdate
  operation?: PluginOperation
  disabled: boolean
  onClose: () => void
  onEnabled: (enabled: boolean) => void
  onProvider: (provider: PackProvider, enabled: boolean) => void
  onRemove: () => void
  onUpdate: () => void
  onNotes: () => void
}) {
  const [search, setSearch] = React.useState('')
  const idPrefix = React.useId()
  const keyword = search.trim().toLocaleLowerCase()
  const providers = pack.providers.filter(provider => !keyword
    || `${t(provider.label)} ${provider.id}`.toLocaleLowerCase().includes(keyword))

  return <Dialog open onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='w-[min(560px,calc(100vw-48px))] text-foreground'>
      <DialogHeader className='gap-3'>
        <PluginArtwork src={pack.icon || store?.icon} kind={pack.id === 'agent2api-web-providers' ? 'web' : 'providers'} />
        <div className='min-w-0 flex-1'>
          <DialogTitle className='m-0 break-words'>{pluginName(pack.id, store?.name || pack.name)}</DialogTitle>
          <div className='mt-1 text-[11.5px] text-muted-foreground'>v{pack.version} · {store?.official ? t('官方') : t('社区插件')}</div>
        </div>
      </DialogHeader>
      <DialogBody className='min-h-0 gap-4'>
        <DialogDescription className='text-[12.5px] leading-relaxed text-subtle'>
          {pluginDescription(pack.id, store?.description || pack.description)}
        </DialogDescription>
        <DialogSection>
          <Label htmlFor={`${idPrefix}-enabled`} className='justify-between text-[12.5px] text-foreground'>
            {t('启用')}<Switch id={`${idPrefix}-enabled`} checked={pack.enabled} disabled={disabled} onCheckedChange={onEnabled} />
          </Label>
          <div className='flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-muted-foreground'>
            <span className={pack.signed ? undefined : 'text-warning'}>{pack.signed ? t('已签名') : t('未签名')}</span>
            <span aria-hidden='true'>·</span><span>{t('协议版本')} {pack.protocol}</span>
            {pack.installedAt ? <><span aria-hidden='true'>·</span>
              <span>{t('安装于 {time}', { time: formatTime(pack.installedAt) })}</span></> : null}
          </div>
        </DialogSection>
        <DialogSection>
          <h3 className='m-0'>{t('版本')}</h3>
          <PluginUpdateDetails pack={pack} update={update} operation={operation} disabled={disabled} onUpdate={onUpdate} onNotes={onNotes} />
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
          <div className='max-h-[36vh] min-h-0 overflow-y-auto overscroll-contain'>
            {providers.map(provider => <div key={provider.id} className='flex items-center justify-between gap-3 border-t border-hairline py-2 first:border-t-0'>
              <Label htmlFor={`${idPrefix}-${provider.id}`} className='min-w-0 flex-1 flex-wrap text-[12.5px] text-foreground'>
                <span className='break-words'>{t(provider.label)}</span>
                {provider.enabled && !provider.loaded ? <Badge variant='secondary' shape='tag'>{t('未加载')}</Badge> : null}
              </Label>
              <Switch id={`${idPrefix}-${provider.id}`} checked={provider.enabled} disabled={disabled || !pack.enabled}
                onCheckedChange={value => onProvider(provider, value)} />
            </div>)}
            {!providers.length ? <p className='m-0 py-3 text-center'>{t('没有匹配「{keyword}」的提供商', { keyword: search.trim() })}</p> : null}
          </div>
        </DialogSection>
      </DialogBody>
      <DialogFooter>
        <Button variant='destructive' size='sm' className='mr-auto' disabled={disabled} onClick={onRemove}>{t('卸载插件')}</Button>
        <Button variant='outline' size='sm' onClick={onClose}>{t('完成')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
