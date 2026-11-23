import { Capacitor, registerPlugin } from '@capacitor/core'
import { apiCall } from './client'
import { nativePlatform } from './servers'

declare const __APP_VERSION__: string

export const APP_VERSION: string =
  typeof __APP_VERSION__ === 'string' && __APP_VERSION__ ? __APP_VERSION__ : '3.5.0-beta.8'

const UPDATE_WEBSITE = 'https://agent2api.dev'
const UPDATE_CDN = 'https://cdn.agent2api.dev'

function isAllowedWebsiteDownload(value: string): boolean {
  try {
    const url = new URL(value)
    if (url.username || url.password || url.search || url.hash) return false
    if (url.origin === UPDATE_WEBSITE) return url.pathname.startsWith('/downloads/')
    if (url.origin === UPDATE_CDN) {
      return (
        url.pathname.startsWith('/agent2api/installers/') ||
        url.pathname.startsWith('/agent2api/plugins/')
      )
    }
    return false
  } catch {
    return false
  }
}

export function resolveUpdatePageUrl(
  value?: string | null,
  options?: {
    platform?: 'android' | 'ios' | 'web'
    version?: string | null
    prerelease?: boolean
  },
): string {
  let target: URL
  try {
    const parsed = value ? new URL(value) : new URL(`${UPDATE_WEBSITE}/downloads`)
    target =
      parsed.origin === UPDATE_WEBSITE && !parsed.username && !parsed.password && !parsed.hash
        ? parsed
        : new URL(`${UPDATE_WEBSITE}/downloads`)
  } catch {
    target = new URL(`${UPDATE_WEBSITE}/downloads`)
  }
  if (target.pathname === '/downloads' || target.pathname === '/downloads/') {
    if (
      (options?.platform === 'android' || options?.platform === 'ios') &&
      !target.searchParams.has('platform')
    ) {
      target.searchParams.set('platform', options.platform)
    }
    if (options?.version && !target.searchParams.has('version')) {
      const normalized = options.version.replace(/^v/i, '')
      if (normalized) {
        target.searchParams.set('version', `v${normalized}`)
      }
    }
    if (options?.version && typeof options.prerelease === 'boolean' && !target.searchParams.has('channel')) {
      target.searchParams.set('channel', options.prerelease ? 'beta' : 'stable')
    }
  }
  return target.href
}

export type MobileUpdateAsset = { name?: string; size?: number; url?: string }

export type MobileUpdateInfo = {
  currentVersion: string
  latestVersion: string | null
  hasUpdate: boolean | null
  notes: string
  publishedAt: string
  pageUrl: string
  prerelease: boolean
  asset: MobileUpdateAsset | null
}

export type ApkUpdateInfo = MobileUpdateInfo

type RawCheckResult = {
  updateSource?: string
  latestVersion?: string | null
  hasUpdate?: boolean | null
  notes?: string | null
  publishedAt?: string | null
  pageUrl?: string | null
  prerelease?: boolean
  androidAsset?: MobileUpdateAsset | null
  iosAsset?: MobileUpdateAsset | null
  checked?: boolean
  channelChanged?: boolean
}

function normalizeUpdateResult(raw: RawCheckResult): MobileUpdateInfo {
  const platform = nativePlatform()
  const rawAsset =
    platform === 'ios'
      ? raw.iosAsset
      : platform === 'android'
        ? raw.androidAsset
        : null
  if (rawAsset?.url && !isAllowedWebsiteDownload(rawAsset.url)) {
    throw new Error('官网安装包校验地址不合法，请重新检查')
  }
  const asset = rawAsset && rawAsset.url ? rawAsset : null
  const prerelease = Boolean(raw.prerelease)
  const latestVersion = raw.latestVersion || null
  const hasUpdate =
    platform === 'web'
      ? raw.hasUpdate === true
        ? true
        : raw.hasUpdate === false
          ? false
          : null
      : raw.hasUpdate === true && asset !== null
        ? true
        : raw.hasUpdate === false
          ? false
          : null
  return {
    currentVersion: APP_VERSION,
    latestVersion,
    hasUpdate,
    notes: raw.notes || '',
    publishedAt: raw.publishedAt || '',
    pageUrl: resolveUpdatePageUrl(raw.pageUrl, {
      platform,
      version: latestVersion,
      prerelease,
    }),
    prerelease,
    asset,
  }
}

export async function getCachedMobileUpdate(
  signal?: AbortSignal,
): Promise<MobileUpdateInfo | null> {
  const raw = await apiCall<RawCheckResult>(
    `/api/update/status?current=${encodeURIComponent(APP_VERSION)}`,
    { quiet: true, signal },
  )
  if (!raw.checked || raw.channelChanged) return null
  return normalizeUpdateResult(raw)
}

export async function checkMobileUpdate(): Promise<MobileUpdateInfo> {
  const raw = await apiCall<RawCheckResult>(
    `/api/update/check?current=${encodeURIComponent(APP_VERSION)}`,
  )
  if (raw.channelChanged && raw.checked === false) {
    throw new Error('更新通道已变化，请再次点击检查')
  }
  return normalizeUpdateResult(raw)
}

export const checkApkUpdate = checkMobileUpdate

type PluginListenerHandle = { remove: () => Promise<void> }

type ApkUpdaterPlugin = {
  download(options: { url: string; sha256: string; size: number }): Promise<{
    path: string
    size: number
  }>
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

const ApkUpdater = registerPlugin<ApkUpdaterPlugin>('ApkUpdater')

function updaterPlugin(): ApkUpdaterPlugin | null {
  if (nativePlatform() !== 'android' || !Capacitor.isPluginAvailable('ApkUpdater')) return null
  return ApkUpdater
}

export function canSelfUpdate(): boolean {
  return updaterPlugin() !== null
}

export type DownloadProgress = {
  received: number
  total: number
  percent: number
}

export async function hasDownloadedApk(): Promise<{ exists: boolean; size: number }> {
  const api = updaterPlugin()
  if (!api) return { exists: false, size: 0 }
  return api.downloaded().catch(() => ({ exists: false, size: 0 }))
}

export async function downloadApk(
  url: string,
  onProgress?: (progress: DownloadProgress) => void,
): Promise<{ path: string; size: number }> {
  const api = updaterPlugin()
  if (!api) {
    throw new Error(
      nativePlatform() === 'ios'
        ? 'iOS 端请点击打开官网页面获取更新'
        : '当前运行在网页端，刷新页面即可获取最新界面',
    )
  }
  if (!isAllowedWebsiteDownload(url)) {
    throw new Error('更新包必须来自官网可信地址')
  }
  const plan = await apiCall<{ url: string; sha256: string; size: number }>(
    '/api/update/download-plan',
    { method: 'POST', body: { url } },
  )
  if (
    plan.url !== url ||
    !isAllowedWebsiteDownload(plan.url) ||
    !/^[0-9a-fA-F]{64}$/.test(plan.sha256 || '') ||
    !Number.isSafeInteger(plan.size) ||
    plan.size <= 0
  ) {
    throw new Error('安装包校验摘要无效，已中止下载')
  }

  let handle: PluginListenerHandle | null = null
  if (onProgress) {
    handle = await api.addListener('downloadProgress', data => onProgress(data))
  }
  try {
    onProgress?.({ received: 0, total: plan.size, percent: 0 })
    return await api.download(plan)
  } finally {
    if (handle) void handle.remove()
  }
}

export async function cancelApkDownload(): Promise<void> {
  await updaterPlugin()?.cancel().catch(() => {})
}

export async function installDownloadedApk(): Promise<'needs_permission' | 'launched'> {
  const api = updaterPlugin()
  if (!api) throw new Error('当前平台不支持应用内安装')
  const { allowed } = await api.canInstall()
  if (!allowed) {
    await api.openInstallPermissionSettings()
    return 'needs_permission'
  }
  await api.install()
  return 'launched'
}
