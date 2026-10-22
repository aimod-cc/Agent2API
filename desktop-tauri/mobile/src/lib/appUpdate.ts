/**
 * 手机端应用更新。
 *
 * ── 三种形态，三条路 ────────────────────────────────────────────
 *   · **PWA / 浏览器**：更新走 Service Worker（见 lib/pwa.ts），这里不参与。
 *     浏览器里的「新版本」是网关托管的 `ui/m/*` 变了，刷新即可。
 *   · **APK**：界面打在包里，必须换安装包。这条路是这里实现的：
 *       1. 调网关 `GET /api/update/check` 拿最新版本与 `androidAsset`；
 *       2. 用原生插件**直连 GitHub** 下载 APK（手机自己出网）；
 *       3. 调起系统安装器。
 *     下载地址是 GitHub 的 `browser_download_url`，不经过网关中转 ——
 *     手机出不了网时用户自己开代理即可，网关不替它代劳。
 *   · **iOS（IPA）**：苹果不允许 App 自行下载并安装可执行代码，所以这里
 *     **只能跳转** —— 拿 `pageUrl` 打开 GitHub Release 页面，由用户自己走
 *     TestFlight / App Store / AltStore。`downloadApk` / `installApk` 在 iOS
 *     上直接报错，界面也据此不渲染那几个按钮（见 AppUpdatePanel）。
 *
 * ── 为什么下载走原生而不是 fetch ────────────────────────────────
 * APK 有几十 MB，`fetch` + `blob` 会把整包读进 WebView 内存；`CapacitorHttp`
 * 也一样（它把响应体整个序列化回来）。原生插件用 HttpURLConnection 边读边
 * 写文件，内存占用只有缓冲区大小，还能实时上报进度。
 *
 * ── 版本号从哪来 ────────────────────────────────────────────────
 * `__APP_VERSION__` 是构建期由 vite 从 `mobile/package.json` 注入的常量
 * （见 vite.config.ts），与 APK 的 `versionName`（android/variables.gradle
 * 读同一个字段）永远一致；iOS 的 `MARKETING_VERSION` 由 CI 从同一个字段写进
 * xcodebuild 参数，所以两边不会漂。
 *
 * ── 自动检查 ────────────────────────────────────────────────────
 * 间隔与开关**读电脑端的「软件版本检查」任务配置**（`/api/scheduled-tasks`
 * 里的 `updateCheck` 条目），不另起一套设置 —— 用户在一处配好，两端一致。
 * 手机端本地只留一个「本机不自动检查」的开关（存在 localStorage），
 * 用于「我不想让手机耗流量」这种纯本地偏好。
 */

import { apiCall } from '../api/client'
import { nativePlatform } from '../lib/servers'

/** 构建期注入的本工程版本号 */
declare const __APP_VERSION__: string

export const APP_VERSION: string = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '0.0.0'

/** 更新检查结果里手机端关心的那部分 */
export type ApkUpdateInfo = {
  currentVersion: string
  latestVersion: string | null
  hasUpdate: boolean | null
  notes: string
  publishedAt: string
  pageUrl: string
  prerelease: boolean
  /** 这一版挂的 APK 资产；null = 该版本没提供 Android 包 */
  asset: { name?: string; size?: number; url?: string } | null
}

type RawCheckResult = {
  currentVersion?: string | null
  latestVersion?: string | null
  hasUpdate?: boolean | null
  notes?: string | null
  publishedAt?: string | null
  pageUrl?: string | null
  prerelease?: boolean
  androidAsset?: { name?: string; size?: number; url?: string } | null
}

/**
 * 检查更新。
 *
 * 传本机版本号而不是让服务端猜：网关可能跑在电脑上、版本与手机端不同，
 * 「手机端有没有新版本」只有手机自己知道当前是哪个版本。
 *
 * 走网关的 `/api/update/check` 而不是手机直接打 GitHub API：这样能**复用
 * 电脑端已配置的 GitHub 令牌**（匿名限额 60 次/小时，带令牌 5000），
 * 也不必让手机再存一份凭证。注意这里只复用「查询」，下载仍是手机直连。
 */
export async function checkApkUpdate(): Promise<ApkUpdateInfo> {
  const raw = await apiCall<RawCheckResult>(
    `/api/update/check?current=${encodeURIComponent(APP_VERSION)}`,
  )
  const asset = raw.androidAsset && raw.androidAsset.url ? raw.androidAsset : null
  return {
    currentVersion: APP_VERSION,
    latestVersion: raw.latestVersion || null,
    // 有新版但没挂 APK 时，对手机端来说等于「没有可更新的包」：
    // 不谎报「有更新」把用户引到一个点不动的按钮上。
    hasUpdate: raw.hasUpdate === true && asset !== null ? true : raw.hasUpdate === false ? false : null,
    notes: raw.notes || '',
    publishedAt: raw.publishedAt || '',
    pageUrl: raw.pageUrl || '',
    prerelease: Boolean(raw.prerelease),
    asset,
  }
}

/* ─── 自动检查的配置（读电脑端） ─────────────────────────────── */

/** 手机端本地的「不自动检查」开关（纯本地偏好，不上报） */
const AUTO_CHECK_KEY = 'agent2api.mobile.updateAutoCheck'

/** 本机是否允许自动检查（默认允许） */
export function autoCheckEnabled(): boolean {
  try {
    return localStorage.getItem(AUTO_CHECK_KEY) !== 'off'
  } catch {
    return true
  }
}

export function setAutoCheckEnabled(enabled: boolean): void {
  try {
    if (enabled) localStorage.removeItem(AUTO_CHECK_KEY)
    else localStorage.setItem(AUTO_CHECK_KEY, 'off')
  } catch {
    /* 隐私模式：只在本页生效 */
  }
}

/**
 * 从电脑端的「软件版本检查」任务读检查间隔（毫秒）。
 *
 * 电脑端把这条任务藏在「更新设置」弹窗里（定时任务页不渲染它），但配置本身
 * 仍由 `/api/scheduled-tasks` 返回 —— 手机端直接读同一个来源，不另做设置页。
 * 任务被关掉时返回 0（= 不自动检查），与电脑端语义一致。
 *
 * 读不到（老网关 / 请求失败）时退回 20 分钟：那是电脑端的缺省值
 * （`DEFAULT_UPDATE_CHECK_MINUTES`），行为与「没配过」一致。
 */
export async function autoCheckIntervalMs(): Promise<number> {
  try {
    const payload = await apiCall<{
      tasks?: Array<{ id?: string; enabled?: boolean; interval?: number; unit?: string }>
    }>('/api/scheduled-tasks')
    const task = (payload.tasks || []).find(item => item.id === 'updateCheck')
    if (!task) return 20 * 60_000
    if (task.enabled === false) return 0
    const interval = Number(task.interval) || 0
    if (interval <= 0) return 20 * 60_000
    // 电脑端这条任务的单位是分钟（见 scheduled_tasks.rs 的 TaskDef）
    return interval * 60_000
  } catch {
    return 20 * 60_000
  }
}

/* ─── 原生插件（只在 APK 里存在） ─────────────────────────────── */

type PluginListenerHandle = { remove: () => Promise<void> }

type ApkUpdaterPlugin = {
  download(options: { url: string }): Promise<{ path: string; size: number }>
  cancel(): Promise<void>
  canInstall(): Promise<{ allowed: boolean }>
  openInstallPermissionSettings(): Promise<void>
  install(): Promise<void>
  downloaded(): Promise<{ exists: boolean; size: number }>
  addListener(
    event: 'downloadProgress',
    handler: (data: { received: number; total: number; percent: number }) => void,
  ): Promise<PluginListenerHandle>
}

type CapacitorGlobal = {
  isNativePlatform?: () => boolean
  Plugins?: Record<string, unknown>
}

/** 取原生插件；不在 APK 里（或插件没注册）时返回 null */
function plugin(): ApkUpdaterPlugin | null {
  // iOS 没有这个插件（也不可能有：苹果不允许 App 自装可执行代码）
  if (nativePlatform() !== 'android') return null
  const capacitor = (window as unknown as { Capacitor?: CapacitorGlobal }).Capacitor
  const found = capacitor?.Plugins?.ApkUpdater
  return (found as ApkUpdaterPlugin | undefined) || null
}

/** 当前壳能不能在应用内下载并安装安装包（只有 Android 能） */
export function canSelfUpdate(): boolean {
  return nativePlatform() === 'android' && plugin() !== null
}

export type DownloadProgress = {
  received: number
  total: number
  /** 0–100；总长度未知时为 -1 */
  percent: number
}

/**
 * 下载 APK 到应用缓存。返回下载完成后的本地路径。
 *
 * `url` 必须是 GitHub 的资产地址（由 `checkApkUpdate` 的 `asset.url` 给出）。
 * 手机自己出网 —— 连不上 GitHub 时用户自己开代理，不在这里做中转。
 */
export async function downloadApk(
  url: string,
  onProgress?: (progress: DownloadProgress) => void,
): Promise<{ path: string; size: number }> {
  const api = plugin()
  if (!api) {
    // iOS 与浏览器都会走到这里：两者都不允许应用内自装安装包
    throw new Error(
      nativePlatform() === 'ios'
        ? 'iOS 不支持应用内安装，请到 GitHub Release 页面下载'
        : '当前不是 APK 形态，无法在应用内更新',
    )
  }
  if (!url) throw new Error('缺少下载地址')

  let handle: PluginListenerHandle | null = null
  if (onProgress) {
    handle = await api.addListener('downloadProgress', data => onProgress(data))
  }
  try {
    return await api.download({ url })
  } finally {
    if (handle) void handle.remove()
  }
}

export async function cancelDownload(): Promise<void> {
  await plugin()?.cancel()
}

/** 是否已获「安装未知来源应用」授权 */
export async function canInstallApk(): Promise<boolean> {
  const api = plugin()
  if (!api) return false
  const result = await api.canInstall()
  return Boolean(result.allowed)
}

/** 跳到系统的「安装未知应用」授权页 */
export async function openInstallPermission(): Promise<void> {
  await plugin()?.openInstallPermissionSettings()
}

/** 调起系统安装器（APK 已下载时） */
export async function installApk(): Promise<void> {
  const api = plugin()
  if (!api) {
    throw new Error(
      nativePlatform() === 'ios'
        ? 'iOS 不支持应用内安装，请到 GitHub Release 页面下载'
        : '当前不是 APK 形态，无法安装',
    )
  }
  await api.install()
}

/** 缓存里是否已有下载好的安装包（重进设置页时显示「已下载，待安装」） */
export async function hasDownloadedApk(): Promise<{ exists: boolean; size: number }> {
  const api = plugin()
  if (!api) return { exists: false, size: 0 }
  return api.downloaded()
}
