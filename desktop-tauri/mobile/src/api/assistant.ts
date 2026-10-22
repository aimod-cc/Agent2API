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

import { ApiError, authHeaders, refreshPanelAuth, resolveUrl } from '../api/client'

export type ChatRole = 'user' | 'assistant' | 'tool'

/** 一次工具调用（聚合后的完整形态） */
export type ToolCall = { id: string; name: string; arguments: string }

export type ChatMessage = {
  role: ChatRole
  content: string
  /** assistant 消息上的工具调用（回灌给上游时要带上） */
  toolCalls?: ToolCall[]
  /** role = 'tool' 时对应哪一次调用 */
  toolCallId?: string
}

export type ChatEvent =
  | { type: 'delta'; text: string }
  | { type: 'toolCalls'; calls: ToolCall[] }
  | { type: 'done'; finishReason: string }

/** 助手配置（只读用得到的几项） */
export type AssistantConfig = {
  model?: string | null
  reasoning?: string | null
  /** 工具勾选表：tool id → 是否勾选（后端已把缺省值展开） */
  tools?: Record<string, boolean>
}

export async function fetchAssistantConfig(): Promise<AssistantConfig> {
  const response = await fetch(resolveUrl('/api/assistant/config'), {
    headers: authHeaders(),
  })
  const payload = await response.json().catch(() => null)
  if (!response.ok || payload?.success === false) {
    throw new ApiError(
      String(payload?.error?.message || payload?.error || `HTTP ${response.status}`),
      response.status,
    )
  }
  return (payload?.data?.config || {}) as AssistantConfig
}

/**
 * 发一轮对话并把流式事件回调出去。
 *
 * `signal` 用来实现「停止」：abort 之后读取循环以 AbortError 结束 —— 那是
 * 用户的意图，不是错误（调用方按 `signal.aborted` 区分）。
 */
export async function streamAssistantChat(options: {
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

  const send = async (): Promise<Response> =>
    fetch(resolveUrl('/api/assistant/chat'), {
      method: 'POST',
      headers: { ...authHeaders(), 'Content-Type': 'application/json', Accept: 'text/event-stream' },
      body: JSON.stringify(body),
      signal: options.signal,
    })

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
    if (response.status === 404) {
      throw new ApiError(
        `这一版网关还没有 AI 助手接口（HTTP 404），请升级网关后重试。${message}`,
        404,
      )
    }
    throw new ApiError(message || `HTTP ${response.status}`, response.status)
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let finished = false
  /** 正文累积 + 工具调用按 index 聚合（参数会被拆成多片） */
  const calls = new Map<number, ToolCall>()
  let finishReason = ''

  /** 处理一帧；返回 true 表示收到 [DONE] */
  const handleFrame = (frame: string): boolean => {
    for (const rawLine of frame.split('\n')) {
      const line = rawLine.trimEnd()
      if (!line.startsWith('data:')) continue
      const payload = line.slice(5).trim()
      if (!payload) continue
      if (payload === '[DONE]') return true
      let chunk: Record<string, unknown>
      try {
        chunk = JSON.parse(payload) as Record<string, unknown>
      } catch {
        continue
      }
      const choice = Array.isArray(chunk.choices)
        ? (chunk.choices[0] as Record<string, unknown> | undefined)
        : undefined
      const delta = (choice?.delta || {}) as Record<string, unknown>
      // 思考型模型的 reasoning_content 不进正文：手机上只显示最终回答，
      // 把思考过程混进气泡会让长回答读起来像两段话
      const content = typeof delta.content === 'string' ? delta.content : ''
      if (content) options.onEvent({ type: 'delta', text: content })

      if (Array.isArray(delta.tool_calls)) {
        for (const raw of delta.tool_calls as Array<Record<string, unknown>>) {
          const index = Number(raw.index ?? 0)
          const slot = calls.get(index) || { id: '', name: '', arguments: '' }
          if (typeof raw.id === 'string' && raw.id) slot.id = raw.id
          const fn = (raw.function || {}) as Record<string, unknown>
          if (typeof fn.name === 'string' && fn.name) slot.name = fn.name
          if (typeof fn.arguments === 'string') slot.arguments += fn.arguments
          calls.set(index, slot)
        }
      }
      if (typeof choice?.finish_reason === 'string') finishReason = choice.finish_reason
    }
    return false
  }

  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let index = buffer.indexOf('\n\n')
      while (index >= 0) {
        const frame = buffer.slice(0, index)
        buffer = buffer.slice(index + 2)
        if (handleFrame(frame)) {
          finished = true
          // 收到 [DONE]：剩下的字节不必再读
          await reader.cancel().catch(() => {})
          index = -1
          break
        }
        index = buffer.indexOf('\n\n')
      }
      if (finished) break
    }
    // 尾帧没有空行收尾时也要处理（部分上游不发最后的 \n\n）
    if (!finished && buffer.trim()) handleFrame(buffer)
    // 聚合好的工具调用一次性交出去（正文已逐段给过）
    const done = [...calls.entries()].sort((a, b) => a[0] - b[0]).map(([, call]) => call)
    if (done.length) options.onEvent({ type: 'toolCalls', calls: done })
    options.onEvent({ type: 'done', finishReason })
  } finally {
    try {
      reader.releaseLock()
    } catch {
      /* 已取消 */
    }
  }
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
  return { role: 'user', content: message.content }
}
