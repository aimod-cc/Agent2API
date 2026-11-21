import * as React from 'react'
import { Button, Spinner } from '@ui'
import { t } from '../i18n'
import { bridgeApi, errorMessage, loadCenter, toast, type UmidState } from './checkin-store'

export function CheckinUmid(): React.ReactElement | null {
  const [state, setState] = React.useState<UmidState | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [queryError, setQueryError] = React.useState('')
  const [installError, setInstallError] = React.useState('')
  const active = React.useRef(false)
  const installing = React.useRef(false)
  const request = React.useRef(0)

  const load = React.useCallback(async (): Promise<UmidState | null> => {
    const id = ++request.current
    try {
      const getStatus = bridgeApi().getQoderUmid
      if (!getStatus) throw new Error(t('UMID 组件管理接口不可用，请刷新页面或更新网关。'))
      const next = await getStatus()
      if (!next || typeof next.available !== 'boolean') {
        throw new Error(t('后端未返回 UMID 组件状态'))
      }
      if (active.current && id === request.current) {
        setState(next)
        setQueryError('')
      }
      return next
    } catch (error) {
      if (active.current && id === request.current) setQueryError(errorMessage(error))
      return null
    }
  }, [])

  React.useEffect(() => {
    active.current = true
    void load()
    return () => {
      active.current = false
      request.current += 1
    }
  }, [load])

  React.useEffect(() => {
    const timer = window.setInterval(() => { void load() }, busy || state?.installing ? 3000 : 60_000)
    return () => window.clearInterval(timer)
  }, [busy, state?.installing, load])

  const install = async (): Promise<void> => {
    if (installing.current || state?.installing) return
    installing.current = true
    setBusy(true)
    setInstallError('')
    try {
      const installComponent = bridgeApi().installQoderUmid
      if (!installComponent) throw new Error(t('UMID 组件管理接口不可用，请刷新页面或更新网关。'))
      const result = await installComponent()
      if (!result) throw new Error(t('后端未返回 UMID 组件安装结果'))
      const next = await load()
      if (!active.current) return
      if (next?.available) {
        toast(t('UMID 组件已安装，可重新签到。'))
        await loadCenter(true)
      } else if (next && !next.installing) {
        throw new Error(t('安装后仍未检测到 UMID 组件，请重新检查。'))
      }
    } catch (error) {
      if (active.current) {
        const message = t('UMID 组件安装失败：{error}', { error: errorMessage(error) })
        setInstallError(message)
        toast(message, 'err')
      }
    } finally {
      installing.current = false
      if (active.current) {
        setBusy(false)
        void load()
      }
    }
  }

  if (state?.available) return null
  const bridge = bridgeApi()
  const unavailable = !bridge.getQoderUmid || (state?.installSupported && !bridge.installQoderUmid)
  const error = installError || queryError
  const pending = busy || state?.installing === true

  return (
    <div className='flex flex-wrap items-center gap-2' aria-live='polite'>
      <span>{state?.authorized === false
        ? (state.message || t('国际版签到需要 UMID 组件生成设备风控身份（本机未检测到）。'))
        : state ? t('国际版签到需要 UMID 组件生成设备风控身份（本机未检测到）。')
        : error ? t('网关设备组件') : t('正在读取 UMID 组件状态…')}</span>
      {state?.installSupported && state.authorized !== false ? (
        <Button variant='outline' size='sm' disabled={pending || !bridge.installQoderUmid}
          onClick={() => void install()}>
          {pending ? <><Spinner className='size-3.5' />{t('安装中…')}</> : t('一键安装组件')}
        </Button>
      ) : state && state.authorized !== false ? <span>{t('安装 Qoder 客户端或 qodercli 后即可（无需重启网关）。')}</span> : null}
      <Button variant='ghost' size='sm' onClick={() => { setInstallError(''); void load() }}>
        {t('重新检查')}
      </Button>
      {installError ? <span className='text-destructive'>{installError}</span>
        : queryError ? <span className='text-destructive'>{t('查询失败：{error}', { error: queryError })}</span> : null}
      {unavailable ? <span>
        {t('可使用管理员凭据调用：')} <code>POST /api/qoder-umid/install</code>
      </span> : null}
    </div>
  )
}
