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

/** 目录条目：自定义提供商（wbProviders.customList）与预置目录（wbPresetProviders）共用形状 */
export type CustomProviderRecord = {
  id: string
  name?: string
  protocol?: string
  baseUrl?: string
  accountCount?: number
  /** 客户端形态伪装（预置卡写入的记录字段）：'opencode' = 按官方 CLI 形状补齐请求 */
  clientEmulation?: string
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
  quirks?: CustomProviderRecord['quirks']
  /** 客户端形态伪装：随创建写进提供商记录（OpenCode Zen 用它过免费档的三道校验） */
  clientEmulation?: string
  /** 该家账号的默认取值（表单初始勾选态）：noAuth = 预勾「该上游无需鉴权」 */
  account?: { noAuth?: boolean }
}

/** web-login.js 的控制器（只列本子系统用到的成员） */
export type WebLoginController = {
  provider: string
  syncTexts(): void
  applyState(): void
  start(): void
  cancel(): void
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
    startLogin(edition: string, mode: string, provider: string, socialRestore?: boolean): Promise<unknown>
    cancelLogin?(): Promise<unknown>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok' | 'warn') => void
    refresh?: () => Promise<unknown> | unknown
    getState?: () => { accounts?: { accounts?: AccountRecord[] } } | null | undefined
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
      start: () => unknown
      onSuccess: (result?: unknown) => unknown
    }): WebLoginController | null | undefined
    refresh?: () => Promise<unknown>
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
    all?: () => Array<{ id?: string; label?: string; count?: number }>
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

/**
 * 取可读的错误文案。
 *
 * 壳侧命令签名是 `Result<Value, String>`，Tauri 把 Err 里的 String 原样序列化给
 * JS —— rejection 携带的是一个**字符串**而不是 Error 对象，`error.message` 于是
 * 是 undefined（真实踩过：界面显示「导入失败：undefined」）。旧实现的
 * describeError 就是为这条兜底，这里逐字保留。
 */
export function describeError(error: unknown): string {
  if (error instanceof Error && error.message) return error.message
  const text = String(error ?? '').trim()
  return text || t('未知错误')
}

/** 该提供商此刻名下的账号数（读主状态的全量列表，不跟筛选走） */
export function accountCountOf(providerId: string): number {
  const accounts = shared().wbApp?.getState?.()?.accounts?.accounts || []
  return accounts.filter(account => (account?.provider || 'workbuddy') === providerId).length
}

/* ─── 提交与收尾 ─────────────────────────────── */

/** 统一提交入口：POST /api/accounts，保留各提供商自己的凭证字段 */
export async function postAccount(payload: Record<string, unknown>): Promise<unknown> {
  const internals = shared().__TAURI_INTERNALS__
  if (!internals || typeof internals.invoke !== 'function') {
    throw new Error(t('桌面运行时不可用（Tauri 未初始化）'))
  }
  return internals.invoke('api_request', {
    request: { method: 'POST', path: '/api/accounts', body: payload },
  })
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
