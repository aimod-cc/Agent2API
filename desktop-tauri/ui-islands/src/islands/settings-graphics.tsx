import * as React from 'react'
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Label,
  Switch,
} from '@ui'
import { t } from '../i18n'
import { shared, type GraphicsSettings } from './settings-model'
import { useSettings } from './settings-state'

export function GraphicsSettingsPanel() {
  const api = shared().workbuddyDesktop
  const snap = useSettings()
  const [settings, setSettings] = React.useState<GraphicsSettings | null>(null)
  const [loadError, setLoadError] = React.useState('')
  const [error, setError] = React.useState('')
  const [confirm, setConfirm] = React.useState<boolean | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [restarting, setRestarting] = React.useState(false)
  const busyRef = React.useRef(false)
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)
  const isLinux = api?.platform === 'linux'

  React.useEffect(() => {
    if (!isLinux || !api?.getGraphicsSettings) return
    let cancelled = false
    api.getGraphicsSettings().then(value => {
      if (!cancelled) setSettings(value)
    }).catch(() => {
      if (!cancelled) setLoadError(t('未能读取硬件加速设置'))
    })
    return () => { cancelled = true }
  }, [isLinux, api])

  if (!isLinux) return null

  function requestChange(next: boolean) {
    if (!settings || busyRef.current || restarting || snap.busy !== null) return
    setError('')
    setConfirm(next)
  }

  async function save() {
    if (confirm === null || busyRef.current || snap.busy !== null || !api?.saveGraphicsSettings) return
    busyRef.current = true
    setBusy(true)
    setError('')
    try {
      await api.saveGraphicsSettings(confirm)
      setConfirm(null)
      setRestarting(true)
    } catch (failure) {
      setError(String(failure instanceof Error ? failure.message : failure))
      try {
        if (api.getGraphicsSettings) setSettings(await api.getGraphicsSettings())
      } catch { /* 保留最后一次读到的状态，错误仍展示在确认框内。 */ }
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const pending = settings?.restartRequired === true
  const checked = settings?.restartRequired ? settings.enabled : settings?.active ?? true
  const stateText = restarting
    ? t('✅ 设置已保存，应用正在重启…')
    : loadError || (settings
      ? settings.environmentOverride
        ? t('启动环境已关闭硬件加速；重新开启前需移除 WEBKIT_DISABLE_COMPOSITING_MODE。')
        : pending
          ? t('设置已保存，需要重启应用才能生效。')
          : settings.active ? '' : t('当前使用兼容模式。')
      : t('检测中…'))

  return (
    <>
      <Card render={<section />} className='panel'>
        <CardHeader className='panel-head'>
          <h2>{t('界面渲染')}</h2>
        </CardHeader>
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <Label className='switch'>
              <Switch
                id='settings-hardware-acceleration'
                checked={checked}
                disabled={!settings || busy || restarting || snap.busy !== null || settings.environmentOverride}
                onCheckedChange={requestChange}
              />
              <span>{t('硬件加速')}</span>
            </Label>
          </div>
          <div className='hint retention-note'>
            {t('遇到白屏、画面不更新或点击无响应时可关闭。修改后需重启应用。')}
          </div>
          {stateText ? <div className='settings-state'>{stateText}</div> : null}
          {pending && !restarting && !settings.environmentOverride ? (
            <Button variant='outline' disabled={busy || snap.busy !== null} onClick={() => requestChange(settings.enabled)}>
              {t('保存并重启')}
            </Button>
          ) : null}
          {settings?.compatibilityMode && settings.enabled && !pending && !restarting ? (
            <Button variant='outline' disabled={busy || snap.busy !== null} onClick={() => requestChange(false)}>
              {t('以后也使用兼容模式')}
            </Button>
          ) : null}
        </CardContent>
      </Card>

      <Dialog open={confirm !== null} onOpenChange={open => { if (!open && !busyRef.current) setConfirm(null) }}>
        <DialogContent className='w-[min(480px,calc(100vw-48px))]' initialFocus={cancelRef}>
          <DialogHeader>
            <DialogTitle>{confirm ? t('开启硬件加速') : t('关闭硬件加速')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <p>{confirm
              ? t('开启后恢复加速渲染。若此前出现白屏或卡住，可从托盘选择「以兼容模式重启」。')
              : t('关闭后使用兼容模式，可能降低滚动和动画的流畅度。')}</p>
            <p className='mt-3'>{t('保存后将重启应用，网关会短暂中断。')}</p>
            {error ? <div className='mt-3 whitespace-pre-wrap text-destructive'>{error}</div> : null}
          </DialogBody>
          <DialogFooter>
            <Button ref={cancelRef} variant='outline' disabled={busy} onClick={() => setConfirm(null)}>{t('取消')}</Button>
            <Button disabled={busy} onClick={() => void save()}>{busy ? t('正在保存…') : t('保存并重启')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
