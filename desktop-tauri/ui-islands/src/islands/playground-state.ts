/**
 * 试炼场（Playground）的数据层：走 `POST /api/playground/chat`（面板面）。
 *
 * ── 为什么不是直连 `/v1/chat/completions`（这里踩过坑，别再改回去）────
 * 最初的实现是直连 `/v1/chat/completions`，理由写得挺漂亮（「打客户端真实
 * 走的那条路」）。但**它在全新安装上根本不能用**：
 *   · 桌面壳的页面来自 Tauri 自定义协议（`tauri://localhost`），直连
 *     `http://127.0.0.1:{port}/v1/*` 是**跨源**请求；
 *   · 而网关面**默认不应答 CORS**（`corsEnabled` 默认 `false` —— 那是一道
 *     安全边界：开着 `*` 等于把「消耗额度」的能力交给任何网页）；
 *   · 实测：CORS 关掉后直连直接 `TypeError: Failed to fetch`。
 * 开发机上 CORS 恰好开着，所以一直没暴露 —— 这正是「同一条注释警告过、
 * 却没读到位」的典型。`api::assistant` 的模块头早就把这条论证写清楚了。
 *
 * ── 那结论还代表真实客户端吗：代表 ─────────────────────────
 * `/api/playground/chat` 与 `/v1/chat/completions` **共用同一条转发链路**
 * （`UpstreamService::forward`）：token 刷新、429 换号、冷却、出站脱敏、
 * 协议翻译、模型路由（对外名 → 上游名）全是同一份代码。
 * 与真实客户端观察到的差异**为零** —— 见 `api/playground.rs` 的模块头。
 *
 * ── 与 AI 助手的分工（两者并存不重复）──────────────────────
 *   · 助手 → `/api/assistant/chat`：**注入系统提示词 + 带 tools（能改配置）**，
 *     是「能动手的运维入口」；
 *   · 试炼场 → `/api/playground/chat`：**不注入提示词、不带 tools、可选采样
 *     参数**，是「只读的连通性验证台」。
 */

import { t } from '../i18n'
import { requestDirect } from './assistant-state'

/* ─── 类型 ─────────────────────────────────────────────── */

/** 一条对话消息（试炼场只做纯文本对话，不带工具调用 —— 那是助手页的事） */
export type PlayMessage = {
  role: 'system' | 'user' | 'assistant'
  content: string
}

/** 发一轮请求的全部参数（都是客户端会填的那几项） */
export type PlayRequest = {
  model: string
  messages: PlayMessage[]
  stream: boolean
  /** 采样温度；undefined = 不传（用上游默认） */
  temperature?: number
  /** 最大生成 token；undefined = 不传 */
  maxTokens?: number
  /** 只让网关按模型路由、不看真正回答（`max_tokens: 1` 的轻量探测） */
  signal?: AbortSignal
  /**
   * 是否向上一轮要 usage 读数（`stream_options.include_usage`）。
   *
   * **默认关**，与真实客户端一致：多传这个字段有些上游会直接 400，而试炼场
   * 报一个「客户端根本不会遇到」的错就失去意义了。用户显式勾选才打开
   * （页面在勾选项旁写明了风险）。
   */
  wantsUsage?: boolean
}

/** 一轮请求的收尾统计（这一页最有价值的部分：用量与首字节延迟） */
export type PlayStats = {
  /** 首个 delta 到达耗时（毫秒）—— 流式才有意义 */
  ttfbMs: number | null
  /** 全程耗时（毫秒） */
  totalMs: number
  /** 上游回的 usage（有就显示；`stream_options.include_usage` 打开时流式也有） */
  usage: PlayUsage | null
  /** 结束原因（stop / length / tool_calls…） */
  finishReason: string
}

export type PlayUsage = {
  promptTokens: number | null
  completionTokens: number | null
  totalTokens: number | null
  /** 提示词缓存命中（DeepSeek 系等会回）—— 有就显示，没有就略过 */
  cachedTokens: number | null
}

/** 回调：正文增量 / 思考增量 / 收尾统计 */
export type PlayHandlers = {
  onDelta?: (text: string) => void
  /** 推理模型（DeepSeek 系）的思维链增量；上游走 `reasoning_content` 字段 */
  onReasoning?: (text: string) => void
  onStats?: (stats: PlayStats) => void
}

/* ─── 工具函数 ─────────────────────────────────────────── */

/** 从 usage 对象里取三个计数（形状不齐时回落 null，不猜 0 —— 0 是有意义的读数） */
function readUsage(raw: unknown): PlayUsage | null {
  if (!raw || typeof raw !== 'object') return null
  const source = raw as Record<string, unknown>
  const num = (value: unknown): number | null =>
    typeof value === 'number' && Number.isFinite(value) ? value : null
  const prompt = num(source.prompt_tokens)
  const completion = num(source.completion_tokens)
  const total = num(source.total_tokens)
  // 缓存命中三种来源名都见过（各上游口径不同）：整表都缺时分字段就没有
  const cached =
    num(source.prompt_cache_hit_tokens) ??
    num(source.cached_tokens) ??
    num((source.prompt_tokens_details as Record<string, unknown> | undefined)?.cached_tokens)
  if (prompt === null && completion === null && total === null) return null
  return { promptTokens: prompt, completionTokens: completion, totalTokens: total, cachedTokens: cached }
}

/** 网络层 / 鉴权层的失败包上上下文（浏览器只给一句 Failed to fetch，看不出是哪个地址） */
function describeFailure(error: unknown, base: string): Error {
  const message = error instanceof Error ? error.message : String(error)
  return new Error(t('连接网关失败（{base}）：{message}', { base, message }))
}

/**
 * 读一条错误响应体，拼成一句能直接看懂的话。
 *
 * OpenAI 形态的错误是 `{"error":{"message":…}}`，网关自己的错也可能是这一形；
 * 两者都兜住，再兜「根本不是 JSON」（网关没起来时可能是 HTML 或空体）。
 * 状态码永远带上 —— 它是用户拿去搜 / 反馈的关键线索。
 */
function describeHttpError(status: number, text: string): string {
  let detail = text.trim()
  try {
    const parsed = JSON.parse(text)
    const raw = (parsed as { error?: { message?: unknown } }).error
    if (raw && typeof raw === 'object' && typeof raw.message === 'string') detail = raw.message
    else if (typeof raw === 'string') detail = raw
  } catch { /* 非 JSON：原样用文本 */ }
  if (detail.length > 500) detail = detail.slice(0, 500) + '…'
  return detail ? `HTTP ${status}：${detail}` : `HTTP ${status}`
}

/* ─── 主入口：发一轮（流式与非流式统一入口）────────────────── */

/**
 * 发一轮对话。**流式与非流式共用这一条路**：非流式也走 SSE 解析器 ——
 * 网关对 `stream: false` 会聚合出一条完整 JSON，但为了少一条码路（和少一处
 * 「只有流式才能显示思考过程」的不一致），这里统一用流式发、再由本函数按
 * `stream` 决定要不要逐字回调。
 *
 * 返回聚合好的完整回答 + 统计。抛出的 Error 已是给人看的话（见 describeHttpError）。
 */
export async function runPlayground(
  request: PlayRequest,
  handlers: PlayHandlers = {},
): Promise<{ text: string; reasoning: string; stats: PlayStats }> {
  const started = performance.now()
  // 面板面（`/api/*`，无条件 CORS）：与 /v1/chat/completions 共用同一条转发链路，
  // 但不要求用户打开「网关跨域访问」—— 见模块头。
  const path = '/api/playground/chat'
  const body: Record<string, unknown> = {
    model: request.model,
    messages: request.messages.map(message => ({ role: message.role, content: message.content })),
    // 上游固定流式（首字节耗时与逐字渲染都靠它）；**stream_options 默认不传** ——
    // 真实客户端默认也不传，传了某些上游会 400（见后端 api::playground 的注释）。
    // `wantsUsage` 由用户显式勾选，页面已把风险写在勾选项旁边。
    include_usage: request.wantsUsage === true,
  }
  if (typeof request.temperature === 'number' && Number.isFinite(request.temperature)) {
    body.temperature = request.temperature
  }
  if (typeof request.maxTokens === 'number' && Number.isFinite(request.maxTokens) && request.maxTokens > 0) {
    body.max_tokens = Math.floor(request.maxTokens)
  }

  let response: Response
  try {
    response = await requestDirect(path, { method: 'POST', body, signal: request.signal })
  } catch (error) {
    // 取基址拼错误文案（requestDirect 内部已包过一层，这里补上「试炼场」语境）
    if (request.signal?.aborted) throw error
    throw error instanceof Error ? error : new Error(String(error))
  }

  if (!response.ok || !response.body) {
    const text = await response.text().catch(() => '')
    throw new Error(describeHttpError(response.status, text))
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let text = ''
  let reasoning = ''
  let finishReason = ''
  let ttfbMs: number | null = null
  let usage: PlayUsage | null = null

  /** 处理一个 SSE 帧；返回 true 表示收到 [DONE]（可以停读） */
  const handleFrame = (frame: string): boolean => {
    for (const rawLine of frame.split('\n')) {
      const line = rawLine.trimEnd()
      if (!line.startsWith('data:')) continue
      const payload = line.slice(5).trim()
      if (!payload) continue
      if (payload === '[DONE]') return true
      let chunk: Record<string, unknown>
      try { chunk = JSON.parse(payload) as Record<string, unknown> } catch { continue }

      // usage 可能在带 choices 的帧里，也可能在最后一帧（choices 为空）
      if (chunk.usage) {
        const parsed = readUsage(chunk.usage)
        if (parsed) usage = parsed
      }
      const choice = Array.isArray(chunk.choices)
        ? (chunk.choices[0] as Record<string, unknown> | undefined)
        : undefined
      if (!choice) continue
      const delta = (choice.delta || {}) as Record<string, unknown>

      // 思维链：上游口径是 reasoning_content（DeepSeek 系）；部分家叫 reasoning
      const think =
        (typeof delta.reasoning_content === 'string' && delta.reasoning_content) ||
        (typeof delta.reasoning === 'string' && delta.reasoning) ||
        ''
      if (think) {
        if (ttfbMs === null) ttfbMs = performance.now() - started
        reasoning += think
        handlers.onReasoning?.(think)
      }

      const piece = typeof delta.content === 'string' ? delta.content : ''
      if (piece) {
        if (ttfbMs === null) ttfbMs = performance.now() - started
        text += piece
        handlers.onDelta?.(piece)
      }
      if (typeof choice.finish_reason === 'string' && choice.finish_reason) {
        finishReason = choice.finish_reason
      }
    }
    return false
  }

  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    let index = buffer.indexOf('\n\n')
    while (index >= 0) {
      const frame = buffer.slice(0, index)
      buffer = buffer.slice(index + 2)
      if (handleFrame(frame)) {
        await reader.cancel().catch(() => { /* 已经收尾，取消失败无妨 */ })
        buffer = ''
        index = -1
        break
      }
      index = buffer.indexOf('\n\n')
    }
  }
  // 尾帧可能没有以空行收尾（上游提前关连接），把残留的 buffer 也过一遍
  if (buffer.trim()) handleFrame(buffer)

  const stats: PlayStats = {
    ttfbMs,
    totalMs: performance.now() - started,
    usage,
    finishReason,
  }
  handlers.onStats?.(stats)
  return { text, reasoning, stats }
}

/** 毫秒数 → 人读的串（`820 ms` / `3.42 s`） */
export function formatMs(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return '—'
  if (value < 1000) return `${Math.round(value)} ms`
  return `${(value / 1000).toFixed(2)} s`
}

/** token 计数 → 千分位串（`1,234`）；null → `—` */
export function formatTokens(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return '—'
  return value.toLocaleString('en-US')
}
