/**
 * AI 助手的状态与请求层：AI 助手配置的读写（`/api/assistant/config`）与
 * 流式对话（`/api/assistant/chat`，SSE）。
 *
 * ── 为什么这两条不走壳的桥 ───────────────────────────────────
 * 桥（`window.workbuddyDesktop.xxx`）是「一次性拿结果」的形态：没有流式，
 * 而且有 60 秒总超时（见 `src/gateway.rs` 的 REQUEST_TIMEOUT_MS）—— 对话回答
 * 动辄跑几十秒，走桥会被截断。所以这里直接 fetch 到**网关基址**：
 *   · web 形态（面板由网关自己托管）：基址 = 地址栏 origin，同源，cookie 自动带；
 *   · 桌面形态：页面来自 Tauri 自定义协议，基址只能问壳要真实端口
 *     （`getBackendStatus`），再拼 `http://127.0.0.1:{port}`。
 * 两条路都落在面板路由 `/api/*`（无条件 CORS），所以不需要用户打开网关面的
 * CORS 开关 —— 那条开关管的是 `/v1/*`，与这里无关。
 *
 * ── 流式协议 ────────────────────────────────────────────────
 * 后端把上游的 OpenAI 格式 SSE 原样透传（`data: {...}` / `data: [DONE]`）。
 * 本模块只做三件事：分帧、按 delta 累积（正文 + tool_calls 分片）、把事件交给
 * 调用方。**不做 agent loop** —— 工具执行与「确认卡」的流程在页面层（那里才
 * 有 UI 状态），本模块给出的是「一轮请求」的纯粹形态。
 */

import { t } from '../i18n'

const FALLBACK_PORT = 3065

/** 网关基址缓存（每次进对话页最多问一次壳） */
let cachedBase: string | null = null

type BackendStatus = { port?: number }
type DesktopBridge = {
  platform?: string
  getBackendStatus?: () => Promise<BackendStatus | null | undefined>
  /** 直连网关要带的凭证头（桌面壳回第一把 Key、网页端 shim 回面板令牌/Key） */
  directCredentials?: () => Promise<unknown>
}

function bridge(): DesktopBridge | undefined {
  return (window as unknown as { workbuddyDesktop?: DesktopBridge }).workbuddyDesktop
}

/**
 * 请求的凭据模式：**默认的 `same-origin`，不要改成 `include`**。
 *
 * ── 为什么不能带跨源凭据 ────────────────────────────────────
 * 面板路由回的 CORS 头固定是 `Access-Control-Allow-Origin: *`（见
 * `http::attach_cors`），而按 Fetch 规范，**带凭据的跨源请求遇到 `*` 会被浏览器
 * 直接判为 CORS 失败** —— 表现就是一句 `TypeError: Failed to fetch`，且服务端
 * 日志里什么都看不到（请求其实到过、也回过了）。桌面形态正是跨源
 * （页面 `http://tauri.localhost` → 网关 `http://127.0.0.1:{port}`），
 * 用 `include` 会让助手页一进去就报「加载助手设置失败：Failed to fetch」。
 *
 * ── 为什么也不能是 `omit` ───────────────────────────────────
 * `omit` 是「永远不带凭据」，连**同源**都不带 —— web 形态（面板由网关自己托管、
 * 需要管理员会话）会被这条打成未登录。而 `same-origin` 恰好是两全的那个：
 *   · web 形态同源 → 照常带 cookie，面板会话认证不受影响；
 *   · 桌面形态跨源 → 浏览器自动不带 cookie，于是能通过 `*` 的 CORS 检查
 *     —— 凭证改由请求头显式表达，见下。
 *
 * ── 凭证头从哪来（桌面端靠它才不是裸请求）────────────────────
 * 桌面形态跨源 = cookie 一个都带不上，而助手这三条接口挂在**受保护**的
 * `/api/*` 上，两种情形都要求凭证：
 *   · 配置了网关 Key 时（正常部署都有）：`require_api_key` 逐把比对 ——
 *     未配置 Key 才是「免鉴权放行」，一旦有 Key，裸请求拿到的是 401
 *     `invalid_api_key`；
 *   · 面板认证开启后（设置页开过局域网访问就会注册管理员，见
 *     `commands::panel_register`）：要「面板会话或一把启用的 Key」——
 *     两者都没有时是 401 `panel_login_required`，界面显示「请先登录管理
 *     面板」，而桌面上并没有登录页（登录页只服务浏览器面板）。
 * 所以这里的每条直连请求都先问桥要一份凭证头（`directCredentials`）：
 *   · 桌面壳回第一把启用的 Key（与壳自己发管理请求用的同一把）；
 *   · 网页端 shim 回面板令牌 / 用户输入的 Key（口径与它自己的 httpCall 一致）。
 * Key 那一栏为空时（还没建过 Key）多带一个头也不会被拒，所以两条形态、
 * 有没有 Key 都共用同一条码路。
 */
const CREDENTIALS: RequestCredentials = 'same-origin'

/** 直连网关的凭证头（桥给的，见上面一段）。取一次即缓存：
 *  网关换 Key 后旧值会一直 401，靠 [`requestDirect`] 的「401 清缓存重试一次」
 *  自愈，不必每次请求都问一遍桥。 */
let cachedAuthHeaders: Record<string, string> | null = null

/** 桥返回的凭证表 → 只保留值为非空字符串的项（形状校验，防老壳/异常值） */
function asHeaderTable(value: unknown): Record<string, string> {
  const table: Record<string, string> = {}
  if (!value || typeof value !== 'object') return table
  for (const [name, raw] of Object.entries(value as Record<string, unknown>)) {
    if (typeof raw === 'string' && raw) table[name] = raw
  }
  return table
}

async function directAuthHeaders(force = false): Promise<Record<string, string>> {
  if (cachedAuthHeaders && !force) return cachedAuthHeaders
  let table: Record<string, string> = {}
  try {
    table = asHeaderTable(await bridge()?.directCredentials?.())
  } catch { /* 桥没就绪 / 这一版壳还没有这条方法：按无凭证发，401 的提示自己会说话 */ }
  cachedAuthHeaders = table
  return table
}

/** 当前该用哪个基址发请求（见模块头两条路） */
export async function gatewayBase(force = false): Promise<string> {
  if (cachedBase && !force) return cachedBase
  const api = bridge()
  if (api?.platform === 'web') {
    const origin = typeof window === 'undefined' ? '' : window.location?.origin
    if (origin && origin !== 'null') {
      cachedBase = origin
      return cachedBase
    }
  }
  let port = 0
  try {
    const status = await api?.getBackendStatus?.()
    const value = Number(status?.port)
    if (Number.isFinite(value) && value > 0) port = value
  } catch { /* 壳没就绪：走下面的默认端口路径 */ }
  if (port > 0) {
    cachedBase = `http://127.0.0.1:${port}`
    return cachedBase
  }
  // 端口还没上报（壳仍在启动）：先用默认端口，但**不缓存** —— 下一次调用会
  // 重新问一次壳，端口一就位就自动纠正。缓存住一个猜的端口，会让「面板刚起来
  // 那几秒先点了助手页」变成此后一直打错端口的顽固故障
  return `http://127.0.0.1:${FALLBACK_PORT}`
}

/* ─── 配置读写 ─────────────────────────────────────────────── */

/** 助手配置（与后端 `AssistantConfig::to_value` 同形）。
 *  `model` 是**对外模型名**（与 /v1/models 的 `data[].id` 同一口径：原始 ID 或
 *  映射名都行），不再是「提供商 + 模型」的组合 —— 助手的路由语义与普通客户端
 *  完全一致，同名在多家时按账号优先级主备。 */
export type AssistantConfig = {
  model: string | null
  systemPromptEnabled: boolean
  systemPrompt: string
  tools: Record<string, boolean>
}

/**
 * 从壳的桥组装**对外可请求的模型名列表**（选择器的数据源）。
 *
 * 口径与 `/v1/models` 对齐（那就是客户端能请求的名字）：
 *   · 已启用模型的 ID（默认绑定）；
 *   · 已启用模型之上的**已启用映射名**（用户建的 `ca/xxx` 这类，同样可请求）。
 * 走桥而不是直连 `/v1/models`：桥在桌面形态稳定可用（其它页面全在用它），
 * 而直连 fetch 是助手自己的通道，配置接口不通时列表没必要跟着陪葬。
 */
export async function loadModelNames(): Promise<string[]> {
  const bridge = (window as unknown as {
    workbuddyDesktop?: { getModelManage?: () => Promise<unknown> }
  }).workbuddyDesktop
  const manage = (await bridge?.getModelManage?.()) as {
    models?: Array<Record<string, unknown>>
    mappings?: Array<Record<string, unknown>>
  } | undefined
  const models = Array.isArray(manage?.models) ? manage.models : []
  const mappings = Array.isArray(manage?.mappings) ? manage.mappings : []
  // 先记「provider×id 是启用的」—— 映射名能不能请求，取决于它指向的模型开没开
  const enabledKeys = new Set<string>()
  const names: string[] = []
  const seen = new Set<string>()
  for (const row of models) {
    if (row.enabled !== true) continue
    const provider = String(row.provider ?? '')
    const id = String(row.id ?? '')
    if (!provider || !id) continue
    enabledKeys.add(`${provider}\u0001${id}`)
    if (!seen.has(id)) { seen.add(id); names.push(id) }
  }
  for (const mapping of mappings) {
    if (mapping.enabled !== true || mapping.isDefault === true) continue
    const provider = String(mapping.provider ?? '')
    const target = String(mapping.target ?? '')
    const alias = String(mapping.alias ?? '')
    if (!alias || !seen.has(alias) && enabledKeys.has(`${provider}\u0001${target}`)) {
      seen.add(alias)
      names.push(alias)
    }
  }
  return names
}

/** 内置默认系统提示词（与后端 `DEFAULT_SYSTEM_PROMPT` 逐字相同：编辑弹窗里的
 *  「恢复默认」与「当前是否等于默认」都按这份比对） */
export const DEFAULT_SYSTEM_PROMPT = `你是 Agent2API 的内置助手，帮用户管理本机的模型转发配置。

说话简洁，别铺垫。配置项一律用等宽字写出准确的 ID（模型名、账号名、提供商名），不要用「那个模型」这类含糊说法。

调用改动类工具之前，先用一句话说清将要改动什么；确认卡由界面渲染，不要跳过它直接执行。

拿不准的配置先去读（获取模型列表 / 查询账号余额），不要凭印象改。`

/**
 * 一次管理请求的公共部分（JSON in / JSON out）。
 *
 * ── 信封要自己拆 ────────────────────────────────────────────
 * 管理接口统一回 `{success: true, data: ...}`（失败是 `{success:false, error}`，
 * 见 `http::ok_json`）。走壳的桥那条路已经在 Rust 侧把 `data` 剥出来了，这里
 * 是**直连 HTTP**，所以得自己拆一层 —— 漏拆的话拿到的是信封本身，
 * 表现为「配置读了但全是 undefined」。
 */
/**
 * 错误体 → 可读文本。
 *
 * 管理接口的失败体是 `{success:false, error:"文本"}`，但**没走到的路由**回的是
 * axum 的兜底 404：`{error:{message:"Not found: ..."}}` —— 直接把那个对象塞进
 * `Error` 的 message 只会得到一句 `[object Object]`，所以这里把三种形态都捋一遍。
 */
function describeError(value: unknown, status: number): string {
  if (typeof value === 'string' && value.trim()) return value.trim()
  if (value && typeof value === 'object') {
    const message = (value as { message?: unknown }).message
    if (typeof message === 'string' && message.trim()) return message.trim()
    try { return JSON.stringify(value) } catch { /* 循环引用等：走下面的兜底 */ }
  }
  return `HTTP ${status}`
}

/** 把 fetch 的网络层失败包成带网关地址的 Error。
 *  浏览器对连接不上的回应只有一句 `TypeError: Failed to fetch`，不带任何上下文
 *  —— 把「连的是哪个地址」拼进去，用户对着侧栏的网关端口一眼就能看出对不对。 */
function wrapFetchError(error: unknown, base: string): Error {
  const message = error instanceof Error ? error.message : String(error)
  return new Error(t('连接网关失败（{base}）：{message}', { base, message }))
}

/**
 * 带凭证的一次直连请求（凭证来源见上面 CREDENTIALS 一段）。
 *
 * `body` 给对象即按 JSON 发（直连的两个接口都是 JSON 进出），`undefined` 表示
 * 不带体 —— 与原先每个调用点自己拼 headers/body 相比，凭证合并、Content-Type
 * 与重试策略都只有这一处实现。
 *
 * 401 时清掉凭证缓存重试一次：面板认证开启后网关要的是「面板会话或一把启用
 * 的 Key」，而那把 Key 可能在两次请求之间被换掉（管理页里重新生成第一把），
 * 缓存住的旧值会一直 401 —— 重问一次桥就自愈。**401 表示服务端没有处理这次
 * 请求**，重发不会有副作用（对话也一样）。
 */
async function requestDirect(
  path: string,
  options: { method: string; body?: unknown; signal?: AbortSignal },
  retried = false,
): Promise<Response> {
  const base = await gatewayBase()
  const headers: Record<string, string> = { ...(await directAuthHeaders()) }
  const payload = options.body === undefined ? undefined : JSON.stringify(options.body)
  if (payload !== undefined) headers['Content-Type'] = 'application/json'
  let response: Response
  try {
    response = await fetch(base + path, {
      method: options.method,
      headers,
      body: payload,
      signal: options.signal,
      credentials: CREDENTIALS,
    })
  } catch (error) {
    // 用户主动停止（abort）不是故障，原样交回 —— 调用方按 signal.aborted 区分
    if (options.signal?.aborted) throw error
    throw wrapFetchError(error, base)
  }
  if (response.status === 401 && !retried) {
    // 先把响应体读掉再重试：不消费会让这条连接一直吊着（同一把凭证再来一次
    // 也还是同一个 401，内容没有价值）
    await response.text().catch(() => '')
    cachedAuthHeaders = null
    return requestDirect(path, options, true)
  }
  return response
}

async function callJson<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await requestDirect(path, { method, body })
  const text = await response.text()
  let parsed: unknown = null
  try { parsed = text ? JSON.parse(text) : null } catch { parsed = null }
  const envelope = parsed as { success?: boolean; data?: unknown; error?: unknown } | null
  if (!response.ok || envelope?.success === false) {
    const raw = envelope?.error ?? (parsed as { error?: unknown } | null)?.error
    const message = describeError(raw, response.status)
    // 404 只有一种来源：这一版网关**没有助手接口**（前端比后端新）。把话说全，
    // 否则用户只会看到一句「HTTP 404」去猜是不是自己哪里配错了
    if (response.status === 404) {
      throw new Error(t('这一版网关还没有 AI 助手接口（HTTP 404）：后端还是旧的，重新编译 / 重启客户端后再试。{message}', { message }))
    }
    throw new Error(message)
  }
  const payload = envelope && typeof envelope === 'object' && 'data' in envelope ? envelope.data : parsed
  return payload as T
}

export async function loadAssistantConfig(): Promise<AssistantConfig> {
  const data = await callJson<{ config?: AssistantConfig }>('GET', '/api/assistant/config')
  const config = data?.config
  if (!config || typeof config !== 'object') throw new Error(t('配置响应不完整'))
  // model 兼容两种形态：字符串（现行）/ {provider, id}（改成对外名之前存过的）
  const model = typeof config.model === 'string'
    ? config.model
    : (config.model as { id?: unknown } | null)?.id
  return {
    model: typeof model === 'string' && model.trim() ? model : null,
    systemPromptEnabled: config.systemPromptEnabled !== false,
    systemPrompt: typeof config.systemPrompt === 'string' ? config.systemPrompt : '',
    tools: config.tools && typeof config.tools === 'object' ? config.tools : {},
  }
}

export async function saveAssistantConfig(config: AssistantConfig): Promise<AssistantConfig> {
  const data = await callJson<{ config?: AssistantConfig }>('PUT', '/api/assistant/config', { config })
  return data?.config && typeof data.config === 'object' ? data.config : config
}

/* ─── 流式对话 ─────────────────────────────────────────────── */

/** 一个工具调用（流式分片聚合后的完整形态） */
export type ToolCall = { id: string; name: string; arguments: string }

export type ChatMessage =
  | { role: 'user'; content: string }
  | { role: 'assistant'; content: string; toolCalls?: ToolCall[] }
  | { role: 'tool'; toolCallId: string; content: string }

export type ChatEvent =
  | { type: 'delta'; text: string }
  | { type: 'toolCalls'; calls: ToolCall[] }
  | { type: 'done'; finishReason: string }

/** 把页面层的消息数组转成上游认的形态（OpenAI 协议） */
export function toWireMessages(messages: ChatMessage[]): Array<Record<string, unknown>> {
  return messages.map(message => {
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
  })
}

/**
 * 发一轮对话（一次上游调用）并把 SSE 事件回调出去。
 *
 * `signal` 用来实现「停止」：abort 之后读者循环会以 AbortError 结束 ——
 * 那是用户的意图，不是错误，调用方按 `signal.aborted` 区分即可。
 */
export async function streamChat(options: {
  messages: ChatMessage[]
  tools: Array<Record<string, unknown>>
  signal?: AbortSignal
  onEvent: (event: ChatEvent) => void
}): Promise<void> {
  const body: Record<string, unknown> = { messages: toWireMessages(options.messages) }
  if (options.tools.length) body.tools = options.tools
  // 与 config 两条走同一个通道（凭证、跨源模式、401 重试都在 requestDirect 里）
  const response = await requestDirect('/api/assistant/chat', {
    method: 'POST',
    body,
    signal: options.signal,
  })
  if (!response.ok || !response.body) {
    const text = await response.text().catch(() => '')
    let parsed: unknown = null
    try { parsed = text ? JSON.parse(text) : null } catch { /* 非 JSON 错误体：保住状态码 */ }
    const raw = (parsed as { error?: unknown } | null)?.error
    const message = describeError(raw, response.status)
    if (response.status === 404) {
      throw new Error(t('这一版网关还没有 AI 助手接口（HTTP 404）：后端还是旧的，重新编译 / 重启客户端后再试。{message}', { message }))
    }
    throw new Error(message)
  }
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  // 累积槽：正文与 tool_calls 分片（按 index 聚合，见 OpenAI 流式约定）
  let text = ''
  const calls = new Map<number, ToolCall>()
  let finishReason = ''

  const handleFrame = (frame: string): boolean => {
    for (const rawLine of frame.split('\n')) {
      const line = rawLine.trimEnd()
      if (!line.startsWith('data:')) continue
      const payload = line.slice(5).trim()
      if (!payload) continue
      if (payload === '[DONE]') return true
      let chunk: Record<string, unknown>
      try { chunk = JSON.parse(payload) as Record<string, unknown> } catch { continue }
      const choice = Array.isArray(chunk.choices) ? (chunk.choices[0] as Record<string, unknown> | undefined) : undefined
      const delta = (choice?.delta || {}) as Record<string, unknown>
      const content = typeof delta.content === 'string' ? delta.content : ''
      if (content) {
        text += content
        options.onEvent({ type: 'delta', text: content })
      }
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

  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    let index = buffer.indexOf('\n\n')
    while (index >= 0) {
      const frame = buffer.slice(0, index)
      buffer = buffer.slice(index + 2)
      if (handleFrame(frame)) {
        // [DONE]：上游已收尾，剩下的字节不必再读
        await reader.cancel().catch(() => {})
        buffer = ''
        index = -1
        break
      }
      index = buffer.indexOf('\n\n')
    }
  }
  // 收尾：把聚合好的 tool_calls 一次性交出去（正文已逐段给过）
  const finished = [...calls.entries()].sort((a, b) => a[0] - b[0]).map(([, call]) => call)
  if (finished.length) options.onEvent({ type: 'toolCalls', calls: finished })
  options.onEvent({ type: 'done', finishReason })
}

/** 「这轮结束了吗」：有工具调用就还没结束（页面据此决定要不要回灌结果） */
export function hasToolCalls(calls: ToolCall[] | undefined): boolean {
  return Boolean(calls && calls.length)
}

/** 工具参数的宽松解析（模型偶尔给出不合法 JSON：给个空对象让它自己从报错里学） */
export function parseToolArguments(raw: string): Record<string, unknown> {
  const text = String(raw || '').trim()
  if (!text) return {}
  try {
    const parsed = JSON.parse(text)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, unknown> : {}
  } catch {
    return {}
  }
}
