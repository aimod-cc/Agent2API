import type { OnboardingCache } from './checkin-store'
import type { CheckinCenter, OnboardingTask } from './checkin-types'

export function normalizeOnboardingTasks(raw: unknown): OnboardingTask[] {
  if (!Array.isArray(raw)) return []
  return raw.flatMap(item => {
    const row = (item ?? {}) as Record<string, unknown>
    const key = typeof row.key === 'string' ? row.key : ''
    if (!key) return []
    return [{
      key,
      title: typeof row.title === 'string' && row.title ? row.title : key,
      points: Number(row.points) || 0,
      done: row.done === true,
      blocked: row.blocked === true,
      claiming: row.claiming === true,
      error: typeof row.error === 'string' ? row.error : '',
    }]
  })
}

export function currentOnboardingCache(
  provider: string | undefined,
  cache: OnboardingCache | undefined,
  now = Date.now(),
): OnboardingCache | undefined {
  if (provider !== 'officeace' || !cache?.checkedAt || cache.status === 'loading' || cache.claiming) return cache
  // 果办奖励按北京时间每日结算，不能把当天结果当作永久结清。
  const day = (stamp: number) => Math.floor((stamp + 8 * 3_600_000) / 86_400_000)
  return day(cache.checkedAt) === day(now) ? cache : undefined
}

export function onboardingSummary(cache: OnboardingCache) {
  const checked = cache.status === 'loaded' || cache.settled || Boolean(cache.checkedAt)
  const pending = checked && !cache.settled
    ? cache.tasks.filter(task => !task.done && !task.blocked).length : 0
  const claimed = checked && (cache.settled
    || cache.tasks.length > 0 && cache.tasks.every(task => task.done))
  return { checked, pending, claimed }
}

export function onboardingFromCenter(
  center: CheckinCenter,
  previous: Map<string, OnboardingCache>,
): Map<string, OnboardingCache> {
  const next = new Map<string, OnboardingCache>()
  const now = Date.now()
  for (const row of center.extras.onboarding) {
    const cached = currentOnboardingCache(row.provider, previous.get(row.id), now)
    if (cached?.status === 'loading' || cached?.claiming) {
      next.set(row.id, cached)
      continue
    }
    if (row.settled) {
      next.set(row.id, {
        status: 'loaded',
        tasks: normalizeOnboardingTasks(row.settled.tasks),
        earned: Number(row.settled.earned) || 0,
        claiming: false,
        settled: true,
        settledAt: Number(row.settled.at) || undefined,
      })
    } else if (cached && (!cached.settled || row.provider === 'officeace')) {
      next.set(row.id, cached)
    }
  }
  return next
}
