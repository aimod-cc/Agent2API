import { useId, useState } from 'react'
import { Badge, Button, DialogSection, Label, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, Switch } from '@ui'
import type { PackSummary } from '../../api/plugins'
import { apiCall } from '../../api/client'
import { serverIdentity } from '../../lib/servers'
import { t } from '../../i18n'
import { PluginHelpers } from '../../../../ui-shared/plugin-helpers'
import { PluginStartupDetails, requestPluginDiagnostics, usePluginLoadStatus } from '../../../../ui-shared/plugin-startup-view'
import { pluginPhaseLabel } from '../../../../ui-shared/plugin-startup'

export function PluginPackSettingsSheet({ pack, disabled, onClose, onEnabled, onProvider }: {
  pack: PackSummary
  disabled: boolean
  onClose: () => void
  onEnabled: (enabled: boolean) => void
  onProvider: (id: string, enabled: boolean) => void
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const [permissionBusy, setPermissionBusy] = useState(false)
  const prefix = useId()
  const locked = disabled || permissionBusy
  const identity = serverIdentity()
  return <Sheet open onOpenChange={(open, details) => {
    if (open) return
    if (permissionBusy) { details.cancel(); return }
    onClose()
  }}>
    <SheetContent showCloseButton={!permissionBusy}>
      <SheetHeader><SheetTitle>{t(pack.name || pack.id)}</SheetTitle></SheetHeader>
      <SheetBody className='gap-4'>
        {pack.description ? <p className='m-0 text-sm text-muted-foreground'>{t(pack.description)}</p> : null}
        <DialogSection>
          <Label htmlFor={`${prefix}-enabled`} className='justify-between text-foreground'>
            {t('启用')}<Switch id={`${prefix}-enabled`} checked={pack.enabled} disabled={locked} onCheckedChange={onEnabled} />
          </Label>
          <p className='m-0'>v{pack.version} · {t('社区插件')} · {pack.signed ? t('已签名') : t('未签名')}</p>
        </DialogSection>
        <DialogSection>
          <h3 className='m-0'>{t('加载状态')}</h3>
          <PluginStartupDetails id={pack.id} status={startup} t={t} />
          <Button variant='link' size='sm' onClick={() => { onClose(); requestPluginDiagnostics(pack.id) }}>{t('加载诊断')}</Button>
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
            <Switch id={`${prefix}-${provider.id}`} checked={provider.enabled && !provider.overriddenBy}
              disabled={locked || !pack.enabled || Boolean(provider.overriddenBy)}
              onCheckedChange={enabled => onProvider(provider.id, enabled)} />
          </div>)}
        </DialogSection>
        <PluginHelpers key={`${identity}:${pack.id}:${pack.version}`} packId={pack.id} providers={pack.providers}
          enabled={pack.enabled} disabled={disabled} onBusyChange={setPermissionBusy} t={t}
          request={(method, path, body) => apiCall(path, { method, body })} />
      </SheetBody>
      <SheetFooter><Button variant='outline' disabled={permissionBusy} onClick={onClose}>{t('完成')}</Button></SheetFooter>
    </SheetContent>
  </Sheet>
}
