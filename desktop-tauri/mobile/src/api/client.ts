/**
 * 移动端与网关之间的 HTTP 客户端。
 *
 * ── 两种运行形态，一套代码 ────────────────────────────────────
 *   · **PWA / 浏览器**：页面由网关出，`/api/*` 是同源相对路径，服务端用
 *     HttpOnly cookie 认会话 —— 与桌面网页端完全一致。
 *   · **原生 App**：界面打进包里，页面 origin 是原生壳的本地地址，请求网关
 *     必然跨源。跨源请求浏览器不会带 cookie，所以必须显式把令牌放进请求头
 *     （`x-panel-token` / `x-panel-refresh`），并且登录时要让服务端把令牌
 *     放进响应体（`x-panel-auth-mode: body`，见服务端 `tokens_in_body`）。
 *
 * 两条路径的差别只在「地址怎么拼」与「令牌从哪来」，收敛在 `request()`
 * 一处，业务层不必区分。
 *
 * ── 凭证按服务器隔离 ──────────────────────────────────────────
 * 见 lib/servers.ts：令牌是按服务器签发的，混着存会让切换后立刻 401。
 *
 * ── 401 的两种语义 ────────────────────────────────────────────
 *   · `error.type === 'panel_login_required'` → 管理员已注册但会话没了：
 *     先静默 refresh，失败才交给界面弹登录；
 *   · 其余 401 → 兼容模式没配 Key：交给界面提示输入。
 */

import type { PanelCaptchaStatus } from '../../../ui-shared/panel-captcha'

import {
  activeServerId,
  apiBase,
  clearServerAuth,
  isPackaged,
  readServerAuth,
  readServerKey,
  serverIdentity,
  storeServerAuth,
  storeServerKey,
  type PanelAuth,
} from '../lib/servers'

/** 管理 API 的统一错误 */
export class ApiError extends Error {
  status: number
  /** 后端 `error.type`（`panel_login_required` 之类），没有则为空串 */
  type: string
  constructor(message: string, status: number, type = '') {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.type = type
  }
}

/** 把相对路径拼成完整地址（PWA 下原样返回，交给浏览器按同源处理） */
export function url(path: string): string {
  if (!isPackaged()) return path
  const base = apiBase()
  if (!base) throw new ApiError('还没有配置服务器地址', 0)
  return `${base}${path}`
}

/** 把相对路径拼成完整地址（PWA 下原样返回，交给浏览器按同源处理） */
export function resolveUrl(path: string): string {
  return url(path)
}

/**
 * 当前会话的鉴权头（给「不能走 apiCall」的调用方用，例如 SSE 流）。
 *
 * 与 {@link apiCall} 内部 `buildHeaders` 是同一份实现 —— 两处各写一份的话，
 * 「登录过期」在不同页面上会表现成不同的样子。
 */
export function authHeaders(extra?: Record<string, string>): Record<string, string> {
  return buildHeaders(extra)
}

/** 静默续期（对话流 401 时用）。成功返回 true */
export function refreshPanelAuth(): Promise<boolean> {
  return tryRefresh()
}

/**
 * 到期前主动续期（给不走 `apiCall` 的调用方用，例如 SSE 对话流与文件上传）。
 *
 * `apiCall` 内部已经调过它；这些直连 fetch 的路径没有，所以要在发请求前自己
 * 调一次，否则它们仍会在 access 过期那一刻产生一条 401 —— 而那正是本次要
 * 消掉的噪音（见 `ensureFreshSession` 的说明）。永不抛错。
 */
export function refreshPanelAuthIfExpiring(): Promise<void> {
  return ensureFreshSession()
}

export function readApiKey(): string {
  return readServerKey(activeServerId())
}

export function storeApiKey(value: string): void {
  storeServerKey(activeServerId(), value)
}

export function readPanelAuth(): PanelAuth | null {
  return readServerAuth(activeServerId())
}

export function storePanelTokens(access: string, refresh: string, expiresIn?: number): void {
  storeServerAuth(activeServerId(), access, refresh, expiresIn)
}

export function clearPanelAuth(): void {
  clearServerAuth(activeServerId())
}

/** 会话是否还有效（只用于界面提示，不阻塞渲染） */
export function hasStoredCredential(): boolean {
  const auth = readPanelAuth()
  return Boolean(readApiKey() || auth?.access)
}

/** 401 时由界面接管（弹登录 / 输入 Key）；客户端自己不做跳转 */
let onAuthLost: ((mode: 'login' | 'key') => void) | null = null
export function setAuthLostHandler(handler: ((mode: 'login' | 'key') => void) | null): void {
  onAuthLost = handler
}

function buildHeaders(extra?: Record<string, string>): Record<string, string> {
  const headers: Record<string, string> = { Accept: 'application/json', ...extra }
  const apiKey = readApiKey()
  if (apiKey) headers['x-api-key'] = apiKey
  const auth = readPanelAuth()
  if (auth?.access) {
    headers['x-panel-token'] = auth.access
    // Authorization 是标准头：连自定义头都剥的中转也会放它过去
    headers.Authorization = `Bearer ${auth.access}`
  }
  return headers
}

/** 续期按服务器隔离；换服务器后不能沿用上一台的在途令牌轮换。 */
const refreshInFlight = new Map<string, Promise<boolean>>()

function requireCurrentServer(identity: string): void {
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
}

/**
 * 静默续期，并发 401 单飞：access 过期那一刻页面会同时收到多个 401，
 * 各自去换的话，后到的带着同一把（刚被轮换的）refresh 上行，只能靠服务端
 * 的宽限窗兜底（`access::ROTATION_GRACE_MS`）。这里同一时刻只放行一次轮换
 * 请求，其余等同一个结果 —— 先到者换到的新令牌直接被所有等待者共用。
 * 成功返回 true
 */
async function tryRefresh(): Promise<boolean> {
  const identity = serverIdentity()
  const pending = refreshInFlight.get(identity)
  if (pending) return pending
  const request = doRefresh(identity).finally(() => refreshInFlight.delete(identity))
  refreshInFlight.set(identity, request)
  return request
}

async function doRefresh(identity: string): Promise<boolean> {
  const serverId = activeServerId()
  const auth = readServerAuth(serverId)
  const headers = buildHeaders({ 'Content-Type': 'application/json' })
  if (auth?.refresh) {
    headers['x-panel-refresh'] = auth.refresh
    // 要求令牌进响应体：APK 下没有可用的 cookie 通道，只认 body
    headers['x-panel-auth-mode'] = 'body'
  }
  try {
    const response = await fetch(url('/api/panel/refresh'), {
      method: 'POST',
      headers,
      body: '{}',
    })
    if (!response.ok) return false
    const payload = (await response.json().catch(() => null)) as
      | { data?: { accessToken?: string; refreshToken?: string; expiresIn?: number } }
      | null
    if (serverIdentity() !== identity) return false
    const data = payload?.data
    if (data?.accessToken && data?.refreshToken) {
      storeServerAuth(serverId, data.accessToken, data.refreshToken, data.expiresIn)
    }
    return true
  } catch {
    return false
  }
}

/**
 * 主动续期的提前量（毫秒）：到期前这么久就把令牌换掉。
 *
 * 两分钟够覆盖「这一轮请求刚好在到期瞬间发出」的抖动，又远小于 2 小时的寿命 ——
 * 不会因为时钟偏差或一次慢请求就提前换掉刚签发的令牌（那会白耗一次轮换）。
 */
const REFRESH_AHEAD_MS = 2 * 60 * 1000

/**
 * 到期前主动续期：把「access 过期 → 401 → 续期」提前到过期之前。
 *
 * 只靠 401 被动续期时，**每次**过期都必然产生一条 401（服务端按「refresh
 * 还活着」已把它降成 verbose，但请求本身仍是一次多余往返）。手机端挂着不动
 * 时也一样会撞上，这里从源头把那条请求消掉。
 *
 * 失败一律静默放行，让请求照常发出去：`doRefresh` 返回 false 可能是会话链
 * 真没了（该走 401 → 弹登录），也可能只是网络抖动。真正的判据只有服务端知道，
 * 交回给 `apiCall` 里既有的 401 分支处理。
 */
async function ensureFreshSession(): Promise<void> {
  const auth = readPanelAuth()
  const expiresAt = Number(auth?.expiresAt)
  if (!Number.isFinite(expiresAt) || expiresAt <= 0) return
  if (Date.now() < expiresAt - REFRESH_AHEAD_MS) return
  await tryRefresh()
}

type RequestInitLite = {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  body?: unknown
  /** 不触发 401 接管（探活类请求用） */
  quiet?: boolean
  signal?: AbortSignal
  apiKey?: string
}

/**
 * 发一条管理 API 请求并解开 `{success, data}` 信封。
 *
 * 返回值是 `data` 本身；失败一律抛 {@link ApiError}。
 */
export function apiCall<T = unknown>(path: string, init: RequestInitLite = {}): Promise<T> {
  return requestApi<T>(path, init, serverIdentity(), true)
}

async function requestApi<T>(path: string, init: RequestInitLite, identity: string, retry: boolean): Promise<T> {
  requireCurrentServer(identity)
  if (init.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
  await ensureFreshSession()
  requireCurrentServer(identity)
  const method = init.method || 'GET'
  const headers = buildHeaders()
  if (init.apiKey !== undefined) {
    headers['x-api-key'] = init.apiKey
    headers.Authorization = `Bearer ${init.apiKey}`
  }
  const requestInit: RequestInit = { method, headers, signal: init.signal }
  if (init.body !== undefined || method !== 'GET') {
    headers['Content-Type'] = 'application/json'
    requestInit.body = JSON.stringify(init.body ?? {})
  }

  let response: Response
  try {
    response = await fetch(url(path), requestInit)
  } catch (error) {
    requireCurrentServer(identity)
    if (init.signal?.aborted || (error instanceof DOMException && error.name === 'AbortError')) throw error
    throw new ApiError(`无法连接网关：${error instanceof Error ? error.message : String(error)}`, 0)
  }

  const text = await response.text()
  requireCurrentServer(identity)
  let payload: unknown = null
  try {
    payload = text ? JSON.parse(text) : null
  } catch {
    throw new ApiError(text.trim() || `网关返回了空响应（HTTP ${response.status}）`, response.status)
  }

  if (response.status === 401 && !init.quiet) {
    const type = readErrorType(payload)
    if (type === 'panel_login_required') {
      const refreshed = retry && !path.startsWith('/api/assistant/images/') && await tryRefresh()
      requireCurrentServer(identity)
      if (refreshed) return requestApi<T>(path, init, identity, false)
      clearPanelAuth()
      onAuthLost?.('login')
    } else if (init.apiKey === undefined) {
      onAuthLost?.('key')
    }
  }

  const envelope = payload as { success?: boolean; data?: unknown; error?: unknown } | null
  const ok = response.status >= 200 && response.status < 300
  const failed = envelope?.success === false
  if (!ok || failed) {
    throw new ApiError(errorMessage(envelope?.error, response.status), response.status, readErrorType(payload))
  }
  return (envelope && 'data' in envelope ? envelope.data : envelope) as T
}

function readErrorType(payload: unknown): string {
  const error = (payload as { error?: unknown } | null)?.error
  if (error && typeof error === 'object' && 'type' in error) {
    const type = (error as { type?: unknown }).type
    return typeof type === 'string' ? type : ''
  }
  return ''
}

function errorMessage(error: unknown, status: number): string {
  if (typeof error === 'string' && error) return error
  if (error && typeof error === 'object') {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string' && message) return message
  }
  return `HTTP ${status}`
}

/** 探活：不带凭证也能打（`/health` 免鉴权） */
export async function ping(): Promise<boolean> {
  try {
    const response = await fetch(url('/health'), { headers: { Accept: 'application/json' } })
    return response.ok
  } catch {
    return false
  }
}

/** 探活并带回版本信息（配置服务器时用，能顺便显示是不是本产品） */
export async function probe(base: string): Promise<{ ok: boolean; version?: string }> {
  try {
    const response = await fetch(`${base}/health`, { headers: { Accept: 'application/json' } })
    if (!response.ok) return { ok: false }
    const payload = (await response.json().catch(() => null)) as { version?: string; data?: { version?: string } } | null
    return { ok: true, version: payload?.version || payload?.data?.version }
  } catch {
    return { ok: false }
  }
}

/** 面板登录状态（决定「登录」还是「创建管理员」） */
export type PanelStatus = PanelCaptchaStatus & { registered?: boolean }
export async function panelStatus(): Promise<PanelStatus> {
  const response = await fetch(url('/api/panel/status'), { headers: { Accept: 'application/json' } })
  const payload = (await response.json().catch(() => null)) as { data?: PanelStatus } | null
  return payload?.data || {}
}
