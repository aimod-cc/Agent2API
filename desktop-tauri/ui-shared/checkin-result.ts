import type { ActivityRunStatus, ActivityStepFailure, CheckinAccount, CheckinResultRow, DailyCheckinState } from './checkin'

export type AccountCheckinStatus = 'done' | 'pending' | 'confirming' | 'inactive' | 'ineligible' | 'captcha' | 'failed'

/** 新插件的当日状态优先；旧插件仍按 checkedInToday 展示。 */
export function accountCheckinStatus(account: CheckinAccount): AccountCheckinStatus {
  if (account.checkedInToday) return 'done'
  switch (account.dailyCheckin?.status) {
    case 'confirming': return 'confirming'
    case 'inactive': return 'inactive'
    case 'ineligible': return 'ineligible'
    case 'captcha': return 'captcha'
    case 'failed': return 'failed'
    default: return 'pending'
  }
}

export function checkinStatusLabel(status: AccountCheckinStatus): string {
  switch (status) {
    case 'done': return '已签'
    case 'confirming': return '待确认'
    case 'inactive': return '暂无活动'
    case 'ineligible': return '不适用'
    case 'captcha': return '需人工处理'
    case 'failed': return '失败'
    default: return '待签'
  }
}

export function checkinStatusTone(status: AccountCheckinStatus): 'ok' | 'warn' | 'bad' | 'mute' | 'brand' {
  switch (status) {
    case 'done': return 'ok'
    case 'confirming':
    case 'captcha': return 'warn'
    case 'inactive':
    case 'ineligible': return 'mute'
    case 'failed': return 'bad'
    default: return 'brand'
  }
}

export function checkinActionLabel(account: CheckinAccount): string {
  if (account.dailyCheckin?.queryOnly) return '查询结果'
  const status = accountCheckinStatus(account)
  if (status === 'inactive' || status === 'ineligible') return '重新查询'
  return account.checkedInToday ? '重签' : '签到'
}

/** 套餐旧状态未声明 status；新的普通签到状态不套用“份套餐”的单位。 */
export function checkinPlanProgress(state?: DailyCheckinState | null): boolean {
  return Boolean(state && (state.kind === 'plans' || state.status === undefined) && state.totalCount > 0)
}

export function claimCompleted(claim: CheckinResultRow['claim']): boolean {
  if (!claim || claim.error?.trim()) return false
  if (claim.outcome !== undefined) return claim.outcome === 'claimed' || claim.outcome === 'done'
  if (claim.success === true || claim.alreadyCompleted === true) return true
  const message = claim.msg ?? ''
  return !/可能|未能|未见|失败/.test(message) && /已签到|已领取/.test(message)
}

function stepFailureText(step?: NonNullable<CheckinResultRow['activity']>['poke']): string {
  if (!step) return ''
  const rows = Array.isArray(step.failures) ? step.failures : []
  if (rows.length > 0) {
    const parts = rows.slice(0, 3).map(row => {
      const model = row.model?.trim() || ''
      const msg = (row.message ?? row.msg)?.trim() || ''
      if (model && msg) return `${model}: ${msg}`
      return model || msg
    }).filter(Boolean)
    if (parts.length > 0) {
      return rows.length > 3 ? `${parts.join('；')} 等 ${rows.length} 个模型失败` : parts.join('；')
    }
  }
  return step.msg?.trim() || ''
}

export function activityFailureDetail(activity?: CheckinResultRow['activity']): string {
  if (!activity) return ''
  const summary = activity.summary?.trim()
  if (summary) return summary
  if (activity.modelCallsSkipped && activity.error?.trim()) {
    return `未调用保活模型：${activity.error.trim()}`
  }
  const parts: string[] = []
  if (activity.poke && activity.poke.succeeded !== true) {
    const detail = stepFailureText(activity.poke)
    if (detail) parts.push(`模型保活：${detail}`)
  }
  const bothUncertain = activity.poke?.uncertain === true && activity.webConversation?.uncertain === true
  if (activity.webConversation && activity.webConversation.succeeded !== true && !bothUncertain) {
    const detail = stepFailureText(activity.webConversation)
    if (detail) parts.push(`网页会话：${detail}`)
  }
  if (parts.length > 0) return parts.join('；')
  return activity.error?.trim() || ''
}

export function rowCompleted(row: CheckinResultRow): boolean {
  if (row.error?.trim()) return false
  const kept = row.activity?.pokeSucceeded === true || row.activity?.webConversation?.succeeded === true
  if (row.mode === 'keepalive') return kept
  const active = row.activity?.report?.succeeded === true || row.activity?.webConversation?.succeeded === true
  return claimCompleted(row.claim) || active
}

export function rowPending(row: CheckinResultRow): boolean {
  if (row.error?.trim() || row.claim?.error?.trim() || rowCompleted(row)) return false
  const claim = row.claim
  if (claim?.outcome !== undefined) return claim.outcome === 'inactive' || claim.outcome === 'ineligible'
  return /没有可领取的签到活动|未下发每日积分活动|暂未给该账号下发每日积分活动|暂无可领取活动|等待重试/.test(claim?.msg ?? '')
}

export function rowMessage(row: CheckinResultRow): string {
  const base = row.error?.trim() || row.claim?.error?.trim() || row.claim?.msg?.trim() || ''
  const detail = activityFailureDetail(row.activity)
  if (!detail) return base
  if (!base || (row.mode === 'keepalive' && /未确认|请查看具体原因/.test(base))) return detail
  return base.includes(detail) ? base : `${base}（${detail}）`
}

export type ActivityStepView = {
  key: 'model' | 'web' | 'report'
  state: 'done' | 'pending' | 'idle'
  model?: string
  reason?: string
  at?: number | null
}

export type NormalizedFailure = {
  model?: string
  status?: number
  message: string
}

function normalizeFailures(rows?: ActivityStepFailure[]): NormalizedFailure[] {
  if (!Array.isArray(rows)) return []
  return rows
    .map(row => ({
      model: row.model?.trim() || undefined,
      status: typeof row.status === 'number' ? row.status : undefined,
      message: (row.message ?? row.msg)?.trim() || '',
    }))
    .filter(row => Boolean(row.model || row.message))
}

export function describeActivityStatus(status: ActivityRunStatus) {
  if (!status) return null
  const steps: ActivityStepView[] = (['model', 'web', 'report'] as const).map(key => {
    const raw = status.steps?.[key]
    const state: ActivityStepView['state'] = raw?.done
      ? 'done'
      : raw?.pending
        ? 'pending'
        : 'idle'
    return {
      key,
      state,
      model: raw?.model?.trim() || undefined,
      reason: raw?.reason?.trim() || undefined,
      at: raw?.at ?? null,
    }
  })
  const activity = status.last?.activity
  return {
    running: status.job?.running === true,
    startedAt: status.job?.startedAt ?? null,
    finishedAt: status.job?.finishedAt ?? null,
    lastAt: status.last?.at ?? null,
    hasPending: steps.some(step => step.state === 'pending'),
    steps,
    skippedError: activity?.modelCallsSkipped ? activity.error?.trim() || null : null,
    poke: activity?.poke
      ? {
          succeeded: activity.poke.succeeded === true,
          uncertain: activity.poke.uncertain === true,
          model: activity.poke.model?.trim() || undefined,
          msg: activity.poke.msg?.trim() || undefined,
          failures: normalizeFailures(activity.poke.failures),
        }
      : null,
    web: activity?.webConversation
      ? {
          succeeded: activity.webConversation.succeeded === true,
          uncertain: activity.webConversation.uncertain === true,
          model: activity.webConversation.model?.trim() || undefined,
          conversationId: activity.webConversation.conversationId?.trim() || undefined,
          msg: activity.webConversation.msg?.trim() || undefined,
          failures: normalizeFailures(activity.webConversation.failures),
        }
      : null,
    report: activity?.report
      ? {
          succeeded: activity.report.succeeded === true,
          uncertain: activity.report.uncertain === true,
          streakDays: activity.report.streakDays ?? null,
          msg: activity.report.msg?.trim() || undefined,
        }
      : null,
  }
}
