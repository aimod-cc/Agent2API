import * as React from 'react'
import { Alert, Button } from '@ui'
import { PluginDesktopLaunchDialog } from '../../../ui-shared/plugin-desktop-launch-dialog'
import { launchMessage, readLaunchRecoveries, type LaunchClient, type LaunchPendingRecovery as Pending } from '../../../ui-shared/plugin-desktop-launch'
import { t } from '../i18n'
import { api } from './plugins-shared'

type NativeWindow = { __TAURI_INTERNALS__?: { invoke?: unknown }; workbuddyDesktop?: { platform?: string; pickDesktopClient?: (path?: string) => Promise<{ canceled: boolean; path?: string }> } }

export function DesktopLaunchRecovery() {
  const [rows, setRows] = React.useState<Pending[]>([])
  const [selected, setSelected] = React.useState<Pending | null>(null)
  const [error, setError] = React.useState('')
  const alive = React.useRef(false)
  const native = window as unknown as NativeWindow
  const enabled = native.workbuddyDesktop?.platform === 'windows' && !!native.__TAURI_INTERNALS__?.invoke
  const load = React.useCallback(async () => {
    if (!enabled) return
    try {
      const result = await api<{ pending: Pending[] }>('GET', '/api/plugins/desktop-launch/pending', undefined, { retry: 'never' })
      if (alive.current) { setRows(readLaunchRecoveries(result)); setError('') }
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
    }
  }, [enabled])
  React.useEffect(() => {
    alive.current = true
    const refresh = () => { if (alive.current) void load() }
    refresh()
    window.addEventListener('wb:desktop-launch-changed', refresh)
    window.addEventListener('wb:management-session-changed', refresh)
    return () => { alive.current = false; window.removeEventListener('wb:desktop-launch-changed', refresh); window.removeEventListener('wb:management-session-changed', refresh) }
  }, [load])
  if (!enabled || !rows.length && !error && !selected) return null
  const clients: LaunchClient[] = selected ? [{ id: selected.client, label: selected.label, platform: 'windows' }] : []
  return <>
    {error ? <Alert variant='warning' className='mx-4 mt-4'>
      <div className='flex flex-wrap items-center justify-between gap-3'><span>{t('本机凭证备份读取失败，请重新读取后处理')}</span>
        <Button size='sm' variant='outline' onClick={() => void load()}>{t('重新读取')}</Button></div>
      <p className='mt-2 text-xs'>{launchMessage(error, t)}</p>
    </Alert> : null}
    {rows.length ? <Alert variant='warning' className='mx-4 mt-4'>
      <div className='flex flex-wrap items-center justify-between gap-3'>
        <span>{t('有未完成的本机凭证切换，原凭证备份已保留')}</span>
        <div className='flex flex-wrap gap-2'>{rows.map(row => <Button key={row.backup} size='sm' variant='outline'
          title={row.executable} onClick={() => setSelected(row)}>{t('恢复 {client}', { client: row.label })}</Button>)}</div>
      </div>
    </Alert> : null}
    {selected ? <PluginDesktopLaunchDialog provider={selected.provider} account={selected.account} name={selected.account} clients={clients}
      recoveryBackup={selected.backup} request={api} pickProgram={path => native.workbuddyDesktop?.pickDesktopClient?.(path) || Promise.resolve({ canceled: true })}
      onClose={() => { setSelected(null); void load().catch(() => {}) }} onChanged={load} t={t} /> : null}
  </>
}
