import { useEffect, useRef, useState } from 'react'
import { SectionHead } from './ui'
import { Icon } from './Icon'
import { store } from '../state/store'
import {
  APP_VERSION,
  autoCheckEnabled,
  autoCheckIntervalMs,
  canInstallApk,
  canSelfUpdate,
  cancelDownload,
  checkApkUpdate,
  downloadApk,
  hasDownloadedApk,
  installApk,
  openInstallPermission,
  setAutoCheckEnabled,
  type ApkUpdateInfo,
  type DownloadProgress,
} from '../lib/appUpdate'
import { openExternal } from '../lib/community'
import { t } from '../i18n'

/**
 * 应用内更新面板（Android / iOS 共用，行为按平台分叉）。
 *
 * ── 状态机 ──────────────────────────────────────────────────────
 *   idle → checking → (up-to-date | available) → downloading → downloaded
 * 任一步失败回落到 idle 并给错误提示。
 *
 * ── iOS 为什么只有「去下载」 ────────────────────────────────────
 * 苹果不允许 App 自行下载并安装可执行代码，IPA 只能由系统装（App Store /
 * TestFlight / 侧载工具）。所以 iOS 上这个面板到「发现新版本」为止，动作
 * 按钮改成打开 GitHub Release 页面，由用户自己选渠道 —— 不渲染下载进度条
 * 与「安装」按钮，避免给一个必然失败的承诺。
 *
 * ── 自动检查 ────────────────────────────────────────────────────
 * 挂载时按电脑端配置的间隔轮询检查（间隔读 `/api/scheduled-tasks` 的
 * `updateCheck` 条目，见 lib/appUpdate）。页面不可见时停表 —— 与各列表页的
 * 轮询同一套可见性规则，手机上不该在后台耗流量。
 *
 * ── 为什么「已下载」要单独一档 ──────────────────────────────────
 * 用户下完包、跳去授权、再回来时组件会重新挂载 —— 如果只按内存状态判断，
 * 界面会回到「有新版本」让他重下一遍（几十 MB）。所以挂载时问一次原生
 * `downloaded()`，有包就直接进「待安装」。
 *
 * ── 授权顺序 ────────────────────────────────────────────────────
 * 先下载再要授权：下载不需要任何权限，而授权会跳走系统设置页 —— 先要权限
 * 再下载，用户跳回来时下载还没开始，体验是断的。
 */
type Phase = 'idle' | 'checking' | 'latest' | 'available' | 'downloading' | 'downloaded'

export function AppUpdatePanel() {
  const [phase, setPhase] = useState<Phase>('idle')
  const [info, setInfo] = useState<ApkUpdateInfo | null>(null)
  const [progress, setProgress] = useState<DownloadProgress | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [auto, setAuto] = useState(autoCheckEnabled)
  /** 自动检查的间隔（0 = 电脑端关了这条任务） */
  const [interval, setIntervalMs] = useState(0)
  /** 当前壳能不能自装安装包：只有 Android 能（见 lib/appUpdate） */
  const selfUpdate = canSelfUpdate()

  // 挂载时先看缓存里有没有上次下好的包（iOS 上恒为 false）
  useEffect(() => {
    let cancelled = false
    void hasDownloadedApk().then(result => {
      if (!cancelled && result.exists) setPhase('downloaded')
    })
    return () => {
      cancelled = true
    }
  }, [])

  /**
   * 自动检查：间隔读电脑端配置，页面不可见时停表。
   *
   * 为什么把 interval 放进依赖：用户在电脑端改了间隔，手机端下次进来就该
   * 按新间隔走 —— 组件重挂时重新读一次即可（设置页每次进入都会重挂）。
   */
  useEffect(() => {
    if (!auto) return
    let disposed = false
    let timer: number | null = null

    const clear = () => {
      if (timer !== null) {
        window.clearInterval(timer)
        timer = null
      }
    }
    const start = (ms: number) => {
      clear()
      if (disposed || ms <= 0) return
      timer = window.setInterval(() => {
        if (document.visibilityState === 'visible') void runCheck(true)
      }, ms)
    }

    void (async () => {
      const ms = await autoCheckIntervalMs()
      if (disposed) return
      setIntervalMs(ms)
      // 进页面先查一次（用户打开设置就是想看有没有新版），再按间隔轮询
      await runCheck(true)
      start(ms)
    })()

    const onVisibility = () => {
      if (document.visibilityState === 'visible') void runCheck(true)
    }
    document.addEventListener('visibilitychange', onVisibility)

    return () => {
      disposed = true
      clear()
      document.removeEventListener('visibilitychange', onVisibility)
    }
    // auto 变化时重来一轮；interval 由 effect 内部读取
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto])

  // 避免自动检查覆盖用户正在进行的下载 / 已下载待安装
  const phaseRef = useRef(phase)
  phaseRef.current = phase

  /** `silent` = 自动检查：失败不弹错误、不打断当前状态 */
  const runCheck = async (silent = false) => {
    const current = phaseRef.current
    if (current === 'downloading' || current === 'downloaded') return
    if (!silent) {
      setBusy(true)
      setPhase('checking')
    }
    if (!silent) setError('')
    try {
      const result = await checkApkUpdate()
      setInfo(result)
      // hasUpdate 为 null = 版本无法比较（上游 tag 不是 semver）；
      // 这时只要挂了 APK 就允许手动更新，而不是卡在「未知」上
      const canUpdate = result.hasUpdate === true || (result.hasUpdate === null && result.asset !== null)
      setPhase(prev => (prev === 'downloading' || prev === 'downloaded' ? prev : canUpdate ? 'available' : 'latest'))
    } catch (failure) {
      if (!silent) {
        setError(failure instanceof Error ? failure.message : String(failure))
        setPhase('idle')
      }
    } finally {
      if (!silent) setBusy(false)
    }
  }

  const download = async () => {
    const url = info?.asset?.url
    if (!url) {
      setError(t('这一版没有提供 Android 安装包'))
      return
    }
    setBusy(true)
    setError('')
    setProgress({ received: 0, total: 0, percent: -1 })
    setPhase('downloading')
    try {
      await downloadApk(url, setProgress)
      setPhase('downloaded')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      setPhase('available')
    } finally {
      setBusy(false)
      setProgress(null)
    }
  }

  const install = async () => {
    setBusy(true)
    setError('')
    try {
      // 先确认授权：没有的话跳到系统设置页，回来后用户再点一次安装
      if (!(await canInstallApk())) {
        store.toast(t('请允许安装未知来源应用后重试'))
        await openInstallPermission()
        return
      }
      await installApk()
      store.toast(t('已启动安装器'))
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setBusy(false)
    }
  }

  const stop = async () => {
    await cancelDownload()
    setPhase('available')
    setProgress(null)
    setBusy(false)
  }

  /**
   * iOS 的「更新」只能跳转：打开 GitHub Release 页面，由用户自己选渠道
   * （TestFlight / App Store / 侧载）。`pageUrl` 是网关从 GitHub API 拿到的
   * release 页面地址，没有它时退回仓库的 releases 列表。
   */
  const openReleasePage = () => {
    const target = info?.pageUrl || 'https://github.com/shadyrispy/Agent2API/releases'
    openExternal(target)
  }

  const toggleAuto = () => {
    const next = !auto
    setAutoCheckEnabled(next)
    setAuto(next)
    store.toast(next ? t('已开启自动检查') : t('已关闭自动检查'))
  }

  const percent = progress && progress.percent >= 0 ? progress.percent : null
  const sizeText = (bytes?: number) => {
    if (!bytes) return ''
    const mb = bytes / 1024 / 1024
    return `${mb >= 100 ? mb.toFixed(0) : mb.toFixed(1)} MB`
  }

  return (
    <div className='m-section'>
      <SectionHead title={t('软件更新')} />
      <div className='m-card'>
        <div className='m-row' style={{ alignItems: 'flex-start' }}>
          <span className='m-logo' style={{ background: 'var(--m-primary-soft)', borderColor: 'transparent', color: 'var(--m-primary)' }}>
            <Icon name='up' />
          </span>
          <div className='m-grow' style={{ minWidth: 0 }}>
            <div className='m-t1'>
              {phase === 'latest'
                ? t('已是最新版本')
                : phase === 'available'
                  ? t('发现新版本 {version}', { version: info?.latestVersion || '' })
                  : phase === 'downloading'
                    ? t('正在下载…')
                    : phase === 'downloaded'
                      ? t('已下载，待安装')
                      : phase === 'checking'
                        ? t('正在检查…')
                        : t('当前版本 {version}', { version: APP_VERSION })}
            </div>
            <div className='m-t2' style={{ wordBreak: 'break-word' }}>
              {phase === 'available' && !selfUpdate
                ? t('请在 Release 页面下载后通过 TestFlight 或 App Store 安装')
                : phase === 'available' && info?.asset
                  ? t('{size} · 下载后可直接安装，无需跳转浏览器', { size: sizeText(info.asset.size) })
                  : phase === 'downloaded'
                    ? t('安装包已就绪，点「安装」即可完成更新')
                    : phase === 'downloading' && percent !== null
                      ? t('已完成 {percent}%', { percent })
                    : phase === 'latest'
                      ? t('当前版本 {version}', { version: APP_VERSION })
                      : selfUpdate
                        ? t('版本 {version} · 从 GitHub 直接下载安装包', { version: APP_VERSION })
                        : t('版本 {version} · 更新包需从 Release 页面获取', { version: APP_VERSION })}
            </div>

            {/* 进度条：总长度未知（percent = null）时不画，只显示已下载量 */}
            {phase === 'downloading' ? (
              <div style={{ marginTop: 8 }}>
                <div
                  style={{
                    height: 4,
                    borderRadius: 2,
                    background: 'var(--m-surface-3)',
                    overflow: 'hidden',
                  }}
                >
                  <div
                    style={{
                      height: '100%',
                      width: percent === null ? '30%' : `${percent}%`,
                      borderRadius: 2,
                      background: 'var(--m-primary)',
                      transition: 'width .2s linear',
                    }}
                  />
                </div>
                {progress ? (
                  <div style={{ marginTop: 4, fontSize: 11, color: 'var(--m-text-3)' }}>
                    {sizeText(progress.received)}
                    {progress.total > 0 ? ` / ${sizeText(progress.total)}` : ''}
                  </div>
                ) : null}
              </div>
            ) : null}

            {error ? (
              <div style={{ marginTop: 6, fontSize: 11.5, color: 'var(--m-danger)', wordBreak: 'break-word' }}>
                {error}
              </div>
            ) : null}
          </div>

          {/* 动作按钮：按状态给唯一的主动作，避免一屏好几个可点的东西 */}
          {phase === 'downloading' ? (
            <button type='button' className='m-btn m-ghost m-sm' onClick={() => void stop()}>
              {t('取消')}
            </button>
          ) : phase === 'downloaded' ? (
            <button type='button' className='m-btn m-sm' disabled={busy} onClick={() => void install()}>
              {t('安装')}
            </button>
          ) : phase === 'available' && !selfUpdate ? (
            // iOS：只能跳出去，由系统渠道安装
            <button type='button' className='m-btn m-sm' onClick={openReleasePage}>
              {t('去下载')}
            </button>
          ) : phase === 'available' ? (
            <button type='button' className='m-btn m-sm' disabled={busy} onClick={() => void download()}>
              {t('立即更新')}
            </button>
          ) : (
            <button
              type='button'
              className='m-btn m-ghost m-sm'
              disabled={busy}
              onClick={() => void runCheck()}
            >
              {phase === 'checking' ? t('检查中…') : t('检查更新')}
            </button>
          )}
        </div>

        {/* 更新说明：有内容才展开，避免空卡片 */}
        {info?.notes && (phase === 'available' || phase === 'downloaded') ? (
          <div
            style={{
              padding: '10px 13px',
              borderTop: '1px solid var(--m-line)',
              fontSize: 11.5,
              lineHeight: 1.6,
              color: 'var(--m-text-2)',
              whiteSpace: 'pre-wrap',
              maxHeight: 180,
              overflowY: 'auto',
              wordBreak: 'break-word',
            }}
          >
            {info.notes}
          </div>
        ) : null}

        {/* 自动检查开关：间隔跟电脑端一致，这里只控制「这台手机查不查」 */}
        <div className='m-row' style={{ borderTop: '1px solid var(--m-line)' }}>
          <span className='m-logo'>
            <Icon name='refresh' />
          </span>
          <div className='m-grow'>
            <div className='m-t1'>{t('自动检查更新')}</div>
            <div className='m-t2'>
              {interval > 0
                ? t('与电脑端一致：每 {n} 分钟', { n: Math.round(interval / 60_000) })
                : t('电脑端已关闭「软件版本检查」，此处只做手动检查')}
            </div>
          </div>
          <button
            type='button'
            role='switch'
            aria-checked={auto}
            aria-label={t('自动检查更新')}
            onClick={toggleAuto}
            style={{
              width: 44,
              height: 26,
              borderRadius: 999,
              border: auto ? '0' : '1px solid var(--m-line-strong)',
              background: auto ? 'var(--m-ok)' : 'var(--m-surface-3)',
              position: 'relative',
              padding: 0,
              flex: 'none',
              transition: 'background .18s, border-color .18s',
            }}
          >
            <span
              style={{
                position: 'absolute',
                top: 3,
                left: auto ? 21 : 3,
                width: 20,
                height: 20,
                borderRadius: '50%',
                background: '#fff',
                boxShadow: '0 1px 3px rgba(0,0,0,.25)',
                transition: 'left .18s cubic-bezier(.3,.9,.4,1)',
              }}
            />
          </button>
        </div>
      </div>
    </div>
  )
}
