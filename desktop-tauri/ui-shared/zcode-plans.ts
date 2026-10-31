import type { DailyCheckinState } from './checkin'

export type Entitlement = {
  showName?: string
  unitType?: string
  grantUnits?: number
  period?: string
  effectiveAt?: number | null
}

export type ClaimState = 'claimable' | 'claimed' | 'unknown' | 'ineligible'
export type PlanSource = 'preview' | 'marketing' | 'catalog' | 'ledger'

export type OwnedPlan = {
  planId?: string
  userPlanId?: string | null
  name?: string
  state?: string
  startsAt?: number | null
  endsAt?: number | null
  pendingUntil?: number | null
  entitlements?: Entitlement[]
}

export type PreviewPlan = {
  planId?: string
  name?: string
  description?: string
  priority?: number
  startsAt?: number | null
  endsAt?: number | null
  entitlements?: Entitlement[]
  claimed?: boolean
  claimedAt?: number | null
  claimState?: ClaimState
  claimMessage?: string | null
  claimStartsAt?: number | null
  claimEndsAt?: number | null
  source?: PlanSource
  sources?: PlanSource[]
  ownedPlan?: OwnedPlan | null
}

export type PreviewResult = {
  deployed?: boolean
  plans?: PreviewPlan[]
  activated?: boolean | null
  activationError?: string | null
  previewError?: string | null
  marketingError?: string | null
  catalogError?: string | null
  ownershipError?: string | null
  ledgerError?: string | null
  dailyCheckin?: DailyCheckinState | null
}

export type ClaimResult = {
  ok?: boolean
  failure?: string
  failureLabel?: string
  message?: string
  planId?: string
  claimedAt?: number
  startsAt?: number | null
  endsAt?: number | null
  marketingError?: string | null
  dailyCheckin?: DailyCheckinState | null
}

/** 日期汇总不能决定套餐是否已领；同一套餐跨日仍按领取单元确认。 */
export function planClaimState(plan: PreviewPlan, settled: readonly string[] = []): ClaimState {
  if (settled.includes(String(plan.planId || ''))) return 'claimed'
  if (plan.claimState) return plan.claimState
  return plan.claimed ? 'claimed' : 'claimable'
}

export function planClaimResultApplies(plan: PreviewPlan, result: ClaimResult | undefined): boolean {
  if (!result) return false
  if (!result.ok && result.failure !== 'already_claimed') return true
  const start = Number(plan.startsAt)
  const end = Number(result.endsAt)
  const at = Number(result.claimedAt) / 1000
  if (!Number.isFinite(start) || start <= 0) return true
  if (Number.isFinite(end) && end > 0) return end > start
  return Number.isFinite(at) && at > 0 && at >= start
}

export function planQueryErrors(result: PreviewResult): string[] {
  return [result.previewError, result.marketingError, result.catalogError, result.ownershipError, result.ledgerError]
    .filter((error): error is string => Boolean(error))
}
