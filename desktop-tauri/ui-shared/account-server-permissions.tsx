import * as React from 'react'
import { Alert, Button, Spinner } from '@ui'
import { PluginAccountOriginPermissions } from './plugin-account-origin-permissions'
import type { PackSummary, PluginsPayload } from './plugins'
import type { PermissionRequest } from './plugin-permission-reads'
import type { PluginTranslate } from './plugins-store'

export function AccountServerPermissions({ provider, scopeIdentity, disabled, onBusyChange, onChanged, request, t }: {
  provider: string
  scopeIdentity: string
  disabled?: boolean
  onBusyChange?: (busy: boolean) => void
  onChanged?: () => void
  request: PermissionRequest
  t: PluginTranslate
}) {
  const scope = JSON.stringify([scopeIdentity, provider])
  const [view, setView] = React.useState<{ scope: string; pack?: PackSummary; error?: boolean } | null>(null)
  const [revision, setRevision] = React.useState(0)
  React.useEffect(() => {
    let current = true
    setView(null)
    void request<PluginsPayload>('GET', '/api/plugins').then(payload => {
      if (!current) return
      const pack = payload.packs.find(pack => pack.providers.some(item => item.id === provider && !item.overriddenBy))
      setView({ scope, pack, error: !pack })
    }).catch(() => { if (current) setView({ scope, error: true }) })
    return () => { current = false }
  }, [scope, revision, request])
  if (!view || view.scope !== scope) return <p className='flex items-center gap-2 text-xs text-muted-foreground'>
    <Spinner className='size-3' />{t('正在读取服务器访问权限…')}
  </p>
  if (view.error || !view.pack) return <Alert>
    <p className='m-0'>{t('服务器访问权限暂不可用，请确认对应插件已安装并启用。')}</p>
    <Button type='button' variant='link' size='sm' disabled={disabled} onClick={() => setRevision(value => value + 1)}>{t('重试')}</Button>
  </Alert>
  return <div className='flex flex-col gap-3'>
    <p className='m-0 text-xs text-muted-foreground'>{t('确认后立即生效，不需要再点底部的保存按钮。该权限作用于当前插件，更新插件后需要重新确认。')}</p>
    <PluginAccountOriginPermissions key={`${scope}:${view.pack.id}:${view.pack.version}`} packId={view.pack.id}
      providers={view.pack.providers} enabled={view.pack.enabled} disabled={disabled} onBusyChange={onBusyChange}
      onChanged={onChanged} request={request} t={t} />
  </div>
}
