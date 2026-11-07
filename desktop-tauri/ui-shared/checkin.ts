import type { CampaignEntry } from './checkin-campaign-types'

export type DailyCheckinStatus = 'claimed' | 'done' | 'confirming' | 'inactive' | 'ineligible' | 'captcha' | 'failed'

export type DailyCheckinState = {
  at: number
  completed: boolean
  completedCount: number
  totalCount: number
  sourcesComplete: boolean
  kind?: 'plans' | 'rolling'
  until?: number | null
  status?: DailyCheckinStatus
  message?: string
  retryAt?: number | null
  queryOnly?: boolean
}

export type CheckinAccount = {
  id: string
  name: string
  available: boolean
  checkinAt: number | null
  checkedInToday: boolean
  dailyCheckin?: DailyCheckinState | null
  edition: string | null
  streakDays: number
}

export type ProviderGroup = {
  id: string
  label: string
  accounts: CheckinAccount[]
  doneCount: number
  totalCount: number
  description?: string | null
  modes?: string[] | null
  uiHooks?: string[] | null
  manualPlans?: boolean
}

export type OutOfScope = { label: string; reason: string; count: number }

export type CheckinSection = 'daily' | 'onboarding' | 'plans' | 'growth'

export const CHECKIN_SECTION_OPTIONS = [
  { value: 'daily', label: '每日签到' },
  { value: 'onboarding', label: '新手福利' },
  { value: 'plans', label: '活动福利' },
  { value: 'growth', label: '成长任务' },
] as const

export type OnboardingSettlement = {
  at: number
  tasks: OnboardingTask[]
  earned: number | null
  total: number | null
}

export type OnboardingEntry = {
  id: string
  name: string
  provider: string
  settled: OnboardingSettlement | null
}

export type OnboardingTask = {
  key: string
  title: string
  points: number
  done: boolean
  blocked: boolean
  claiming: boolean
  error: string
}

export type PlanEntry = {
  id: string
  name: string
  provider: string
  claimAt: number | null
  claimPlans: Record<string, number> | null
}

export type GrowthEntry = { id: string; name: string; provider: string }

type GrowthTaskState = {
  status?: string
  current?: number
  target?: number
  supported?: boolean
  unforgeable?: boolean
  locked?: boolean
}

export function growthTaskCompleted(task: GrowthTaskState): boolean {
  return task.status === 'completed' || (Number.isFinite(task.current) && Number.isFinite(task.target)
    && Number(task.target) > 0 && Number(task.current) >= Number(task.target))
}

export function growthTaskRunnable(task: GrowthTaskState, mode: 'full' | 'claim'): boolean {
  if (!task.supported || task.locked || task.status === 'claimed') return false
  const completed = growthTaskCompleted(task)
  return mode === 'claim' ? completed : !task.unforgeable || completed
}


/** 条目里的一家：生效时刻 + 今天跑过没有（`builtin` 有值 = 该家自带专属时刻） */
export type ScheduleProviderRow = {
  id: string
  label: string
  time: string
  timeEnd?: string
  builtin: string | null
  firedToday: boolean
  accountCount?: number
  attemptedCount?: number
  pendingCount?: number
  nextAccountAt?: number | null
  /** 到点了但还没跑（`false` 且 `firedToday=false` = 在等窗口） */
  overdue?: boolean
}

export type ScheduleEntry = {
  id: string
  label: string
  displayLabel: string
  enabled: boolean
  time: string
  /** 随机签到区间的结束时刻，与 time 同属北京时间当天 */
  timeEnd?: string
  providers: string[]
  /** 逐家的时刻覆盖（`{providerId: "HH:MM"}`；空串 = 撤销覆盖） */
  providerTimes: Record<string, string>
  /** 逐家的生效时刻与今日状态（后端算好，界面直接渲染） */
  providerRows: ScheduleProviderRow[]
  /** 补签轮时刻（空串 = 关掉补签轮） */
  retryTime: string
  /** 逐家的补签时刻（主签到区间结束 + 等待间隔，不早于 retryTime） */
  providerRetryTimes?: Record<string, string>
  /** 中性未领取的重试间隔（分钟；0 = 不重试） */
  retryGapMinutes: number
  /** 每账号每天的重试次数上限（0 = 不重试） */
  retryLimit: number
  lastFiredDate: string | null
  lastFiredToday: boolean
  /** 今天还没跑的家（界面显示「待执行」） */
  pendingProviders: string[]
  /** 中性未领取、等窗口重试的账号数 */
  retryCount: number
  /**
   * 今天重试次数已用完的账号 id（后端按 `retryLimit` 记账算出来的）。
   * 这些账号已经从 `retryCount` 里摘掉了，界面必须单独说一句
   * 「今天不再重试」，否则数字归零看起来像「没事了」。
   */
  retryExhausted?: string[]
  nextRunAt: number | null
  lastResult: CheckinResult | null
}

export type CheckinResultRow = {
  id?: string
  name?: string
  error?: string | null
  mode?: string
  claim?: {
    success?: boolean
    alreadyCompleted?: boolean
    outcome?: string
    retryAt?: number | null
    msg?: string
    error?: string
    points?: number
  } | null
  activity?: {
    error?: string
    pokeSucceeded?: boolean
    report?: { succeeded?: boolean }
    webConversation?: { succeeded?: boolean }
  } | null
}

export type CheckinResult = {
  at?: number
  reason?: string
  succeeded?: number
  active?: number
  total?: number
  skipped?: number
  failedCount?: number
  failed?: string[] | number
  results?: CheckinResultRow[]
  retryAccounts?: Record<string, number>
  outcome?: 'completed' | 'failed'
  error?: string
  accounts?: number
  credit?: number
  actions?: TravelAction[]
}

export type TravelAction = {
  accountId?: string
  name?: string
  outcome?: string
  message?: string
  credit?: number
}

export type AutoState = {
  enabled: boolean
  lastFiredToday: boolean
  running: boolean
  providerOptions: { id: string; label: string; builtinTime?: string | null }[]
  /** 缺省补签时刻（「补签时刻」输入框的 placeholder） */
  defaultRetryTime?: string
  schedules: ScheduleEntry[]
}

export type KeepaliveState = {
  models: string[]
  defaultModels: string[]
  apiKeyId?: string | null
  /** 没显式选过、用的是第一把可用 Key（界面可以提示一句） */
  apiKeyDefaulted?: boolean
  authError?: string | null
}

export type TravelState = {
  enabled: boolean
  available?: boolean
  pausedReason?: string | null
  times: string[]
  firedToday: string[]
  lastFiredDate: string | null
  lastFiredTime: string | null
  lastResult: CheckinResult | null
  running: boolean
}

export type TravelRunResult = {
  outcome: 'completed' | 'skipped'
  message?: string
  summary: CheckinResult | null
  schedule: TravelState
}

export type HistoryRow = {
  at: number
  date?: string
  reason?: string
  succeeded?: number
  total?: number
  skipped?: number
  failed?: number
  failedCount?: number
}

export type RecordRow = {
  rowId?: number
  ts: number
  date?: string
  accountId?: string
  accountName?: string
  /** 展示名（后端按 provider id 换过） */
  provider?: string
  /** 提供商 id（筛选器的取值，历史记录里被移除的提供商也靠它还原） */
  providerId?: string
  source?: string
  outcome?: string
  reason?: string
  message?: string
  error?: string
  keptActive?: boolean
}

export type CheckinCenter = {
  daily: {
    providers: ProviderGroup[]
    outOfScope: OutOfScope[]
    todayDone: number
    todayEligible: number
  }
  extras: {
    onboarding: OnboardingEntry[]
    plans: PlanEntry[]
    campaigns?: CampaignEntry[]
    growth: GrowthEntry[]
  }
  auto: AutoState
  keepalive: KeepaliveState
  travel: TravelState
  history: HistoryRow[]
  adminPanels: string[]
  checkinFeatures: Record<string, {
    description?: string | null
    modes?: string[] | null
    uiHooks?: string[] | null
  }>
}

export type RecordsPage = {
  records: RecordRow[]
  total: number
}

export function modesOf(group: ProviderGroup | undefined): string[] {
  const modes = group?.modes
  if (!Array.isArray(modes) || modes.length === 0) return ['checkin']
  return modes.filter(mode => typeof mode === 'string' && mode)
}
