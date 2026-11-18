export type UpstreamAccountState = {
  enabled?: boolean
  upstreamCooldownUntil?: number | null
  upstreamBlockedReason?: string
  routingCooldowns?: Record<string, { resetAt?: number | null; message?: string; timeSource?: 'upstream' | 'fallback' }>
  providerCooldown?: { resetAt?: number | null; message?: string; timeSource?: 'upstream' | 'fallback' }
}

export type AccountCooldown = { resetAt: number; message: string; scope: 'account' | 'provider'; timeSource?: 'upstream' | 'fallback' }

export function upstreamCooldownOf(account: UpstreamAccountState | null | undefined, now = Date.now()): AccountCooldown | null {
  const entries = [
    { resetAt: Number(account?.upstreamCooldownUntil), message: String(account?.upstreamBlockedReason || '').trim(), scope: 'account' as const },
    { resetAt: Number(account?.routingCooldowns?.['@account']?.resetAt), message: String(account?.routingCooldowns?.['@account']?.message || '').trim(),
      scope: 'account' as const, timeSource: account?.routingCooldowns?.['@account']?.timeSource },
    { resetAt: Number(account?.providerCooldown?.resetAt), message: String(account?.providerCooldown?.message || '').trim(),
      scope: 'provider' as const, timeSource: account?.providerCooldown?.timeSource },
  ].filter(entry => Number.isFinite(entry.resetAt) && entry.resetAt > now && entry.resetAt <= 8.64e15)
  return entries.sort((left, right) => right.resetAt - left.resetAt || Number(right.scope === 'provider') - Number(left.scope === 'provider'))[0] || null
}

export function upstreamSuspendedReasonOf(account: UpstreamAccountState | null | undefined): string {
  return account?.enabled === false ? String(account.upstreamBlockedReason || '').trim() : ''
}
