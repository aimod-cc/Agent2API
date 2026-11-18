/** CodeArts 首次登记的固定 IPC 与公开结果；不解析凭据原文、不访问账号或代理。 */
import { getPluginStartup, subscribePluginStartup } from './plugin-startup'
import { managedLoginSourceGeneration } from './plugin-login-managed'

export const CODEARTS_ENROLLMENT_BASE = '/api/plugins/enrollment/codearts'
export const CODEARTS_CREDENTIAL_LIMIT = 128 * 1024
const PHASES = ['awaiting_confirmation', 'saving', 'saved', 'failed', 'canceled', 'expired'] as const
export type CodeArtsEnrollmentPhase = (typeof PHASES)[number]
export type CodeArtsEnrollmentAccount = { id: string; provider: 'codearts' }
export type CodeArtsEnrollmentPayload = {
  provider: 'codearts'; intent: string; phase: CodeArtsEnrollmentPhase
  saved: boolean | null; readyRegistered: boolean | null; storageClosed: boolean | null
  expiresAt: number; remainingMs: number; message: string; account: CodeArtsEnrollmentAccount | null
  package: { id: string; version: string; archiveDigest: string; moduleDigest: string; declarationDigest: string }
}
export type CodeArtsEnrollmentRequest = <T>(method: 'GET' | 'POST', path: string, body: unknown,
  options: { retry: 'never' }) => Promise<T>
export type CodeArtsEnrollmentSource = { current(): boolean; request: CodeArtsEnrollmentRequest }
type ShellWindow = Window & {
  workbuddyDesktop?: { platform?: string }
  __TAURI_INTERNALS__?: { invoke?: (command: string, args: unknown) => Promise<unknown> }
  wbApp?: {
    getState?: () => { pluginStartup?: { bootId?: string; availabilityRevision?: string } } | null
    subscribeRefresh?: (listener: () => void) => (() => void) | undefined
  }
}
const shell = () => window as ShellWindow
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value)
const exact = (value: unknown, keys: readonly string[]): value is Record<string, unknown> => record(value)
  && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key))
const text = (value: unknown, limit: number): value is string => typeof value === 'string' && value.length > 0
  && value.length <= limit && value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value)
const digest = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value)
const triState = (value: unknown): value is boolean | null => value === null || typeof value === 'boolean'

/** 仅核原文 UTF-8 大小；不 trim 后发送、不 JSON.parse 或重新序列化凭据。 */
export function codeArtsCredentialFits(value: string): boolean {
  return value.length > 0 && value.length <= CODEARTS_CREDENTIAL_LIMIT && Boolean(value.trim())
    && new TextEncoder().encode(value).byteLength <= CODEARTS_CREDENTIAL_LIMIT
}
function startupIdentity(host: ShellWindow): string {
  const startup = getPluginStartup().value
  const fallback = host.wbApp?.getState?.()?.pluginStartup
  const bootId = startup?.bootId || fallback?.bootId || ''
  if (startup?.bootId && fallback?.bootId && startup.bootId !== fallback.bootId) return ''
  return bootId ? JSON.stringify([startup?.bootId || '', fallback?.bootId || '',
    startup?.availabilityRevision || '', fallback?.availabilityRevision || '']) : ''
}
export function codeArtsEnrollmentAvailable(): boolean {
  return shell().workbuddyDesktop?.platform === 'windows' && typeof shell().__TAURI_INTERNALS__?.invoke === 'function'
    && Boolean(startupIdentity(shell()))
}
export function subscribeCodeArtsEnrollmentSource(listener: () => void): () => void {
  const unsubscribeStartup = subscribePluginStartup(listener)
  const unsubscribeRefresh = shell().wbApp?.subscribeRefresh?.(listener)
  return () => { unsubscribeStartup(); unsubscribeRefresh?.() }
}

function fixedRequest(method: 'GET' | 'POST', path: string, body: unknown): boolean {
  if (method === 'POST' && path === `${CODEARTS_ENROLLMENT_BASE}/prepare`) {
    return exact(body, ['credentialJson']) && typeof body.credentialJson === 'string' && codeArtsCredentialFits(body.credentialJson)
  }
  if (method === 'POST' && path === `${CODEARTS_ENROLLMENT_BASE}/confirm`) {
    return exact(body, ['intent', 'confirmation']) && text(body.intent, 512)
      && exact(body.confirmation, ['schema', 'initialRegistration', 'oneOpportunity'])
      && body.confirmation.schema === 1 && body.confirmation.initialRegistration === true && body.confirmation.oneOpportunity === true
  }
  if (method === 'POST' && path === `${CODEARTS_ENROLLMENT_BASE}/cancel`) return exact(body, ['intent']) && text(body.intent, 512)
  if (method !== 'GET' || body !== undefined && body !== null) return false
  const prefix = `${CODEARTS_ENROLLMENT_BASE}/status?intent=`
  if (!path.startsWith(prefix)) return false
  try {
    const intent = decodeURIComponent(path.slice(prefix.length))
    return text(intent, 512) && path === `${prefix}${encodeURIComponent(intent)}`
  } catch { return false }
}

/** 原窗口、壳、IPC 对象/入口、启动代次及管理代次一起封闭；新来源不能接续旧意图。 */
export function captureCodeArtsEnrollmentSource(enabled: () => boolean): CodeArtsEnrollmentSource | null {
  if (!codeArtsEnrollmentAvailable() || !enabled()) return null
  const host = shell(), documentSource = document, desktop = host.workbuddyDesktop, ipc = host.__TAURI_INTERNALS__
  const invoke = ipc?.invoke
  if (!invoke) return null
  const origin = host.location.origin, startup = startupIdentity(host), generation = managedLoginSourceGeneration()
  const current = () => window === host && document === documentSource && host.location.origin === origin
    && host.workbuddyDesktop === desktop && desktop?.platform === 'windows'
    && host.__TAURI_INTERNALS__ === ipc && ipc?.invoke === invoke
    && Boolean(startup) && startupIdentity(host) === startup && generation === managedLoginSourceGeneration() && enabled()
  const request: CodeArtsEnrollmentRequest = async <T,>(method: 'GET' | 'POST', path: string, body: unknown,
    options: { retry: 'never' }): Promise<T> => {
    if (!current() || options.retry !== 'never' || !fixedRequest(method, path, body)) throw new Error('codearts_enrollment_unavailable')
    // 只调用具名 IPC 一次；不转普通 POST、不恢复认证后重发，也不缓存原文。
    const pending = invoke.call(ipc, 'api_request', { request: { method, path, body: body === undefined ? null : body, retry: 'never' } })
    body = undefined
    const raw = await pending
    if (!current()) throw new Error('codearts_enrollment_source_changed')
    if (record(raw) && raw.success === false) throw new Error('codearts_enrollment_failed')
    return (record(raw) && raw.success === true && Object.hasOwn(raw, 'data') ? raw.data : raw) as T
  }
  return { current, request }
}

/** 只保留合同中的公开字段；saved=true 的提交事实不依赖 phase 或存储收尾是否成功。 */
export function readCodeArtsEnrollmentPayload(raw: unknown): CodeArtsEnrollmentPayload {
  if (record(raw) && raw.success === true) raw = raw.data
  if (!exact(raw, ['provider', 'intent', 'phase', 'saved', 'readyRegistered', 'storageClosed', 'expiresAt', 'remainingMs', 'package', 'message', 'account'])
    || raw.provider !== 'codearts' || !text(raw.intent, 512) || !PHASES.includes(raw.phase as CodeArtsEnrollmentPhase)
    || !triState(raw.saved) || !triState(raw.readyRegistered) || !triState(raw.storageClosed)
    || typeof raw.expiresAt !== 'number' || !Number.isSafeInteger(raw.expiresAt) || raw.expiresAt <= 0
    || typeof raw.remainingMs !== 'number' || !Number.isSafeInteger(raw.remainingMs) || raw.remainingMs < 0 || raw.remainingMs > 2_147_483_647
    || typeof raw.message !== 'string' || raw.message.length > 8192) throw new Error('invalid_codearts_enrollment')
  const pkg = raw.package
  if (!exact(pkg, ['id', 'version', 'archiveDigest', 'moduleDigest', 'declarationDigest']) || !text(pkg.id, 128) || !text(pkg.version, 128)
    || !digest(pkg.archiveDigest) || !digest(pkg.moduleDigest) || !digest(pkg.declarationDigest)) throw new Error('invalid_codearts_enrollment_package')
  let account: CodeArtsEnrollmentAccount | null = null
  if (raw.account !== null) {
    if (!exact(raw.account, ['id', 'provider']) || !text(raw.account.id, 1024) || raw.account.provider !== 'codearts'
      || raw.saved !== true) throw new Error('invalid_codearts_enrollment_account')
    account = { id: raw.account.id, provider: 'codearts' }
  }
  if (raw.phase === 'saved' && raw.saved !== true) throw new Error('invalid_codearts_enrollment_saved')
  return { provider: 'codearts', intent: raw.intent, phase: raw.phase as CodeArtsEnrollmentPhase,
    saved: raw.saved, readyRegistered: raw.readyRegistered, storageClosed: raw.storageClosed,
    expiresAt: raw.expiresAt, remainingMs: raw.remainingMs, message: raw.message, account,
    package: { id: pkg.id, version: pkg.version, archiveDigest: pkg.archiveDigest, moduleDigest: pkg.moduleDigest, declarationDigest: pkg.declarationDigest } }
}
export function codeArtsEnrollmentIdentity(value: CodeArtsEnrollmentPayload): string {
  return JSON.stringify([value.provider, value.intent, value.expiresAt, value.package])
}
