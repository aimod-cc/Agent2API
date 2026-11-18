/** 只使用现有 methods/start/wait/callback/cancel 契约；不猜提供商协议。 */
import { apiCall, ApiError } from './client'
import { serverIdentity } from '../lib/servers'
import { isSuspendedProvider } from '../../../ui-shared/provider-availability'
import { managedLoginWaitRecord, readManagedLoginWait, type ManagedLoginWait, type ManagedLoginWaitOwner } from '../../../ui-shared/plugin-login-managed'
import { loginText } from '../../../ui-islands/src/islands/add-account-dynamic-fields'
import { loginDiagnosticOf, type LoginDiagnostic } from '../../../ui-shared/plugin-login-diagnostic'
import { locale, t } from '../i18n'

export type DynamicLoginStart = {
  state?: string
  flow?: string
  provider?: string
  url?: string
  authUrl?: string
  instructions?: string
  manualCallback?: boolean
  callbackInput?: 'url' | 'code'
  localOnly?: boolean
  pollIntervalMs?: number
  browserMode?: 'choice' | 'external' | 'plugin' | 'none'
  interactionType?: 'sms'
  managedLogin?: boolean
  methodIndex?: number
  timeoutMs?: number
  remainingMs?: number
}

export type DynamicLoginWait = {
  managedLogin?: boolean
  provider?: string
  flow?: string
  methodIndex?: number
  phase?: ManagedLoginWait['phase']
  saved?: boolean | null
  pending?: boolean
  done?: boolean
  error?: string
  diagnostic?: LoginDiagnostic
  instructions?: string
  hint?: string
  session?: { accountUid?: string; accountId?: string; nickname?: string; provider?: string; proxySaved?: boolean; warning?: string } | null
}

export type LoginInput = { provider: string; methodIndex: number; inputs: Record<string, string>; proxy?: unknown }

function requireAvailableProvider(provider: string): void {
  if (isSuspendedProvider(provider)) {
    throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
  }
}

export function readDynamicMethods(provider: string, signal?: AbortSignal): Promise<unknown> {
  requireAvailableProvider(provider)
  return apiCall(`/api/session/login/methods?provider=${encodeURIComponent(provider)}&locale=${encodeURIComponent(locale())}`, { signal })
}

export function startDynamicLogin(input: LoginInput): Promise<DynamicLoginStart> {
  requireAvailableProvider(input.provider)
  return apiCall('/api/session/login/start', { method: 'POST', body: input, quiet: true })
}

export async function waitDynamicLogin(state: string, owner?: ManagedLoginWaitOwner, signal?: AbortSignal): Promise<DynamicLoginWait> {
  const reply = await apiCall<DynamicLoginWait>(`/api/session/login/wait?state=${encodeURIComponent(state)}`,
    owner ? { signal, quiet: true, noAuthReplay: true } : {})
  // 只在已知 managed 的原流程走新合同；旧 wait 回包和错误语义保持原样。
  if (owner) return readManagedLoginWait(reply, { ...owner, flow: state })
  if (managedLoginWaitRecord(reply)) throw new Error(t('账号保存未确认，请查询本次状态；不会再次保存。'))
  return { ...reply, diagnostic: loginDiagnosticOf(reply) }
}

export async function cancelDynamicLogin(state: string, identity = serverIdentity()): Promise<void> {
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
  const result = await apiCall<{ canceled?: boolean }>('/api/session/login/cancel', { method: 'POST', body: { state }, quiet: true })
  if (result?.canceled !== true) throw new Error(t('登录取消未确认，请刷新状态后重试。'))
}

export function submitDynamicCallback(state: string, callbackUrl: string): Promise<{
  accountId?: string; accepted?: boolean; continue?: boolean; nextUrl?: string; retryable?: boolean; hint?: string
}> {
  // URL / 裸 code 原文必须保留，不能 trim、拆参数或重建 state；401 后不自动重发。
  return apiCall('/api/session/login/callback', { method: 'POST', body: { state, callbackUrl }, quiet: true })
}

export function dynamicLoginError(error: unknown): string {
  return error instanceof Error ? loginText(error.message) : t('未知错误')
}

export async function addDynamicApiAccount(input: LoginInput & { name?: string; importDesktop?: boolean }): Promise<string> {
  requireAvailableProvider(input.provider)
  const identity = serverIdentity()
  const result = await apiCall<{ account?: { id?: string; provider?: string }; proxySaved?: boolean; warning?: string }>(
    '/api/accounts', { method: 'POST', body: input },
  )
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
  if (!result?.account?.id || result.account.provider !== input.provider) {
    throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
  }
  const warning = typeof result.warning === 'string' ? t(result.warning) : ''
  return saveDynamicAccountProxy(result.account.id, input.proxy, result.proxySaved === true, warning, identity)
}

export async function saveDynamicAccountProxy(
  accountId: string, proxy: unknown, saved: boolean, warning = '', identity = serverIdentity(),
): Promise<string> {
  if (saved || proxy === undefined) return warning
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
  let proxyWarning = t('旧网关未确认代理同次保存，已使用兼容接口保存；请在账号设置中确认代理选择。')
  try {
    await apiCall(`/api/accounts/${encodeURIComponent(accountId)}`, {
      method: 'PATCH', body: { proxy: proxy ?? { source: 'direct' } },
    })
  } catch (error) {
    if (serverIdentity() !== identity) throw error
    proxyWarning = t('账号已添加，但代理选择未确认保存；请在账号设置中重新检查。')
  }
  return [warning, proxyWarning].filter(Boolean).join('\n')
}

export function methodsUnavailable(error: unknown): boolean {
  return error instanceof ApiError && error.status === 501
}
