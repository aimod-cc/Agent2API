import { useEffect, useState } from 'react'
import { Sheet, SheetContent, SheetHeader, SheetBody, SheetTitle, SheetFooter } from '@ui'
import { apiCall } from '../api/client'
import { APP_VERSION } from '../lib/appUpdate'
import { isPackaged, nativePlatform } from '../lib/servers'
import { Badge, copyWithToast } from './ui'
import { Icon } from './Icon'
import { t } from '../i18n'

/**
 * 「版本信息」抽屉（手机端设置页底部那一行点开）。
 *
 * ── 为什么手机端也要这一块 ──────────────────────────────────
 * 手机端原来只在设置页底部显示**自己的** `v{APP_VERSION}`，用户连的是哪台
 * 网关、那台网关是什么形态 / 什么版本，一概看不到。而手机端出问题时，
 * 第一个要问的恰恰是「网关那边是哪一版、跑在什么环境」—— 让用户再跑到
 * 电脑前翻一遍设置页不现实。
 *
 * ── 两段版本号分开显示 ──────────────────────────────────────
 *   · **本机**：APK 的 versionName（构建期注入）/ PWA 的界面版本。它来自
 *     安装包，与网关无关；
 *   · **网关**：`GET /api/system-info` 里那份（版本 / 形态 / 系统 / 路径）。
 * 两者可能不同（手机装 3.1.0、网关还是 3.0.9），混成一个「版本」会让人
 * 拿着手机版本去比对网关的更新日志。
 *
 * ── 复用桌面端的接口与脱敏口径 ──────────────────────────────
 * 同一个 `/api/system-info`，同样的 `?hideUsername=1`（后端脱敏，见
 * `api::system_info` 的模块头）。手机端不单独造一份后端契约。
 */

type GatewayInfo = {
  app?: {
    version?: string | null
    channel?: string | null
    schemaVersion?: number | null
    dbSchemaVersion?: number | null
  }
  runtime?: {
    deployment?: string | null
    os?: string | null
    osVersion?: string | null
    arch?: string | null
    webviewVersion?: string | null
    uptimeSeconds?: number | null
    debugBuild?: boolean | null
  }
  deployment?: {
    host?: string | null
    port?: number | null
    panelPort?: number | null
    configDir?: string | null
    dbFile?: string | null
    dbBytes?: number | null
    logFile?: string | null
    exePath?: string | null
  }
  security?: {
    adminRegistered?: boolean | null
    apiKeyCount?: number | null
    v1FailClosed?: boolean | null
  }
  stats?: {
    accounts?: number | null
    enabledAccounts?: number | null
    models?: number | null
    inFlight?: number | null
  }
}

/** 运行形态 → 展示文案（与桌面端 system-info-pane 同一份取值表） */
const DEPLOYMENT_LABEL: Record<string, string> = {
  desktop: t('桌面端'),
  'windows-service': t('Windows 服务'),
  docker: t('Docker 容器'),
  systemd: t('systemd 服务'),
  foreground: t('前台运行'),
}

const OS_LABEL: Record<string, string> = {
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
}

function text(value: unknown): string {
  if (value === null || value === undefined) return '—'
  const result = String(value).trim()
  return result || '—'
}

/**
 * 系统名 + 版本拼成一句。
 *
 * Windows 的 `ProductName` 自带 "Windows"、Linux 的 `PRETTY_NAME` 自带发行版名，
 * 直接加前缀会变成「Windows Windows 11 Pro」。与桌面端 system-info-pane.tsx
 * 的同名函数同一口径（两处各写一份：手机端不 import 桌面端源码）。
 */
function formatOs(os: string | null | undefined, version: string | null | undefined): string {
  const name = OS_LABEL[String(os)] || text(os)
  const detail = text(version)
  if (detail === '—') return name
  if (name !== '—' && detail.toLowerCase().startsWith(name.toLowerCase())) return detail
  return `${name} ${detail}`
}

function formatBytes(bytes: number | null | undefined): string {
  const value = Number(bytes)
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  if (value < 1024) return `${value} B`
  const kb = value / 1024
  if (kb < 1024) return `${kb.toFixed(1)} KB`
  const mb = kb / 1024
  if (mb < 1024) return `${mb.toFixed(1)} MB`
  return `${(mb / 1024).toFixed(2)} GB`
}

function formatUptime(seconds: number | null | undefined): string {
  const total = Number(seconds)
  if (!Number.isFinite(total) || total <= 0) return '—'
  const days = Math.floor(total / 86400)
  const hours = Math.floor((total % 86400) / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  if (days > 0) return t('{d} 天 {h} 小时', { d: days, h: hours })
  if (hours > 0) return t('{h} 小时 {m} 分钟', { h: hours, m: minutes })
  return t('{m} 分钟', { m: minutes })
}

/** 一行「键 —— 值」 */
function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className='m-row' style={{ alignItems: 'flex-start', gap: 12 }}>
      <div className='m-grow' style={{ minWidth: 0 }}>
        <div className='m-t2' style={{ fontSize: 12 }}>{label}</div>
        <div
          className={mono ? 'm-t1 m-mono' : 'm-t1'}
          style={{
            fontSize: mono ? 12 : 13,
            wordBreak: 'break-all',
            lineHeight: 1.5,
          }}
        >
          {value}
        </div>
      </div>
    </div>
  )
}

export function SystemInfoSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [info, setInfo] = useState<GatewayInfo | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [hideUsername, setHideUsername] = useState(false)

  const load = async (hide: boolean) => {
    setLoading(true)
    try {
      const result = await apiCall<GatewayInfo>(
        `/api/system-info${hide ? '?hideUsername=1' : ''}`,
      )
      setInfo(result || null)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setLoading(false)
    }
  }

  // 每次打开都重拉：运行时长、在途请求这些读数会随时间变
  useEffect(() => {
    if (open) void load(hideUsername)
    // hideUsername 变化由下面的开关自己触发，不放进依赖（否则拨开关会双拉）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const toggleHide = () => {
    const next = !hideUsername
    setHideUsername(next)
    void load(next)
  }

  const app = info?.app || {}
  const runtime = info?.runtime || {}
  const deployment = info?.deployment || {}
  const security = info?.security || {}
  const stats = info?.stats || {}
  const deploymentLabel = DEPLOYMENT_LABEL[String(runtime.deployment)] || text(runtime.deployment)
  const osFull = formatOs(runtime.os, runtime.osVersion)
  // 原生壳里「本机版本」是包版本；PWA / 浏览器里是构建期注入的界面版本
  const platform = nativePlatform()
  const localLabel = isPackaged() ? t('本机应用版本') : t('本机界面版本')
  const formLabel =
    platform === 'android' ? t('Android 应用（APK）') : platform === 'ios' ? t('iOS 应用') : t('浏览器 / PWA')

  const copyAll = () => {
    const lines = [
      `[${t('本机')}]`,
      `${localLabel}: ${APP_VERSION}`,
      `platform: ${platform === 'android' ? 'android-apk' : platform === 'ios' ? 'ios-ipa' : 'browser'}`,
      '',
      `[${t('网关')}]`,
      `${t('版本')}: ${text(app.version)}`,
      `${t('更新通道')}: ${app.channel === 'beta' ? t('beta 版') : t('正式版')}`,
      `${t('运行形态')}: ${deploymentLabel}`,
      `${t('操作系统')}: ${osFull} (${text(runtime.arch)})`,
      `${t('监听地址')}: ${text(deployment.host)}:${text(deployment.port)}`,
      `${t('已运行')}: ${formatUptime(runtime.uptimeSeconds)}`,
      `${t('数据目录')}: ${text(deployment.configDir)}`,
      `${t('日志文件')}: ${text(deployment.logFile)}`,
      `${t('账号')}: ${text(stats.enabledAccounts)} / ${text(stats.accounts)}`,
      `${t('模型')}: ${text(stats.models)}`,
      `${t('网关 Key 数量')}: ${text(security.apiKeyCount)}`,
    ]
    void copyWithToast(lines.join('\n'), t('版本信息'))
  }

  return (
    <Sheet open={open} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('版本信息')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {/* 本机：与网关无关的那一半 */}
          <div style={{ fontSize: 11.5, fontWeight: 600, color: 'var(--m-text-3)', padding: '2px 2px 6px' }}>
            {t('本机')}
          </div>
          <div className='m-card'>
            <Row label={localLabel} value={`v${APP_VERSION}`} mono />
            <Row
              label={t('运行形态')}
              value={formLabel}
            />
          </div>

          {/* 网关：连的那台服务器的读数 */}
          <div style={{ fontSize: 11.5, fontWeight: 600, color: 'var(--m-text-3)', padding: '14px 2px 6px' }}>
            {t('网关')}
          </div>
          {error ? (
            <div className='m-card' style={{ padding: '12px 14px', color: 'var(--m-danger)', fontSize: 12.5, lineHeight: 1.6 }}>
              {t('读取网关信息失败：{message}', { message: error })}
            </div>
          ) : (
            <div className='m-card'>
              <Row label={t('软件版本')} value={text(app.version)} mono />
              <Row label={t('更新通道')} value={app.channel === 'beta' ? t('beta 版') : t('正式版')} />
              <Row label={t('运行形态')} value={deploymentLabel} />
              <Row
                label={t('操作系统')}
                value={`${osFull} · ${text(runtime.arch)}`}
              />
              <Row
                label={t('监听地址')}
                value={`${text(deployment.host)}:${text(deployment.port)}`}
                mono
              />
              <Row label={t('已运行')} value={formatUptime(runtime.uptimeSeconds)} />
              <Row
                label={t('账号')}
                value={`${text(stats.enabledAccounts)} / ${text(stats.accounts)} ${t('启用')}`}
                mono
              />
              <Row label={t('模型')} value={text(stats.models)} mono />
              <Row label={t('网关 Key 数量')} value={text(security.apiKeyCount)} mono />
            </div>
          )}

          {/* 路径：只在拿得到时显示（读不到就不占版面） */}
          {deployment.configDir || deployment.logFile ? (
            <>
              <div style={{ fontSize: 11.5, fontWeight: 600, color: 'var(--m-text-3)', padding: '14px 2px 6px' }}>
                {t('位置')}
              </div>
              <div className='m-card'>
                {deployment.configDir ? (
                  <Row label={t('数据目录')} value={text(deployment.configDir)} mono />
                ) : null}
                {deployment.dbFile ? (
                  <Row
                    label={t('数据库文件')}
                    value={`${text(deployment.dbFile)}（${formatBytes(deployment.dbBytes)}）`}
                    mono
                  />
                ) : null}
                {deployment.logFile ? (
                  <Row label={t('日志文件')} value={text(deployment.logFile)} mono />
                ) : null}
              </div>
            </>
          ) : null}

          {/* 隐藏用户名：与桌面端同一个后端参数 */}
          <button
            type='button'
            className='m-row'
            onClick={toggleHide}
            style={{ marginTop: 14, borderTop: '1px solid var(--m-line)' }}
          >
            <span className='m-logo'>
              <Icon name='info' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('隐藏用户名')}</div>
              <div className='m-t2'>
                {t('把路径里的主目录显示成 ~，截图分享时不暴露用户名')}
              </div>
            </div>
            <Badge tone={hideUsername ? 'ok' : 'mute'}>
              {hideUsername ? t('已隐藏') : t('显示')}
            </Badge>
          </button>

          {loading ? (
            <div style={{ fontSize: 12, color: 'var(--m-text-3)', padding: '10px 2px' }}>
              {t('读取中…')}
            </div>
          ) : null}
        </SheetBody>
        <SheetFooter>
          <button type='button' className='m-btn' onClick={copyAll}>
            <Icon name='copy' />
            {t('复制全部信息')}
          </button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}
