export type UpstreamAccountState = {
  enabled?: boolean
  upstreamCooldownUntil?: number | null
  upstreamBlockedReason?: string
}

export type AccountCooldown = { resetAt: number; message: string }

export function upstreamCooldownOf(account: UpstreamAccountState | null | undefined, now = Date.now()): AccountCooldown | null {
  const resetAt = Number(account?.upstreamCooldownUntil)
  if (!Number.isFinite(resetAt) || resetAt <= now || resetAt > 8.64e15) return null
  return { resetAt, message: String(account?.upstreamBlockedReason || '').trim() }
}

export function upstreamSuspendedReasonOf(account: UpstreamAccountState | null | undefined): string {
  return account?.enabled === false ? String(account.upstreamBlockedReason || '').trim() : ''
}
