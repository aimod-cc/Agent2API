import { useId } from 'react'
import { Alert, Badge, DialogSection, Label, Switch } from '@ui'
import type { PackSummary } from '../../api/plugins'
import { t } from '../../i18n'
import { PluginDetailsDialog, type PluginDetailsPane } from '../../../../ui-shared/plugins-details'
import { requestPluginDiagnostics, usePluginLoadStatus } from '../../../../ui-shared/plugin-startup-view'
import { PluginDiagnosticsMenu, PluginStartupSummary } from '../../../../ui-shared/plugins-startup-summary'
import { pluginPhaseLabel } from '../../../../ui-shared/plugin-startup'

export function PluginPackSettingsSheet({ pack, disabled, initialPane, onClose, onEnabled, onProvider, onReload }: {
  pack: PackSummary
  disabled: boolean
  initialPane?: PluginDetailsPane
  onClose: () => void
  onEnabled: (enabled: boolean) => void
  onProvider: (id: string, enabled: boolean) => void
  onReload: () => Promise<unknown>
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const prefix = useId()
  const locked = disabled
  const close = () => { if (!locked) onClose() }
  const diagnostics = () => { if (!locked) { onClose(); requestPluginDiagnostics(pack.id) } }

  return <PluginDetailsDialog name={t(pack.name || pack.id)} description={t(pack.description || '')}
    version={pack.version} sourceLabel={t('社区插件')} icon={pack.icon} artwork='providers'
    initialPane={initialPane ?? 'settings'} closeDisabled={locked} onClose={close} t={t}
    more={<PluginDiagnosticsMenu disabled={locked} onDiagnostics={diagnostics} t={t} />}
    settings={<>
      <DialogSection>
        <Label htmlFor={`${prefix}-enabled`} className='justify-between text-foreground'>
          {t('启用')}<Switch id={`${prefix}-enabled`} checked={pack.supportState !== 'legacy_unsupported' && pack.enabled} disabled={locked || Boolean(pack.unavailableReason)} onCheckedChange={onEnabled} />
        </Label>
        {pack.unavailableReason ? <Alert>{t(pack.unavailableReason)}</Alert> : null}
        <PluginStartupSummary id={pack.id} status={startup} enabled={pack.enabled}
          running={pack.host?.running && pack.entryPresent && !pack.updateRequired} error={pack.host?.lastError}
          disabled={locked} diagnosticsDisabled={locked} onDiagnostics={diagnostics} retry={onReload} t={t} />
        <p className='m-0'>v{pack.version} · {t('社区插件')} · {pack.signed ? t('已签名') : t('未签名')}</p>
      </DialogSection>
      <DialogSection>
        <h3 className='m-0'>{t('提供商启停')}</h3>
        {pack.providers.map(provider => <div key={provider.id} className='flex items-center justify-between gap-3 border-t border-hairline py-2 first:border-t-0'>
          <Label htmlFor={`${prefix}-${provider.id}`} className='min-w-0 flex-1 flex-wrap text-foreground'>
            <span>{t(provider.label)}</span>
            {provider.overriddenBy ? <Badge variant='secondary' shape='tag'
              title={t('已由其他插件接管，请到对应插件中管理。')}>{t('由其他插件接管')}</Badge>
              : provider.enabled && !provider.loaded ? <Badge variant='secondary' shape='tag'>{pluginPhaseLabel(startup, t)}</Badge> : null}
          </Label>
          <Switch id={`${prefix}-${provider.id}`} checked={pack.supportState !== 'legacy_unsupported' && provider.enabled && !provider.overriddenBy}
            disabled={locked || !pack.enabled || Boolean(pack.unavailableReason) || Boolean(provider.overriddenBy)}
            onCheckedChange={enabled => onProvider(provider.id, enabled)} />
        </div>)}
      </DialogSection>
    </>} />
}
