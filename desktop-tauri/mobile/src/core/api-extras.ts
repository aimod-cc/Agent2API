import {
  ApiError,
  apiCall,
  authHeaders,
  refreshPanelAuth,
  refreshPanelAuthIfExpiring,
  resolveUrl,
} from './client'
import { serverIdentity } from './servers'

/* ─── 对话（AI 助手） ────────────────────────────────────── */

export type ToolCallRecord = {
  id: string
  name: string
  arguments: string
}

export type ChatMessage = {
  id: string
  role: 'user' | 'assistant' | 'tool'
  content: string
  reasoning?: string
  toolCalls?: ToolCallRecord[]
  toolCallId?: string
  createdAt?: number
}

export type AssistantConfig = {
  model?: string | null
  apiKeyId?: string | null
  reasoning?: string | null
  mode?: 'confirm' | 'full'
  systemPromptEnabled?: boolean
  systemPrompt?: string
  tools?: Record<string, boolean>
}

export type AssistantSessionSummary = {
  id: string
  title: string
  model: string | null
  createdAt: number
  updatedAt: number
}

export type AssistantSessionDetail = AssistantSessionSummary & {
  messages: ChatMessage[]
}

export async function fetchAssistantConfig(signal?: AbortSignal): Promise<AssistantConfig> {
  const payload = await apiCall<{ config?: AssistantConfig }>('/api/assistant/config', { signal })
  return payload?.config || {}
}

export async function saveAssistantConfig(patch: Partial<AssistantConfig>): Promise<AssistantConfig> {
  const current = await fetchAssistantConfig().catch(() => ({}))
  const merged = { ...current, ...patch }
  const payload = await apiCall<{ config?: AssistantConfig }>('/api/assistant/config', {
    method: 'PUT',
    body: { config: merged },
  })
  return payload?.config || merged
}

export async function listAssistantSessions(signal?: AbortSignal): Promise<AssistantSessionSummary[]> {
  const data = await apiCall<{ sessions?: AssistantSessionSummary[] }>('/api/assistant/sessions', {
    signal,
  })
  return data?.sessions || []
}

export async function getAssistantSession(id: string): Promise<AssistantSessionDetail> {
  const data = await apiCall<{ session: AssistantSessionDetail }>(
    `/api/assistant/sessions/${encodeURIComponent(id)}`,
  )
  return data.session
}

export async function saveAssistantSession(
  id: string,
  messages: ChatMessage[],
  model?: string | null,
  title?: string,
): Promise<AssistantSessionSummary> {
  const fallbackTitle =
    title ||
    messages
      .find(m => m.role === 'user')
      ?.content?.trim()
      .slice(0, 32) ||
    '新对话'
  const data = await apiCall<{ session: AssistantSessionSummary }>('/api/assistant/sessions', {
    method: 'POST',
    body: { id, title: fallbackTitle, messages, model: model || null },
  })
  return data.session
}

export async function deleteAssistantSession(id: string): Promise<void> {
  await apiCall(`/api/assistant/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export async function streamAssistantChat(options: {
  apiKey: string
  messages: ChatMessage[]
  model?: string | null
  reasoning?: string | null
  signal?: AbortSignal
  onDelta: (delta: { content?: string; reasoning?: string }) => void
}): Promise<void> {
  const wireMessages = options.messages.map(m => ({
    role: m.role,
    content: m.content || '',
  }))
  const body: Record<string, unknown> = { messages: wireMessages }
  if (options.model) body.model = options.model
  if (options.reasoning) body.reasoning = options.reasoning

  const identity = serverIdentity()
  const target = resolveUrl('/api/assistant/chat')
  const send = async (): Promise<Response> => {
    if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
    const headers: Record<string, string> = {
      ...authHeaders(),
      'Content-Type': 'application/json',
      Accept: 'text/event-stream',
    }
    if (options.apiKey) {
      headers['x-api-key'] = options.apiKey
      headers.Authorization = `Bearer ${options.apiKey}`
    }
    return fetch(target, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
      signal: options.signal,
    })
  }

  await refreshPanelAuthIfExpiring()
  let response = await send()
  if (response.status === 401 && (await refreshPanelAuth())) {
    response = await send()
  }

  if (!response.ok || !response.body) {
    const text = await response.text().catch(() => '')
    let message = text.trim()
    try {
      const parsed = JSON.parse(text)
      message = String(parsed?.error?.message || parsed?.error || message)
    } catch {
      /* ignore */
    }
    throw new ApiError(message || `HTTP ${response.status}`, response.status)
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  while (true) {
    const { value, done } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split(/\r?\n/)
    buffer = lines.pop() || ''
    for (const rawLine of lines) {
      const line = rawLine.trim()
      if (!line.startsWith('data:')) continue
      const dataText = line.slice(5).trim()
      if (!dataText || dataText === '[DONE]') continue
      try {
        const chunk = JSON.parse(dataText) as {
          error?: { message?: string } | string
          choices?: Array<{
            delta?: {
              content?: string
              reasoning_content?: string
              reasoning?: string
            }
          }>
        }
        if (chunk.error) {
          const errMsg =
            typeof chunk.error === 'string'
              ? chunk.error
              : chunk.error.message || '上游流式返回错误'
          throw new ApiError(errMsg, 500)
        }
        const delta = chunk.choices?.[0]?.delta
        if (delta) {
          const content = typeof delta.content === 'string' ? delta.content : undefined
          const reasoning =
            typeof delta.reasoning_content === 'string'
              ? delta.reasoning_content
              : typeof delta.reasoning === 'string'
                ? delta.reasoning
                : undefined
          if (content || reasoning) {
            options.onDelta({ content, reasoning })
          }
        }
      } catch (err) {
        if (err instanceof ApiError) throw err
      }
    }
  }
}

/* ─── 请求日志与运行日志 ─────────────────────────────────── */

export type RequestRow = {
  rowId?: number
  ts?: number
  id?: string
  model?: string
  clientModel?: string
  upstreamModel?: string
  clientProtocol?: string
  upstreamProtocol?: string
  accountId?: string
  accountName?: string
  provider?: string
  providerLabel?: string
  status?: number
  durationMs?: number
  firstResponseMs?: number | null
  attempts?: number
  error?: string | null
  promptTokens?: number
  completionTokens?: number
  totalTokens?: number
  cacheReadTokens?: number
  cacheCreationTokens?: number
  costUsd?: number | null
  priceSource?: 'custom' | 'provider_wildcard' | 'model_wildcard' | 'official' | 'same_as' | null
  client?: string
  clientLabel?: string
  apiKeyName?: string
  attemptDetails?: Array<{ provider?: string; status?: number; error?: string }>
}

export type RequestPage = {
  entries?: RequestRow[]
  total?: number
  matched?: number
  running?: number
}

export function fetchRequests(
  params: {
    offset?: number
    limit?: number
    status?: 'ok' | 'error' | 'running'
    model?: string
    provider?: string
  },
  signal?: AbortSignal,
): Promise<RequestPage> {
  const query = new URLSearchParams()
  query.set('limit', String(params.limit || 40))
  query.set('view', 'list')
  if (params.offset) query.set('offset', String(params.offset))
  if (params.status) query.set('status', params.status)
  if (params.model) query.set('model', params.model)
  if (params.provider) query.set('provider', params.provider)
  return apiCall<RequestPage>(`/api/stats/requests?${query.toString()}`, { signal })
}

export function fetchRequestDetail(
  params: { rowId?: number; id?: string; ts?: number },
  signal?: AbortSignal,
): Promise<RequestRow> {
  const query = new URLSearchParams()
  if (params.rowId) query.set('rowId', String(params.rowId))
  if (params.id) query.set('id', params.id)
  if (params.ts) query.set('ts', String(params.ts))
  return apiCall<RequestRow>(`/api/stats/requests/detail?${query.toString()}`, { signal })
}

export function clearRequests(mode: 'raw' | 'all' = 'all'): Promise<unknown> {
  return apiCall(`/api/stats/requests?mode=${mode}`, { method: 'DELETE' })
}

export type LogEntry = {
  ts?: number
  level?: string
  category?: string
  message?: string
  provider?: string
  accountId?: string
  model?: string
  details?: unknown
}

export type LogPage = {
  entries?: LogEntry[]
  total?: number
}

export function fetchLogs(
  params: { level?: string; keyword?: string; limit?: number },
  signal?: AbortSignal,
): Promise<LogPage> {
  const query = new URLSearchParams()
  query.set('limit', String(params.limit || 60))
  if (params.level && params.level !== 'all') query.set('level', params.level)
  if (params.keyword?.trim()) query.set('keyword', params.keyword.trim())
  return apiCall<LogPage>(`/api/logs?${query.toString()}`, { signal })
}

export function clearLogs(): Promise<unknown> {
  return apiCall('/api/logs', { method: 'DELETE' })
}

/* ─── 转发参数、存储、调试、密码 ─────────────────────────── */

export type ForwardingBundle = {
  autoContinue: { autoContinue?: boolean; autoContinueMaxRounds?: number }
  retry: {
    retryCount?: number
    retryCrossProviderCount?: number
    retryIntervalSeconds?: number
  }
  timeouts: {
    connectTimeoutSeconds?: number
    headersTimeoutSeconds?: number
    streamIdleTimeoutSeconds?: number
    bodyTimeoutSeconds?: number
  }
  sanitize: boolean
  privacyEnabled: boolean
  privacyEphemeral: boolean
  privacyRevision: string
  keepaliveStrip: boolean
  developerRoleCompat: boolean
  reasoningCollapse: boolean
  corsEnabled: boolean
}

export async function fetchForwardingBundle(signal?: AbortSignal): Promise<ForwardingBundle> {
  const [ac, retry, timeouts, san, priv, ka, dev, rc, cors] = await Promise.all([
    apiCall<{ autoContinue?: boolean; autoContinueMaxRounds?: number }>('/api/auto-continue', {
      signal,
    }).catch(() => ({})),
    apiCall<{
      retryCount?: number
      retryCrossProviderCount?: number
      retryIntervalSeconds?: number
    }>('/api/retry', { signal }).catch(() => ({})),
    apiCall<{
      connectTimeoutSeconds?: number
      headersTimeoutSeconds?: number
      streamIdleTimeoutSeconds?: number
      bodyTimeoutSeconds?: number
    }>('/api/timeouts', { signal }).catch(() => ({})),
    apiCall<{ sanitizeBlacklistFingerprints?: boolean }>('/api/sanitize', { signal }).catch(
      () => ({}),
    ),
    apiCall<{ enabled?: boolean; ephemeralPlaceholders?: boolean; revision?: string }>('/api/privacy', { signal }).catch(
      () => ({}),
    ),
    apiCall<{ stripNewlineKeepalive?: boolean }>('/api/keepalive-strip', { signal }).catch(
      () => ({}),
    ),
    apiCall<{ developerRoleCompat?: boolean }>('/api/developer-role-compat', { signal }).catch(
      () => ({}),
    ),
    apiCall<{ thirdPartyReasoningCollapse?: boolean }>('/api/reasoning-collapse', { signal }).catch(
      () => ({}),
    ),
    apiCall<{ corsEnabled?: boolean }>('/api/cors', { signal }).catch(() => ({})),
  ])
  return {
    autoContinue: ac,
    retry,
    timeouts,
    sanitize: Boolean((san as { sanitizeBlacklistFingerprints?: boolean }).sanitizeBlacklistFingerprints),
    privacyEnabled: Boolean((priv as { enabled?: boolean }).enabled),
    privacyEphemeral: Boolean((priv as { ephemeralPlaceholders?: boolean }).ephemeralPlaceholders),
    privacyRevision: typeof (priv as { revision?: unknown }).revision === 'string'
      ? String((priv as { revision?: string }).revision)
      : '',
    keepaliveStrip: Boolean((ka as { stripNewlineKeepalive?: boolean }).stripNewlineKeepalive),
    developerRoleCompat: Boolean((dev as { developerRoleCompat?: boolean }).developerRoleCompat),
    reasoningCollapse: Boolean(
      (rc as { thirdPartyReasoningCollapse?: boolean }).thirdPartyReasoningCollapse,
    ),
    corsEnabled: Boolean((cors as { corsEnabled?: boolean }).corsEnabled),
  }
}

export function saveAutoContinue(patch: {
  autoContinue?: boolean
  autoContinueMaxRounds?: number
}): Promise<unknown> {
  return apiCall('/api/auto-continue', { method: 'PUT', body: patch })
}

export function saveRetrySettings(patch: {
  retryCount?: number
  retryCrossProviderCount?: number
  retryIntervalSeconds?: number
}): Promise<unknown> {
  return apiCall('/api/retry', { method: 'PUT', body: patch })
}

export function saveTimeoutSettings(patch: {
  connectTimeoutSeconds?: number
  headersTimeoutSeconds?: number
  streamIdleTimeoutSeconds?: number
  bodyTimeoutSeconds?: number
}): Promise<unknown> {
  return apiCall('/api/timeouts', { method: 'PUT', body: patch })
}

export function saveCompatSwitch(
  kind: 'sanitize' | 'privacy' | 'keepalive-strip' | 'developer-role-compat' | 'reasoning-collapse' | 'cors',
  enabled: boolean,
): Promise<unknown> {
  const map: Record<typeof kind, { path: string; body: Record<string, boolean> }> = {
    sanitize: { path: '/api/sanitize', body: { sanitizeBlacklistFingerprints: enabled } },
    privacy: { path: '/api/privacy', body: { enabled } },
    'keepalive-strip': { path: '/api/keepalive-strip', body: { stripNewlineKeepalive: enabled } },
    'developer-role-compat': {
      path: '/api/developer-role-compat',
      body: { developerRoleCompat: enabled },
    },
    'reasoning-collapse': {
      path: '/api/reasoning-collapse',
      body: { thirdPartyReasoningCollapse: enabled },
    },
    cors: { path: '/api/cors', body: { corsEnabled: enabled } },
  }
  const target = map[kind]
  return apiCall(target.path, { method: 'PUT', body: target.body })
}

export function savePrivacyQuickSettings(patch: {
  enabled?: boolean
  ephemeralPlaceholders?: boolean
  revision?: string
}): Promise<{ enabled?: boolean; ephemeralPlaceholders?: boolean; revision?: string }> {
  return apiCall('/api/privacy', { method: 'PUT', body: patch })
}

export function fetchDebugMode(signal?: AbortSignal): Promise<{ debugMode?: boolean }> {
  return apiCall('/api/debug', { signal })
}

export function saveDebugMode(enabled: boolean): Promise<{ debugMode?: boolean }> {
  return apiCall('/api/debug', { method: 'PUT', body: { debugMode: enabled } })
}

export type StorageSummary = {
  configDir?: string
  database?: {
    file?: string
    bytes?: number
    available?: boolean
    accounts?: number
    logs?: number
    requests?: number
  }
}

export function fetchStorageInfo(signal?: AbortSignal): Promise<StorageSummary> {
  return apiCall<StorageSummary>('/api/storage', { signal })
}

export function compactRequestsDb(): Promise<unknown> {
  return apiCall('/api/stats/requests/compact', { method: 'POST' })
}

export function changePanelPassword(
  oldPassword: string,
  newPassword: string,
): Promise<{ changed?: boolean }> {
  return apiCall('/api/panel/password', { method: 'POST', body: { oldPassword, newPassword } })
}
