export type UsageQuerySettings = { enabled: boolean; interval: number; retries: number; timeoutSeconds?: number }

export const USAGE_QUERY_MIN_SECONDS = 30
export const USAGE_QUERY_MAX_SECONDS = 86_400
export const DEFAULT_USAGE_INTERVAL_SECONDS = 60
export const DEFAULT_USAGE_RETRIES = 3
export const USAGE_QUERY_MAX_RETRIES = 10
export const USAGE_QUERY_DEFAULT_TIMEOUT = 60
export const USAGE_QUERY_MAX_TIMEOUT = 180

export function usageQueryOf(account: { usageQuery?: Partial<UsageQuerySettings> } | null | undefined): UsageQuerySettings {
  const config = account?.usageQuery
  const enabled = config?.enabled === undefined ? true : config.enabled === true
  const interval = Number(config?.interval) || 0
  const valid = interval >= USAGE_QUERY_MIN_SECONDS && interval <= USAGE_QUERY_MAX_SECONDS
  const retries = config?.retries
  return {
    enabled,
    interval: enabled && !valid ? DEFAULT_USAGE_INTERVAL_SECONDS : interval,
    retries: typeof retries === 'number' && Number.isInteger(retries) && retries >= 0 && retries <= USAGE_QUERY_MAX_RETRIES
      ? retries : DEFAULT_USAGE_RETRIES,
    timeoutSeconds: typeof config?.timeoutSeconds === 'number' && Number.isInteger(config.timeoutSeconds)
      && config.timeoutSeconds >= 15 && config.timeoutSeconds <= USAGE_QUERY_MAX_TIMEOUT ? config.timeoutSeconds : USAGE_QUERY_DEFAULT_TIMEOUT,
  }
}
