import { apiCall } from './client'
import type {
  ActivityRunStatus,
  AutoState,
  BenefitKind,
  CheckinAccount,
  CheckinCenter,
  CheckinResult,
  CheckinResultRow,
  CheckinSection,
  DailyCheckinState,
  GrowthEntry,
  HistoryRow,
  KeepaliveState,
  OnboardingEntry,
  PlanEntry,
  ProviderGroup,
  RecordRow,
  RecordsPage,
  ScheduleEntry,
  TravelRunResult,
  TravelState,
} from '../../../ui-shared/checkin'
import type { BenefitRedemptionResult, CampaignClaimResult, CampaignEntry, CampaignPreview } from '../../../ui-shared/checkin-campaign-types'
import type { OfficeaceSubscriptionResult, OfficeaceSubscriptionState } from '../../../ui-shared/checkin-officeace-types'
import type { PluginTaskRow } from '../../../ui-shared/plugin-task-types'

export type {
  ActivityRunStatus,
  AutoState,
  BenefitKind,
  BenefitRedemptionResult,
  CampaignClaimResult,
  CampaignEntry,
  CampaignPreview,
  CheckinAccount,
  CheckinCenter,
  CheckinResult,
  CheckinResultRow,
  CheckinSection,
  DailyCheckinState,
  GrowthEntry,
  HistoryRow,
  KeepaliveState,
  OfficeaceSubscriptionResult,
  OfficeaceSubscriptionState,
  OnboardingEntry,
  PlanEntry,
  PluginTaskRow,
  ProviderGroup,
  RecordRow,
  RecordsPage,
  ScheduleEntry,
  TravelRunResult,
  TravelState,
}

export { CHECKIN_SECTION_OPTIONS, growthTaskCompleted, growthTaskRunnable, modesOf } from '../../../ui-shared/checkin'
export {
  accountCheckinStatus,
  activityFailureDetail,
  checkinActionLabel,
  checkinPlanProgress,
  checkinStatusLabel,
  checkinStatusTone,
  describeActivityStatus,
  rowCompleted,
  rowMessage,
  rowPending,
} from '../../../ui-shared/checkin-result'
export { isCheckinTask, pluginTaskRunning } from '../../../ui-shared/plugin-task-types'

export type SchedulePatch = {
  id?: string
  label?: string
  enabled: boolean
  time: string
  timeEnd?: string
  providers: string[]
  providerTimes?: Record<string, string>
  retryTime?: string
  retryGapMinutes?: number
  retryLimit?: number
}

export function checkinSummary(result?: CheckinResult | null): {
  succeeded: number
  skipped: number
  failed: number
} {
  if (!result) return { succeeded: 0, skipped: 0, failed: 0 }
  const succeeded = result.succeeded ?? result.active ?? 0
  const skipped = result.skipped ?? 0
  const failed = result.failedCount ?? (Array.isArray(result.failed) ? result.failed.length : Number(result.failed || 0))
  return { succeeded, skipped, failed }
}

export type CheckinRecordRange = 'today' | '7d' | '30d' | 'all'

export type CheckinRecordFilters = {
  range?: CheckinRecordRange
  outcome?: string
  kind?: string
  provider?: string
  keyword?: string
  start?: number
  end?: number
}

export type OnboardingTask = {
  key?: string
  title?: string
  group?: string
  points?: number
  done?: boolean
  blocked?: boolean
  error?: string
}

export type OnboardingResult = {
  tasks?: OnboardingTask[]
  results?: Array<{ key?: string; ok?: boolean; already?: boolean; error?: string }>
  earned?: number
  total?: number
  unclaimed?: number
  settled?: boolean
  claimed?: number
  failed?: number
  claimedPoints?: number
}

export type GrowthTask = {
  code: string
  title: string
  description?: string
  status?: string
  current?: number
  target?: number
  locked?: boolean
  supported?: boolean
  unforgeable?: boolean
}

export type GrowthResult = {
  energy?: number | null
  streak?: number | null
  tasks?: { tasks?: GrowthTask[]; error?: string }
  travel?: Record<string, unknown> | null
  school?: Record<string, unknown> | null
}

export type GrowthRun = {
  code?: string
  outcome?: string
  message?: string
  error?: string
  credit?: number
}

export type UmidState = {
  available?: boolean
  installing?: boolean
  installSupported?: boolean
  authorized?: boolean
  source?: string
  message?: string
}

const accountPath = (id: string) => `/api/accounts/${encodeURIComponent(id)}`

export function beijingDayKey(ts = Date.now()): string {
  return new Date(ts + 8 * 3_600_000).toISOString().slice(0, 10)
}

export function recordRangeStart(range?: CheckinRecordRange): number | undefined {
  if (!range || range === 'all') return undefined
  const todayKey = beijingDayKey()
  const [y, m, d] = todayKey.split('-').map(Number)
  const dayStartMs = Date.UTC(y, m - 1, d) - 8 * 3_600_000
  if (range === 'today') return dayStartMs
  const days = range === '7d' ? 6 : 29
  return dayStartMs - days * 86_400_000
}

export function claimedPlansToday(raw?: Record<string, unknown> | null): string[] {
  if (!raw || typeof raw !== 'object') return []
  const day = typeof raw.day === 'string' ? raw.day : typeof raw.date === 'string' ? raw.date : ''
  if (day && day !== beijingDayKey()) return []
  const ids = Array.isArray(raw.planIds) ? raw.planIds : Array.isArray(raw.ids) ? raw.ids : []
  return ids.filter((v): v is string => typeof v === 'string' && v.length > 0)
}

function buildRecordQuery(filters: CheckinRecordFilters & { offset?: number; limit?: number; all?: number }): string {
  const params = new URLSearchParams()
  const start = filters.start ?? recordRangeStart(filters.range)
  if (start) params.set('start', String(start))
  if (filters.end) params.set('end', String(filters.end))
  if (filters.outcome && filters.outcome !== 'all') params.set('outcome', filters.outcome)
  if (filters.kind && filters.kind !== 'all') params.set('kind', filters.kind)
  if (filters.provider && filters.provider !== 'all') params.set('provider', filters.provider)
  if (filters.keyword?.trim()) params.set('keyword', filters.keyword.trim())
  if (filters.offset !== undefined) params.set('offset', String(filters.offset))
  if (filters.limit !== undefined) params.set('limit', String(filters.limit))
  if (filters.all) params.set('all', String(filters.all))
  const str = params.toString()
  return str ? `?${str}` : ''
}

export function getFullCheckinCenter(signal?: AbortSignal): Promise<CheckinCenter> {
  return apiCall<CheckinCenter>('/api/checkin-center', { signal })
}

export function runCheckinActivity(id: string, mode: string): Promise<CheckinResultRow> {
  return apiCall<CheckinResultRow>('/api/checkin-activity', { method: 'POST', body: { id, mode } })
}

export function getCheckinActivityStatus(id: string): Promise<ActivityRunStatus> {
  return apiCall<ActivityRunStatus>(`/api/checkin-activity/status?id=${encodeURIComponent(id)}`)
}

export function resetCheckinActivity(id: string): Promise<{ cleared?: string[]; message?: string }> {
  return apiCall<{ cleared?: string[]; message?: string }>('/api/checkin-activity/reset', { method: 'POST', body: { id } })
}

export function saveAutoCheckin(enabled: boolean): Promise<AutoState> {
  return apiCall<AutoState>('/api/auto-checkin', { method: 'POST', body: { enabled } })
}

export function saveCheckinSchedule(body: SchedulePatch): Promise<AutoState> {
  return apiCall<AutoState>('/api/checkin-schedules', { method: 'POST', body })
}

export function deleteCheckinSchedule(id: string): Promise<unknown> {
  return apiCall(`/api/checkin-schedules/${encodeURIComponent(id)}/delete`, { method: 'POST' })
}

export function runCheckinSchedule(id: string): Promise<CheckinResult & AutoState> {
  return apiCall<CheckinResult & AutoState>(`/api/checkin-schedules/${encodeURIComponent(id)}/run`, { method: 'POST' })
}

export function saveKeepalive(models: string[], apiKeyId?: string | null): Promise<KeepaliveState> {
  return apiCall<KeepaliveState>('/api/checkin-keepalive', { method: 'POST', body: { models, apiKeyId } })
}

export function saveTravel(body: { enabled?: boolean; times?: string[] }): Promise<TravelState> {
  return apiCall<TravelState>('/api/travel-schedule', { method: 'POST', body })
}

export function runTravel(): Promise<TravelRunResult> {
  return apiCall<TravelRunResult>('/api/travel-schedule/run', { method: 'POST' })
}

export function getCheckinRecords(
  filters: CheckinRecordFilters & { offset: number; limit: number },
  signal?: AbortSignal,
): Promise<RecordsPage> {
  return apiCall<RecordsPage>(`/api/checkin-records${buildRecordQuery(filters)}`, { signal })
}

export function clearCheckinRecords(filters: CheckinRecordFilters, all = false): Promise<{ removed: number }> {
  return apiCall<{ removed: number }>(
    `/api/checkin-records${buildRecordQuery(all ? { all: 1 } : filters)}`,
    { method: 'DELETE' },
  )
}

export function getCampaignRewards(id: string): Promise<CampaignPreview> {
  return apiCall<CampaignPreview>(`${accountPath(id)}/campaigns`)
}

export function claimCampaignReward(id: string, campaignId: string): Promise<CampaignClaimResult> {
  return apiCall<CampaignClaimResult>(`${accountPath(id)}/campaigns/claim`, {
    method: 'POST',
    body: { campaignId },
  })
}

export function redeemCampaignBenefit(id: string, code: string): Promise<BenefitRedemptionResult> {
  return apiCall<BenefitRedemptionResult>(`${accountPath(id)}/campaigns/redeem`, {
    method: 'POST',
    body: { code },
  })
}

export function getOfficeaceSubscription(id: string): Promise<OfficeaceSubscriptionState> {
  return apiCall<OfficeaceSubscriptionState>(`${accountPath(id)}/officeace-subscription`)
}

export function activateOfficeaceSubscription(id: string, confirmAgreements: boolean): Promise<OfficeaceSubscriptionResult> {
  return apiCall<OfficeaceSubscriptionResult>(`${accountPath(id)}/officeace-subscription/activate`, {
    method: 'POST',
    body: { confirmAgreements },
  })
}

export function getOnboarding(id: string, refresh = false): Promise<OnboardingResult> {
  return apiCall<OnboardingResult>(`${accountPath(id)}/onboarding${refresh ? '?refresh=1' : ''}`)
}

export function claimOnboarding(id: string): Promise<OnboardingResult> {
  return apiCall<OnboardingResult>(`${accountPath(id)}/onboarding/claim`, { method: 'POST' })
}

export function getGrowth(id: string): Promise<GrowthResult> {
  return apiCall<GrowthResult>(`${accountPath(id)}/growth`)
}

export function runGrowthTask(id: string, code: string, mode: 'full' | 'claim'): Promise<GrowthRun> {
  return apiCall<GrowthRun>(`${accountPath(id)}/growth/task`, { method: 'POST', body: { code, mode } })
}

export function runGrowthTravel(id: string): Promise<GrowthRun> {
  return apiCall<GrowthRun>(`${accountPath(id)}/growth/travel`, { method: 'POST' })
}

export function runGrowthSchool(id: string, lottery: boolean): Promise<GrowthRun> {
  return apiCall<GrowthRun>(`${accountPath(id)}/growth/school`, { method: 'POST', body: { lottery } })
}

export function getUmid(): Promise<UmidState> {
  return apiCall<UmidState>('/api/qoder-umid')
}

export function installUmid(): Promise<UmidState> {
  return apiCall<UmidState>('/api/qoder-umid/install', { method: 'POST' })
}

export function updatePluginTaskSettings(
  provider: string,
  task: string,
  settings: { enabled?: boolean; times?: string[] },
): Promise<{ accepted?: boolean }> {
  return apiCall<{ accepted?: boolean }>('/api/plugins/tasks', {
    method: 'PATCH',
    body: { provider, task, settings },
  })
}

export function runPluginTaskNow(provider: string, task: string): Promise<{ accepted?: boolean }> {
  return apiCall<{ accepted?: boolean }>('/api/plugins/tasks/run', {
    method: 'POST',
    body: { provider, task, background: true },
  })
}
