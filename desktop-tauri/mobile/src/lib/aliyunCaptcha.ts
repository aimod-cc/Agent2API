/**
 * 阿里云无痕验证（滑块）求解器 —— 手机端移植版。
 *
 * ── 为什么需要它 ──────────────────────────────────────────────
 * ZCode 的「领套餐」要过阿里云风控：上游的领取接口要
 * `X-Aliyun-Captcha-Verify-Param`，要不要由它下发的风控配置说了算
 * （`enabled: false` 的那一刻**不该**弹滑块）。桌面端这份逻辑在
 * `ui/aliyun-captcha.js`（从官方客户端逐条移植），手机端是独立工程，
 * 不能直接引用那个经典脚本，所以按同一份流程重写一遍。
 *
 * ── 与桌面版的差异（只有三处，都是工程形态带来的）──────────
 *   1. 文案走手机端的 `t()`（那边是 `wbI18n.t`）；
 *   2. 去掉静默铸串（`mintTraceless`）—— 那是活动套餐**转发**通道的令牌
 *      来源，手机端不跑那条后台链路；
 *   3. 去掉 AutoClaw 登录那一半（手机端没有 OAuth 添加账号的入口）。
 * 流程本身（容器、预热、代际号、超时、取消）逐条照搬：这些坑都在细节里，
 * 抄一半会得到「滑块弹出来但一直转圈」这种最难查的形态。
 *
 * ── 一次只允许一个验证码流程 ──────────────────────────────────
 * 模块级状态：同时发起两轮会互相踩。界面上的领取按钮互斥，这里再用代际号
 * 让后发起的那一轮作废前一轮（迟到的回调发现代际不符就丢弃）。
 */

import { t } from '../i18n'

const SCRIPT_URL = 'https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js'
const SCRIPT_ID = 'aliyun-captcha-sdk'
const ELEMENT_ID = 'aliyun-captcha-element'
const BUTTON_ID = 'aliyun-captcha-trigger'

const SCRIPT_LOAD_TIMEOUT_MS = 40_000
const INIT_TIMEOUT_MS = 40_000
const VERIFY_TIMEOUT_MS = 120_000
/** 初始化后至少等 2.1 秒再点按钮（官方客户端实测：SDK 预热没完成时点击无效） */
const MINIMUM_WARMUP_MS = 2_100
/** 初始化结果最多复用 19 分钟（超过则重建，避免实例内部状态过期） */
const INITIALIZATION_MAX_AGE_MS = 19 * 60_000

export type CaptchaConfig = {
  region?: string
  prefix?: string
  sceneId?: string
}

/** 验证码流程失败（带一句人话，界面直接展示） */
export class CaptchaError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'CaptchaError'
  }
}

/** 用户主动取消（与「失败」分开，界面不报红） */
export class CaptchaCancelledError extends CaptchaError {
  constructor() {
    super(t('已取消验证码'))
    this.name = 'CaptchaCancelledError'
  }
}

type CaptchaInstance = { destroy?: () => void }

type InitOptions = {
  SceneId?: string
  mode?: string
  element?: string
  button?: string
  captchaVerifyCallback?: (param: string) => Promise<{ captchaResult: boolean; bizResult: boolean }>
  onBizResultCallback?: () => void
  getInstance?: (instance: CaptchaInstance) => void
  slideStyle?: { width: number; height: number }
  language?: string
  onError?: (error: { message?: string } | null) => void
}

type CaptchaGlobal = {
  initAliyunCaptcha?: (options: InitOptions) => void
  AliyunCaptchaConfig?: { region?: string; prefix?: string }
}

const globals = window as unknown as CaptchaGlobal & {
  AliyunCaptchaConfig?: { region?: string; prefix?: string }
}

/* ─── 模块级状态 ─────────────────────────────── */

let scriptLoadPromise: Promise<void> | null = null
let initializationPromise: Promise<void> | null = null
let initializationKey = ''
let initializationGeneration = 0
let initializedAt = 0
let captchaInstance: CaptchaInstance | null = null

type Pending = {
  generation: number
  instance: CaptchaInstance
  request: (verifyParam: string) => Promise<{ captchaResult: boolean; bizResult: boolean }>
  resolve: (value: unknown) => void
  reject: (error: Error) => void
  timer: number
  settled: boolean
}

let pendingVerification: Pending | null = null

const sleep = (ms: number) => new Promise(resolve => window.setTimeout(resolve, ms))

/**
 * 语言映射（官方客户端的 `resolveAliyunCaptchaLanguage`）。
 *
 * 阿里云只认这几个短码，传别的会被它当成不认识而回落到英文 —— 必须在这里
 * 归一，不能直接把 `zh-CN` 递给 SDK。
 */
function resolveLanguage(language: string): string {
  const normalized = String(language || '').trim().replace(/_/g, '-').toLowerCase()
  if (normalized === 'zh-tw' || normalized.startsWith('zh-hant')) return 'tw'
  if (normalized.startsWith('zh')) return 'cn'
  if (normalized.startsWith('ar')) return 'ar'
  if (normalized.startsWith('de')) return 'de'
  if (normalized.startsWith('es')) return 'es'
  if (normalized.startsWith('fr')) return 'fr'
  if (normalized.startsWith('id') || normalized.startsWith('in')) return 'in'
  if (normalized.startsWith('it')) return 'it'
  if (normalized.startsWith('ja')) return 'ja'
  if (normalized.startsWith('ko')) return 'ko'
  if (normalized.startsWith('pt')) return 'pt'
  if (normalized.startsWith('ru')) return 'ru'
  if (normalized.startsWith('th')) return 'th'
  if (normalized.startsWith('tr')) return 'tr'
  if (normalized.startsWith('vi')) return 'vi'
  return 'en'
}

const currentLanguage = () => document.documentElement.lang || 'zh-CN'

function removeCaptchaElements(): void {
  document.getElementById(ELEMENT_ID)?.remove()
  document.getElementById(BUTTON_ID)?.remove()
}

/** 作废当前实例（配置变了 / 验证码用完了要重建时调） */
function invalidateInitialization(): void {
  const instance = captchaInstance
  initializationGeneration += 1
  captchaInstance = null
  initializationPromise = null
  initializedAt = 0
  removeCaptchaElements()
  try {
    instance?.destroy?.()
  } catch {
    /* 实例已不可用 */
  }
}

/**
 * 备好滑块容器与触发按钮（官方客户端的 `ensureCaptchaElements`）。
 *
 * 两个元素都由 SDK 按 id 找：`element` 是滑块面板的落点，`button` 是
 * 「点它才弹」的触发器。按钮做成 1×1 透明且 `pointer-events: none` ——
 * 用户看不到也点不到，触发完全由代码控制。
 */
function ensureCaptchaElements(): HTMLButtonElement {
  let element = document.getElementById(ELEMENT_ID)
  if (!element) {
    element = document.createElement('div')
    element.id = ELEMENT_ID
    element.style.position = 'relative'
    element.style.zIndex = '2147483000'
    document.body.appendChild(element)
  }
  let button = document.getElementById(BUTTON_ID) as HTMLButtonElement | null
  if (!button) {
    button = document.createElement('button')
    button.id = BUTTON_ID
    button.type = 'button'
    button.tabIndex = -1
    button.setAttribute('aria-hidden', 'true')
    button.style.position = 'fixed'
    button.style.width = '1px'
    button.style.height = '1px'
    button.style.opacity = '0'
    button.style.pointerEvents = 'none'
    button.style.overflow = 'hidden'
    document.body.appendChild(button)
  }
  return button
}

/**
 * 加载 SDK 脚本。
 *
 * `window.AliyunCaptchaConfig` 必须在脚本加载**之前**设好 —— SDK 读它决定
 * 打哪个阿里云站点（`region` / `prefix`）。设晚了 SDK 会用默认站点，表现是
 * 「滑块弹出来但一直转圈」。
 */
function loadScript(config: CaptchaConfig): Promise<void> {
  globals.AliyunCaptchaConfig = { region: config.region, prefix: config.prefix }
  if (typeof globals.initAliyunCaptcha === 'function') return Promise.resolve()
  if (scriptLoadPromise) return scriptLoadPromise

  scriptLoadPromise = new Promise<void>((resolve, reject) => {
    const existing = document.getElementById(SCRIPT_ID) as HTMLScriptElement | null
    const script = existing || document.createElement('script')
    let timer = 0
    const cleanup = () => {
      window.clearTimeout(timer)
      script.removeEventListener('load', onLoad)
      script.removeEventListener('error', onError)
    }
    const fail = (error: Error) => {
      cleanup()
      scriptLoadPromise = null
      script.remove()
      reject(error)
    }
    const onLoad = () => {
      cleanup()
      if (typeof globals.initAliyunCaptcha === 'function') resolve()
      else fail(new CaptchaError(t('验证码组件加载异常，请重试')))
    }
    const onError = () => fail(new CaptchaError(t('验证码组件加载失败，请检查网络后重试')))
    timer = window.setTimeout(
      () => fail(new CaptchaError(t('验证码组件加载超时，请检查网络后重试'))),
      SCRIPT_LOAD_TIMEOUT_MS,
    )
    script.addEventListener('load', onLoad)
    script.addEventListener('error', onError)
    if (!existing) {
      script.id = SCRIPT_ID
      script.async = true
      script.src = SCRIPT_URL
      document.head.appendChild(script)
    }
  })
  return scriptLoadPromise
}

/** 把一次等待落定 */
function settlePending(pending: Pending, outcome: { error?: Error; value?: unknown }): boolean {
  if (pendingVerification !== pending || pending.settled) return false
  pending.settled = true
  window.clearTimeout(pending.timer)
  if (outcome.error) pending.reject(outcome.error)
  else pending.resolve(outcome.value)
  return true
}

/**
 * SDK 回调：拿到不透明验证串 → 交给调用方去干它那件事。
 *
 * 返回值必须是 `{captchaResult, bizResult}`：SDK 据此决定「这一关过了没有」。
 * `captchaResult` 是验证码本身是否通过，`bizResult` 是调用方的业务是否接受
 * 它。两个都为 true SDK 才收起滑块。
 */
async function verifyCallback(generation: number, param: string) {
  const pending = pendingVerification
  if (!pending || pending.generation !== generation) {
    return { captchaResult: false, bizResult: false }
  }
  if (typeof param !== 'string' || param.length === 0) {
    settlePending(pending, { error: new CaptchaError(t('验证码校验失败，请重试')) })
    return { captchaResult: false, bizResult: false }
  }
  try {
    const result = await pending.request(param)
    if (pendingVerification !== pending || pending.settled) {
      return {
        captchaResult: Boolean(result?.captchaResult),
        bizResult: Boolean(result?.bizResult),
      }
    }
    settlePending(pending, { value: result })
    return {
      captchaResult: Boolean(result?.captchaResult),
      bizResult: Boolean(result?.bizResult),
    }
  } catch (error) {
    settlePending(pending, { error: error instanceof Error ? error : new Error(String(error)) })
    return { captchaResult: false, bizResult: false }
  }
}

/**
 * 初始化 SDK 实例。
 *
 * 同一个配置只初始化一次，19 分钟内复用；配置变了或过期就重建。
 * `getInstance` 回调是「SDK 准备好了」的信号。
 */
function initialize(config: CaptchaConfig): Promise<void> {
  const language = resolveLanguage(currentLanguage())
  const key = `${config.region}:${config.prefix}:${config.sceneId}:${language}`
  if (initializationPromise && initializationKey === key) {
    const inFlight = initializedAt === 0
    const fresh = Date.now() - initializedAt < INITIALIZATION_MAX_AGE_MS
    if (inFlight || fresh) return initializationPromise
    invalidateInitialization()
  }
  if (initializationKey && initializationKey !== key) invalidateInitialization()
  initializationKey = key
  const generation = ++initializationGeneration

  initializationPromise = (async () => {
    await loadScript(config)
    ensureCaptchaElements()
    const init = globals.initAliyunCaptcha
    if (typeof init !== 'function') throw new CaptchaError(t('验证码组件不可用，请重试'))

    await new Promise<void>((resolve, reject) => {
      let settled = false
      let boundInstance: CaptchaInstance | null = null
      const timer = window.setTimeout(() => {
        if (settled) return
        settled = true
        reject(new CaptchaError(t('验证码组件初始化超时，请重试')))
      }, INIT_TIMEOUT_MS)
      const settle = (callback: () => void) => {
        if (settled) return false
        settled = true
        window.clearTimeout(timer)
        callback()
        return true
      }
      try {
        init({
          SceneId: config.sceneId,
          mode: 'popup',
          element: `#${ELEMENT_ID}`,
          button: `#${BUTTON_ID}`,
          captchaVerifyCallback: param => verifyCallback(generation, param),
          onBizResultCallback: () => {},
          getInstance: instance => {
            boundInstance = instance
            if (generation !== initializationGeneration) {
              try {
                instance.destroy?.()
              } catch {
                /* 忽略 */
              }
              return
            }
            if (
              !settle(() => {
                captchaInstance = instance
                initializedAt = Date.now()
                resolve()
              })
            ) {
              try {
                instance.destroy?.()
              } catch {
                /* 忽略 */
              }
            }
          },
          slideStyle: { width: 360, height: 40 },
          language,
          onError: error => {
            const pending = pendingVerification
            if (pending && pending.generation === generation && pending.instance === boundInstance) {
              settlePending(pending, {
                error: new CaptchaError(error?.message || t('验证码校验失败，请重试')),
              })
              return
            }
            if (settled) {
              if (
                generation === initializationGeneration &&
                (!boundInstance || captchaInstance === boundInstance)
              ) {
                invalidateInitialization()
              }
              return
            }
            settle(() => reject(new CaptchaError(error?.message || t('验证码组件不可用，请重试'))))
          },
        })
      } catch (error) {
        settle(() =>
          reject(
            new CaptchaError(
              error instanceof Error ? error.message : t('验证码组件不可用，请重试'),
            ),
          ),
        )
      }
    })
    if (!captchaInstance) throw new CaptchaError(t('验证码组件不可用，请重试'))
  })()
  return initializationPromise
}

/** 等 SDK 预热完成（太快点击弹不出滑块） */
async function waitForWarmup(): Promise<void> {
  const remaining = MINIMUM_WARMUP_MS - (Date.now() - initializedAt)
  if (remaining > 0) await sleep(remaining)
}

/**
 * 走一次完整验证码：初始化 → 预热 → 弹滑块 → 用户拖 → 拿串交给调用方。
 *
 * `request(verifyParam)` 是调用方注入的「用这个串去干那件事」，返回
 * `{captchaResult, bizResult}`；它 resolve 出来的值会原样成为 `solve()` 的
 * 返回值（调用方借它把业务结果带回来）。
 *
 * 超时只包着「拖滑块 + 那一次请求」：调用方不要在 `request` 里做长流程。
 */
export async function solveCaptcha<T>(
  config: CaptchaConfig,
  request: (verifyParam: string) => Promise<{ captchaResult: boolean; bizResult: boolean; result?: T }>,
): Promise<T | undefined> {
  await initialize(config)
  await waitForWarmup()
  const button = ensureCaptchaElements()
  const instance = captchaInstance
  if (!instance) throw new CaptchaError(t('验证码组件不可用，请重试'))
  const generation = initializationGeneration

  return new Promise<T | undefined>((resolve, reject) => {
    const pending: Pending = {
      generation,
      instance,
      request,
      resolve: value => resolve(value as T | undefined),
      reject,
      timer: 0,
      settled: false,
    }
    pending.timer = window.setTimeout(() => {
      settlePending(pending, { error: new CaptchaError(t('验证码校验超时，请重试')) })
    }, VERIFY_TIMEOUT_MS)
    pendingVerification = pending
    button.click()
  })
}

/** 用户取消：把等待中的那次落定成「已取消」并作废实例 */
export function cancelCaptcha(): boolean {
  const pending = pendingVerification
  if (!pending) return false
  const cancelled = settlePending(pending, { error: new CaptchaCancelledError() })
  if (cancelled) invalidateInitialization()
  return cancelled
}

/** 有没有正在等的验证码流程（关抽屉时据此决定要不要提示） */
export function captchaBusy(): boolean {
  return pendingVerification !== null
}
