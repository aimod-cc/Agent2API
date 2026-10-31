/**
 * Agent2API · 「登录 / 添加账号」弹窗的共享桥（window 读取 / 提交 / 草稿 / 偏好）。
 *
 * 从 add-account.js + add-provider-forms.js 合并而来的一层：这两份旧脚本都直接
 * 解构 app.js 的顶层全局（$ / toast / esc / refresh）与 window 上的引擎，迁到岛上
 * 之后这些读取集中在这里一处，各表单块只 import 本模块的函数。
 *
 * 为什么不用 declare global 往 Window 上加属性：workbuddyDesktop / wbApp /
 * wbProviders 是多个岛共用的桥，每个岛各 declare 一份会因同名属性类型不一致
 * 报 TS2717（并行迁移必然撞车）。这里只声明本子系统**独占**的那几个接口
 * （见 add-account-modal.tsx 末尾）。
 */

import { isSuspendedProvider } from '../../../ui-shared/provider-availability'
import { t } from '../i18n'

/* ─── window 上的共享桥（窄类型 + 转型读取）────────────── */

/** 账号记录里本子系统用到的字段（其余不关心） */
export type AccountRecord = {
  id?: string
  name?: string
  uid?: string
  userId?: string
  provider?: string
}

/** 只使用当前运行时摘要中的 provider；旧握手元数据若已下发，供 501 兼容路径读取。 */
export type RuntimeProviderRecord = {
  id?: string
  label?: string
  count?: number
  category?: 'agent' | 'web'
  source?: 'builtin' | 'plugin'
  meta?: Record<string, unknown>
  login_methods?: unknown[]
  loginMethods?: unknown[]
  capabilities?: Record<string, unknown>
}

export type LoginOptions = {
  /** 动态方法索引；缺省 = 旧入口（没有动态方法清单的家，例如内置表单） */
  methodIndex?: number
  inputs?: Record<string, string>
  proxy?: unknown
}

/** 共享登录桥推送的公开进度；设备授权只展示 userCode，不展示 device_code 或凭证。 */
export type LoginProgress = {
  active?: boolean
  provider?: string
  state?: string
  manualCallback?: boolean
  callbackInput?: 'url' | 'code'
  instructions?: string
  hint?: string
  userCode?: string
}

/** 桌面 ok/payload、网页登录 session 的统一终态；未结束/空响应绝不补成成功。 */
export type LoginResult = {
  ok: boolean
  canceled: boolean
  silent?: boolean
  account?: AccountRecord
  session?: Record<string, unknown>
  proxySaved?: boolean
  warning?: string
  instructions?: string
}

/** 目录条目：自定义提供商（wbProviders.customList）与预置目录（wbPresetProviders）共用形状 */
export type CustomProviderRecord = {
  id: string
  name?: string
  protocol?: string
  baseUrl?: string
  presetKey?: string
  accountCount?: number
  /** 客户端形态伪装（预置卡写入的记录字段）：'opencode' = 按官方 CLI 形状补齐请求 */
  clientEmulation?: string
  /**
   * 余额查询方案（预置卡写入的记录字段，见 `custom_providers::BALANCE_SCHEMES`）：
   * 有它 = 这一家能查余额，且账号设置里会出现「查询设置」的凭证字段。
   * **只读**：方案是「这一家是谁」的派生事实，界面上不给用户改。
   */
  balanceScheme?: string
  quirks?: {
    urlSuffix?: string
    headers?: Record<string, string>
    anthropicToolType?: string
  }
}

export type PresetRecord = {
  key: string
  name: string
  protocol?: string
  baseUrl?: string
  hint?: string
  /** Base URL 输入框的占位提示（自建面板没有固定地址时给一句「填你自己的站点」） */
  placeholder?: string
  quirks?: CustomProviderRecord['quirks']
  /** 客户端形态伪装：随创建写进提供商记录（OpenCode Zen 用它过免费档的三道校验） */
  clientEmulation?: string
  /** 该家账号的默认取值（表单初始勾选态）：noAuth = 预勾「该上游无需鉴权」 */
  account?: { noAuth?: boolean }
  /** 余额查询方案：创建时随记录写入 `balanceScheme`（见 `CustomProviderRecord`） */
  balance?: { scheme?: string }
}

export type WebLoginView = {
  label: string
  hint: string
  disabled: boolean
  waiting: boolean
}

/** web-login.js 的控制器（只列本子系统用到的成员） */
export type WebLoginController = {
  provider: string
  syncTexts(): void
  applyState(): void
  start(): void
  cancel(): void
  dispose(): void
}

/** autoclaw-oauth.js 的控制器 */
export type OauthController = {
  provider: string
  start(vendor: 'zai' | 'google'): void
  syncTexts(): void
  cancel(): void
}

export type SharedWindow = {
  workbuddyDesktop?: {
    /** 壳的编译目标平台（'macos' / 'windows' / 'linux' / 'web'） */
    readonly platform?: string
    startLogin(edition: string, mode: string, provider: string, socialRestore?: boolean, options?: LoginOptions): Promise<unknown>
    submitLoginCallback?: (state: string, callbackUrl: string) => Promise<unknown>
    onLoginState?: (callback: (progress: LoginProgress) => void) => (() => void) | void
    onLoginProgress?: (callback: (progress: LoginProgress) => void) => (() => void) | void
    cancelLogin?(): Promise<unknown>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok' | 'warn') => void
    refresh?: () => Promise<unknown> | unknown
    getState?: () => {
      accounts?: { accounts?: AccountRecord[]; providers?: RuntimeProviderRecord[] }
    } | null | undefined
    /** 切页（app.js 的出口）：空态里「去安装插件包」用它跳到插件页 */
    showPage?: (page: string) => void
  }
  /** 网页登录引擎（web-login.js）：本子系统只调 create / refresh / cancelIfActive */
  wbWebLogin?: {
    create(config: {
      provider: string
      buttonId: string
      cancelId: string
      hintId: string
      busyText?: string
      texts?: () => { button?: string; hint?: string }
      onStateChange?: (state: WebLoginView) => void
      start: () => unknown
      onSuccess: (result?: unknown) => unknown
    }): WebLoginController | null | undefined
    refresh?: () => Promise<unknown>
    isActive?: () => boolean
    activeProvider?: () => string
    cancelIfActive?: (provider: string) => Promise<boolean>
  }
  /** 手机验证码引擎（sms-login.js）：只调 create，交互全在引擎里 */
  wbSmsLogin?: {
    create(config: { provider: string; onSuccess: (data?: unknown) => unknown }): unknown
  }
  /** AutoClaw 国际版 OAuth 引擎（autoclaw-oauth.js）：create / cancel（模块级遍历） */
  wbAutoclawOauth?: {
    create(config: {
      provider: string
      mode: () => string
      hint: () => string
      cancelId?: string
      onSuccess: (result?: unknown) => unknown
    }): OauthController | null | undefined
    cancel?: () => void
  }
  /** 自定义提供商的目录操作（custom-provider-ui.js，账号页那一侧的实现） */
  wbCustomProvidersUi?: {
    remove?: (providerId: string) => Promise<boolean>
  }
  /** 本弹窗的命令式外壳（本文件所在子系统提供，跨模块调用走它） */
  wbAddAccountModal?: { open?: () => void; close?: () => void }
  /** 各家表单块的注册表（本子系统提供；account-panel / 模型管理页要调它的两个入口） */
  wbAccountAddForms?: {
    syncAddProvider?: () => void
    openNewCustomForm?: () => void
  }
  wbProviders?: {
    all?: () => RuntimeProviderRecord[]
    labelOf?: (id: string) => string
    load?: () => Promise<unknown>
    customList?: () => CustomProviderRecord[]
    refreshCustom?: () => Promise<CustomProviderRecord[] | unknown>
    customRequest?: (method: string, path: string, body?: unknown) => Promise<unknown>
    PROTOCOL_OPTIONS?: Array<{ value: string; label: string }>
  }
  wbPresetProviders?: {
    list?: PresetRecord[]
    presetOf?: (key: string) => PresetRecord | null
    iconOf?: (key: string) => string
  }
  /** 壳的原始 IPC：POST /api/accounts 在桥里没有具名方法，只能直连 */
  __TAURI_INTERNALS__?: { invoke?: (cmd: string, args: unknown) => Promise<unknown> }
}

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 通用小工具 ─────────────────────────────── */

export function toast(message: string, kind?: 'err' | 'ok' | 'warn'): void {
  shared().wbApp?.toast?.(message, kind)
}

/** 错误只取文字字段，不序列化整个响应（其中可能带凭证）。 */
function errorText(error: unknown, depth = 0): string {
  if (typeof error === 'string') return error.trim()
  if (error instanceof Error) return error.message.trim()
  if (!error || typeof error !== 'object' || depth > 3) return ''
  const record = error as Record<string, unknown>
  for (const key of ['message', 'error', 'msg', 'reason']) {
    const text = errorText(record[key], depth + 1)
    if (text) return text
  }
  return ''
}

/** Tauri rejection 可能是裸字符串；结构化错误只取 message/error 等文字。 */
export function describeError(error: unknown): string {
  const text = errorText(error)
  return text ? t(text) : t('未知错误')
}

/** 优先读取 HTTP 状态；原生桥仅返回文字时，只认网关明确的能力缺失文案。 */
export function isMethodsUnavailable(error: unknown, depth = 0): boolean {
  if (error && typeof error === 'object' && depth < 4) {
    const record = error as Record<string, unknown>
    for (const key of ['status', 'statusCode', 'status_code', 'httpStatus']) {
      const value = record[key]
      if ((typeof value === 'number' || typeof value === 'string') && Number(value) === 501) return true
    }
    if (record.code === 'not_implemented' || record.code === 'auth_methods_unavailable') return true
    if (record.error && record.error !== error && isMethodsUnavailable(record.error, depth + 1)) return true
  }
  const text = errorText(error)
  return text === '该提供商尚未申报动态登录方式，请使用既有登录入口'
    || /\b501\b|auth\.methods[^\n]*(?:not implemented|no method|未实现|不支持)/i.test(text)
}

export function runtimeProviderOf(provider: string): RuntimeProviderRecord | undefined {
  if (isSuspendedProvider(provider)) return undefined
  const live = shared().wbApp?.getState?.()?.accounts?.providers
  const list = Array.isArray(live) ? live : shared().wbProviders?.all?.() || []
  return list.find(item => item.id === provider)
}

/** 该提供商此刻名下的账号数（读主状态的全量列表，不跟筛选走） */
export function accountCountOf(providerId: string): number {
  const accounts = shared().wbApp?.getState?.()?.accounts?.accounts || []
  return accounts.filter(account => (account?.provider || 'workbuddy') === providerId).length
}

/* ─── 提交与收尾 ─────────────────────────────── */

/** 统一管理 API 入口，兼容原始 data 与 success/data 信封，不把失败信封当成功。 */
export async function accountRequest(method: string, path: string, body?: unknown): Promise<unknown> {
  const internals = shared().__TAURI_INTERNALS__
  if (!internals || typeof internals.invoke !== 'function') {
    throw new Error(t('桌面运行时不可用（Tauri 未初始化）'))
  }
  const result = await internals.invoke('api_request', {
    request: { method, path, body: body === undefined ? null : body },
  })
  if (result && typeof result === 'object') {
    const record = result as Record<string, unknown>
    if (record.success === false) throw result
    if (record.success === true && Object.hasOwn(record, 'data')) return record.data
  }
  return result
}

/** 统一提交入口：POST /api/accounts，保留各提供商自己的凭证字段。 */
export function postAccount(payload: Record<string, unknown>): Promise<unknown> {
  return accountRequest('POST', '/api/accounts', payload)
}

function resultRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined
}

/** 只抽取公开的账号标识，不把原始凭证响应留在组件状态。 */
function publicAccount(value: unknown): AccountRecord | undefined {
  const record = resultRecord(value)
  if (!record) return undefined
  const account: AccountRecord = {}
  for (const key of ['id', 'name', 'uid', 'userId', 'provider'] as const) {
    if (typeof record[key] === 'string') account[key] = record[key]
  }
  return account.id || account.uid || account.userId ? account : undefined
}

export function normalizeLoginResult(value: unknown): LoginResult {
  let record = resultRecord(value)
  if (record?.success === false) throw value
  if (record?.success === true && Object.hasOwn(record, 'data')) record = resultRecord(record.data)
  const payload = resultRecord(record?.payload)
  const session = resultRecord(record?.session) || resultRecord(payload?.session)
  const sources = [record, payload, session].filter((item): item is Record<string, unknown> => Boolean(item))
  if (sources.some(item => item.canceled === true || item.state === 'canceled' || item.status === 'canceled')) {
    return { ok: false, canceled: true, silent: sources.some(item => item.silent === true) }
  }
  for (const item of sources) {
    if (item.error || item.ok === false || item.success === false || item.state === 'failed'
      || item.status === 'failed' || item.type === 'error') {
      throw new Error(describeError(item.error || item))
    }
    if (item.pending === true || resultRecord(item.pending) || item.accepted === true
      || item.done === false || item.state === 'pending' || item.status === 'pending') {
      throw new Error(t('登录尚未完成，请继续通过共享登录桥等待结果'))
    }
  }
  const account = sources.map(item => publicAccount(item.account)).find(Boolean)
  const summary = sources.find(item =>
    (typeof item.accountUid === 'string' && Boolean(item.accountUid))
    || (typeof item.accountId === 'string' && Boolean(item.accountId))
    || (typeof item.nickname === 'string' && Boolean(item.nickname)))
  if (!account && !summary && sources.some(item => (item.flow || item.authUrl) && item.done !== true)) {
    throw new Error(t('登录尚未完成，请继续通过共享登录桥等待结果'))
  }
  if (!account && !summary && record?.ok !== true) {
    throw new Error(session && !Object.keys(session).length
      ? t('登录桥返回了空会话，无法确认账号已添加')
      : t('登录桥未返回已完成的账号或会话，请重试'))
  }
  const text = (key: string): string | undefined => {
    const value = sources.map(item => item[key]).find(item => typeof item === 'string' && item.trim())
    return typeof value === 'string' ? value : undefined
  }
  const proxySaved = sources.map(item => item.proxySaved).find(item => typeof item === 'boolean')
  return {
    ok: true, canceled: false, account, warning: text('warning'), instructions: text('instructions'),
    ...(typeof proxySaved === 'boolean' ? { proxySaved } : {}),
    // 会话只保留公开标识字段，不搬整个响应。
    session: summary ? Object.fromEntries(['accountUid', 'accountId', 'nickname', 'edition', 'provider']
      .filter(key => typeof summary[key] === 'string').map(key => [key, summary[key]])) : undefined,
  }
}

/**
 * provider-first 的岛内入口；现有 window bridge 保持 edition/mode/provider 的旧顺序。
 * 新 options 只能在第 5 个参数，不能顶掉第 4 个 socialRestore。
 * 动态方法在桌面直接交给 start_login 的 options 参数；等待与回调由现有桥负责。
 */
export async function startLogin(
  provider: string, edition = 'cn', mode = 'embedded', socialRestore?: boolean, options?: LoginOptions,
): Promise<LoginResult> {
  // 只有带 methodIndex 的才是动态方法；只带 proxy 的是旧入口加代理
  // （内置表单的网页登录，见 add-provider-blocks 的 WebLoginSection）。
  // 两者的收尾判定不同：动态方法必须回来一个账号 id，旧入口照旧不看。
  const dynamic = options !== undefined && options.methodIndex !== undefined
  if (isSuspendedProvider(provider) || (dynamic && !runtimeProviderOf(provider))) {
    throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
  }
  try {
    const bridge = shared().workbuddyDesktop
    let result: unknown
    if (dynamic && bridge?.platform !== 'web') {
      const internals = shared().__TAURI_INTERNALS__
      if (!internals?.invoke) throw new Error(t('桌面运行时不可用（Tauri 未初始化）'))
      result = await internals.invoke('start_login', {
        provider, edition, mode, socialRestore: socialRestore === true, options,
      })
    } else {
      if (!bridge?.startLogin) throw new Error(t('共享登录桥不可用，请重新打开面板'))
      result = await bridge.startLogin(edition, mode, provider, socialRestore, options)
    }
    const normalized = normalizeLoginResult(result)
    if (dynamic && !normalized.canceled) {
      const accountId = normalized.account?.id || normalized.session?.accountUid || normalized.session?.accountId
      const resultProvider = normalized.account?.provider || normalized.session?.provider
      if (!accountId || resultProvider && resultProvider !== provider) {
        throw new Error(t('登录桥返回了空会话，无法确认账号已添加'))
      }
    }
    return normalized
  } catch (failure) {
    const record = resultRecord(failure)
    if (record?.canceled === true) return { ok: false, canceled: true, silent: record.silent === true }
    // web-login.js 按 error.message 展示错误；原始 invoke 的 rejection 可能只是 String。
    throw new Error(describeError(failure))
  }
}

/** 添加成功后展示的账号名：公开形态里 name 一定在，标识字段按各家兜底 */
export function addedLabelOf(account?: AccountRecord | null): string {
  return account?.name || account?.uid || account?.userId || ''
}

/**
 * 关掉弹窗。真正的关闭动作（含登录等待的取消）归岛的命令式外壳
 * （add-account-modal.tsx 的 closeModal），这里只是把它统一叫出来 ——
 * 脚本 / 模块顺序被改坏时退回直接摘 window 引用。
 */
export function closeAddModals(): void {
  shared().wbAddAccountModal?.close?.()
}

/** 添加成功后统一收尾：关窗、刷新列表、提示 */
export async function afterAdd(name: string, label: string): Promise<void> {
  closeAddModals()
  await shared().wbApp?.refresh?.()
  toast(t('✅ {label}账号已添加{name}', { label, name: name ? `：${name}` : '' }))
}

/* ─── 表单草稿（非受控输入的「关掉再打开还在」）──────────────
 *
 * 旧实现的表单块在加载期注入 DOM 后就再也不卸载：值由浏览器持有，关掉表单弹窗
 * （甚至关掉整个弹窗）再打开时输入框里还是上次的内容，只有添加成功才清空。
 * 岛的表单块会随弹窗卸载，这份模块级草稿就是那份状态的替身 ——
 * 非受控输入（defaultValue + onChange 回写）在重新挂载时从这里取初值。
 *
 * 引擎（web-login / sms-login / autoclaw-oauth）按 id 现读 `.value`，
 * 所以控件仍必须是真实的原生 input / textarea，且 id 与旧实现逐字一致。 */

const draft = new Map<string, string>()

export function draftProps(id: string): {
  defaultValue: string
  onChange: (event: { currentTarget: { value: string } }) => void
} {
  return {
    defaultValue: draft.get(id) ?? '',
    onChange: event => {
      const value = event.currentTarget.value
      if (value) draft.set(id, value)
      else draft.delete(id)
    },
  }
}

/** 手动设值（预置家预填）：草稿与 DOM 一起写，两处不能分叉 */
export function setDraftValue(id: string, value: string): void {
  if (value) draft.set(id, value)
  else draft.delete(id)
  const node = document.getElementById(id)
  if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) node.value = value
}

/** 读当前值：优先 DOM（用户可能刚改过），取不到退回草稿 */
export function readField(id: string): string {
  const node = document.getElementById(id)
  if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) return node.value.trim()
  return (draft.get(id) ?? '').trim()
}

/** 清空若干字段（添加成功后调用；失败时保留内容方便改动重试） */
export function clearFields(ids: string[]): void {
  for (const id of ids) setDraftValue(id, '')
}

/* ─── 弹窗里的登录偏好 ─────────────────────────
 *
 * 旧实现里这三处住在静态 DOM 与模块级 segState 上，随页面存活 ——
 * 关掉弹窗再打开仍是上次的选择（resetAddStep 只复位提供商与步骤，不动它们）。
 * 迁到岛上后用这份模块级对象保真：组件以它为初值，改动同时写回。 */

export type Edition = 'cn' | 'intl'
export type LoginMode = 'embedded' | 'external'

export const loginPrefs: { edition: Edition; loginMode: LoginMode; socialRestore: boolean } = {
  edition: 'cn',
  loginMode: 'embedded',
  socialRestore: false,
}
