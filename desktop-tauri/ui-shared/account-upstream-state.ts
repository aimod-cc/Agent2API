export type UpstreamAccountState = {
  enabled?: boolean
  upstreamCooldownUntil?: number | null
  upstreamBlockedReason?: string
  routingCooldowns?: Record<string, { resetAt?: number | null; message?: string; timeSource?: 'upstream' | 'fallback'; kind?: string; balanceBaseline?: number | null }>
  providerCooldown?: { resetAt?: number | null; message?: string; timeSource?: 'upstream' | 'fallback'; kind?: string }
}

export type AccountCooldown = {
  resetAt: number
  message: string
  scope: 'account' | 'provider'
  timeSource?: 'upstream' | 'fallback'
  kind?: string
  balanceBaseline?: number | null
}

export function upstreamCooldownOf(account: UpstreamAccountState | null | undefined, now = Date.now()): AccountCooldown | null {
  const accountEntry = account?.routingCooldowns?.['@account']
  const entries: AccountCooldown[] = [
    { resetAt: Number(account?.upstreamCooldownUntil), message: String(account?.upstreamBlockedReason || '').trim(), scope: 'account' as const },
    {
      resetAt: Number(accountEntry?.resetAt),
      message: String(accountEntry?.message || '').trim(),
      scope: 'account' as const,
      timeSource: accountEntry?.timeSource,
      kind: accountEntry?.kind,
      balanceBaseline: accountEntry?.balanceBaseline ?? null,
    },
    {
      resetAt: Number(account?.providerCooldown?.resetAt),
      message: String(account?.providerCooldown?.message || '').trim(),
      scope: 'provider' as const,
      timeSource: account?.providerCooldown?.timeSource,
      kind: account?.providerCooldown?.kind,
    },
  ].filter(entry => Number.isFinite(entry.resetAt) && entry.resetAt > now && entry.resetAt <= 8.64e15)
  return entries.sort((left, right) => right.resetAt - left.resetAt || Number(right.scope === 'provider') - Number(left.scope === 'provider'))[0] || null
}

export function upstreamSuspendedReasonOf(account: UpstreamAccountState | null | undefined): string {
  return account?.enabled === false ? String(account.upstreamBlockedReason || '').trim() : ''
}
