import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, NavItem, NavList, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Sidebar, Switch,
} from '@ui'
import { GithubUpdateSettings } from './update-github-settings'
import { api, toast } from './plugins-shared'
import type { PluginSettings } from './plugins-types'
import { t } from '../i18n'

type PaneId = 'check' | 'channel' | 'automatic' | 'network' | 'token'

export function PluginSettingsDialog({ open, onClose, onChanged }: {
  open: boolean
  onClose: () => void
  onChanged: () => Promise<void>
}) {
  const [settings, setSettings] = React.useState<PluginSettings | null>(null)
  const [draft, setDraft] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [pane, setPane] = React.useState<PaneId>('check')
  const contentRef = React.useRef<HTMLDivElement>(null)
  const panes: Array<[PaneId, string]> = [
    ['check', t('检查计划')], ['channel', t('更新通道')], ['automatic', t('自动更新插件')],
    ['network', t('网络')], ['token', t('GitHub 令牌')],
  ]
  const modes = [
    { value: 'follow', label: t('跟随软件') },
    { value: 'custom', label: t('单独设置') },
  ] as const

  React.useEffect(() => {
    if (!open) return
    let alive = true
    setPane('check')
    setSettings(null)
    void api<PluginSettings>('GET', '/api/plugins/settings').then(value => {
      if (alive) { setSettings(value); setDraft(String(value.interval)) }
    }).catch(error => toast(t('读取设置失败：{error}', { error: String(error) }), 'err'))
    return () => { alive = false }
  }, [open])

  async function save(patch: Partial<PluginSettings>): Promise<void> {
    if (busy || !settings) return
    const next = { ...settings, ...patch }
    if (patch.channelMode !== 'follow' && !next.stable && !next.beta) {
      toast(t('至少需要关注正式版或 beta 版中的一个通道'), 'err')
      return
    }
    setBusy(true)
    try {
      const result = await api<PluginSettings>('PATCH', '/api/plugins/settings', patch)
      setSettings(result)
      setDraft(String(result.interval))
      toast(result.saved === false
        ? t('设置已生效，但写入磁盘失败（重启后会恢复原设置）') : t('插件设置已保存'),
      result.saved === false ? 'err' : 'ok')
      if ('beta' in patch || 'stable' in patch || 'channelMode' in patch) await onChanged()
    } catch (error) {
      toast(t('保存失败：{error}', { error: error instanceof Error ? error.message : String(error) }), 'err')
    } finally { setBusy(false) }
  }

  async function saveInterval(): Promise<void> {
    const interval = Number(draft)
    if (!Number.isInteger(interval) || interval < 1 || interval > 1440) {
      toast(t('检查间隔必须是 {min} ~ {max} 分钟', { min: 1, max: 1440 }), 'err')
      return
    }
    await save({ interval })
  }

  const channelLabel = settings?.stable && settings.beta ? t('正式版与 beta 版')
    : settings?.beta ? t('仅 beta 版') : t('仅正式版')
  return (
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='settings-dlg'>
        <DialogHeader><DialogTitle>{t('插件设置')}</DialogTitle></DialogHeader>
        <DialogBody className='settings-dlg-body'>
          <Sidebar className='prov-rail settings-dlg-rail' aria-label={t('插件设置')}>
            <NavList className='rail-scroll' render={<nav />}>
              <div className='rail-label'>{t('插件设置')}</div>
              {panes.map(([id, label]) => (
                <NavItem key={id} active={pane === id} title={label} onClick={() => {
                  setPane(id)
                  if (contentRef.current) contentRef.current.scrollTop = 0
                }}>{label}</NavItem>
              ))}
            </NavList>
          </Sidebar>
          <div className='settings-dlg-main' ref={contentRef}>
            {pane === 'check' ? <div>
              <div className='settings-dlg-pane-head'>
                <h3>{t('检查计划')}</h3>
                <p>{t('默认跟随软件的自动检查和间隔。关闭自动检查后暂停插件自动更新；手动检查、安装和更新不受影响。')}</p>
              </div>
              <div className='settings-dlg-fields'>
                <Select value={settings?.checkMode || 'follow'} disabled={busy || !settings}
                  onValueChange={value => void save({ checkMode: value as PluginSettings['checkMode'] })}>
                  <SelectTrigger className='w-full' aria-label={t('检查计划')}>
                    <SelectValue>{modes.find(mode => mode.value === (settings?.checkMode || 'follow'))?.label}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
                </Select>
                {settings?.checkMode === 'custom' ? <>
                  <div className='flex items-center justify-between gap-2'>
                    <div className='text-[13px] text-foreground'>{t('自动检查更新')}</div>
                    <Switch checked={settings.autoCheck} disabled={busy}
                      onCheckedChange={value => void save({ autoCheck: value === true })}
                      aria-label={t('自动检查插件更新')} />
                  </div>
                  <div className='flex flex-wrap items-center gap-2'>
                    <Input type='number' min={1} max={1440} step={1} className='w-[110px]'
                      disabled={busy || !settings.autoCheck} value={draft}
                      onChange={event => setDraft(event.currentTarget.value)}
                      onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); void saveInterval() } }}
                      aria-label={t('检查间隔（分钟）')} />
                    <span className='text-[12px] text-subtle'>{t('分钟（{min} ~ {max}）', { min: 1, max: 1440 })}</span>
                    <Button variant='outline' size='sm' disabled={busy || !settings.autoCheck || Number(draft) === settings.interval}
                      onClick={() => void saveInterval()}>{t('保存')}</Button>
                  </div>
                </> : <p className='text-[12px] text-subtle'>
                  {settings ? settings.autoCheck ? t('当前：自动检查开启，每 {n} 分钟', { n: settings.interval })
                    : t('当前：自动检查关闭') : t('读取中')}
                </p>}
              </div>
            </div> : null}
            {pane === 'channel' ? <div>
              <div className='settings-dlg-pane-head'>
                <h3>{t('更新通道')}</h3>
                <p>{t('默认跟随软件的正式版和 beta 关注选择；单独设置只影响插件。始终按版本号选择更新，不会降级。')}</p>
              </div>
              <div className='settings-dlg-fields'>
                <Select value={settings?.channelMode || 'follow'} disabled={busy || !settings}
                  onValueChange={value => void save({ channelMode: value as PluginSettings['channelMode'] })}>
                  <SelectTrigger className='w-full' aria-label={t('更新通道')}>
                    <SelectValue>{modes.find(mode => mode.value === (settings?.channelMode || 'follow'))?.label}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
                </Select>
                {settings?.channelMode === 'custom' ? <>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='text-[13px] text-foreground'>{t('关注正式版更新')}</span>
                    <Switch checked={settings.stable} disabled={busy} aria-label={t('关注正式版更新')}
                      onCheckedChange={value => void save({ stable: value === true })} />
                  </div>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='text-[13px] text-foreground'>{t('关注 beta 版更新')}</span>
                    <Switch checked={settings.beta} disabled={busy} aria-label={t('关注 beta 版更新')}
                      onCheckedChange={value => void save({ beta: value === true })} />
                  </div>
                </> : <p className='text-[12px] text-subtle'>{settings ? t('当前关注：{channels}', { channels: channelLabel }) : t('读取中')}</p>}
              </div>
            </div> : null}
            {pane === 'automatic' ? <div>
              <div className='settings-dlg-pane-head'><h3>{t('自动更新插件')}</h3></div>
              <div className='settings-dlg-fields'>
                <div>
                  <div className='flex items-center justify-between gap-2'>
                    <div className='text-[13px] font-semibold text-foreground'>{t('自动更新插件')}</div>
                    <Switch checked={settings?.autoUpdate === true} disabled={busy || !settings}
                      onCheckedChange={value => void save({ autoUpdate: value === true })} aria-label={t('自动更新插件')} />
                  </div>
                  <p className='mt-1.5 text-[12px] text-subtle'>
                    {t('仅自动更新已安装、已启用且通过可信签名校验的社区插件。新插件需手动安装，卸载后不会自动重新安装；未签名插件需手动更新。')}
                  </p>
                </div>
                <p className='text-[12px] text-subtle'>
                  {t('默认跟随软件的自动检查和间隔。关闭自动检查后暂停插件自动更新；手动检查、安装和更新不受影响。')}
                </p>
              </div>
            </div> : null}
            {open ? <GithubUpdateSettings plugin pane={pane === 'network' || pane === 'token' ? pane : null} /> : null}
          </div>
        </DialogBody>
        <DialogFooter className='settings-dlg-footer'>
          <p className='mr-auto text-[12px] text-subtle'>{t('开关和选项即时保存；输入项请点击对应的「保存」按钮。')}</p>
          <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
