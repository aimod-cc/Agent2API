export type ZcodeCaptchaProvider = 'zcode' | 'zcode-intl'
export type ZcodeCaptchaBinding = { provider: ZcodeCaptchaProvider; accountId: string; configId: string }
export type ZcodeCaptchaConfig = {
  enabled?: boolean
  engine?: string
  provider?: string
  accountId?: string
  configId?: string
  prefix?: string
  sceneId?: string
  region?: string
  skipModelRequest?: boolean
}
export type ZcodeCaptchaPoolStats = {
  ready?: number
  target?: number
  required?: boolean
  needsTokens?: boolean
  demandActive?: boolean
  startPlanAccounts?: number
  configError?: string
  error?: string
  provider?: string
  accountId?: string
  captchaAccountId?: string | null
  lastSyncAt?: number
  rejected?: number
  minted?: number
  consumed?: number
  ttlMs?: number
}
export type ZcodeCaptchaPush = ZcodeCaptchaBinding & { tokens: Array<{ param: string }> }
export type ZcodeClaimPayload = {
  planId: string
  captchaVerifyParam: string
  captchaRegion: string
} & (ZcodeCaptchaBinding | { provider?: never; accountId?: never; configId?: never })

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function text(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0 && !/[\u0000-\u001f\u007f]/.test(value)
}

export function isZcodeCaptchaProvider(value: unknown): value is ZcodeCaptchaProvider {
  return value === 'zcode' || value === 'zcode-intl'
}

/** 仅完全没有绑定字段的旧原生配置可走兼容分支；部分绑定不能降级。 */
export function zcodeCaptchaBinding(value: unknown, accountId: string): ZcodeCaptchaBinding | null {
  if (!text(accountId) || !object(value) || typeof value.enabled !== 'boolean') throw new Error('invalid_captcha_config')
  if (value.enabled && (!text(value.prefix) || !text(value.sceneId) || !text(value.region))) throw new Error('invalid_captcha_config')
  const bound = ['provider', 'accountId', 'configId', 'engine'].some(key => Object.prototype.hasOwnProperty.call(value, key))
  if (!bound) return null
  if (!isZcodeCaptchaProvider(value.provider) || value.accountId !== accountId || !text(value.configId)
    || (value.enabled ? value.engine !== 'aliyun' : value.engine !== 'none' && value.engine !== 'aliyun')) {
    throw new Error('invalid_captcha_binding')
  }
  return { provider: value.provider, accountId, configId: value.configId }
}

/** 显式账号查询不能接受概览中别的代表账号，也不能接受不完整的新绑定。 */
export function zcodeCaptchaStatsMatch(value: unknown, accountId: string, provider?: string, requireBinding = false): value is ZcodeCaptchaPoolStats {
  if (!object(value)) return false
  const bound = ['provider', 'accountId'].some(key => Object.prototype.hasOwnProperty.call(value, key))
  if (bound || requireBinding) {
    if (!isZcodeCaptchaProvider(value.provider) || value.accountId !== accountId || value.captchaAccountId !== accountId) return false
    if (provider !== undefined && value.provider !== provider) return false
  } else if (value.captchaAccountId !== undefined && value.captchaAccountId !== null && value.captchaAccountId !== accountId) {
    return false
  }
  return true
}

export function zcodeCaptchaAccepted(value: unknown, binding: ZcodeCaptchaBinding | null): boolean {
  if (!object(value) || value.accepted !== 1) return false
  return binding === null || value.provider === binding.provider && value.accountId === binding.accountId
}
