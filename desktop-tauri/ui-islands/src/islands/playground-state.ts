/**
 * 试炼场（Playground）的数据层：走**客户端真实路径**打 `/v1/chat/completions`。
 *
 * ── 为什么独立成文件、而不是复用 assistant-state ──────────────────
 * 两条链的**目标通道不同**，这是本模块存在的唯一理由：
 *   · AI 助手（assistant-state）打 `/api/assistant/chat` —— **面板面**接口，
 *     自带系统提示词与工具集，回答经过网关自己的编排。它验证的是「助手能用」。
 *   · 试炼场（本模块）打 `/v1/chat/completions` —— **网关面**接口，就是
 *     客户端（ChatBox / Cherry Studio / Claude Code…）实际打的那一条。它验证的
 *     是「我的 base_url + Key 能不能用、模型通不通、实际路由到哪个账号」。
 * 把试炼场接到面板面上会得到一个「看起来能用但客户端打不通」的假结论 ——
 * 那样这一页就失去了存在的意义。
 *
 * ── 凭证从哪来 ─────────────────────────────────────────────────
 * `/v1/*` 需要网关 Key（`require_api_key`）。桌面形态下页面来自 Tauri 自定义
 * 协议、网关在 `127.0.0.1:{port}`，跨源且不带 cookie，所以凭证只能问壳要：
 * 复用 assistant-state 的 `requestDirect`（它内部走桥的 `directCredentials`，
 * 桌面壳回**第一把启用的网关 Key**，见 commands::direct_credentials）。
 * 不自己再写一份取凭证的逻辑 —— 那条链已经处理了「401 清缓存重试一次」
 * 「桥没就绪时按无凭证发」这些边界（见 assistant-state 的 CREDENTIALS 一段）。
 *
 * ── 与 OpenAI SDK 的一致性 ──────────────────────────────────────
 * 请求体按 OpenAI Chat Completions 规范组装（`model` / `messages` / `stream`
 * 加上可选参数），响应按 SSE 逐帧解析。**不省字段、不做网关特化** ——
 * 这一页要能暴露「客户端会遇到的真实问题」，多加一层善意包装就等于捂住它们。
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
  // 网关面直连：相对路径交给 requestDirect 拼基址（它会问壳要真实端口与 Key）
  const path = '/v1/chat/completions'
  const body: Record<string, unknown> = {
    model: request.model,
    messages: request.messages.map(message => ({ role: message.role, content: message.content })),
    // 统一按流式打（见函数头）；非流式也在下面逐帧收完再一次性回调
    stream: true,
    stream_options: { include_usage: true },
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
