import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@ui'
import {
  downloadAsset, handleExternalClick, isSelfUpdate, isManualUpdate, manualUpdateReason, markdownHtml, missingAssetReasonFor,
  openExternal, safeExternal, shared, serviceUpdateTask, type UpdateInfo,
} from './update-shared'
import { t } from '../i18n'
import { serviceTaskActive } from './update-systemd'
import { HtmlContent } from '@ui'
import { Empty } from '@ui'

/**
 * 「检测到更新」弹窗（按需建、关闭即卸）—— 从 update-panel.tsx 拆出。
 *
 * ── 形态（与 port-panel 的命令式外壳同一套路）──────────────
 * 旧实现是 index.html 的 `#update-modal` 一族静态 DOM + app.js 的开关函数；
 * 现在需要时建宿主 div 挂 body、createRoot，关闭即 unmount + remove。
 * **不读写 `#update-modal*` 任何 id**，结构走组件库 Dialog 一族（Esc / 点遮罩 /
 * 焦点陷阱 / 滚动锁定全部内建）。
 *
 * ── 与 update-panel 的分界 ─────────────────────────────────
 * 「去更新」要回到设置页并直接开始下载，那是面板的流程（openAndDownload）——
 * 所以 `showUpdateModal` 收一个 `onGoUpdate` 回调，由 update-panel 把自己的
 * openAndDownload 递进来；弹与不弹的全部判定（跳过版本 / 本会话已弹 / 人在设置页）
 * 连同「跳过此次更新」的 localStorage 记录都归本文件，调用方只管把 checkUpdate /
 * getUpdateStatus 的结果转发进来。
 */

/**
 * 「跳过此次更新」记在 localStorage 的键（值 = 跳过的版本号）。
 *
 * 键名与旧 app.js 的 UPDATE_SKIP_KEY 逐字一致，别改：同一个用户升级过程中跳过的版本
 * 不该因为换了实现又弹一遍。按**版本号**记 —— 跳过的那个版本不再弹，将来更新的版本
 * 照常弹（「取消」什么都不记，见会话守卫 promptedUpdateVersion）。
 */
const UPDATE_SKIP_KEY = 'workbuddy-desktop-update-skip'

/** 读「跳过此次更新」的版本号（隐私模式等取不到就当作没跳过） */
function readSkippedVersion(): string {
  try { return localStorage.getItem(UPDATE_SKIP_KEY) || '' } catch { return '' }
}

/** 记「跳过此次更新」（写不进去只影响下次启动，不影响本次会话） */
function writeSkippedVersion(version: string): void {
  try { localStorage.setItem(UPDATE_SKIP_KEY, version) } catch { /* 忽略：下次照常弹 */ }
}

/** 本会话内已弹过提示的版本号（「取消」不写 localStorage，靠它防同一版本连弹） */
let promptedUpdateVersion = ''

/**
 * 弹与不弹的判定（从旧 app.js 的 maybeShowUpdateModal 原样搬来）：
 *   · 只有确实有新版本、且最新版本号非空才弹；
 *   · 「跳过此次更新」记的是**版本号**：该版本不再弹，将来更新的版本照常弹；
 *   · 「取消」什么都不记，但本会话内同一版本也不再弹（promptedUpdateVersion）——
 *     后端的定时检查到点会再次发现它，没有这道闸就会隔一分钟弹一次；
 *   · 人已经在设置页时不弹 —— 软件更新面板就在眼前，再盖一层弹窗纯属打扰。
 *
 * ── 这一版没有可下载的包时照弹，但按钮换掉 ──────────────────
 * 早先只判 `hasUpdate === true`：没有资产也照弹「去更新」，点进去落到面板上
 * 只会得到一句「没有可下载的安装包」—— 用户被指到一条死路。现在弹窗自己看
 * `downloadAsset`（与面板同一口径）：没有包就不给「去更新」，改成把原因说清楚
 * 并给「到 GitHub 发布页下载」。仍然弹，是因为「有新版本」这件事本身值得知道 ——
 * 静默处理反而让人永远停在旧版本上。
 *
 * 调用点与旧实现一致：check / syncFromCache / openAndDownload 结束时各一次（check 那条
 * 实际上永远被「人已在设置页」挡住，留着是为了与旧路径一一对应），另外作为契约方法供
 * app.js 的定时轮询转发。
 */
export function showUpdateModal(
  info: UpdateInfo | null | undefined,
  onGoUpdate: (info: UpdateInfo) => Promise<void>,
): void {
  if (info?.forceUpdate?.required) return
  if (info?.channelChanged) { unmountUpdateModal(); return }
  if (!info || info.hasUpdate !== true) return
  // 更新已应用到容器里、只差重启（容器内更新）：此刻弹「去更新」是条死路 ——
  // 面板上的那颗按钮已经换成「立即重启」，重启入口在那里
  if (info.pendingRestart === true || serviceTaskActive(serviceUpdateTask(info))) return
  const latest = String(info.latestVersion || '').trim()
  if (!latest) return
  if (shared().wbApp?.currentPage === 'settings') return
  if (latest === readSkippedVersion() || latest === promptedUpdateVersion) return
  promptedUpdateVersion = latest

  unmountUpdateModal()
  modalHost = document.createElement('div')
  document.body.append(modalHost)
  modalRoot = createRoot(modalHost)
  modalRoot.render(<UpdateModal info={info} onGoUpdate={onGoUpdate} onClose={unmountUpdateModal} />)
}

type UpdateModalProps = {
  /** 打开那一刻的检查结果（弹窗里的版本号与日志都是它的，不再重查） */
  info: UpdateInfo
  /** 「去更新」：面板的 openAndDownload（见文件头的分界说明） */
  onGoUpdate: (info: UpdateInfo) => Promise<void>
  onClose: () => void
}

function UpdateModal({ info, onGoUpdate, onClose }: UpdateModalProps) {
  const version = String(info.latestVersion || '')
  const notes = String(info.notes || '').trim()
  const html = notes ? markdownHtml(notes) : ''
  // 「有没有本机的包」与面板同一口径（downloadAsset）：没有就不给「去更新」，
  // 那个按钮此刻只会把人送到一句报错上
  const canDownload = !!downloadAsset(info)?.url
  const manual = isManualUpdate(info)
  const reason = manual ? manualUpdateReason(info) : canDownload ? '' : missingAssetReasonFor(info)
  const pageUrl = safeExternal(info.pageUrl)

  /** 「跳过此次更新」：按版本号记进 localStorage（与旧 app.js 同一个键） */
  function handleSkip(): void {
    writeSkippedVersion(version)
    onClose()
  }

  /** 没有可下的包时那条出口：到发布页自己挑一份（外链一律交给系统浏览器） */
  function handleOpenRelease(): void {
    onClose()
    void openExternal(pageUrl)
  }

  /**
   * 「去更新」：关窗 → 切到设置页的「更新」分类 → 带着弹窗这份结果直接开始下载。
   *
   * 为什么走 wbApp.showPage + wbSettingsPanel.showCategory：与旧 app.js 的
   * `#update-modal-go` 逐字对应（只切视图、不写分类偏好 —— 这是弹窗带来的深链，
   * 不该改用户手点的默认分类）。
   */
  function handleGo(): void {
    onClose()
    const app = shared()
    app.wbApp?.showPage?.('settings')
    app.wbSettingsPanel?.showCategory?.('about')
    void onGoUpdate(info)
  }

  return (
    // 受控 open（恒为 true）：关窗一律由 onClose 收口。Esc / 点遮罩 / 右上角 ✕ 都由
    // Base UI 的 Dialog 内建（旧实现自己听 mask 点击与那个 ✕ 按钮）
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('检测到更新')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <p className='text-[12.5px] leading-[1.6] text-subtle'>
            {t('新版本 ')}<strong className='text-foreground'>{version}</strong>
            {t(' 已发布（当前 {current}），更新日志如下：', { current: info.currentVersion || t('未知') })}
          </p>
          {/* 没有可下的包：先在这里交代原因，再谈更新日志 —— 否则用户读完日志
              点「去更新」才发现无路可走。容器端额外给出宿主机那条命令：它的升级
              本来就不走安装包，光说「没有包」等于没说 */}
          {canDownload && !manual ? null : (
            <Alert variant='warning'>
              <AlertTitle>
                {manual ? t('当前需要手动升级') : t('未找到适用的更新包')}
              </AlertTitle>
              <AlertDescription>
                {reason}
                {isSelfUpdate(info)
                  ? <div className='mt-1.5'>{t('在宿主机执行 docker compose pull && docker compose up -d 即可升级。')}</div>
                  : null}
              </AlertDescription>
            </Alert>
          )}
          {/* 正文限高 + 滚动（旧 .update-modal-notes 的 46vh），长日志不会把弹窗撑出一屏；
              overscroll-contain 拦住滚动链，滚到底不带动外层页面 */}
          <div className='max-h-[46vh] overflow-y-auto overscroll-contain pr-1.5' onClick={handleExternalClick}>
            {html ? <HtmlContent className='md-body' html={html} /> : (
              <div className='md-body'>
                <Empty className='md-body-empty'>{t('这个版本没有填写发布说明。')}</Empty>
              </div>
            )}
          </div>
        </DialogBody>
        <DialogFooter>
          {/* 旧 .modal-foot 的布局：跳过 | spacer | 取消 + 去更新。
              没有可下载的包时「去更新」这条路不存在（见 handleGo 的说明）：
              桌面端换成「到 GitHub 发布页下载」（pageUrl 正常都在，后端在没给
              html_url 时会兜底拼一个 releases 页面地址）；容器端的升级不走安装包，
              上面那条 compose 命令才是正路，这里就不给按钮了 */}
          <Button variant='outline' size='sm' onClick={handleSkip}>{t('跳过此次更新')}</Button>
          <div className='mr-auto' />
          <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
          {canDownload
            ? <Button variant='default' onClick={handleGo}>{manual ? t('下载服务端包') : t('去更新')}</Button>
            : (!isSelfUpdate(info) && pageUrl)
              ? <Button variant='default' onClick={handleOpenRelease}>{t('到 GitHub 发布页下载')}</Button>
              : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

let modalRoot: ReturnType<typeof createRoot> | null = null
let modalHost: HTMLElement | null = null

function unmountUpdateModal(): void {
  if (modalRoot) {
    modalRoot.unmount()
    modalRoot = null
  }
  if (modalHost) {
    modalHost.remove()
    modalHost = null
  }
}
