import * as React from 'react'
import { Alert, Button, Input, Label, Spinner } from '@ui'
import type { LoginRequest } from './plugin-login-interaction'

export type ManagedInteraction = {
  type: 'managed'
  id: string
  provider: string
  flow: string
  methodIndex: number
  status: 'preparing' | 'ready' | 'processing'
  title: string
  message: string
  fields: []
  expiresAt: number
  remainingMs: number
  captcha: null | { engine: 'aliyun'; protocol: 'v2'; configId: string; prefix: string; sceneId: string; region: string }
  browser: null | { url: string; browserMode: 'external'; manualCallback: boolean; callbackInput: 'url' }
}
export type ManagedLoginScope = {
  provider: string
  methodIndex: number
  key: string
  deadline: number
  current(): boolean
}
export type ManagedLoginWaitPhase = 'waiting' | 'saving' | 'saved' | 'failed' | 'canceled' | 'unconfirmed'
export type ManagedLoginSummary = {
  accountUid?: string; accountId?: string; nickname?: string; edition?: string; provider: string
  proxySaved?: boolean; warning?: string
}
export type ManagedLoginWait = {
  managedLogin: true; provider: string; flow: string; methodIndex: number
  phase: ManagedLoginWaitPhase; saved: boolean | null; pending?: boolean; done?: boolean
  error?: string; instructions?: string; hint?: string; session?: ManagedLoginSummary | null
}
export type ManagedLoginWaitOwner = { provider: string; flow?: string; methodIndex?: number }
export const MANAGEMENT_SESSION_CHANGE = 'wb:management-session-changed'
let managementGeneration = 0
// 来自壳/网页/手机的权威通知；只记代次，不读取凭据，也不据恢复会话复活旧 flow。
if (typeof window !== 'undefined') window.addEventListener(MANAGEMENT_SESSION_CHANGE, () => { managementGeneration++ })
export const managedLoginSourceGeneration = () => managementGeneration

type Translate = (key: string, vars?: Record<string, string | number>) => string
const record = (value: unknown): value is Record<string, unknown> => Boolean(value) && typeof value === 'object' && !Array.isArray(value)
const text = (value: unknown, limit = 512): value is string => typeof value === 'string' && value.trim().length > 0
  && value.length <= limit && !/[\u0000-\u001f\u007f]/.test(value)
const managedId = (value: unknown, limit = 512): value is string => text(value, limit) && value.trim() === value
const MANAGED_WAIT_PHASES: readonly string[] = ['waiting', 'saving', 'saved', 'failed', 'canceled', 'unconfirmed']

/** 只查看既有桥包装层；外层 ok/summary 不能覆盖内部 managed 保存事实。 */
export function managedLoginWaitRecord(value: unknown): Record<string, unknown> | undefined {
  const seen = new Set<Record<string, unknown>>()
  const found = new Set<Record<string, unknown>>()
  const inspect = (item: unknown, depth: number) => {
    if (!record(item) || seen.has(item) || depth > 3) return
    seen.add(item)
    if (item.managedLogin === true) found.add(item)
    // 只查现有信封键，不走任意属性或账号字段；有界并防止自引用。
    for (const key of ['data', 'payload', 'session']) inspect(item[key], depth + 1)
  }
  inspect(value, 0)
  if (found.size > 1) throw new Error('conflicting_managed_login_wait')
  return found.values().next().value
}

/** 同源 wait 的严格公开投影；done 只表示任务结束，昵称/空对象不能充当保存摘要。 */
export function readManagedLoginWait(raw: unknown, owner?: ManagedLoginWaitOwner): ManagedLoginWait {
  const value = managedLoginWaitRecord(raw)
  const envelope = record(raw) ? raw : undefined
  const data = envelope && record(envelope.data) ? envelope.data : undefined
  const payload = envelope && record(envelope.payload) ? envelope.payload : data && record(data.payload) ? data.payload : undefined
  if ([envelope, data, payload].some(item => item && (item.ok === false || item.success === false))) {
    throw new Error('conflicting_managed_login_envelope')
  }
  if (!value || !managedId(value.provider, 128) || !managedId(value.flow)
    || typeof value.methodIndex !== 'number' || !Number.isSafeInteger(value.methodIndex) || value.methodIndex < 0
    || owner && (value.provider !== owner.provider || owner.flow !== undefined && value.flow !== owner.flow
      || owner.methodIndex !== undefined && value.methodIndex !== owner.methodIndex)
    || typeof value.phase !== 'string' || !MANAGED_WAIT_PHASES.includes(value.phase)
    || value.saved !== null && typeof value.saved !== 'boolean'
    || value.pending !== undefined && typeof value.pending !== 'boolean'
    || value.done !== undefined && typeof value.done !== 'boolean') throw new Error('invalid_managed_login_wait')
  const phase = value.phase as ManagedLoginWaitPhase
  const pending = value.pending === true
  const done = value.done === true
  const terminal = ['saved', 'failed', 'canceled', 'unconfirmed'].includes(phase)
  const expectedSaved = phase === 'saved' ? true : phase === 'saving' || phase === 'unconfirmed' ? null : false
  if (value.saved !== expectedSaved || (terminal ? !done || pending : done || !pending)) throw new Error('invalid_managed_login_state')
  const optionalText = (key: string): string | undefined => {
    const item = value[key]
    if (item !== undefined && item !== null && (typeof item !== 'string' || item.length > 8192)) throw new Error('invalid_managed_login_text')
    return typeof item === 'string' ? item : undefined
  }
  const error = optionalText('error')
  if (error && ['waiting', 'saving', 'saved'].includes(phase)) throw new Error('conflicting_managed_login_error')
  let session: ManagedLoginSummary | null = null
  if (phase === 'saved') {
    const summary = value.session
    if (!record(summary) || summary.provider !== value.provider
      || !managedId(summary.accountUid) && !managedId(summary.accountId)) throw new Error('invalid_managed_login_summary')
    for (const key of ['accountUid', 'accountId']) {
      if (summary[key] !== undefined && summary[key] !== null && !managedId(summary[key])) throw new Error('invalid_managed_login_account')
    }
    session = { provider: value.provider }
    for (const key of ['accountUid', 'accountId', 'nickname', 'edition', 'warning'] as const) {
      if (typeof summary[key] === 'string' && summary[key].length <= (key === 'warning' ? 8192 : 512)) session[key] = summary[key]
    }
    if (typeof summary.proxySaved === 'boolean') session.proxySaved = summary.proxySaved
  }
  // 非 saved 的任何夹带摘要都丢弃；不能拿另一层账号或旧 session 补一次成功。
  return { managedLogin: true, provider: value.provider, flow: value.flow, methodIndex: value.methodIndex,
    phase, saved: value.saved, ...(value.pending !== undefined ? { pending } : {}), ...(value.done !== undefined ? { done } : {}),
    error, instructions: optionalText('instructions'), hint: optionalText('hint'), session }
}

export function managedLoginWaitMessage(value: ManagedLoginWait, t: Translate): string {
  if (value.phase === 'saving') return t('正在保存账号，保存结果尚未确认')
  if (value.phase === 'unconfirmed') return t('账号保存未确认，请查询本次状态；不会再次保存。')
  if (value.phase === 'failed') return value.error ? t(value.error) : t('托管登录未确认保存账号，未宣布登录成功')
  if (value.phase === 'canceled') return t('已取消登录等待')
  if (value.phase === 'saved') return t('账号已确认保存')
  return t('等待账号登录完成…')
}

/** 只做语法防御；站点许可由宿主核准，不从 URL 派生或扩大授权范围。 */
export function managedBrowserUrl(value: unknown): value is string {
  if (!text(value, 16384) || /[\s\\]/.test(value)) return false
  try {
    const url = new URL(value)
    return url.protocol === 'https:' && Boolean(url.hostname) && !url.username && !url.password
  } catch { return false }
}

export function readManagedInteraction(value: unknown, flow: string, scope: ManagedLoginScope): ManagedInteraction {
  if (!record(value) || value.type !== 'managed' || !text(value.id) || value.provider !== scope.provider || value.flow !== flow
    || value.methodIndex !== scope.methodIndex || !Number.isSafeInteger(value.methodIndex)
    || !['preparing', 'ready', 'processing'].includes(String(value.status)) || typeof value.title !== 'string'
    || typeof value.message !== 'string' || value.title.length > 256 || value.message.length > 8192
    || !Array.isArray(value.fields) || value.fields.length !== 0
    || typeof value.expiresAt !== 'number' || !Number.isFinite(value.expiresAt) || value.expiresAt <= 0
    || typeof value.remainingMs !== 'number' || !Number.isFinite(value.remainingMs) || value.remainingMs < 0) {
    throw new Error('invalid_managed_interaction')
  }
  let captcha: ManagedInteraction['captcha'] = null
  let browser: ManagedInteraction['browser'] = null
  if (value.captcha !== null) {
    const config = value.captcha
    if (!record(config) || config.engine !== 'aliyun' || config.protocol !== 'v2'
      || !text(config.configId) || !text(config.prefix) || !text(config.sceneId) || !text(config.region)) throw new Error('invalid_managed_captcha')
    captcha = { engine: 'aliyun', protocol: 'v2', configId: config.configId, prefix: config.prefix, sceneId: config.sceneId, region: config.region }
  }
  if (value.browser !== null) {
    const page = value.browser
    if (!record(page) || page.browserMode !== 'external' || typeof page.manualCallback !== 'boolean' || page.callbackInput !== 'url'
      || !managedBrowserUrl(page.url)) throw new Error('invalid_managed_browser')
    browser = { url: page.url, browserMode: 'external', manualCallback: page.manualCallback, callbackInput: 'url' }
  }
  if (captcha && browser || value.status === 'ready' && !captcha && !browser) throw new Error('invalid_managed_step')
  return { type: 'managed', id: value.id, provider: scope.provider, flow, methodIndex: scope.methodIndex,
    status: value.status as ManagedInteraction['status'], title: value.title, message: value.message, fields: [],
    expiresAt: value.expiresAt, remainingMs: value.remainingMs, captcha, browser }
}

/** 剩余时间和说明变化不是新步骤；轮询不能重置验证码或再次允许已用票据。 */
export const managedStepKey = (step: ManagedInteraction) => JSON.stringify([
  step.provider, step.flow, step.methodIndex, step.id, step.captcha, step.browser,
])

const captchaOwners = new Set<symbol>()
const captchaListeners = new Set<() => void>()
export const subscribeManagedCaptchaBusy = (listener: () => void) => { captchaListeners.add(listener); return () => { captchaListeners.delete(listener) } }
export const managedCaptchaBusy = () => captchaOwners.size > 0
function captchaBusy(owner: symbol, busy: boolean) {
  if (busy) captchaOwners.add(owner)
  else captchaOwners.delete(owner)
  captchaListeners.forEach(listener => listener())
}

type BusinessReply = { captchaResult: boolean; bizResult: boolean }
type AliyunApi = {
  isBusy(): boolean
  solve(config: NonNullable<ManagedInteraction['captcha']>, request: (param: string) => Promise<BusinessReply>, signal: AbortSignal,
    options: { protocol: 'v2'; onPhase(phase: string): void }): Promise<BusinessReply>
}
let loading: Promise<AliyunApi> | null = null
function loadAliyun(): Promise<AliyunApi> {
  const existing = (window as Window & { wbAliyunCaptcha?: AliyunApi }).wbAliyunCaptcha
  if (typeof existing?.solve === 'function' && typeof existing.isBusy === 'function') return Promise.resolve(existing)
  if (loading) return loading
  // 只加载主程序原有组件；不从插件加载脚本，不改变其 V2 协议或自动验证策略。
  loading = new Promise<AliyunApi>((resolve, reject) => {
    const script = document.createElement('script')
    const finish = (error?: Error) => {
      window.clearTimeout(timer)
      script.onload = null; script.onerror = null
      const api = (window as Window & { wbAliyunCaptcha?: AliyunApi }).wbAliyunCaptcha
      if (error || typeof api?.solve !== 'function' || typeof api.isBusy !== 'function') { script.remove(); reject(error || new Error('captcha_unavailable')) }
      else resolve(api)
    }
    const timer = window.setTimeout(() => finish(new Error('captcha_load_timeout')), 15000)
    script.src = new URL('aliyun-captcha.js', document.baseURI).href
    script.onload = () => finish()
    script.onerror = () => finish(new Error('captcha_load_failed'))
    document.head.appendChild(script)
  }).catch(error => { loading = null; throw error })
  return loading
}

const aborted = () => new DOMException('请求已取消', 'AbortError')
/** abort 只停止本地等待；原请求的迟到落定仍被消费，不表示已撤回上游请求。 */
function untilAborted<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const cancel = () => reject(aborted())
    signal.addEventListener('abort', cancel, { once: true })
    if (signal.aborted) cancel()
    promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', cancel))
  })
}

export function ManagedLoginInteraction({ step, deadline, scope, request, refresh, accept, onSubmitting,
  attemptedBefore, markAttempted, callbackSubmitted, markCallbackSubmitted, openExternal, t }: {
  step: ManagedInteraction
  deadline: number
  scope: ManagedLoginScope
  request: LoginRequest
  refresh(): Promise<void>
  accept(step: ManagedInteraction, startedAt: number): void
  onSubmitting(busy: boolean, owner: symbol): void
  attemptedBefore: boolean
  markAttempted(): void
  callbackSubmitted: boolean
  markCallbackSubmitted(value: boolean): void
  openExternal(url: string): void | Promise<void>
  t: Translate
}) {
  const [busy, setBusy] = React.useState('')
  const [attempted, setAttempted] = React.useState(attemptedBefore)
  const [expired, setExpired] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const [hasCallback, setHasCallback] = React.useState(false)
  const [callbackSent, setCallbackSent] = React.useState(callbackSubmitted)
  const callbackLocked = React.useRef(callbackSubmitted)
  const input = React.useRef<HTMLInputElement>(null)
  const controller = React.useRef<AbortController | null>(null)
  const owner = React.useRef(Symbol(`${scope.key}:${step.id}`))
  const sourceGeneration = React.useRef(managedLoginSourceGeneration())
  const alive = React.useRef(true)
  const used = React.useRef(attemptedBefore)
  const latest = React.useRef({ step, deadline, scope, request, refresh, accept, onSubmitting, openExternal })
  latest.current = { step, deadline, scope, request, refresh, accept, onSubmitting, openExternal }
  const current = () => alive.current && sourceGeneration.current === managedLoginSourceGeneration()
    && latest.current.scope.current() && performance.now() < latest.current.deadline
  const scopeKey = scope.key
  const stepKey = managedStepKey(step)
  const scopeAlive = scope.current()
  const clearInput = () => { if (input.current) input.current.value = ''; setHasCallback(false) }

  React.useEffect(() => {
    alive.current = sourceGeneration.current === managedLoginSourceGeneration()
    const ownerKey = owner.current
    const stop = () => {
      alive.current = false; controller.current?.abort(); captchaBusy(ownerKey, false)
      if (input.current) input.current.value = ''
    }
    const sourceLost = () => {
      stop(); used.current = true; callbackLocked.current = true
      setAttempted(true); setCallbackSent(true); setHasCallback(false); setExpired(true); setBusy('')
      setNotice(''); setError(t('管理会话已变化，本次登录资格已失效；不会继续查询或重发原流程。'))
      latest.current.onSubmitting(false, ownerKey)
    }
    window.addEventListener('pagehide', stop)
    window.addEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost)
    return () => {
      alive.current = false; stop(); window.removeEventListener('pagehide', stop)
      window.removeEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost); latest.current.onSubmitting(false, owner.current)
    }
  }, [scopeKey, stepKey])
  React.useEffect(() => {
    const expire = () => {
      controller.current?.abort()
      setExpired(true)
      clearInput()
      if (sourceGeneration.current === managedLoginSourceGeneration()) void latest.current.refresh()
    }
    const timer = window.setTimeout(expire, Math.max(0, deadline - performance.now()))
    return () => window.clearTimeout(timer)
  }, [deadline])
  React.useEffect(() => {
    if (step.status !== 'ready' || !scopeAlive) {
      controller.current?.abort()
      clearInput()
    }
  }, [step.status, scopeAlive])

  async function verify() {
    if (controller.current || used.current || !current() || step.status !== 'ready' || !step.captcha) return
    const abort = new AbortController()
    controller.current = abort
    setBusy('verifying'); setError(''); setNotice('')
    const context = latest.current
    const next: { value?: { step: ManagedInteraction; startedAt: number } } = {}
    let sent = false
    const live = () => current() && latest.current.step.status === 'ready' && latest.current.scope.key === scopeKey
      && managedStepKey(latest.current.step) === stepKey && !abort.signal.aborted && controller.current === abort
    try {
      const api = await untilAborted(loadAliyun(), abort.signal)
      if (!live()) throw aborted()
      if (api.isBusy()) throw new Error('captcha_busy')
      used.current = true; markAttempted(); setAttempted(true)
      captchaBusy(owner.current, true)
      const result = await untilAborted(api.solve(step.captcha, async param => {
        if (!live() || sent || !text(param, 32768)) return { captchaResult: false, bizResult: false }
        sent = true
        context.onSubmitting(true, owner.current)
        const startedAt = performance.now()
        try {
          const response = await untilAborted(context.request<unknown>('POST', '/api/session/login/interaction', {
            state: step.flow, interaction: step.id, inputs: {},
            captcha: { engine: 'aliyun', configId: step.captcha!.configId, param },
          }, { signal: abort.signal }), abort.signal)
          if (!live() || !record(response) || typeof response.captchaResult !== 'boolean' || typeof response.bizResult !== 'boolean') {
            return { captchaResult: false, bizResult: false }
          }
          if (response.captchaResult && response.bizResult) {
            const browser = readManagedInteraction(response.interaction, step.flow, scope)
            if (response.accepted !== true || !browser.browser || browser.captcha || browser.id === step.id || browser.status !== 'ready'
              || Math.min(scope.deadline, startedAt + browser.remainingMs) <= performance.now()) {
              return { captchaResult: false, bizResult: false }
            }
            next.value = { step: browser, startedAt }
          }
          // V2 必须收到这次换授权地址的真实业务布尔值；不等待整个 OAuth，也不以 accepted 代替。
          return { captchaResult: response.captchaResult, bizResult: response.bizResult }
        } catch { return { captchaResult: false, bizResult: false } }
      }, abort.signal, { protocol: 'v2', onPhase: phase => { if (live()) setBusy(phase) } }), abort.signal)
      if (!live()) return
      if (result.captchaResult && result.bizResult && next.value) {
        context.accept(next.value.step, next.value.startedAt)
      } else setError(t('登录验证结果未确认，请刷新当前步骤；不会自动重发验证凭据。'))
    } catch {
      if (current() && !abort.signal.aborted) setError(t('登录验证未完成，请刷新当前步骤后重试。'))
    } finally {
      context.onSubmitting(false, owner.current)
      captchaBusy(owner.current, false)
      if (controller.current === abort) controller.current = null
      if (alive.current) setBusy('')
      if (current()) await context.refresh()
    }
  }

  async function submitCallback() {
    if (controller.current || callbackLocked.current || !current() || step.status !== 'ready' || !step.browser?.manualCallback) return
    const callbackUrl = input.current?.value || ''
    if (!callbackUrl.trim()) return
    try {
      const url = new URL(callbackUrl)
      if (callbackUrl.length > 8192 || /[\u0000-\u0020\u007f\\]/.test(callbackUrl)
        || !['http:', 'https:'].includes(url.protocol) || !url.hostname || url.username || url.password) throw new Error()
    } catch { setError(t('请粘贴本次授权完成后的完整回调地址，不要修改其中的参数。')); return }
    const abort = new AbortController()
    controller.current = abort
    const context = latest.current
    callbackLocked.current = true; markCallbackSubmitted(true)
    setBusy('submitting'); setError(''); setNotice(''); setCallbackSent(true); clearInput()
    context.onSubmitting(true, owner.current)
    try {
      const reply = await untilAborted(context.request<unknown>('POST', '/api/session/login/interaction', {
        state: step.flow, interaction: step.id, inputs: { callbackUrl }, captcha: null,
      }, { signal: abort.signal }), abort.signal)
      if (!current() || abort.signal.aborted || managedStepKey(latest.current.step) !== stepKey || latest.current.scope.key !== scopeKey) return
      if (record(reply) && reply.retryable === true && reply.accepted !== true) {
        callbackLocked.current = false; markCallbackSubmitted(false)
        setCallbackSent(false)
        setError(t('回调未被接受，请重新复制本次登录的完整回调地址'))
      } else if (record(reply) && reply.accepted === true) {
        setNotice(t('回调已提交，仍需等待网关确认账号登录完成。'))
      } else setError(t('授权回调提交结果未确认，请刷新当前步骤；不会自动重发。'))
    } catch {
      if (current() && !abort.signal.aborted) setError(t('授权回调提交结果未确认，请刷新当前步骤；不会自动重发。'))
    } finally {
      context.onSubmitting(false, owner.current)
      if (controller.current === abort) controller.current = null
      if (alive.current) setBusy('')
      if (current()) await context.refresh()
    }
  }

  async function open() {
    if (!current() || busy || callbackSent || step.status !== 'ready' || !step.browser || !managedBrowserUrl(step.browser.url)) return
    setError('')
    try { await latest.current.openExternal(step.browser.url) }
    catch { if (current()) setError(t('打开授权页失败，请刷新当前步骤后重试。')) }
  }

  const disabled = Boolean(busy) || expired || !scopeAlive || step.status !== 'ready'
    || sourceGeneration.current !== managedLoginSourceGeneration()
  return <section className='flex flex-col gap-3 rounded-lg border border-hairline p-3' aria-label={step.title}>
    <h3 className='text-sm font-semibold'>{step.title}</h3>
    {step.message ? <p className='whitespace-pre-wrap text-xs text-muted-foreground'>{step.message}</p> : null}
    {step.status !== 'ready' ? <p role='status' className='flex items-center gap-2 text-xs text-muted-foreground'>
      <Spinner className='size-3' />{step.status === 'preparing' ? t('正在准备安全登录，请稍候…') : t('正在处理本次登录，请勿重复提交。')}
    </p> : null}
    {step.captcha ? <Button disabled={disabled || attempted} onClick={() => void verify()}>
      {busy ? <Spinner /> : null}{t('开始登录验证')}
    </Button> : null}
    {step.browser ? <>
      <Button variant='outline' disabled={disabled || callbackSent} onClick={() => void open()}>{t('打开官方授权页')}</Button>
      {step.browser.manualCallback ? <>
        <p className='text-xs text-muted-foreground'>{t('授权完成后如未自动返回，请粘贴本次完整回调地址。打开浏览器不代表登录成功。')}</p>
        <Label>{t('完整回调地址（原样提交）')}
          <Input ref={input} type='password' autoComplete='off' autoCapitalize='none' spellCheck={false}
            disabled={disabled || callbackSent} maxLength={8192} onChange={event => { setHasCallback(Boolean(event.currentTarget.value)); setError(''); setNotice('') }} />
        </Label>
        <Button variant='outline' disabled={disabled || callbackSent || !hasCallback} onClick={() => void submitCallback()}>
          {busy ? <Spinner /> : null}{t('提交授权回调')}
        </Button>
      </> : null}
    </> : null}
    {expired ? <Alert variant='warning'>{t('本次登录步骤已过期，请取消后重新发起登录。')}</Alert> : null}
    {notice ? <p role='status' className='text-xs text-muted-foreground'>{notice}</p> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {error || attempted || callbackSent || expired ? <Button variant='outline'
      disabled={Boolean(busy) || sourceGeneration.current !== managedLoginSourceGeneration() || !scopeAlive}
      onClick={() => { if (sourceGeneration.current === managedLoginSourceGeneration() && scope.current()) void refresh() }}>{t('刷新当前步骤')}</Button> : null}
  </section>
}
