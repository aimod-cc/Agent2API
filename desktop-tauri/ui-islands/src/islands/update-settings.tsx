import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, NavItem, NavList, Sidebar, Switch,
} from '@ui'
import { errorMessage, forwardUpdateResult, shared, toast, type UpdateChannels } from './update-shared'
import { UpdateNetworkSettings } from './update-network-settings'
import { t } from '../i18n'

const TASK_ID = 'updateCheck'
const MIN_INTERVAL = 1
const MAX_INTERVAL = 1440

type PaneId = 'check' | 'channel' | 'network'

export function UpdateSettingsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [checkTask, setCheckTask] = React.useState<{ enabled: boolean; interval: number } | null>(null)
  const [intervalDraft, setIntervalDraft] = React.useState('')
  const [checkBusy, setCheckBusy] = React.useState(false)
  const [channels, setChannels] = React.useState<UpdateChannels | null>(null)
  const [channelBusy, setChannelBusy] = React.useState(false)
  const [pane, setPane] = React.useState<PaneId>('check')
  const contentRef = React.useRef<HTMLDivElement>(null)
  const panes: Array<[PaneId, string]> = [
    ['check', t('检查计划')], ['channel', t('更新通道')],
    ['network', t('出网代理')],
  ]

  React.useEffect(() => {
    if (!open) return
    let alive = true
    setPane('check')
    setCheckTask(null)
    setChannels(null)
    void shared().workbuddyDesktop?.getUpdateChannels().then(result => {
      if (alive && result) setChannels(result)
    }).catch(error => toast(t('读取设置失败：{error}', { error: errorMessage(error) }), 'err'))
    void shared().workbuddyDesktop?.getScheduledTasks?.().then(result => {
      const task = result?.tasks?.find(task => task.id === TASK_ID)
      if (!alive || !task) return
      const interval = Math.max(MIN_INTERVAL, Math.round(Number(task.interval) || 0))
      setCheckTask({ enabled: task.enabled === true, interval })
      setIntervalDraft(String(interval))
    }).catch(error => toast(t('读取设置失败：{error}', { error: errorMessage(error) }), 'err'))
    return () => { alive = false }
  }, [open])

  async function saveCheck(patch: { enabled?: boolean; interval?: number }): Promise<void> {
    if (checkBusy) return
    setCheckBusy(true)
    try {
      const result = await shared().workbuddyDesktop?.saveScheduledTask?.(TASK_ID, patch)
      if (!result) throw new Error(t('请求失败'))
      const interval = Math.max(MIN_INTERVAL, Math.round(Number(result.interval) || 0))
      setCheckTask({ enabled: result.enabled === true, interval })
      setIntervalDraft(String(interval))
      toast(result.saved === false
        ? t('设置已生效，但写入磁盘失败（重启后会恢复原设置）') : t('✅ 自动检查更新已保存'),
      result.saved === false ? 'err' : 'ok')
    } catch (error) {
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally { setCheckBusy(false) }
  }

  async function saveInterval(): Promise<void> {
    const interval = Number(intervalDraft)
    if (!Number.isInteger(interval) || interval < MIN_INTERVAL || interval > MAX_INTERVAL) {
      toast(t('检查间隔必须是 {min} ~ {max} 分钟', { min: MIN_INTERVAL, max: MAX_INTERVAL }), 'err')
      return
    }
    if (interval !== checkTask?.interval) await saveCheck({ interval })
  }

  async function saveChannel(patch: Partial<Pick<UpdateChannels, 'stable' | 'beta'>>): Promise<void> {
    if (channelBusy || !channels) return
    const next = { ...channels, ...patch }
    if (!next.stable && !next.beta) {
      toast(t('至少需要关注正式版或 beta 版中的一个通道'), 'err')
      return
    }
    setChannelBusy(true)
    try {
      const result = await shared().workbuddyDesktop?.setUpdateChannels(patch)
      if (!result) throw new Error(t('请求失败'))
      setChannels(result)
      toast(result.saved === false
        ? t('通道已切换，但写入磁盘失败（重启后会回到原设置）') : t('更新通道已保存'),
      result.saved === false ? 'err' : 'ok')
      await forwardUpdateResult()
      try { await shared().workbuddyDesktop?.checkUpdate() } catch { /* 检查仍遵守请求间隔和冷却 */ }
      await forwardUpdateResult()
    } catch (error) {
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally { setChannelBusy(false) }
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='settings-dlg'>
        <DialogHeader><DialogTitle>{t('更新设置')}</DialogTitle></DialogHeader>
        <DialogBody className='settings-dlg-body'>
          <Sidebar className='prov-rail settings-dlg-rail' aria-label={t('更新设置')}>
            <NavList className='rail-scroll' render={<nav />}>
              <div className='rail-label'>{t('更新设置')}</div>
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
                <p>{t('插件默认跟随这里的更新通道、自动检查和间隔；可在插件设置中单独设置。')}</p>
              </div>
              <div className='settings-dlg-fields'>
                <div className='flex items-center justify-between gap-2'>
                  <div className='text-[13px] font-semibold text-foreground'>{t('自动检查更新')}</div>
                  <Switch checked={checkTask?.enabled === true} disabled={checkBusy || !checkTask}
                    onCheckedChange={next => void saveCheck({ enabled: next === true })}
                    aria-label={t('自动检查更新')} />
                </div>
                <div className='flex flex-wrap items-center gap-2'>
                  <Input type='number' min={MIN_INTERVAL} max={MAX_INTERVAL} step={1} className='w-[110px]'
                    disabled={checkBusy || !checkTask?.enabled} value={intervalDraft}
                    onChange={event => setIntervalDraft(event.currentTarget.value)}
                    onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); void saveInterval() } }}
                    aria-label={t('检查间隔（分钟）')} />
                  <span className='text-[12px] text-subtle'>{t('分钟（{min} ~ {max}）', {
                    min: MIN_INTERVAL, max: MAX_INTERVAL,
                  })}</span>
                  <Button variant='outline' size='sm' disabled={checkBusy || !checkTask?.enabled
                    || Number(intervalDraft) === checkTask.interval} onClick={() => void saveInterval()}>{t('保存')}</Button>
                </div>
                <p className='text-[12px] text-subtle'>
                  {t('到点自动查询新版本，查到就走「检测到更新」弹窗；关闭后「检查更新」按钮仍可手动触发')}
                </p>
              </div>
            </div> : null}
            {pane === 'channel' ? <div>
              <div className='settings-dlg-pane-head'>
                <h3>{t('更新通道')}</h3>
                <p>{t('插件默认跟随这里的更新通道、自动检查和间隔；可在插件设置中单独设置。')}</p>
              </div>
              <div className='settings-dlg-fields'>
                <div>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='text-[13px] text-foreground'>{t('关注正式版更新')}</span>
                    <Switch checked={channels?.stable === true} disabled={channelBusy || !channels}
                      onCheckedChange={next => void saveChannel({ stable: next === true })} aria-label={t('关注正式版更新')} />
                  </div>
                  <p className='mt-1.5 text-[12px] text-subtle'>{t('默认开启；即使当前使用 beta 版，也可收到版本号更高的正式版更新。')}</p>
                </div>
                <div className='h-px bg-hairline' />
                <div>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='text-[13px] text-foreground'>{t('关注 beta 版更新')}</span>
                    <Switch checked={channels?.beta === true} disabled={channelBusy || !channels}
                      onCheckedChange={next => void saveChannel({ beta: next === true })} aria-label={t('关注 beta 版更新')} />
                  </div>
                  <p className='mt-1.5 text-[12px] text-subtle'>{t('beta 版可能不稳定。两项同时开启时，按版本号选择最高版本，不会降级；安装到正式版或 beta 版后仍保留关注选择。')}</p>
                </div>
              </div>
            </div> : null}
            {open && pane === 'network' ? <UpdateNetworkSettings /> : null}
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
