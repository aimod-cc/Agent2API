import { t } from '../ui-kit/src/i18n'
import type { MaintenanceItem, MaintenanceRun, MaintenanceRunStatus, MaintenanceTaskId } from './maintenance-types'

export const maintenanceLabel = (id: MaintenanceTaskId): string => id === 'credentialMaintenance' ? t('凭证维护') : t('模型目录更新')
export const sourceLabel = (source: string): string => source === 'manual' ? t('手动触发') : source === 'passive' ? t('客户端触发') : t('自动触发')
export const statusLabel = (status: MaintenanceRunStatus): string => ({
  running: t('执行中…'), completed: t('已完成'), partial: t('部分失败'), failed: t('失败'),
  skipped: t('全部跳过'), interrupted: t('执行中断'),
})[status] ?? status
export const statusTone = (status: MaintenanceRunStatus): 'success' | 'warning' | 'destructive' | 'outline' =>
  status === 'completed' ? 'success' : status === 'partial' || status === 'interrupted' ? 'warning' : status === 'failed' ? 'destructive' : 'outline'
export const resultLabel = (status: MaintenanceItem['status']): string => status === 'refreshed' ? t('成功') : status === 'failed' ? t('失败') : t('跳过')
export const stamp = (value: number | null | undefined): string => value ? new Date(value).toLocaleString() : '—'
export const duration = (value: number | null | undefined): string => value == null ? '—' : t('{n} 秒', { n: Number((value / 1000).toFixed(1)) })
export const hasFailures = (run: MaintenanceRun): boolean => run.failed > 0 || run.metadata?.status === 'failed'

export function maintenanceMessage(message: string): string {
  const known: Record<string, string> = {
    '凭证未过期或临期': t('凭证未过期或临期'),
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
  }
  return Object.prototype.hasOwnProperty.call(known, message) ? known[message] : t(message)
}

export function itemMessage(item: MaintenanceItem, taskId: MaintenanceTaskId): string {
  if (item.fixed) return t('该提供商使用固定模型清单')
  if (item.message) return maintenanceMessage(item.message)
  if (item.status === 'refreshed') return taskId === 'credentialMaintenance' ? t('已续期并确认凭证更新') : t('已获取最新模型目录')
  if (item.status === 'skipped') return t('本次无需更新，沿用现有状态')
  return t('未返回失败原因')
}

export function itemChange(item: MaintenanceItem, taskId: MaintenanceTaskId): string {
  if (item.status !== 'refreshed') return t('未变更')
  if (taskId === 'modelRefresh') return item.count == null ? '—' : t('{n} 个模型', { n: item.count })
  return item.expiresBefore && item.expiresAfter && item.expiresAfter > item.expiresBefore ? t('有效期已延长') : t('凭证已更新')
}
