import * as React from 'react'
import { Button } from '@ui'
import { launchClients, type LaunchRequest } from '../../../ui-shared/plugin-desktop-launch'
import { PluginDesktopLaunchDialog } from '../../../ui-shared/plugin-desktop-launch-dialog'
import { t } from '../i18n'
import { shared, type AccountRecord } from './accounts-shared'
import { providerOf, displayNameOf } from './accounts-domain'
import { api } from './plugins-shared'

type NativeWindow = {
  __TAURI_INTERNALS__?: { invoke?: (command: string, args?: unknown) => Promise<unknown> }
  workbuddyDesktop?: { platform?: string; pickDesktopClient?: (path?: string) => Promise<{ canceled: boolean; path?: string }> }
}

let activeLaunch: string | null = null
const listeners = new Set<() => void>()
const publish = () => listeners.forEach(listener => listener())

export function DesktopLaunchButton({ account }: { account: AccountRecord }) {
  const [open, setOpen] = React.useState(false)
  const [, redraw] = React.useReducer(value => value + 1, 0)
  React.useEffect(() => {
    listeners.add(redraw)
    return () => { listeners.delete(redraw); if (activeLaunch === account.id) { activeLaunch = null; publish() } }
  }, [account.id])
  const native = window as unknown as NativeWindow
  const provider = providerOf(account)
  const clients = launchClients(shared().wbApp?.getState?.()?.desktopLaunch, provider)
  if (native.workbuddyDesktop?.platform !== 'windows' || !native.__TAURI_INTERNALS__?.invoke || !clients.length) return null
  const unavailable = account.desktopLaunchAvailable === false || account.needsRelogin === true
  const request: LaunchRequest = (method, path, body, options) => api(method, path, body, options)
  return <>
    <Button variant='outline' size='xs' disabled={unavailable || activeLaunch !== null && activeLaunch !== account.id} title={unavailable
      ? t('该账号没有可切换的完整凭证，请重新登录或导入完整凭证')
      : t('切换本机客户端到此账号')} onClick={() => {
        if (activeLaunch !== null && activeLaunch !== account.id) return
        activeLaunch = account.id; publish(); setOpen(true)
      }}>{t('切号')}</Button>
    {open ? <PluginDesktopLaunchDialog provider={provider} account={account.id} name={displayNameOf(account)} clients={clients}
      request={request} pickProgram={path => {
        const pick = native.workbuddyDesktop?.pickDesktopClient
        if (!pick) return Promise.reject(new Error(t('本机程序选择不可用，请更新桌面端')))
        return pick(path)
      }} onClose={() => { activeLaunch = null; publish(); setOpen(false) }} onChanged={() => shared().wbApp?.refresh?.()} t={t} /> : null}
  </>
}
