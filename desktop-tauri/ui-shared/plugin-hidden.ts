import type { HiddenAccountProvider } from './plugins'
import { pluginPhaseDetail, pluginPhaseLabel } from './plugin-startup'

type Translate = (key: string, params?: Record<string, string | number>) => string
export type HiddenFilter = 'all' | 'loading' | 'missing' | 'disabled' | 'broken'
export const isReplacedByTemplate = (item: HiddenAccountProvider) => item.reason === 'replaced_by_template'
export const allHiddenReplacedByTemplate = (items: readonly HiddenAccountProvider[]) =>
  items.length > 0 && items.every(isReplacedByTemplate)
export function retiredTemplateKey(item: HiddenAccountProvider): string {
  return item.templateKey || (item.id === 'cline-pass' ? 'clinepass' : '')
}
export const hiddenFailed = (item: HiddenAccountProvider) => item.reason === 'load_failed' || item.reason === 'load_timeout'
export function hiddenInFilter(item: HiddenAccountProvider, filter: HiddenFilter): boolean {
  if (filter === 'all') return true
  if (filter === 'loading') return item.reason === 'loading' || item.reason === 'not_loaded'
  if (filter === 'missing') return item.reason === 'not_installed' || item.reason === 'legacy_unsupported'
  if (filter === 'disabled') return item.reason === 'pack_disabled' || item.reason === 'provider_disabled'
  return hiddenFailed(item)
}
export function hiddenLabel(item: HiddenAccountProvider, t: Translate): string {
  switch (item.reason) {
    case 'replaced_by_template': return t('已改用 API 模板')
    case 'not_installed': return t('插件未安装')
    case 'legacy_unsupported': return t('旧插件已停止支持')
    case 'pack_disabled': return t('插件包已停用')
    case 'provider_disabled': return t('提供商已停用')
    case 'load_failed': return t('加载失败')
    case 'load_timeout': return t('初始化超时')
    case 'loading': return item.startup ? pluginPhaseLabel(item.startup, t) : t('正在加载')
    default: return pluginPhaseLabel(item.startup, t)
  }
}
export function hiddenDetail(item: HiddenAccountProvider, t: Translate, now?: number, canOpenTemplate = true): string {
  switch (item.reason) {
    case 'replaced_by_template':
      return canOpenTemplate
        ? t('无需插件，请改用「{name}」API 模板填入 API Key 继续使用', { name: t(item.label) })
        : t('已改用内置「{name}」API 模板，请在桌面端或网页端「添加账号 → API 模板」填入 API Key 继续使用', { name: t(item.label) })
    case 'not_installed': return t('重新安装这个插件后自动回来')
    case 'legacy_unsupported': return t('安装对应的 WASM 插件后可重新读取已有账号')
    case 'pack_disabled': return t('启用插件包后自动回来')
    case 'provider_disabled': return t('启用这家提供商后自动回来')
    default: return pluginPhaseDetail(item.startup, t, now)
  }
}
export function hiddenAction(item: HiddenAccountProvider, t: Translate): string {
  if (item.reason === 'replaced_by_template') return t('使用 API 模板')
  if (item.reason === 'not_installed' || item.reason === 'legacy_unsupported') return t('去安装')
  if (item.reason === 'pack_disabled' || item.reason === 'provider_disabled') return t('去启用')
  return hiddenFailed(item) || item.startup?.phase === 'blocked' ? t('查看原因') : t('查看进度')
}
export function hiddenLeadText(items: readonly HiddenAccountProvider[], total: number, t: Translate): string {
  if (allHiddenReplacedByTemplate(items)) {
    return t('{providers} 家提供商、共 {total} 个旧账号。对应能力已改由软件内置「API 模板」提供，旧账号数据仍完整保留。', {
      providers: items.length, total,
    })
  }
  return t('{providers} 家提供商、共 {total} 个账号。数据仍在，只是当前没有可用的插件来执行它们。', {
    providers: items.length, total,
  })
}
export function hiddenFootText(items: readonly HiddenAccountProvider[], t: Translate): string {
  if (allHiddenReplacedByTemplate(items)) {
    return t('旧账号不会参与请求；改用 API 模板并确认无需保留后，可随时点击「移除」清理。')
  }
  return t('这里只影响「显示」：卸载或停用插件不会删除账号；重新安装、启用并成功加载后，这些账号会自动回到列表。')
}
export function retiredRemoveConfirmText(item: HiddenAccountProvider, t: Translate): string {
  return t('确定移除已下线的 {name} 旧账号（共 {n} 个）？改用「{name}」API 模板不会受到影响，此操作不可恢复。', {
    name: t(item.label), n: item.count,
  })
}
