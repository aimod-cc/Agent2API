import type { CheckinAccount, CheckinResultRow, DailyCheckinState } from './checkin'

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

export function rowCompleted(row: CheckinResultRow): boolean {
  if (row.error?.trim()) return false
  const active = row.activity?.report?.succeeded === true || row.activity?.webConversation?.succeeded === true
  return row.mode === 'keepalive' ? active || row.activity?.pokeSucceeded === true : claimCompleted(row.claim) || active
}

export function rowPending(row: CheckinResultRow): boolean {
  if (row.error?.trim() || row.claim?.error?.trim() || rowCompleted(row)) return false
  const claim = row.claim
  if (claim?.outcome !== undefined) return claim.outcome === 'inactive' || claim.outcome === 'ineligible'
  return /没有可领取的签到活动|未下发每日积分活动|暂未给该账号下发每日积分活动|暂无可领取活动|等待重试/.test(claim?.msg ?? '')
}

export function rowMessage(row: CheckinResultRow): string {
  return row.error?.trim() || row.claim?.error?.trim() || row.claim?.msg?.trim() || row.activity?.error?.trim() || ''
}
