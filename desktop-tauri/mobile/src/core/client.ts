import {
  activeServerId,
  apiBase,
  clearServerAuth,
  isPackaged,
  managementSessionGeneration,
  notifyManagementSessionChanged,
  readServerAuth,
  readServerKey,
  serverIdentity,
  storeServerAuth,
  storeServerKey,
  type PanelAuth,
} from './servers'

export type PanelStatus = {
  registered?: boolean
  captchaEnabled?: boolean
  captchaProvider?: 'altcha' | 'turnstile' | string | null
  turnstileSiteKey?: string | null
  captchaError?: string | null
  oauth?: {
    google?: boolean
    github?: boolean
  } | null
}

export class ApiError extends Error {
  status: number
  type: string
  constructor(message: string, status: number, type = '') {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.type = type
  }
}

export function url(path: string): string {
  if (!isPackaged()) return path
  const base = apiBase()
  if (!base) throw new ApiError('还没有配置服务器地址', 0)
  return `${base}${path}`
}

export function resolveUrl(path: string): string {
  return url(path)
}

export function authHeaders(extra?: Record<string, string>): Record<string, string> {
  return buildHeaders(extra)
}

export function refreshPanelAuth(): Promise<boolean> {
  return tryRefresh()
}

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

export function hasStoredCredential(): boolean {
  const auth = readPanelAuth()
  return Boolean(readApiKey() || auth?.access)
}

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
    headers.Authorization = `Bearer ${auth.access}`
  }
  return headers
}

const refreshInFlight = new Map<string, Promise<boolean>>()

function requireCurrentServer(identity: string): void {
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
}

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
    } else if (!isPackaged()) {
      notifyManagementSessionChanged(serverId)
    } else {
      return false
    }
    return true
  } catch {
    return false
  }
}

const REFRESH_AHEAD_MS = 2 * 60 * 1000

async function ensureFreshSession(): Promise<void> {
  const auth = readPanelAuth()
  const expiresAt = Number(auth?.expiresAt)
  if (!Number.isFinite(expiresAt) || expiresAt <= 0) return
  if (Date.now() < expiresAt - REFRESH_AHEAD_MS) return
  await tryRefresh()
}

export type RequestInitLite = {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  body?: unknown
  quiet?: boolean
  noAuthReplay?: boolean
  signal?: AbortSignal
  apiKey?: string
}

export function apiCall<T = unknown>(path: string, init: RequestInitLite = {}): Promise<T> {
  return requestApi<T>(path, init, serverIdentity(), true)
}

async function requestApi<T>(
  path: string,
  init: RequestInitLite,
  identity: string,
  retry: boolean,
): Promise<T> {
  requireCurrentServer(identity)
  if (init.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
  const method = init.method || 'GET'
  const noAuthReplay =
    init.noAuthReplay === true ||
    (method === 'POST' &&
      (path === '/api/session/login/start' ||
        path === '/api/session/login/callback' ||
        path === '/api/session/login/interaction' ||
        path === '/api/accounts/refresh' ||
        path === '/api/accounts/refresh-expiring' ||
        path === '/api/accounts/checkin' ||
        path === '/api/checkin-activity'))

  const management = managementSessionGeneration()
  await ensureFreshSession()
  requireCurrentServer(identity)
  if (noAuthReplay && managementSessionGeneration() !== management) {
    throw new ApiError('管理会话已变化，请重新操作', 401, 'panel_login_required')
  }
  if (init.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
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
    if (init.signal?.aborted || (error instanceof DOMException && error.name === 'AbortError')) {
      throw error
    }
    throw new ApiError(
      `无法连接网关：${error instanceof Error ? error.message : String(error)}`,
      0,
    )
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
      const refreshed = retry && (await tryRefresh())
      requireCurrentServer(identity)
      if (refreshed && !noAuthReplay) return requestApi<T>(path, init, identity, false)
      if (!refreshed) {
        clearPanelAuth()
        onAuthLost?.('login')
      }
    } else if (type === 'invalid_api_key' && init.apiKey === undefined) {
      onAuthLost?.('key')
    }
  }

  const envelope = payload as { success?: boolean; data?: unknown; error?: unknown } | null
  const ok = response.status >= 200 && response.status < 300
  const failed = envelope?.success === false
  if (!ok || failed) {
    throw new ApiError(
      errorMessage(envelope?.error, response.status),
      response.status,
      readErrorType(payload),
    )
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

export async function ping(): Promise<boolean> {
  try {
    const response = await fetch(url('/health'), { headers: { Accept: 'application/json' } })
    return response.ok
  } catch {
    return false
  }
}

export async function probe(base: string): Promise<{ ok: boolean; version?: string; latencyMs?: number }> {
  const started = Date.now()
  try {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), 4500)
    const response = await fetch(`${base}/health`, {
      headers: { Accept: 'application/json' },
      signal: controller.signal,
    }).finally(() => clearTimeout(timer))
    if (!response.ok) return { ok: false }
    const payload = (await response.json().catch(() => null)) as {
      version?: string
      data?: { version?: string }
    } | null
    return {
      ok: true,
      version: payload?.version || payload?.data?.version,
      latencyMs: Math.max(1, Date.now() - started),
    }
  } catch {
    return { ok: false }
  }
}

export async function panelStatus(): Promise<PanelStatus> {
  const response = await fetch(url('/api/panel/status'), { headers: { Accept: 'application/json' } })
  const payload = (await response.json().catch(() => null)) as { data?: PanelStatus } | null
  return payload?.data || {}
}
