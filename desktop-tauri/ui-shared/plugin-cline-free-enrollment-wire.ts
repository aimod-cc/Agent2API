/** Free 首次新账号登记的固定 IPC 与公开结果；不解析凭据，不读取账号或代理。 */
import { getPluginStartup, subscribePluginStartup } from './plugin-startup'
import { managedLoginSourceGeneration } from './plugin-login-managed'

export const CLINE_FREE_ENROLLMENT_BASE = '/api/plugins/enrollment/cline-free'
export const CLINE_FREE_CREDENTIAL_LIMIT = 128 * 1024
const PHASES = ['awaiting_confirmation', 'saving', 'saved', 'failed', 'canceled', 'expired'] as const
export type ClineFreeEnrollmentPhase = (typeof PHASES)[number]
export type ClineFreeEnrollmentAccount = { id: string; provider: 'cline-free' }
export type ClineFreeEnrollmentPayload = {
  provider: 'cline-free'; intent: string; phase: ClineFreeEnrollmentPhase
  saved: boolean | null; readyRegistered: boolean | null; storageClosed: boolean | null
  expiresAt: number; remainingMs: number; message: string; account: ClineFreeEnrollmentAccount | null
  package: { id: string; version: string; archiveDigest: string; moduleDigest: string; declarationDigest: string }
}
export type ClineFreeEnrollmentRequest = <T>(method: 'GET' | 'POST', path: string, body: unknown,
  options: { retry: 'never' }) => Promise<T>
export type ClineFreeEnrollmentSource = { current(): boolean; request: ClineFreeEnrollmentRequest }
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
const intentId = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{48}$/.test(value)
const triState = (value: unknown): value is boolean | null => value === null || typeof value === 'boolean'

/** 仅核 UTF-8 长度；发送同一原文，不 trim 后发送、JSON.parse 或重新拼接。 */
export function clineFreeCredentialFits(value: string): boolean {
  if (!value.length || value.length > CLINE_FREE_CREDENTIAL_LIMIT || !/\S/.test(value)) return false
  // TextEncoder 会替换孤立代理项；拒绝它们，避免 UTF-8 传输悄悄改变原文。
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index)
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++index)
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false
    } else if (unit >= 0xdc00 && unit <= 0xdfff) return false
  }
  return new TextEncoder().encode(value).byteLength <= CLINE_FREE_CREDENTIAL_LIMIT
}

function startupIdentity(host: ShellWindow): string {
  const startup = getPluginStartup().value
  const fallback = host.wbApp?.getState?.()?.pluginStartup
  const bootId = startup?.bootId || fallback?.bootId || ''
  if (startup?.bootId && fallback?.bootId && startup.bootId !== fallback.bootId) return ''
  return bootId ? JSON.stringify([startup?.bootId || '', fallback?.bootId || '',
    startup?.availabilityRevision || '', fallback?.availabilityRevision || '']) : ''
}
export function clineFreeEnrollmentAvailable(): boolean {
  return shell().workbuddyDesktop?.platform === 'windows' && typeof shell().__TAURI_INTERNALS__?.invoke === 'function'
    && Boolean(startupIdentity(shell()))
}
export function subscribeClineFreeEnrollmentSource(listener: () => void): () => void {
  const unsubscribeStartup = subscribePluginStartup(listener)
  const unsubscribeRefresh = shell().wbApp?.subscribeRefresh?.(listener)
  return () => { unsubscribeStartup(); unsubscribeRefresh?.() }
}

function fixedRequest(method: 'GET' | 'POST', path: string, body: unknown): boolean {
  const base = CLINE_FREE_ENROLLMENT_BASE
  if (method === 'POST' && path === `${base}/prepare`) {
    return exact(body, ['credentialJson']) && typeof body.credentialJson === 'string' && clineFreeCredentialFits(body.credentialJson)
  }
  if (method === 'POST' && path === `${base}/confirm`) {
    return exact(body, ['intent', 'confirmation']) && intentId(body.intent)
      && exact(body.confirmation, ['schema', 'newAccountOnly', 'oneOpportunity', 'autoRefresh'])
      && body.confirmation.schema === 2 && body.confirmation.newAccountOnly === true && body.confirmation.oneOpportunity === true
      && body.confirmation.autoRefresh === true
  }
  if (method === 'POST' && path === `${base}/cancel`) return exact(body, ['intent']) && intentId(body.intent)
  if (method !== 'GET' || body !== undefined && body !== null) return false
  const prefix = `${base}/status?intent=`
  if (!path.startsWith(prefix)) return false
  try {
    const intent = decodeURIComponent(path.slice(prefix.length))
    return intentId(intent) && path === `${prefix}${encodeURIComponent(intent)}`
  } catch { return false }
}

/** 原窗口、壳/IPC 对象与入口、启动与管理代次整体绑定；不为旧意图重选来源。 */
export function captureClineFreeEnrollmentSource(enabled: () => boolean): ClineFreeEnrollmentSource | null {
  if (!clineFreeEnrollmentAvailable() || !enabled()) return null
  const host = shell(), documentSource = document, desktop = host.workbuddyDesktop, ipc = host.__TAURI_INTERNALS__
  const invoke = ipc?.invoke, app = host.wbApp
  if (!invoke) return null
  const origin = host.location.origin, startup = startupIdentity(host), generation = managedLoginSourceGeneration()
  let revoked = false
  const current = () => {
    if (revoked) return false
    const valid = window === host && document === documentSource && host.location.origin === origin
      && host.workbuddyDesktop === desktop && desktop?.platform === 'windows' && host.wbApp === app
      && host.__TAURI_INTERNALS__ === ipc && ipc?.invoke === invoke
      && Boolean(startup) && startupIdentity(host) === startup && generation === managedLoginSourceGeneration() && enabled()
    // 观察到一次来源失效就永久撤销，原对象或同名启动信息随后恢复也不复活旧意图。
    if (!valid) revoked = true
    return valid
  }
  const request: ClineFreeEnrollmentRequest = async <T,>(method: 'GET' | 'POST', path: string, body: unknown,
    options: { retry: 'never' }): Promise<T> => {
    if (!current() || options.retry !== 'never' || !fixedRequest(method, path, body)) throw new Error('cline_free_enrollment_unavailable')
    // 具名 IPC 只调用一次；不恢复认证重发、不改用普通新增账号或缓存原文。
    const pending = invoke.call(ipc, 'api_request', { request: { method, path, body: body === undefined ? null : body, retry: 'never' } })
    body = undefined
    const raw = await pending
    if (!current()) throw new Error('cline_free_enrollment_source_changed')
    if (record(raw) && raw.success === false) throw new Error('cline_free_enrollment_failed')
    return (record(raw) && raw.success === true && Object.hasOwn(raw, 'data') ? raw.data : raw) as T
  }
  return { current, request }
}

/** 仅摘固定 11 字段及 package 5 字段；提交 true 不依赖存储收尾，也不能退回待确认。 */
export function readClineFreeEnrollmentPayload(raw: unknown): ClineFreeEnrollmentPayload {
  if (record(raw) && raw.success === true) raw = raw.data
  if (!exact(raw, ['provider', 'intent', 'phase', 'saved', 'readyRegistered', 'storageClosed', 'expiresAt', 'remainingMs', 'package', 'message', 'account'])
    || raw.provider !== 'cline-free' || !intentId(raw.intent) || !PHASES.includes(raw.phase as ClineFreeEnrollmentPhase)
    || !triState(raw.saved) || !triState(raw.readyRegistered) || !triState(raw.storageClosed)
    || typeof raw.expiresAt !== 'number' || !Number.isSafeInteger(raw.expiresAt) || raw.expiresAt <= 0
    || typeof raw.remainingMs !== 'number' || !Number.isSafeInteger(raw.remainingMs) || raw.remainingMs < 0 || raw.remainingMs > 2_147_483_647
    || typeof raw.message !== 'string' || raw.message.length > 8192) throw new Error('invalid_cline_free_enrollment')
  const pkg = raw.package
  if (!exact(pkg, ['id', 'version', 'archiveDigest', 'moduleDigest', 'declarationDigest']) || !text(pkg.id, 128) || !text(pkg.version, 128)
    || !digest(pkg.archiveDigest) || !digest(pkg.moduleDigest) || !digest(pkg.declarationDigest)) throw new Error('invalid_cline_free_enrollment_package')
  let account: ClineFreeEnrollmentAccount | null = null
  if (raw.account !== null) {
    if (!exact(raw.account, ['id', 'provider']) || !text(raw.account.id, 1024) || raw.account.provider !== 'cline-free'
      || raw.saved !== true) throw new Error('invalid_cline_free_enrollment_account')
    account = { id: raw.account.id, provider: 'cline-free' }
  }
  if (raw.phase === 'saved' && raw.saved !== true) throw new Error('invalid_cline_free_enrollment_saved')
  return { provider: 'cline-free', intent: raw.intent, phase: raw.phase as ClineFreeEnrollmentPhase,
    saved: raw.saved, readyRegistered: raw.readyRegistered, storageClosed: raw.storageClosed,
    expiresAt: raw.expiresAt, remainingMs: raw.remainingMs, message: raw.message, account,
    package: { id: pkg.id, version: pkg.version, archiveDigest: pkg.archiveDigest, moduleDigest: pkg.moduleDigest, declarationDigest: pkg.declarationDigest } }
}
export function clineFreeEnrollmentIdentity(value: ClineFreeEnrollmentPayload): string {
  return JSON.stringify([value.provider, value.intent, value.expiresAt, value.package])
}
