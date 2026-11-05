/**
 * AI 助手的直连管理 API 层：网关地址、管理凭证、JSON 信封与错误归一化。
 *
 * ── 为什么单独成文件 ────────────────────────────────────────
 * 对话流、配置读写、会话存删、图片下载，以及工具执行（账号导入）都要发管理
 * 请求。工具定义在 `assistant-tools` 里，而那个模块又被 `assistant-state`
 * 转出（模型选择器复用 `loadModelNames`）—— 两边互相 import 会形成环。把
 * 「怎么发一条管理请求」沉到这里：`assistant-state` 与工具模块都只依赖它，
 * 依赖方向始终是单向的。
 *
 * ── 为什么这些请求不走壳的桥 ─────────────────────────────────
 * 桥（`window.workbuddyDesktop.xxx`）是「一次性拿结果」的形态：没有流式，
 * 而且有 60 秒总超时（见 `src/gateway.rs` 的 REQUEST_TIMEOUT_MS）—— 对话回答
 * 动辄跑几十秒，走桥会被截断。所以这里直接 fetch 到**网关基址**：
 *   · web 形态（面板由网关自己托管）：基址 = 地址栏 origin，同源，cookie 自动带；
 *   · 桌面形态：页面来自 Tauri 自定义协议，基址只能问壳要真实端口
 *     （`getBackendStatus`），再拼 `http://127.0.0.1:{port}`。
 * 两条路都落在面板路由 `/api/*`（无条件 CORS），所以不需要用户打开网关面的
 * CORS 开关 —— 那条开关管的是 `/v1/*`，与这里无关。
 *
 * ── 请求的凭据模式：**默认的 `same-origin`，不要改成 `include`** ──
 *
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
 * ── 管理凭证与模型 Key 分开 ───────────────────────────────────
 * 管理接口使用 `directCredentials` 返回的会话头；模型调用另外携带用户选择的
 * `x-api-key`，本机会话不能替代 Key，也不改变 Key 的模型或提供商限制。
 */

import { t } from '../i18n'

const FALLBACK_PORT = 3065

/** 网关基址缓存（每次进对话页最多问一次壳） */
let cachedBase: string | null = null

type BackendStatus = { port?: number }
type DesktopBridge = {
  platform?: string
  getBackendStatus?: () => Promise<BackendStatus | null | undefined>
  waitForBackend?: () => Promise<BackendStatus | null | undefined>
  /** 当前管理凭证；网页端 refresh=true 时先静默续期，桌面端由壳自行换证。 */
  directCredentials?: (refresh?: boolean) => Promise<unknown>
}

function bridge(): DesktopBridge | undefined {
  return (window as unknown as { workbuddyDesktop?: DesktopBridge }).workbuddyDesktop
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

/** 同源请求照常带 cookie；跨源桌面请求由当前会话头表达凭证。 */
const CREDENTIALS: RequestCredentials = 'same-origin'

/** 桥返回的凭证表 → 只保留值为非空字符串的项（形状校验，防老壳/异常值） */
function asHeaderTable(value: unknown): Record<string, string> {
  const table: Record<string, string> = {}
  if (!value || typeof value !== 'object') return table
  for (const [name, raw] of Object.entries(value as Record<string, unknown>)) {
    if (typeof raw === 'string' && raw) table[name] = raw
  }
  return table
}

/** 不跨请求缓存：桌面壳会提前换证，网页端也可能已在另一标签页完成续期。 */
async function directAuthHeaders(refresh = false): Promise<Record<string, string>> {
  return asHeaderTable(await bridge()?.directCredentials?.(refresh))
}

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

export class AssistantRequestError extends Error {
  constructor(message: string, readonly status: number, readonly code: string) {
    super(message)
    this.name = 'AssistantRequestError'
  }
}

export function requestError(parsed: unknown, status: number, method: string, path: string): AssistantRequestError {
  const envelope = parsed && typeof parsed === 'object'
    ? parsed as { error?: unknown; code?: unknown } : null
  const raw = envelope?.error ?? parsed
  const detail = raw && typeof raw === 'object' ? raw as { code?: unknown } : null
  const code = typeof detail?.code === 'string' ? detail.code
    : typeof envelope?.code === 'string' ? envelope.code : ''
  const message = describeError(raw, status)
  // 只有路由兜底才表示接口缺失，模型和会话的业务错误也可能是 404。
  if (status === 404 && message === `Not found: ${method} ${path}`) {
    return new AssistantRequestError(t('这一版网关还没有 AI 助手接口（HTTP 404）：后端还是旧的，重新编译 / 重启客户端后再试。{message}', { message }), status, 'api_not_found')
  }
  return new AssistantRequestError(message, status, code)
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
 * 明确的管理会话失效时重新获取凭证，网页端同时续期；最多重试一次，模型 Key 错误不重试。
 */
export async function requestDirect(
  path: string,
  options: { method: string; body?: unknown; signal?: AbortSignal; apiKey?: string },
  retried = false,
): Promise<Response> {
  const api = bridge()
  if (api?.platform !== 'web' && api?.waitForBackend) await api.waitForBackend()
  const base = await gatewayBase()
  const headers: Record<string, string> = { ...(await directAuthHeaders(retried)) }
  if (options.apiKey !== undefined) {
    headers['x-api-key'] = options.apiKey
    headers.Authorization = `Bearer ${options.apiKey}`
  }
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
    type AuthFailure = { error?: { type?: string; code?: string } }
    let rejected: AuthFailure | null = null
    try { rejected = await response.clone().json() as AuthFailure } catch { /* 非 JSON 不触发会话恢复 */ }
    const kind = rejected?.error?.type || rejected?.error?.code
    const desktopRecovery = api?.platform !== 'web' && (kind === 'panel_login_required'
      || kind === 'invalid_api_key' && options.apiKey === undefined)
    const webRetry = api?.platform === 'web' && kind === 'panel_login_required' && options.apiKey === undefined
      && (options.method === 'GET' || !path.startsWith('/api/assistant/images/'))
    if (desktopRecovery || webRetry) {
      await response.text().catch(() => '')
      return requestDirect(path, options, true)
    }
  }
  return response
}

export async function callJson<T>(method: string, path: string, body?: unknown, signal?: AbortSignal, apiKey?: string): Promise<T> {
  const response = await requestDirect(path, { method, body, signal, apiKey })
  const text = await response.text()
  let parsed: unknown = null
  try { parsed = text ? JSON.parse(text) : null } catch { parsed = null }
  const envelope = parsed as { success?: boolean; data?: unknown; error?: unknown } | null
  if (!response.ok || envelope?.success === false) {
    throw requestError(parsed, response.status, method, path)
  }
  const payload = envelope && typeof envelope === 'object' && 'data' in envelope ? envelope.data : parsed
  return payload as T
}
