/** 只使用现有 methods/start/wait/callback/cancel 契约；不猜提供商协议。 */
import { apiCall, ApiError } from './client'
import { serverIdentity } from '../lib/servers'
import { isSuspendedProvider } from '../../../ui-shared/provider-availability'
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
}

export type DynamicLoginWait = {
  pending?: boolean
  done?: boolean
  error?: string
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
  return apiCall('/api/session/login/start', { method: 'POST', body: input })
}

export function waitDynamicLogin(state: string): Promise<DynamicLoginWait> {
  return apiCall(`/api/session/login/wait?state=${encodeURIComponent(state)}`)
}

export async function cancelDynamicLogin(state: string): Promise<void> {
  const result = await apiCall<{ canceled?: boolean }>('/api/session/login/cancel', { method: 'POST', body: { state } })
  if (result?.canceled !== true) throw new Error(t('登录取消未确认，请刷新状态后重试。'))
}

export function submitDynamicCallback(state: string, callbackUrl: string): Promise<{
  accountId?: string; accepted?: boolean; continue?: boolean; nextUrl?: string; retryable?: boolean; hint?: string
}> {
  // URL / 裸 code 原文必须保留，不能 trim、拆参数或重建 state。
  return apiCall('/api/session/login/callback', { method: 'POST', body: { state, callbackUrl } })
}

export function dynamicLoginError(error: unknown): string {
  return error instanceof Error ? t(error.message) : t('未知错误')
}

export async function addDynamicApiAccount(input: LoginInput & { name?: string }): Promise<string> {
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
