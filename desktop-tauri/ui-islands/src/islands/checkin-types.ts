export type {
  AutoState, CheckinAccount, CheckinCenter, CheckinResult, CheckinSection, GrowthEntry, HistoryRow,
  KeepaliveState, OnboardingEntry, OnboardingSettlement, OnboardingTask, OutOfScope, PlanEntry,
  ProviderGroup, RecordsPage, RecordRow, ScheduleEntry, TravelAction,
  TravelRunResult, TravelState,
} from '../../../ui-shared/checkin'
export { CHECKIN_SECTION_OPTIONS, modesOf } from '../../../ui-shared/checkin'

/** 档位 → 按钮文案（`checkin` 是兜底档，不是插件申报值） */
export const MODE_LABEL: Record<string, string> = {
  checkin: '签到',
  keepalive: '保活',
  claim: '领取',
  full: '保活 + 领取',
}

/** 档位 → 悬停说明 */
export const MODE_TITLE: Record<string, string> = {
  checkin: '走这一家的每日签到接口',
  keepalive: '用免费模型与网页会话维持账号活跃 · 不领取奖励',
  claim: '只领取当天的奖励 · 不做保活',
  full: '保活并领取奖励',
}

/** 签到来源 → 展示名 */
export const SOURCE_LABEL: Record<string, string> = {
  schedule: '定时',
  makeup: '补签',
  startup: '补签',
  manual: '手动',
  single: '单签',
  activity: '活动',
}

/** 结果 → 展示名与色调 */
export const OUTCOME_VIEW: Record<string, { label: string; variant: 'success' | 'brand' | 'warning' | 'destructive' | 'outline' }> = {
  success: { label: '成功领取', variant: 'success' },
  already: { label: '今日已领', variant: 'brand' },
  pending: { label: '未领取', variant: 'warning' },
  captcha: { label: '需要人工验证', variant: 'warning' },
  failed: { label: '失败', variant: 'destructive' },
}

/** 毫秒时间戳 → 本地可读时间；缺失 / 非法给 `—` */
export function formatTime(value: unknown): string {
  const stamp = Number(value)
  if (!Number.isFinite(stamp) || stamp <= 0) return '—'
  const date = new Date(stamp)
  if (Number.isNaN(date.getTime())) return '—'
  return date.toLocaleString('zh-CN', { hour12: false })
}

/** 毫秒 → `HH:MM`（只关心当天时刻的场合） */
export function formatClock(value: unknown): string {
  const stamp = Number(value)
  if (!Number.isFinite(stamp) || stamp <= 0) return ''
  const date = new Date(stamp)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/** 毫秒 → `MM-DD HH:MM`（记录表的时间列） */
export function formatShort(value: unknown): string {
  const stamp = Number(value)
  if (!Number.isFinite(stamp) || stamp <= 0) return '—'
  const date = new Date(stamp)
  if (Number.isNaN(date.getTime())) return '—'
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}

declare global {
  interface Window {
    wbApp?: {
      showPage?: (name: string) => void
      toast?: (message: string, tone?: string) => void
      readonly currentPage?: string
    }
    wbZcodePlans?: { open(options: { id: string; name?: string }): void }
  }
}
