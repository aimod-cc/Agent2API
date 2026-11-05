import type { ServiceUpdateTask } from '../../../ui-shared/updates'
import { errorMessage, serviceUpdateTask, shared, type UpdateInfo } from './update-shared'
import { t } from '../i18n'

export type ServiceProgress = {
  info: UpdateInfo
  text: string
  badge: string
  active: boolean
  failed: boolean
  percent: number
}

type Listener = (progress: ServiceProgress) => void
let polling = false
let lastInfo: UpdateInfo | null = null
let loadedVersion = ''
let listener: Listener | null = null

export function serviceTaskActive(task?: ServiceUpdateTask | null): boolean {
  return !!task && ['downloading', 'verifying', 'installing', 'restarting'].includes(task.phase)
}

export function serviceProgress(info: UpdateInfo): ServiceProgress | null {
  const task = serviceUpdateTask(info)
  if (!task) return null
  const names = {
    downloading: t('下载中'), verifying: t('校验中'), installing: t('安装中'),
    restarting: t('重启中'), completed: t('更新完成'), failed: t('更新失败'),
  }
  let text = ''
  switch (task.phase) {
    case 'downloading': text = t('正在下载 {version} 的服务端更新包…', { version: task.version }); break
    case 'verifying': text = t('正在独立核验官方更新包，校验通过前不会修改现有程序…'); break
    case 'installing': text = t('正在备份并替换服务端程序与界面，请勿关闭服务器…'); break
    case 'restarting': text = t('正在重启服务并确认新版本，页面恢复后会自动刷新…'); break
    case 'completed': text = info.currentVersion === task.version
      ? t('已更新至 {version}，服务已恢复。', { version: task.version })
      : t('更新任务已结束，但当前运行版本与目标不一致，请检查服务日志。'); break
    case 'failed': text = t('更新失败：{error}', { error: errorMessage(task.error || t('请检查服务日志')) }); break
  }
  return {
    info, text, badge: names[task.phase], active: serviceTaskActive(task),
    failed: task.phase === 'failed' || (task.phase === 'completed' && info.currentVersion !== task.version),
    percent: 0,
  }
}

export function watchServiceUpdate(info: UpdateInfo, onProgress: Listener): void {
  listener = onProgress
  if (!loadedVersion) loadedVersion = String(info.currentVersion || '')
  lastInfo = info
  const progress = serviceProgress(info)
  if (progress) listener(progress)
  if (!progress?.active || polling) return
  polling = true
  void poll().finally(() => { polling = false })
}

async function poll(): Promise<void> {
  let disconnectedAt = 0
  while (lastInfo && serviceTaskActive(serviceUpdateTask(lastInfo))) {
    await new Promise<void>(resolve => { window.setTimeout(resolve, 1000) })
    try {
      const info = await shared().workbuddyDesktop?.getUpdateStatus()
      const task = serviceUpdateTask(info)
      if (!info || !task) throw new Error('Update status unavailable')
      lastInfo = info
      const progress = serviceProgress(info)
      if (!progress) continue
      if (task.phase === 'downloading') {
        const download = await shared().workbuddyDesktop?.updateProgress()
        progress.percent = Number(download?.percent) || 0
      }
      listener?.(progress)
      disconnectedAt = 0
      if (!progress.active) {
        if (task.phase === 'completed' && info.currentVersion === task.version
          && loadedVersion !== info.currentVersion) window.location.reload()
        return
      }
    } catch {
      if (!disconnectedAt) disconnectedAt = Date.now()
      if (Date.now() - disconnectedAt > 180_000) {
        const info = lastInfo
        listener?.({ info, badge: t('等待恢复'), text: t('暂未确认服务恢复，请检查服务器状态后刷新页面；不会自动重复安装。'),
          active: false, failed: true, percent: 0 })
        return
      }
    }
  }
}

export async function startServiceUpdate(info: UpdateInfo, onProgress: Listener): Promise<void> {
  const version = info.latestVersion
  const api = shared().workbuddyDesktop
  const start = info.updateCapability?.mode === 'windows-service' ? api?.startWindowsServiceUpdate : api?.startSystemdUpdate
  if (!version || !start) throw new Error(t('一键更新入口不可用，请刷新页面后重试。'))
  const confirmed = await shared().wbConfirm?.ask({
    title: t('一键更新服务端？'),
    text: t('将更新至 {version}。下载和校验完成后会短暂停止服务、替换程序并自动重启；账号与配置会保留。确认后无需再次点击安装。', { version }),
    okText: t('开始更新'), cancelText: t('取消'), bodyClass: '',
  })
  if (!confirmed) return
  const task = await start({ version })
  watchServiceUpdate({ ...info, serviceTask: task }, onProgress)
}
