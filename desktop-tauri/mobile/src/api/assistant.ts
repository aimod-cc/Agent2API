/**
 * AI 助手的对话流（SSE）。
 *
 * ── 请求走哪条路 ──────────────────────────────────────────────
 * `POST /api/assistant/chat`，**由网关转发到上游**。手机端只负责把消息发出去、
 * 把流回来的 delta 画出来 —— 模型路由、账号选取、429 换号、协议翻译、记账
 * 全在网关那一侧（见 `api::assistant` 的模块头）。手机不持有任何上游凭证，
 * 也不直连任何模型服务。
 *
 * ── 为什么不复用 apiCall ──────────────────────────────────────
 * `apiCall` 会把响应体整个读成文本再解 `{success, data}` 信封；对话是 SSE，
 * 要边读边渲染。所以这里单独发一次 fetch，只共用 client.ts 的地址拼接、
 * 鉴权头与 401 续期（那三件事必须与其它请求同一份实现，否则「登录过期」
 * 在对话页会表现成另一种样子）。
 *
 * ── 帧格式 ────────────────────────────────────────────────────
 * 与 OpenAI 流式一致：`data: {choices:[{delta:{content, tool_calls}}]}`，
 * 以 `data: [DONE]` 收尾。正文按 delta 累加；`tool_calls` 按 `index` 聚合
 * （同一只工具的参数会被拆成多片，不能逐片当独立调用）。
 */

import { ApiError, apiCall, authHeaders, refreshPanelAuth, refreshPanelAuthIfExpiring, resolveUrl } from '../api/client'
import type { ChatMessage } from '../../../ui-shared/assistant-history'
import { parseAssistantMessages, assistantTitle } from '../../../ui-shared/assistant-history'
export type { ChatMessage } from '../../../ui-shared/assistant-history'
import { readAssistantStream, type ChatEvent, type ToolCall } from '../../../ui-shared/assistant-stream'
import { userContentWithFiles } from '../../../ui-shared/assistant-files'
import { t } from '../i18n'
import { serverIdentity, apiBase } from '../lib/servers'
import { readModelApiKeySelection, rememberModelApiKeySelection } from '../../../ui-shared/model-api-key-memory'
export type { ChatEvent, ToolCall } from '../../../ui-shared/assistant-stream'

export type ChatRole = 'user' | 'assistant' | 'tool'

/** 助手配置（只读用得到的几项） */
export type AssistantConfig = {
  model?: string | null
  apiKeyId?: string | null
  reasoning?: string | null
  mode?: 'confirm' | 'full'
  systemPromptEnabled?: boolean
  systemPrompt?: string
  titleModel?: string | null
  titlePrompt?: string
  /** 工具勾选表：tool id → 是否勾选（后端已把缺省值展开） */
  tools?: Record<string, boolean>
}

export async function fetchAssistantConfig(): Promise<AssistantConfig> {
  const identity = serverIdentity()
  const scope = apiBase()
  const payload = await apiCall<{ config?: AssistantConfig }>('/api/assistant/config')
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
  if (!payload.config) throw new Error(t('配置响应不完整'))
  const apiKeyId = typeof payload.config.apiKeyId === 'string' && payload.config.apiKeyId.trim() ? payload.config.apiKeyId.trim()
    : readModelApiKeySelection(scope, 'assistant')
  if (apiKeyId) rememberModelApiKeySelection(scope, 'assistant', apiKeyId)
  return { ...payload.config, apiKeyId }
}

/**
 * 发一轮对话并把流式事件回调出去。
 *
 * `signal` 用来实现「停止」：abort 之后读取循环以 AbortError 结束 —— 那是
 * 用户的意图，不是错误（调用方按 `signal.aborted` 区分）。
 */
export async function streamAssistantChat(options: {
  apiKey: string
  messages: ChatMessage[]
  /** 勾选过的工具表（OpenAI tools 形态）；空数组 = 纯聊天 */
  tools?: Array<Record<string, unknown>>
  model?: string | null
  reasoning?: string | null
  signal?: AbortSignal
  onEvent: (event: ChatEvent) => void
}): Promise<void> {
  const body: Record<string, unknown> = {
    messages: options.messages.map(wireMessage),
  }
  // 不传 model 时由网关用「助手设置」里存的默认模型兜底
  if (options.model) body.model = options.model
  if (options.reasoning) body.reasoning = options.reasoning
  // 工具表为空时**不要发空数组**：后端按「有没有 tools」决定注不注入系统提示词
  // （见 config::AssistantConfig::system_prompt），空数组与不发不是一回事
  if (options.tools && options.tools.length) body.tools = options.tools

  const identity = serverIdentity()
  const target = resolveUrl('/api/assistant/chat')
  const send = async (): Promise<Response> => {
    if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
    return fetch(target, {
      method: 'POST',
      headers: { ...authHeaders(), 'x-api-key': options.apiKey, Authorization: `Bearer ${options.apiKey}`, 'Content-Type': 'application/json', Accept: 'text/event-stream' },
      body: JSON.stringify(body),
      signal: options.signal,
    })
  }

  // 到期前先换令牌：这条不走 apiCall，不补这一步就会在 access 过期那一刻
  // 产生一条 401（见 client.ts 的 refreshPanelAuthIfExpiring）
  await refreshPanelAuthIfExpiring()
  let response = await send()
  // 401 静默续期一次（与 apiCall 同一语义）：续上了就重发，续不上交给界面
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
      /* 非 JSON 错误体：原样用文本 */
    }
    if (response.status === 404 && message === 'Not found: POST /api/assistant/chat') {
      throw new ApiError(t('这一版网关还没有 AI 助手接口（HTTP 404），请升级网关后重试。'), 404)
    }
    throw new ApiError(message || `HTTP ${response.status}`, response.status)
  }

  await readAssistantStream(response, {
    onEvent: options.onEvent,
    signal: options.signal,
    incompleteMessage: t('模型响应未完整结束，请重试；未执行其中的工具调用。'),
  })
}

/**
 * 一条消息 → 发给上游的 wire 形态。
 *
 * 与 OpenAI 的约定逐字对齐：assistant 带工具调用时要有 `tool_calls`，
 * 工具结果用 `role: "tool"` + `tool_call_id` 回灌 —— 少任何一个字段，
 * 上游都会拒掉整轮请求（或在下一轮丢掉上下文）。
 */
function wireMessage(message: ChatMessage): Record<string, unknown> {
  if (message.role === 'tool') {
    return { role: 'tool', tool_call_id: message.toolCallId, content: message.content }
  }
  if (message.role === 'assistant') {
    const wire: Record<string, unknown> = { role: 'assistant', content: message.content || '' }
    if (message.toolCalls?.length) {
      wire.tool_calls = message.toolCalls.map(call => ({
        id: call.id,
        type: 'function',
        function: { name: call.name, arguments: call.arguments },
      }))
    }
    return wire
  }
  const images = (message.images || []).filter((image): image is string => typeof image === 'string')
  const text = userContentWithFiles(message.content, message.files || [])
  if (!images.length) return { role: 'user', content: text }
  return { role: 'user', content: [
    { type: 'text', text },
    ...images.map(url => ({ type: 'image_url', image_url: { url } })),
  ] }
}

export type SessionSummary = { id: string; title: string; model: string | null; reasoning: string | null; createdAt: number; updatedAt: number; titleGenerated: boolean }
export type SessionDetail = SessionSummary & { messages: ChatMessage[] }

export async function saveAssistantSession(id: string, messages: ChatMessage[], model?: string | null, title?: string): Promise<SessionSummary> {
  const data = await apiCall<{ session: SessionSummary }>('/api/assistant/sessions', { method: 'POST', body: { id, title: title ?? assistantTitle(messages), messages, model: model || null } })
  return data.session
}

export async function listAssistantSessions(): Promise<SessionSummary[]> {
  const data = await apiCall<{ sessions: SessionSummary[] }>('/api/assistant/sessions')
  return data.sessions
}

export async function getAssistantSession(id: string): Promise<SessionDetail> {
  const data = await apiCall<{ session: SessionDetail }>(`/api/assistant/sessions/${encodeURIComponent(id)}`)
  return { ...data.session, messages: parseAssistantMessages(data.session.messages) }
}

export async function forkAssistantSession(id: string, messageId: string): Promise<SessionDetail> {
  const data = await apiCall<{ session: SessionDetail }>(`/api/assistant/sessions/${encodeURIComponent(id)}/fork`, { method: 'POST', body: { id: crypto.randomUUID(), messageId } })
  return { ...data.session, messages: parseAssistantMessages(data.session.messages) }
}

export async function summarizeAssistantSession(id: string, apiKey: string): Promise<SessionSummary> {
  const data = await apiCall<{ session: SessionSummary }>(`/api/assistant/sessions/${encodeURIComponent(id)}/title`, { method: 'POST', apiKey })
  return data.session
}

export async function saveAssistantConfig(config: AssistantConfig): Promise<AssistantConfig> {
  const identity = serverIdentity()
  const scope = apiBase()
  rememberModelApiKeySelection(scope, 'assistant', config.apiKeyId)
  const data = await apiCall<{ config: AssistantConfig }>('/api/assistant/config', { method: 'PUT', body: { config } })
  if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
  rememberModelApiKeySelection(scope, 'assistant', data.config.apiKeyId)
  return data.config
}

export async function saveAssistantTitleConfig(config: AssistantConfig, titleModel: string | null, titlePrompt: string): Promise<AssistantConfig> {
  return saveAssistantConfig({ ...config, titleModel, titlePrompt })
}

export async function deleteAssistantSession(id: string): Promise<void> {
  await apiCall(`/api/assistant/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' })
}
