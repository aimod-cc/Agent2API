import { useEffect, useState } from 'react'
import { Button, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Sheet, SheetBody, SheetContent, SheetHeader, SheetTitle, Switch , Label } from '@ui'
import { getPluginSettings, savePluginSettings, type PluginSettings } from '../../api/plugins'
import { GithubAccessSettingsPanel } from '../../components/GithubAccessSettings'
import { serverIdentity } from '../../lib/servers'
import { store } from '../../state/store'
import { t } from '../../i18n'

export function PluginSettingsSheet({ onClose, onChanged }: { onClose: () => void; onChanged: () => Promise<void> }) {
  const [settings, setSettings] = useState<PluginSettings | null>(null)
  const [draft, setDraft] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const identity = serverIdentity()
  const modes = [{ value: 'follow', label: t('跟随软件') }, { value: 'custom', label: t('单独设置') }] as const

  useEffect(() => {
    const controller = new AbortController()
    void getPluginSettings(controller.signal).then(next => {
      if (!controller.signal.aborted) { setSettings(next); setDraft(String(next.interval)) }
    }).catch(cause => {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause))
    })
    return () => controller.abort()
  }, [])

  async function save(patch: Partial<PluginSettings>) {
    if (busy || !settings) return
    const next = { ...settings, ...patch }
    if (patch.channelMode !== 'follow' && !next.stable && !next.beta) {
      setError(t('至少需要关注正式版或 beta 版中的一个通道'))
      return
    }
    setBusy(true)
    try {
      const next = await savePluginSettings(patch)
      if (serverIdentity() !== identity) return
      setSettings(next)
      setDraft(String(next.interval))
      setError('')
      store.toast(next.saved === false ? t('设置已生效，但写入磁盘失败（重启后会恢复原设置）') : t('插件设置已保存'), next.saved === false ? 'err' : 'ok')
      if ('beta' in patch || 'stable' in patch || 'channelMode' in patch) await onChanged()
    } catch (cause) {
      if (serverIdentity() === identity) setError(cause instanceof Error ? cause.message : String(cause))
    } finally { setBusy(false) }
  }

  const saveInterval = () => {
    const interval = Number(draft)
    if (!Number.isInteger(interval) || interval < 1 || interval > 1440) {
      setError(t('检查间隔必须是 {min} ~ {max} 分钟', { min: 1, max: 1440 }))
      return
    }
    void save({ interval })
  }
  const channelLabel = settings?.stable && settings.beta ? t('正式版与 beta 版')
    : settings?.beta ? t('仅 beta 版') : t('仅正式版')

  return <Sheet open onOpenChange={(open, details) => {
    if (open) return
    if (busy) { details.cancel(); return }
    onClose()
  }}>
    <SheetContent showCloseButton={!busy}>
      <SheetHeader><SheetTitle>{t('插件设置')}</SheetTitle></SheetHeader>
      <SheetBody>
        <p className='m-ck-note'>{t('以下设置属于所连接网关，不是手机本地设置。')}</p>
        <div className='m-ck-title'>{t('检查计划')}</div>
        <Select value={settings?.checkMode || 'follow'} disabled={busy || !settings}
          onValueChange={value => void save({ checkMode: value as PluginSettings['checkMode'] })}>
          <SelectTrigger className='w-full' aria-label={t('检查计划')}>
            <SelectValue>{modes.find(mode => mode.value === (settings?.checkMode || 'follow'))?.label}</SelectValue>
          </SelectTrigger>
          <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
        </Select>
        {settings?.checkMode === 'custom' ? <>
          <div className='m-ck-toolbar'>
            <span className='m-ck-title m-ck-grow'>{t('自动检查插件更新')}</span>
            <Switch checked={settings.autoCheck} disabled={busy} aria-label={t('自动检查插件更新')}
              onCheckedChange={value => void save({ autoCheck: value === true })} />
          </div>
          <Label className='m-ck-field'>{t('检查间隔（分钟）')}
            <Input type='number' min={1} max={1440} step={1} className='text-base' value={draft} disabled={busy || !settings.autoCheck}
              onChange={event => setDraft(event.currentTarget.value)} />
          </Label>
          <Button variant='outline' disabled={busy || !settings.autoCheck || Number(draft) === settings.interval} onClick={saveInterval}>{t('保存')}</Button>
        </> : <p className='m-ck-note'>{settings ? settings.autoCheck ? t('当前：自动检查开启，每 {n} 分钟', { n: settings.interval })
          : t('当前：自动检查关闭') : t('读取中')}</p>}
        <p className='m-ck-note'>{t('默认跟随软件的自动检查和间隔。关闭自动检查后暂停插件自动更新；手动检查、安装和更新不受影响。')}</p>
        <div className='m-ck-title'>{t('更新通道')}</div>
        <Select value={settings?.channelMode || 'follow'} disabled={busy || !settings}
          onValueChange={value => void save({ channelMode: value as PluginSettings['channelMode'] })}>
          <SelectTrigger className='w-full' aria-label={t('更新通道')}>
            <SelectValue>{modes.find(mode => mode.value === (settings?.channelMode || 'follow'))?.label}</SelectValue>
          </SelectTrigger>
          <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
        </Select>
        {settings?.channelMode === 'custom' ? <>
          <div className='m-ck-toolbar'>
            <span className='m-ck-title m-ck-grow'>{t('关注正式版更新')}</span>
            <Switch checked={settings.stable} disabled={busy} aria-label={t('关注正式版更新')}
              onCheckedChange={value => void save({ stable: value === true })} />
          </div>
          <div className='m-ck-toolbar'>
            <span className='m-ck-title m-ck-grow'>{t('关注 beta 版更新')}</span>
            <Switch checked={settings.beta} disabled={busy} aria-label={t('关注 beta 版更新')}
              onCheckedChange={value => void save({ beta: value === true })} />
          </div>
        </> : <p className='m-ck-note'>{settings ? t('当前关注：{channels}', { channels: channelLabel }) : t('读取中')}</p>}
        <p className='m-ck-note'>{t('默认跟随软件的正式版和 beta 关注选择；单独设置只影响插件。始终按版本号选择更新，不会降级。')}</p>
        <div>
          <div className='m-ck-toolbar'>
            <span className='m-ck-title m-ck-grow'>{t('自动更新插件')}</span>
            <Switch checked={settings?.autoUpdate === true} disabled={busy || !settings} aria-label={t('自动更新插件')}
              onCheckedChange={value => void save({ autoUpdate: value === true })} />
          </div>
          <p className='m-ck-note'>{t('仅自动更新已安装、已启用且通过可信签名校验的社区插件。新插件需手动安装，卸载后不会自动重新安装；未签名插件需手动更新。')}</p>
        </div>
        {error ? <p className='m-ck-error'>{error}</p> : null}
        <GithubAccessSettingsPanel />
      </SheetBody>
    </SheetContent>
  </Sheet>
}
