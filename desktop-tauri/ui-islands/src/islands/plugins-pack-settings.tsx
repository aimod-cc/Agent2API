import * as React from 'react'
import { Alert, Badge, Button, DialogSection, Input, Label, Switch } from '@ui'
import { api, formatTime, pluginDescription, pluginName } from './plugins-shared'
import type { PackProvider, PackSummary, PluginOperation, PluginUpdate, StorePlugin } from './plugins-types'
import { PluginDetailsDialog, type PluginDetailsPane } from '../../../ui-shared/plugins-details'
import { PluginHelpers } from '../../../ui-shared/plugin-helpers'
import { requestPluginDiagnostics, usePluginLoadStatus } from '../../../ui-shared/plugin-startup-view'
import { PluginDiagnosticsMenu, PluginStartupSummary } from '../../../ui-shared/plugins-startup-summary'
import { pluginPhaseLabel } from '../../../ui-shared/plugin-startup'
import { PluginUpdateDetails } from './plugins-cards'
import { t } from '../i18n'

export function PluginPackSettingsDialog({ pack, store, update, operation, disabled, initialPane, onClose, onEnabled, onProvider,
  onRemove, onUpdate, onNotes, onReload }: {
  pack: PackSummary
  store?: StorePlugin
  update?: PluginUpdate
  operation?: PluginOperation
  disabled: boolean
  initialPane?: PluginDetailsPane
  onClose: () => void
  onEnabled: (enabled: boolean) => void
  onProvider: (provider: PackProvider, enabled: boolean) => void
  onRemove: () => void
  onUpdate: () => void
  onNotes: () => void
  onReload: () => Promise<unknown>
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const [search, setSearch] = React.useState('')
  const [helperBusy, setHelperBusy] = React.useState(false)
  const locked = disabled || helperBusy
  const idPrefix = React.useId()
  const keyword = search.trim().toLocaleLowerCase()
  const providers = pack.providers.filter(provider => !keyword
    || `${t(provider.label)} ${provider.id}`.toLocaleLowerCase().includes(keyword))
  const close = () => { if (!locked) onClose() }
  const diagnostics = () => { if (!locked) { onClose(); requestPluginDiagnostics(pack.id) } }

  return <PluginDetailsDialog name={pluginName(pack.id, store?.name || pack.name)} version={pack.version} sourceLabel={t('社区插件')}
    description={pluginDescription(pack.id, store?.description || pack.description)} icon={pack.icon || store?.icon}
    artwork={pack.id === 'agent2api-web-providers' ? 'web' : 'providers'} initialPane={initialPane ?? 'settings'}
    closeDisabled={locked} onClose={close} t={t}
    more={<PluginDiagnosticsMenu disabled={locked} onDiagnostics={diagnostics} t={t} />}
    settingsActions={<Button variant='destructive' size='sm' disabled={locked} onClick={onRemove}>{t('卸载插件')}</Button>}
    settings={<>
      <DialogSection>
        <Label htmlFor={`${idPrefix}-enabled`} className='justify-between text-[12.5px] text-foreground'>
          {t('启用')}<Switch id={`${idPrefix}-enabled`} checked={pack.supportState !== 'legacy_unsupported' && pack.enabled} disabled={locked || Boolean(pack.unavailableReason)} onCheckedChange={onEnabled} />
        </Label>
        {pack.unavailableReason ? <Alert>{t(pack.unavailableReason)}</Alert> : null}
        <PluginStartupSummary id={pack.id} status={startup} enabled={pack.enabled}
          running={pack.host?.running && pack.entryPresent && !pack.updateRequired} error={pack.host?.lastError}
          disabled={locked} diagnosticsDisabled={locked} onDiagnostics={diagnostics} retry={onReload} t={t} />
        <div className='flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-muted-foreground'>
          <span className={pack.signed ? undefined : 'text-warning'}>{pack.signed ? t('已签名') : t('未签名')}</span>
          <span aria-hidden='true'>·</span><span>{t('协议版本')} {pack.protocol}</span>
          {pack.installedAt ? <><span aria-hidden='true'>·</span>
            <span>{t('安装于 {time}', { time: formatTime(pack.installedAt) })}</span></> : null}
        </div>
      </DialogSection>
      <DialogSection>
        <h3 className='m-0'>{t('版本')}</h3>
        <PluginUpdateDetails pack={pack} update={update} operation={operation} disabled={locked} onUpdate={onUpdate} onNotes={onNotes} />
      </DialogSection>
      <DialogSection>
        <h3 className='m-0 flex items-baseline justify-between gap-3'>
          <span>{t('提供商启停')}</span>
          <span className='text-[11.5px] font-normal text-muted-foreground'>
            {pack.supportState === 'legacy_unsupported' ? 0 : pack.providers.filter(provider => provider.enabled && !provider.overriddenBy).length}/{pack.providers.length} {t('已启用')}
          </span>
        </h3>
        <p className='m-0'>{t('这个包提供的提供商（可单独停用某一家）')}</p>
        {pack.providers.length > 6 ? <Input type='search' value={search} placeholder={t('搜索提供商')} aria-label={t('搜索提供商')}
          onChange={event => setSearch(event.currentTarget.value)} /> : null}
        <div className='max-h-[36vh] min-h-0 overflow-y-auto overscroll-contain'>
          {providers.map(provider => <div key={provider.id} className='flex items-center justify-between gap-3 border-t border-hairline py-2 first:border-t-0'>
            <Label htmlFor={`${idPrefix}-${provider.id}`} className='min-w-0 flex-1 flex-wrap text-[12.5px] text-foreground'>
              <span className='break-words'>{t(provider.label)}</span>
              {provider.overriddenBy ? <Badge variant='secondary' shape='tag'
                title={t('已由其他插件接管，请到对应插件中管理。')}>{t('由其他插件接管')}</Badge>
                : provider.enabled && !provider.loaded ? <Badge variant='secondary' shape='tag'>{pluginPhaseLabel(startup, t)}</Badge> : null}
            </Label>
            <Switch id={`${idPrefix}-${provider.id}`} checked={pack.supportState !== 'legacy_unsupported' && provider.enabled && !provider.overriddenBy}
              disabled={locked || !pack.enabled || Boolean(pack.unavailableReason) || Boolean(provider.overriddenBy)}
              onCheckedChange={value => onProvider(provider, value)} />
          </div>)}
          {!providers.length ? <p className='m-0 py-3 text-center'>{t('没有匹配「{keyword}」的提供商', { keyword: search.trim() })}</p> : null}
        </div>
      </DialogSection>
      <PluginHelpers key={`helpers:${pack.id}:${pack.version}`} packId={pack.id} providers={pack.providers} enabled={pack.enabled}
        disabled={disabled} onBusyChange={setHelperBusy} request={api} t={t} />
    </>} />
}
