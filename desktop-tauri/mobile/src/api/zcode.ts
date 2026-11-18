import { apiCall } from './client'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { t } from '../i18n'
import { isZcodeCaptchaProvider, zcodeCaptchaAccepted, zcodeCaptchaBinding, zcodeCaptchaStatsMatch,
  type ZcodeCaptchaConfig, type ZcodeCaptchaPoolStats, type ZcodeClaimPayload } from '../../../ui-shared/zcode-captcha-binding'

export type { Entitlement, PreviewPlan, OwnedPlan, PreviewResult, ClaimResult } from '../../../ui-shared/zcode-plans'
import type { PreviewResult, ClaimResult } from '../../../ui-shared/zcode-plans'

export type CaptchaPhase = 'preparing' | 'verifying' | 'submitting'
export type CaptchaPoolStats = ZcodeCaptchaPoolStats
type CaptchaConfig = ZcodeCaptchaConfig
type CaptchaApi = {
  solve<T>(config: CaptchaConfig, request: (param: string) => Promise<{ captchaResult: boolean; bizResult: boolean; result: T }>, signal?: AbortSignal,
    options?: { protocol: 'v3'; onPhase?: (phase: CaptchaPhase) => void }): Promise<{ result: T }>
  requestToken(config: CaptchaConfig, signal?: AbortSignal, options?: { onPhase?: (phase: CaptchaPhase) => void }): Promise<{ param: string }>
  cancel(): boolean
  isBusy(): boolean
}

const captchaApi = () => (window as Window & { wbAliyunCaptcha?: CaptchaApi }).wbAliyunCaptcha
let captchaLoading: Promise<CaptchaApi> | null = null

/** 只复用主程序的验证码实现，原生包也携带这份脚本，不从插件加载页面。 */
function loadCaptcha(): Promise<CaptchaApi> {
  const api = captchaApi()
  if (api) return Promise.resolve(api)
  if (captchaLoading) return captchaLoading
  captchaLoading = new Promise<CaptchaApi>((resolve, reject) => {
    const script = document.createElement('script')
    const timer = window.setTimeout(() => finish(new Error(t('验证码组件加载超时，请重试'))), 15_000)
    const finish = (error?: Error) => {
      window.clearTimeout(timer)
      script.onload = null
      script.onerror = null
      const loaded = captchaApi()
      if (error || !loaded) { script.remove(); reject(error || new Error(t('验证码组件未加载，请重试'))) }
      else resolve(loaded)
    }
    script.src = new URL('aliyun-captcha.js', document.baseURI).href
    script.onload = () => finish()
    script.onerror = () => finish(new Error(t('验证码组件加载失败，请重试')))
    document.head.appendChild(script)
  }).catch(error => { captchaLoading = null; throw error })
  return captchaLoading
}

const accountPath = (id: string) => `/api/accounts/${encodeURIComponent(id)}`

export function previewPlans(id: string): Promise<PreviewResult> {
  return apiCall(`${accountPath(id)}/zcode-claim/preview`, { method: 'POST' })
}

export async function claimPlan(id: string, planId: string, signal?: AbortSignal, onPhase?: (phase: CaptchaPhase) => void): Promise<ClaimResult> {
  const identity = serverIdentity()
  const controller = new AbortController()
  const original = store.get().session?.accounts?.accounts?.find(account => account.id === id)
  let provider = original?.provider
  let submitted = false
  const cancel = () => controller.abort()
  const guard = () => {
    if (signal?.aborted || serverIdentity() !== identity) { cancel(); return }
    const accounts = store.get().session?.accounts?.accounts
    if (!Array.isArray(accounts)) { cancel(); return }
    const account = accounts.find(account => account.id === id)
    if (!account || account.available === false || !isZcodeCaptchaProvider(account.provider)
      || provider !== undefined && provider !== account.provider) cancel()
    else provider = account.provider
  }
  const requireCurrent = () => {
    guard()
    if (controller.signal.aborted) throw new DOMException('请求已取消', 'AbortError')
  }
  const reportPhase = (phase: CaptchaPhase) => { requireCurrent(); onPhase?.(phase) }
  signal?.addEventListener('abort', cancel, { once: true })
  window.addEventListener('pagehide', cancel)
  const unsubscribe = store.subscribe(guard)
  try {
    requireCurrent()
    if (!id.trim() || !planId.trim()) throw new Error('claim_target_missing')
    reportPhase('preparing')
    const config = Object.freeze({ ...await apiCall<CaptchaConfig>(`${accountPath(id)}/zcode-claim/captcha-config`, {
      method: 'POST', signal: controller.signal,
    }) })
    requireCurrent()
    const binding = zcodeCaptchaBinding(config, id)
    if (binding && provider !== undefined && provider !== binding.provider) throw new Error('claim_provider_changed')
    if (binding) provider = binding.provider
    const claim = async (param: string) => {
      requireCurrent()
      if (submitted || config.enabled && (typeof param !== 'string' || !param.trim())) throw new Error('claim_submission_invalid')
      reportPhase('submitting')
      submitted = true
      const fields = { planId, captchaVerifyParam: param, captchaRegion: config.region || '' }
      const body: ZcodeClaimPayload = binding ? { ...binding, ...fields } : fields
      const result = await apiCall<ClaimResult>(`${accountPath(id)}/zcode-claim`, {
        method: 'POST', body, quiet: true, signal: controller.signal,
      })
      requireCurrent()
      if (!result || typeof result.ok !== 'boolean' || result.planId && result.planId !== planId) throw new Error('claim_result_unconfirmed')
      return result
    }
    if (!config.enabled) return await claim('')
    const captcha = await loadCaptcha()
    requireCurrent()
    const token = await captcha.requestToken(config, controller.signal, { onPhase: reportPhase })
    requireCurrent()
    return await claim(token.param)
  } catch (error) {
    requireCurrent()
    if (['AbortError', 'CaptchaCancelledError'].includes((error as Error)?.name)) throw error
    throw new Error(submitted ? t('领取提交结果未确认，请查看签到记录，勿重复领取')
      : t('领取验证未完成，请刷新套餐配置后重试'))
  } finally {
    signal?.removeEventListener('abort', cancel)
    window.removeEventListener('pagehide', cancel)
    unsubscribe()
  }
}

export async function captchaPoolStats(accountId: string, signal?: AbortSignal, provider?: string): Promise<CaptchaPoolStats> {
  if (!accountId || provider !== undefined && !isZcodeCaptchaProvider(provider)) throw new Error(t('验证码状态与当前账号不匹配，请刷新后重试'))
  const identity = serverIdentity()
  const query = new URLSearchParams({ accountId })
  if (provider) query.set('provider', provider)
  try {
    const value = await apiCall<unknown>(`/api/zcode/captcha?${query}`, { signal })
    if (signal?.aborted || serverIdentity() !== identity) throw new DOMException('请求已取消', 'AbortError')
    if (!zcodeCaptchaStatsMatch(value, accountId, provider, provider !== undefined)) throw new Error('invalid_captcha_scope')
    return value
  } catch (error) {
    if (signal?.aborted || serverIdentity() !== identity || (error as Error)?.name === 'AbortError') throw new DOMException('请求已取消', 'AbortError')
    throw new Error(t('验证码状态读取失败，请刷新后重试'))
  }
}

export async function verifyPlanChannel(id: string, signal?: AbortSignal, onPhase?: (phase: CaptchaPhase) => void): Promise<boolean> {
  const identity = serverIdentity()
  const requireCurrent = () => {
    if (signal?.aborted || serverIdentity() !== identity) throw new DOMException('请求已取消', 'AbortError')
  }
  const reportPhase = (phase: CaptchaPhase) => { requireCurrent(); onPhase?.(phase) }
  let submitting = false
  let accepted = false
  try {
    requireCurrent()
    if (!id.trim()) throw new Error('captcha_account_missing')
    reportPhase('preparing')
    const config = Object.freeze({ ...await apiCall<CaptchaConfig>(`${accountPath(id)}/zcode-claim/captcha-config`, { method: 'POST', signal }) })
    requireCurrent()
    const binding = zcodeCaptchaBinding(config, id)
    const stats = await captchaPoolStats(id, signal, binding?.provider)
    requireCurrent()
    if (stats.error || stats.configError) throw new Error('captcha_status_unavailable')
    if (stats.required === false) return false
    if (!config.enabled) throw new Error('captcha_config_unavailable')
    const captcha = await loadCaptcha()
    requireCurrent()
    const token = await captcha.requestToken(config, signal, { onPhase: reportPhase })
    requireCurrent()
    if (typeof token?.param !== 'string' || !token.param.trim()) throw new Error('invalid_captcha_token')
    reportPhase('submitting')
    submitting = true
    const result = await apiCall<unknown>('/api/zcode/captcha', {
      method: 'POST', signal, quiet: true,
      body: binding ? { ...binding, tokens: [{ param: token.param }] }
        : { tokens: [{ param: token.param, region: config.region }] },
    })
    requireCurrent()
    if (!zcodeCaptchaAccepted(result, binding)) throw new Error('captcha_submit_unconfirmed')
    accepted = true
    // 提交回执不含库存；成功后只重新读取原账号，绝不重发一次性票据。
    const refreshed = await captchaPoolStats(id, signal, binding?.provider)
    requireCurrent()
    if (refreshed.error || refreshed.configError) throw new Error('captcha_status_unavailable')
    return true
  } catch (error) {
    requireCurrent()
    if ((error as Error)?.name === 'AbortError' || (error as Error)?.name === 'CaptchaCancelledError') throw error
    throw new Error(accepted ? t('验证码已提交，但状态刷新失败，请手动刷新')
      : submitting ? t('验证码提交结果未确认，请刷新状态后重新验证；不会自动重发')
        : t('验证码验证未完成，请刷新配置后重试'))
  }
}

export function cancelPlanCaptcha(): void {
  captchaApi()?.cancel()
}
