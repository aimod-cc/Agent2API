import { t } from '../ui-kit/src/i18n'
import { credentialRefreshMessage } from './plugin-login-refresh-message'
import { summarizeNetworkError } from './updates'
import type { MaintenanceItem, MaintenanceRun, MaintenanceRunStatus, MaintenanceTaskId } from './maintenance-types'

export function maintenanceLabel(id: MaintenanceTaskId): string {
  switch (id) {
    case 'credentialMaintenance': return t('凭证维护')
    case 'modelRefresh': return t('模型目录更新')
    case 'updateCheck': return t('软件检查更新')
    case 'pluginCatalogCheck': return t('社区插件检查')
    case 'magpiePluginCheck': return t('Magpie 插件检查')
  }
}

export function maintenanceDescription(id: MaintenanceTaskId): string {
  switch (id) {
    case 'credentialMaintenance': return t('仅维护过期或即将过期的凭证')
    case 'modelRefresh': return t('逐个提供商更新模型目录，模型基础信息单独列出结果')
    case 'updateCheck': return t('定期向官网检查软件新版本；与「更新设置」同步')
    case 'pluginCatalogCheck': return t('刷新社区插件目录并检查更新，支持自动升级已签名插件')
    case 'magpiePluginCheck': return t('刷新 Magpie 官方与 GitHub 清单，并检查已安装脚本插件更新')
  }
}

export function maintenanceActionLabel(id: MaintenanceTaskId): string {
  return id === 'modelRefresh' ? t('立即更新') : t('立即检查')
}

export const sourceLabel = (source: string): string => source === 'manual' ? t('手动触发') : source === 'passive' ? t('客户端触发') : t('自动触发')
export const statusLabel = (status: MaintenanceRunStatus): string => ({
  running: t('执行中…'), completed: t('已完成'), partial: t('部分失败'), failed: t('失败'),
  skipped: t('全部跳过'), interrupted: t('执行中断'),
})[status] ?? status
export const statusTone = (status: MaintenanceRunStatus): 'success' | 'warning' | 'destructive' | 'outline' =>
  status === 'completed' ? 'success' : status === 'partial' || status === 'interrupted' ? 'warning' : status === 'failed' ? 'destructive' : 'outline'
export const resultLabel = (status: MaintenanceItem['status']): string => status === 'refreshed' ? t('成功') : status === 'failed' ? t('失败') : t('跳过')
export const stamp = (value: number | null | undefined): string => value ? new Date(value).toLocaleString(typeof document === 'undefined' ? undefined : document.documentElement.lang || undefined) : '—'
export const duration = (value: number | null | undefined): string => value == null ? '—' : t('{n} 秒', { n: Number((value / 1000).toFixed(1)) })
export const hasFailures = (run: MaintenanceRun): boolean => run.failed > 0 || run.metadata?.status === 'failed'

export function maintenanceMessage(message: string): string {
  const known: Record<string, string> = {
    '凭证未过期或临期': t('凭证未过期或临期'),
    '插件正在安装或更新，暂时无法检查凭证有效期。': t('插件正在安装或更新，暂时无法检查凭证有效期。'),
    '无法检查凭证有效期，插件返回的结果无效。': t('无法检查凭证有效期，插件返回的结果无效。'),
    '登录授权已失效，需要重新登录该账号': t('登录授权已失效，需要重新登录该账号'),
    '未注册的提供商': t('未注册的提供商'),
    '该提供商不支持主动刷新凭证（没有续期手段）': t('该提供商不支持主动刷新凭证（没有续期手段）'),
    '账号已不存在，未执行维护': t('账号已不存在，未执行维护'),
    '提供商当前不可用，未执行目录更新': t('提供商当前不可用，未执行目录更新'),
    '沿用缓存，尚未到刷新时间': t('沿用缓存，尚未到刷新时间'),
    '沿用缓存或当前没有可用的目录来源': t('沿用缓存或当前没有可用的目录来源'),
    '当前没有可用的目录账号': t('当前没有可用的目录账号'),
    '指定账号不属于该提供商、未启用或没有可用凭证': t('指定账号不属于该提供商、未启用或没有可用凭证'),
    '已在设置里关闭 models.dev 模型元数据': t('已在设置里关闭 models.dev 模型元数据'),
    '任务正在执行中，请稍候': t('任务正在执行中，请稍候'),
    '刚刚已执行，请稍后重试': t('刚刚已执行，请稍后重试'),
    '执行中断，结果可能已部分生效，请先核对账号状态': t('执行中断，结果可能已部分生效，请先核对账号状态'),
    '上次执行中断，没有完整结果；请核对账号状态': t('上次执行中断，没有完整结果；请核对账号状态'),
    '维护已执行，但完整结果未能保存；请先核对账号状态，不要反复执行': t('维护已执行，但完整结果未能保存；请先核对账号状态，不要反复执行'),
    '已经是最新版本': t('已经是最新版本'),
    '插件不在官网目录中，暂无可用更新': t('插件不在官网目录中，暂无可用更新'),
    '发现新版本，手动检查不自动升级': t('发现新版本，手动检查不自动升级'),
    '发现新版本，未开启自动更新': t('发现新版本，未开启自动更新'),
    '发现新版本，插件当前已停用': t('发现新版本，插件当前已停用'),
    '发现新版本，未签名插件需手动更新': t('发现新版本，未签名插件需手动更新'),
    '发现新版本，可前往插件页更新': t('发现新版本，可前往插件页更新'),
    '更新通道已切换，即将按新通道重新检查': t('更新通道已切换，即将按新通道重新检查'),
  }
  if (Object.prototype.hasOwnProperty.call(known, message)) return known[message]
  const credential = credentialRefreshMessage(message, t)
  if (credential !== t(message)) return credential
  return summarizeNetworkError(message, t).summary
}

export function maintenanceRawDetail(message: string | null | undefined): string {
  if (!message) return ''
  return summarizeNetworkError(message, t).detail
}

export function itemMessage(item: MaintenanceItem, taskId: MaintenanceTaskId): string {
  if (item.fixed) return t('该提供商使用固定模型清单')
  if (item.message) return maintenanceMessage(item.message)
  if (item.status === 'refreshed') {
    if (taskId === 'credentialMaintenance') return t('已续期并确认凭证更新')
    if (taskId === 'modelRefresh') return t('已获取最新模型目录')
    if (taskId === 'updateCheck') return t('已完成版本检查')
    if (taskId === 'pluginCatalogCheck') return t('已完成社区插件检查与更新')
    return t('已完成 Magpie 插件检查')
  }
  if (item.status === 'skipped') return t('本次无需更新，沿用现有状态')
  return t('未返回失败原因')
}

export function itemChange(item: MaintenanceItem, taskId: MaintenanceTaskId): string {
  if (taskId === 'modelRefresh') {
    if (item.status !== 'refreshed') return t('未变更')
    return item.count == null ? '—' : t('{n} 个模型', { n: item.count })
  }
  if (taskId === 'credentialMaintenance') {
    if (item.status !== 'refreshed') return t('未变更')
    return item.expiresBefore && item.expiresAfter && item.expiresAfter > item.expiresBefore ? t('有效期已延长') : t('凭证已更新')
  }
  const label = item.changeLabel
  if (!label) return item.status === 'refreshed' ? t('已完成') : t('未变更')
  switch (label) {
    case '已是最新': return t('已是最新')
    case '检查失败': return t('检查失败')
    case '拉取失败': return t('拉取失败')
    case '自动更新失败': return t('自动更新失败')
    case '已停止支持': return t('已停止支持')
    case '信息异常': return t('信息异常')
    case '未在目录中': return t('未在目录中')
    case '已切换通道': return t('已切换通道')
  }
  const found = /^发现 (.+)$/.exec(label)
  if (found) return t('发现 {version}', { version: found[1] })
  const upgraded = /^已升至 (.+)$/.exec(label)
  if (upgraded) return t('已升至 {version}', { version: upgraded[1] })
  const updatable = /^可更新 (.+)$/.exec(label)
  if (updatable) return t('可更新 {version}', { version: updatable[1] })
  const total = /^共 (\d+) 个$/.exec(label)
  if (total) return t('共 {n} 个', { n: total[1] })
  return t(label)
}
