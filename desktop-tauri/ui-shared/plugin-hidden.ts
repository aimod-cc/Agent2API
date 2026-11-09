import type { HiddenAccountProvider } from './plugins'
import { pluginPhaseDetail, pluginPhaseLabel } from './plugin-startup'

type Translate = (key: string, params?: Record<string, string | number>) => string
export type HiddenFilter = 'all' | 'loading' | 'missing' | 'disabled' | 'broken'
export const hiddenFailed = (item: HiddenAccountProvider) => item.reason === 'load_failed' || item.reason === 'load_timeout'
export function hiddenInFilter(item: HiddenAccountProvider, filter: HiddenFilter): boolean {
  if (filter === 'all') return true
  if (filter === 'loading') return item.reason === 'loading' || item.reason === 'not_loaded'
  if (filter === 'missing') return item.reason === 'not_installed'
  if (filter === 'disabled') return item.reason === 'pack_disabled' || item.reason === 'provider_disabled'
  return hiddenFailed(item)
}
export function hiddenLabel(item: HiddenAccountProvider, t: Translate): string {
  switch (item.reason) {
    case 'not_installed': return t('插件未安装')
    case 'pack_disabled': return t('插件包已停用')
    case 'provider_disabled': return t('提供商已停用')
    case 'load_failed': return t('加载失败')
    case 'load_timeout': return t('初始化超时')
    case 'loading': return item.startup ? pluginPhaseLabel(item.startup, t) : t('正在加载')
    default: return pluginPhaseLabel(item.startup, t)
  }
}
export function hiddenDetail(item: HiddenAccountProvider, t: Translate, now?: number): string {
  switch (item.reason) {
    case 'not_installed': return t('重新安装这个插件后自动回来')
    case 'pack_disabled': return t('启用插件包后自动回来')
    case 'provider_disabled': return t('启用这家提供商后自动回来')
    default: return pluginPhaseDetail(item.startup, t, now)
  }
}
export function hiddenAction(item: HiddenAccountProvider, t: Translate): string {
  if (item.reason === 'not_installed') return t('去安装')
  if (item.reason === 'pack_disabled' || item.reason === 'provider_disabled') return t('去启用')
  return hiddenFailed(item) || item.startup?.phase === 'blocked' ? t('查看原因') : t('查看进度')
}
