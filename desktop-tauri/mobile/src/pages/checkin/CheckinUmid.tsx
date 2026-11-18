import { useState } from 'react'
import { Button } from '@ui'
import { getUmid, installUmid, type UmidState } from '../../api/checkin'
import { usePolling } from '../../lib/usePolling'
import { serverIdentity } from '../../lib/servers'
import { store } from '../../state/store'
import { t } from '../../i18n'

export function CheckinUmid() {
  const [state, setState] = useState<UmidState | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const identity = serverIdentity()
  const load = async () => {
    try {
      const next = await getUmid()
      if (serverIdentity() === identity) { setState(next); setError('') }
    } catch (cause) {
      if (serverIdentity() === identity) setError(cause instanceof Error ? cause.message : String(cause))
    }
  }
  usePolling(load, state?.installing ? 3000 : 60_000)

  const install = async () => {
    if (busy) return
    setBusy(true)
    try {
      const next = await installUmid()
      if (serverIdentity() !== identity) return
      setState(next)
      await load()
    } catch (cause) {
      if (serverIdentity() === identity) store.toast(cause instanceof Error ? cause.message : String(cause), 'err')
    } finally { setBusy(false) }
  }

  return (
    <div className='m-card-pad'>
      <p className='m-ck-note'>{t('网关设备组件')}: {state?.available ? t('已就绪') : state?.installing ? t('安装中…') : t('未就绪')}</p>
      {state?.authorized === false ? <p className='m-ck-note'>{t('请在 Qoder 国际版插件设置中授权设备验证组件')}</p> : null}
      {state?.installSupported && state.authorized !== false && !state.available ? <Button variant='outline' size='sm' disabled={busy || state.installing}
        onClick={() => void install()}>{busy || state.installing ? t('安装中…') : t('在网关安装组件')}</Button> : null}
      {error ? <p className='m-ck-error'>{error}</p> : null}
    </div>
  )
}
