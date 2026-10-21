/**
 * AI 助手页（侧边栏「智能」组的那个一级菜单）。
 *
 * ── 一句话结构 ──────────────────────────────────────────────
 * 左列会话历史 + 右列对话（标题栏 / 消息流 / 输入区）+ 右上角设置抽屉。
 * 助手每轮的回答来自 `POST /api/assistant/chat`（SSE，见 assistant-state），
 * 工具的执行与「确认卡」全在这一层（见 assistant-tools 的模块头）。
 *
 * ── agent loop 的形状（本文件里最要紧的一段）────────────────
 * 一轮 = 一次上游调用。上游回的 `tool_calls` 按**顺序逐个处理**：
 *   · 只读工具 → 立即执行，结果攒着，继续下一个；
 *   · 改动 / 消耗类 → 按权限模式分岔：
 *       `confirm` 停下来渲染确认卡（`suspendedRef`），等用户点「执行」或「取消」；
 *       `full`    **直接执行**，卡片标成「已自动执行」留在对话里；
 *     然后从下一个继续（`pump` 的续跑）；
 *   · 全部处理完 → 把攒下的结果作为 `role:"tool"` 消息回灌，再开下一轮。
 * 循环上限 `MAX_ROUNDS` 是硬保险：模型陷进「反复调工具」时用户不该看着它
 * 无限烧额度。
 *
 * ── 状态为什么用 ref + setState 双份 ─────────────────────────
 * 流式回调与异步续跑都是**闭包**，拿不到 setState 之后的新值；所以权威副本
 * 放在 `uiRef` / `wireRef` 两个 ref 里（UI 消息 / 上游协议消息），改完调
 * `syncUI()` 把 ref 的浅拷贝塞进 state 触发渲染。这是本项目多处在用的老办法
 * （见 accounts-page 的渲染范式），比在每处回调里拼 prev 更不容易出错。
 *
 * ── 与「设置页」的关系 ──────────────────────────────────────
 * 助手的三块配置（默认行为 / 系统提示词 / 工具）已从设置页搬进本页的抽屉
 * （assistant-settings-drawer）。数据流不变：仍走 `/api/assistant/config`。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Button, cn } from '@ui'
import { t } from '../i18n'
import {
  DEFAULT_SYSTEM_PROMPT,
  clearSessions,
  deleteSession,
  gatewayBase,
  loadAssistantConfig,
  loadSession,
  loadSessions,
  parseToolArguments,
  saveAssistantConfig,
  saveSession,
  streamChat,
  type AssistantConfig,
  type ChatMessage,
  type PermissionMode,
  type SessionSummary,
  type ToolCall,
} from './assistant-state'
import {
  ASSISTANT_TOOLS,
  errorText,
  loadModelOptions,
  toolById,
  toolsPayload,
  type ModelOption,
  type ToolResult,
} from './assistant-tools'
import {
  MAX_IMAGES,
  MAX_IMAGE_BYTES,
  AssistantComposer,
} from './assistant-composer'
import { prepareImage } from './assistant-images'
import { MessageList, type UiMessage } from './assistant-messages'
import { AssistantSettingsDrawer } from './assistant-settings-drawer'
import { PlusIcon } from './assistant-icons'

/** 一轮对话里最多允许的「上游调用 → 工具 → 上游调用」循环次数 */
const MAX_ROUNDS = 12

/** 侧边栏标题：取第一条用户消息的前若干字（没聊过时给占位） */
const TITLE_MAX_CHARS = 40
function titleFrom(messages: ChatMessage[]): string {
  const first = messages.find(message => message.role === 'user')
  const text = first && first.role === 'user' ? first.content.trim().replace(/\s+/g, ' ') : ''
  if (!text) return ''
  return text.length > TITLE_MAX_CHARS ? `${text.slice(0, TITLE_MAX_CHARS)}…` : text
}

/** 新会话 id（与后端 `core::assistant_sessions` 的「id 由前端生成」配套） */
function newSessionId(): string {
  const cryptoApi = (window as unknown as { crypto?: Crypto }).crypto
  if (cryptoApi?.randomUUID) return cryptoApi.randomUUID()
  return `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

/** 挂起的工具链：确认卡点完之后从 `index` 继续跑 */
type Suspended = {
  calls: ToolCall[]
  index: number
  collected: Array<{ toolCallId: string; content: string }>
  cardIndex: number
}

/* ─── 主组件 ─────────────────────────────────────────────── */

function AssistantPage() {
  const [config, setConfig] = React.useState<AssistantConfig | null>(null)
  const [models, setModels] = React.useState<ModelOption[]>([])
  const [ui, setUi] = React.useState<UiMessage[]>([])
  const [busy, setBusy] = React.useState(false)
  const [awaiting, setAwaiting] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  const [images, setImages] = React.useState<string[]>([])
  const [status, setStatus] = React.useState('')
  const [loadingFailed, setLoadingFailed] = React.useState(false)
  /** 历史会话列表（侧边栏）与当前打开的那一条 */
  const [sessions, setSessions] = React.useState<SessionSummary[]>([])
  const [activeId, setActiveId] = React.useState<string>('')
  /** 侧边栏在窄屏下是否展开（宽屏常驻，见 CSS 的 .asst-layout） */
  const [sidebarOpen, setSidebarOpen] = React.useState(false)
  /** 右上角「助手设置」抽屉 */
  const [settingsOpen, setSettingsOpen] = React.useState(false)

  const uiRef = React.useRef<UiMessage[]>([])
  /** 已选图片的最新值（`addFiles` 要按当前张数算还剩几个名额，而回调用
   *  useCallback([]) 固定住，读不到闭包里的 state） */
  const imagesRef = React.useRef<string[]>([])
  imagesRef.current = images
  /** 正在读取中的张数（读文件是异步的：不占名额就会「连点两次 + 选满 12 张」） */
  const pendingRef = React.useRef(0)
  const wireRef = React.useRef<ChatMessage[]>([])
  const suspendedRef = React.useRef<Suspended | null>(null)
  const controllerRef = React.useRef<AbortController | null>(null)
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const loadedRef = React.useRef(false)
  /** 当前会话 id（`uiRef` 那串消息归属哪一条；新建时换一个） */
  const activeIdRef = React.useRef<string>('')
  /** 会话标题（用户没改过就一直取第一条用户消息，见 `titleFrom`） */
  const titleRef = React.useRef<string>('')
  /** 保存的串行化：每轮回答结束都存一次，但两次保存不该并发（后到的会覆盖先到的） */
  const savingRef = React.useRef<Promise<void>>(Promise.resolve())
  /** 配置的最新值（`persist` 在异步链里读它，不能依赖闭包捕获的那一份） */
  const configRef = React.useRef<AssistantConfig | null>(null)
  configRef.current = config
  /** 配置加载已自动重试的次数（成功即清零；上限见 activate 内的判定） */
  const retryRef = React.useRef(0)
  /**
   * `loop` 的稳定引用（**别删**）。
   *
   * `pump` 与 `loop` 互相调用（工具跑完要开下一轮），而 React 的 useCallback
   * 一旦把它们放进彼此的 deps 就是循环依赖。写法上只能让 `pump` 的 deps 为空、
   * 通过这个 ref 去拿**最新**的 `loop`：没有它，`pump` 会永久持有首次渲染那一刻
   * 的 `loop`，而那一次的 `enabledTools` 还是空数组 —— 表现为「工具勾了但助手
   * 收不到任何工具定义」。
   */
  const loopRef = React.useRef<(() => Promise<void>) | null>(null)
  /** 本轮用户消息已经跑了几个来回（硬保险：见 MAX_ROUNDS） */
  const roundRef = React.useRef(0)

  const syncUI = React.useCallback(() => { setUi(uiRef.current.slice()) }, [])

  /**
   * 把当前会话落库（新建或覆盖）。
   *
   * ── 为什么每次回答完都存、而不是只在切走时存 ────────────────
   * 「切走时存」在崩溃 / 关窗口 / 切页时都会丢最后一轮 —— 而那正是用户最想
   * 留下的东西。每次回答完存一次的代价是一条本地 HTTP + 一次 upsert，可以忽略。
   *
   * ── 为什么要串行 ────────────────────────────────────────────
   * 一轮对话里 `loop` 可能被调多次（工具回灌后继续），每次结束都会触发保存。
   * 并发写同一条会话时，后到的请求**不保证**先落库 —— 那会让「刚存的最新版」
   * 被「稍早的旧版」覆盖。这里用一条 promise 链把保存排成队。
   */
  const persist = React.useCallback((): Promise<void> => {
    const id = activeIdRef.current
    if (!id) return Promise.resolve()
    const messages = wireRef.current
    if (!messages.length) return Promise.resolve()
    const title = titleRef.current || titleFrom(messages)
    const payload = {
      id,
      title,
      messages,
      model: configRef.current?.model ?? null,
      reasoning: configRef.current?.reasoning ?? null,
    }
    savingRef.current = savingRef.current
      .catch(() => { /* 上一次的失败不该阻断这一次 */ })
      .then(async () => {
        try {
          const saved = await saveSession(payload)
          titleRef.current = saved.title
          setSessions(prev => {
            const rest = prev.filter(item => item.id !== saved.id)
            return [saved, ...rest]
          })
        } catch (error) {
          setStatus(t('这条会话没能存进历史：{message}', { message: errorText(error) }))
        }
      })
    return savingRef.current
  }, [])
  /** `persist` 的稳定引用（`loop` 的 deps 里放它会陷入循环，见 loopRef 的说明） */
  const persistRef = React.useRef(persist)
  persistRef.current = persist

  /* 贴底：新消息进来时滚到底（用户往上翻时不打扰 —— 只在接近底部时贴） */
  React.useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < 200
    if (near) el.scrollTop = el.scrollHeight
  }, [ui])

  /* 数据加载：配置 + 模型清单（activate 时触发；重复调用只加载一次）
   *
   * 两件事**各自独立**，刻意不放进同一个 try：配置走的是直连 HTTP
   * （`/api/assistant/config`），模型清单走的是壳的桥 —— 后端的助手接口缺失
   * 或版本偏旧时，不该把「模型选择器」一起打成空的。
   *
   * 配置失败还会**自动重试**（有限次）：dev 刚启动时窗口可能比网关的 HTTP
   * 监听先就绪，那一刻的失败只代表「还早」，不代表「坏了」。 */
  const activate = React.useCallback(async () => {
    if (loadedRef.current) return
    loadedRef.current = true
    try {
      const next = await loadAssistantConfig()
      setConfig(next)
      setLoadingFailed(false)
      retryRef.current = 0
    } catch (error) {
      setLoadingFailed(true)
      setStatus(t('加载助手设置失败：{message}', { message: errorText(error) }))
      if (retryRef.current < 3) {
        retryRef.current += 1
        loadedRef.current = false
        setTimeout(() => { void activate() }, 4000)
      }
    }
    // 模型清单：助手自己只能选「可请求的名字」（启用的模型 ID + 启用的映射名）
    try {
      setModels(await loadModelOptions())
    } catch (error) {
      setStatus(prev => prev || t('读取模型清单失败：{message}', { message: errorText(error) }))
    }
    try {
      setSessions(await loadSessions())
    } catch (error) {
      setStatus(prev => prev || t('读取历史会话失败：{message}', { message: errorText(error) }))
    }
    // 首次进入开一条空白会话（不等用户点「新对话」：直接就能打字）
    if (!activeIdRef.current) {
      const id = newSessionId()
      activeIdRef.current = id
      titleRef.current = ''
      setActiveId(id)
    }
  }, [])

  React.useEffect(() => {
    const api = window as unknown as { wbAssistant?: { activate?: () => void } }
    api.wbAssistant = { activate: () => { void activate() } }
    void activate()
    return () => { delete api.wbAssistant }
  }, [activate])

  const toolEnabled = React.useMemo(() => {
    const map: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) map[tool.id] = config?.tools?.[tool.id] ?? false
    return map
  }, [config])
  const enabledTools = React.useMemo(() => toolsPayload(toolEnabled), [toolEnabled])
  const enabledCount = Object.values(toolEnabled).filter(Boolean).length

  const currentModel = config?.model ?? null
  const currentOption = models.find(item => item.name === currentModel)
  const modelSupportsImages = currentOption?.supportsImages === true
  const mode: PermissionMode = config?.mode ?? 'confirm'

  /* ─── agent loop ───────────────────────────────────────── */

  /** 处理一批 tool_calls（从 `index` 起；confirm 模式遇到改动类会挂起） */
  const pump = React.useCallback(async (calls: ToolCall[], index: number, collected: Suspended['collected']) => {
    for (let i = index; i < calls.length; i += 1) {
      const call = calls[i]
      const tool = toolById(call.name)
      const args = parseToolArguments(call.arguments)
      if (!tool) {
        uiRef.current.push({
          kind: 'tool', call, state: 'failed',
          error: t('未知工具「{name}」：它在设置里可能刚被取消勾选，或这一版助手还不认识它。', { name: call.name }),
        })
        collected.push({ toolCallId: call.id, content: `未知工具 ${call.name}，未执行。` })
        syncUI()
        continue
      }
      if (tool.kind === 'read') {
        const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'running' }) - 1
        syncUI()
        try {
          const result = await tool.run(args)
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'done', summary: result.summary, detail: result.text }
          collected.push({ toolCallId: call.id, content: result.text })
        } catch (error) {
          const message = errorText(error)
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
          collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
        }
        syncUI()
        continue
      }

      // 改动 / 消耗类：看权限模式决定「先问」还是「直接做」
      const plan = tool.plan?.(args) || { lead: t('将执行 {label}。', { label: tool.label }) }
      const auto = (configRef.current?.mode ?? 'confirm') === 'full'
      if (!auto) {
        const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'confirm', plan }) - 1
        suspendedRef.current = { calls, index: i, collected, cardIndex }
        setAwaiting(true)
        setBusy(false)
        syncUI()
        return
      }
      // 完全访问：不停下，执行结果仍以卡片留在消息流里（标「已自动执行」）
      const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'running', plan, auto: true }) - 1
      syncUI()
      try {
        const result = await tool.run(args)
        uiRef.current[cardIndex] = {
          kind: 'tool', call, state: 'done', auto: true,
          summary: result.summary, detail: result.text,
        }
        collected.push({ toolCallId: call.id, content: result.text })
      } catch (error) {
        const message = errorText(error)
        uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', auto: true, error: message }
        collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      syncUI()
    }
    // 一批处理完：回灌工具结果，进入下一轮（经 ref 拿最新的 loop，见它的说明）
    for (const item of collected) {
      wireRef.current.push({ role: 'tool', toolCallId: item.toolCallId, content: item.content })
    }
    await loopRef.current?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** 开一轮上游调用（流式收 delta / tool_calls） */
  const loop = React.useCallback(async () => {
    if (roundRef.current >= MAX_ROUNDS) {
      uiRef.current.push({
        kind: 'notice',
        text: t('这一轮来回已达 {n} 次上限，我先停下等你指示（可以换个说法，或拆成几步让我做）。', { n: MAX_ROUNDS }),
        bad: true,
      })
      syncUI()
      return
    }
    roundRef.current += 1
    setBusy(true)
    setStatus('')
    const controller = new AbortController()
    controllerRef.current = controller
    const botIndex = uiRef.current.push({ kind: 'bot', text: '', streaming: true }) - 1
    syncUI()
    let calls: ToolCall[] = []
    let failed = false
    try {
      await streamChat({
        messages: wireRef.current,
        tools: enabledTools,
        reasoning: configRef.current?.reasoning ?? null,
        signal: controller.signal,
        onEvent: event => {
          if (event.type === 'delta') {
            const current = uiRef.current[botIndex]
            if (current && current.kind === 'bot') {
              uiRef.current[botIndex] = { ...current, text: current.text + event.text }
              syncUI()
            }
          }
          if (event.type === 'toolCalls') calls = event.calls
        },
      })
    } catch (error) {
      failed = true
      const message = errorText(error)
      const current = uiRef.current[botIndex]
      const partial = current && current.kind === 'bot' ? current.text : ''
      if (!partial) uiRef.current.splice(botIndex, 1)
      else uiRef.current[botIndex] = { kind: 'bot', text: partial }
      const aborted = controller.signal.aborted
      uiRef.current.push({
        kind: 'notice',
        text: aborted ? t('已停止这次回答。') : t('请求失败：{message}', { message }),
        bad: !aborted,
      })
      syncUI()
    } finally {
      controllerRef.current = null
    }
    if (failed) {
      setBusy(false)
      return
    }
    const current = uiRef.current[botIndex]
    if (current && current.kind === 'bot') uiRef.current[botIndex] = { ...current, streaming: false }
    const text = current && current.kind === 'bot' ? current.text : ''
    wireRef.current.push({ role: 'assistant', content: text, toolCalls: calls.length ? calls : undefined })
    syncUI()
    // 每轮落一次库（串行，见 persist 的说明）：工具链中途也存，崩了不丢已答的部分
    void persistRef.current()
    if (!calls.length) {
      setBusy(false)
      return
    }
    await pump(calls, 0, [])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabledTools, pump, syncUI])
  loopRef.current = loop

  /** 确认卡上的一次决策：执行或取消，然后继续跑剩下的调用 */
  const resolveConfirm = React.useCallback(async (run: boolean) => {
    const suspended = suspendedRef.current
    if (!suspended) return
    suspendedRef.current = null
    setAwaiting(false)
    const call = suspended.calls[suspended.index]
    const tool = toolById(call.name)
    const args = parseToolArguments(call.arguments)
    if (!run || !tool) {
      uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'cancelled' }
      suspended.collected.push({ toolCallId: call.id, content: '用户取消了这次操作，没有做任何改动。' })
      syncUI()
    } else {
      uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'running' }
      setBusy(true)
      syncUI()
      try {
        const result: ToolResult = await tool.run(args)
        uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'done', summary: result.summary, detail: result.text }
        suspended.collected.push({ toolCallId: call.id, content: result.text })
      } catch (error) {
        const message = errorText(error)
        uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
        suspended.collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      syncUI()
    }
    await pump(suspended.calls, suspended.index + 1, suspended.collected)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pump, syncUI])

  /** 发送一条用户消息（带当前附件） */
  const send = React.useCallback(async (text: string, attached: string[]) => {
    const value = text.trim()
    if ((!value && attached.length === 0) || busy || awaiting) return
    if (!activeIdRef.current) {
      const id = newSessionId()
      activeIdRef.current = id
      setActiveId(id)
    }
    roundRef.current = 0
    uiRef.current.push({ kind: 'user', text: value, ...(attached.length ? { images: attached } : {}) })
    wireRef.current.push({ role: 'user', content: value, ...(attached.length ? { images: attached } : {}) })
    setDraft('')
    setImages([])
    syncUI()
    if (!config) {
      uiRef.current.push({
        kind: 'notice',
        text: t('助手设置没读出来，现在还不能发消息 —— 点右上角「助手设置」看看。'),
        bad: true,
      })
      syncUI()
      return
    }
    if (!config.model) {
      uiRef.current.push({
        kind: 'notice',
        text: t('还没有选择对话模型：在输入框右下角选一个，或点右上角「助手设置」。'),
        bad: true,
      })
      syncUI()
      return
    }
    await loop()
    // 用户这条消息先落库：模型没回（报错 / 用户停止）时，这句话也该留在历史里
    void persistRef.current()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awaiting, busy, config, loop, syncUI])

  const stop = React.useCallback(() => {
    controllerRef.current?.abort()
  }, [])

  /** 重置成一条空白会话（点「新对话」/ 新建时用） */
  const resetToBlank = React.useCallback((id: string) => {
    controllerRef.current?.abort()
    suspendedRef.current = null
    uiRef.current = []
    wireRef.current = []
    activeIdRef.current = id
    titleRef.current = ''
    setActiveId(id)
    setAwaiting(false)
    setBusy(false)
    setStatus('')
    syncUI()
  }, [syncUI])

  const newChat = React.useCallback(() => {
    void persistRef.current()
    resetToBlank(newSessionId())
    setSidebarOpen(false)
  }, [resetToBlank])

  /**
   * 打开一条历史会话。
   *
   * 正在跑的时候不允许切：那一轮的增量还在往 `uiRef` 里写，切走之后它会继续
   * 往**新会话**的数组里追加（同一个 ref）—— 结果是两条对话的内容混在一起。
   */
  const openSession = React.useCallback(async (id: string) => {
    if (busy || awaiting) return
    if (id === activeIdRef.current) { setSidebarOpen(false); return }
    await persistRef.current()
    try {
      const detail = await loadSession(id)
      // 回放：只画有正文的消息。工具调用卡（`role:"tool"`）与「只带 tool_calls、
      // 正文为空」的 assistant 消息**不重建卡片** —— 那张卡的可执行状态在当时的
      // 会话里才有意义，历史里重建出来会让人以为还能再点一次。
      uiRef.current = detail.messages
        .filter(message => message.role !== 'tool')
        .filter(message => message.role === 'user' || message.content.trim().length > 0)
        .map(message => message.role === 'user'
          ? { kind: 'user' as const, text: message.content, ...(message.images?.length ? { images: message.images } : {}) }
          : { kind: 'bot' as const, text: message.content })
      wireRef.current = detail.messages
      activeIdRef.current = detail.id
      titleRef.current = detail.title
      setActiveId(detail.id)
      suspendedRef.current = null
      setAwaiting(false)
      setBusy(false)
      setStatus('')
      syncUI()
      // 会话自己记了模型与等级：打开就恢复（用户在聊天框里改的也是这条会话的）
      if (config && (detail.model || detail.reasoning)) {
        const next: AssistantConfig = {
          ...config,
          model: detail.model ?? config.model,
          reasoning: detail.reasoning ?? config.reasoning,
        }
        setConfig(next)
        void saveAssistantConfig(next).catch(() => { /* 恢复失败不影响这段对话 */ })
      }
    } catch (error) {
      setStatus(t('打开会话失败：{message}', { message: errorText(error) }))
    }
    setSidebarOpen(false)
  }, [awaiting, busy, config, syncUI])

  const removeSession = React.useCallback(async (id: string) => {
    try {
      await deleteSession(id)
    } catch (error) {
      setStatus(t('删除会话失败：{message}', { message: errorText(error) }))
      return
    }
    setSessions(prev => prev.filter(item => item.id !== id))
    if (id === activeIdRef.current) resetToBlank(newSessionId())
  }, [resetToBlank])

  const clearHistory = React.useCallback(async () => {
    try {
      await clearSessions()
    } catch (error) {
      setStatus(t('清空历史失败：{message}', { message: errorText(error) }))
      return
    }
    setSessions([])
    resetToBlank(newSessionId())
  }, [resetToBlank])

  /** 保存助手配置（抽屉与输入区的选择器共用这一条） */
  const saveConfig = React.useCallback((next: AssistantConfig, note?: string) => {
    setConfig(next)
    void (async () => {
      try {
        const saved = await saveAssistantConfig(next)
        setConfig(saved)
        setLoadingFailed(false)
        if (note) setStatus(note)
      } catch (error) {
        setLoadingFailed(true)
        setStatus(t('保存失败：{message}', { message: errorText(error) }))
      }
    })()
  }, [])

  const pickModel = React.useCallback((name: string) => {
    if (!name) return
    if (!config) {
      setStatus(t('助手设置没读出来，暂时存不了对话模型。'))
      return
    }
    saveConfig({ ...config, model: name })
  }, [config, saveConfig])

  const pickReasoning = React.useCallback((level: string | null) => {
    if (!config) {
      setStatus(t('助手设置没读出来，暂时存不了思考等级。'))
      return
    }
    saveConfig({ ...config, reasoning: level })
  }, [config, saveConfig])

  const pickMode = React.useCallback((next: PermissionMode) => {
    if (!config) return
    saveConfig(
      { ...config, mode: next },
      next === 'full' ? t('已切换到完全访问') : t('已切换到变更前确认'),
    )
  }, [config, saveConfig])

  /**
   * 追加图片：类型 / 张数 / 单张大小三道门槛，超了说清楚哪一条不过。
   *
   * 名额按 `imagesRef` 的当前值算，**不在 setState 的 updater 里做副作用**
   * （读文件、再 setState）：updater 在严格模式下会被调两次，那样同一张图
   * 会读两遍、也可能多塞一张。读完的结果统一走 `appendImage` 的 updater。
   */
  const addFiles = React.useCallback((files: FileList | File[]) => {
    const list = Array.from(files)
    const imagesOnly = list.filter(file => file.type.startsWith('image/'))
    if (!imagesOnly.length) {
      setStatus(t('只支持图片文件（PNG / JPG / WebP / GIF）。'))
      return
    }
    const oversized = imagesOnly.find(file => file.size > MAX_IMAGE_BYTES)
    if (oversized) {
      setStatus(t('「{name}」超过 {n} MB，换一张小一点的。', {
        name: oversized.name,
        n: MAX_IMAGE_BYTES / 1024 / 1024,
      }))
      return
    }
    const room = MAX_IMAGES - imagesRef.current.length - pendingRef.current
    if (room <= 0) {
      setStatus(t('一次最多 {n} 张图片。', { n: MAX_IMAGES }))
      return
    }
    const take = imagesOnly.slice(0, room)
    if (take.length < imagesOnly.length) {
      setStatus(t('一次最多 {n} 张图片，多出的没有添加。', { n: MAX_IMAGES }))
    }
    pendingRef.current += take.length
    for (const file of take) {
      void prepareImage(file)
        .then(url => {
          if (url) setImages(current => (current.length >= MAX_IMAGES ? current : [...current, url]))
        })
        .catch(error => setStatus(t('读取图片失败：{message}', { message: errorText(error) })))
        .finally(() => { pendingRef.current = Math.max(0, pendingRef.current - 1) })
    }
  }, [])

  /** 粘贴 / 拖入都走 addFiles（文件名不同也能加，内容相同的图不去重：
   *  用户可能确实想发两张一样的做对比） */
  const addFilesDedup = React.useCallback((files: FileList | File[]) => {
    addFiles(files)
  }, [addFiles])

  /* ─── 渲染 ─── */

  const activeTitle = titleRef.current || titleFrom(wireRef.current) || t('新对话')
  const empty = ui.length === 0

  return (
    <div className={cn('asst-layout', sidebarOpen && 'side-open')}>
      {/* 左边：历史会话（宽屏常驻、窄屏抽屉，见 CSS 的 .asst-side） */}
      <aside className='asst-side'>
        <div className='asst-side-head'>
          <Button variant='default' size='sm' className='asst-new' onClick={newChat} disabled={busy || awaiting}>
            <PlusIcon size={14} /> {t('新对话')}
          </Button>
        </div>
        <div className='asst-side-list'>
          {sessions.length === 0 && <div className='asst-side-empty'>{t('还没有历史对话')}</div>}
          {sessions.map(item => (
            <div key={item.id}
              className={cn('asst-side-item', item.id === activeId && 'active')}
              role='button'
              tabIndex={0}
              onClick={() => void openSession(item.id)}
              onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') void openSession(item.id) }}>
              <span className='title'>{item.title || t('未命名对话')}</span>
              <button type='button' className='del' title={t('删除这条对话')}
                aria-label={t('删除这条对话')}
                onClick={event => { event.stopPropagation(); void removeSession(item.id) }}>✕</button>
            </div>
          ))}
        </div>
        {sessions.length > 0 && (
          <div className='asst-side-foot'>
            <button type='button' className='link' onClick={() => void clearHistory()}>{t('清空全部')}</button>
          </div>
        )}
      </aside>

      {sidebarOpen && <div className='asst-side-mask' onClick={() => setSidebarOpen(false)} />}

      <div className='asst-main'>
        {/* 顶栏：会话标题 + 助手设置 */}
        <div className='asst-top'>
          <button type='button' className='asst-menu' title={t('历史对话')} aria-label={t('历史对话')}
            onClick={() => setSidebarOpen(open => !open)}>☰</button>
          <span className='asst-title' title={activeTitle}>{activeTitle}</span>
          <span className='spacer' />
          <span className='asst-top-hint'>{t('走本地网关 · 消耗所选账号额度')}</span>
          <Button variant='ghost' size='sm' className='asst-settings-btn' onClick={() => setSettingsOpen(true)}>
            <svg viewBox='0 0 24 24' width='14' height='14' aria-hidden='true'>
              <g fill='none' stroke='currentColor' strokeWidth='1.6' strokeLinecap='round' strokeLinejoin='round'>
                <circle cx='12' cy='12' r='3.1' />
                <path d='M19.3 13.8l1.5 1.1-1.8 3-1.8-.6c-.5.4-1 .7-1.6.9l-.3 1.9h-3.5l-.3-1.9c-.6-.2-1.1-.5-1.6-.9l-1.8.6-1.8-3 1.5-1.1a7 7 0 0 1 0-1.9l-1.5-1.1 1.8-3 1.8.6c.5-.4 1-.7 1.6-.9l.3-1.9h3.5l.3 1.9c.6.2 1.1.5 1.6.9l1.8-.6 1.8 3-1.5 1.1c.05.32.07.63.07.95 0 .31-.02.62-.05.9z' />
              </g>
            </svg>
            {t('助手设置')}
          </Button>
        </div>

        {/* 完全访问的常驻提示（只在对话中、且开了完全访问时出现） */}
        {mode === 'full' && !empty && (
          <div className='asst-warnbar'>
            <svg viewBox='0 0 24 24' width='14' height='14' aria-hidden='true'>
              <path d='M13.2 2.5L5.4 13.4h5.2l-1.1 8.1 8.3-11.3h-5.2l.6-7.7z' fill='currentColor' />
            </svg>
            <span>{t('完全访问已开启：改动类工具会直接执行，不再弹出确认卡。')}</span>
            <button type='button' className='asst-textlink' onClick={() => pickMode('confirm')}>
              {t('改回变更前确认')}
            </button>
          </div>
        )}

        <div className='asst-scroll' ref={scrollRef}>
          {empty ? (
            <div className='asst-empty'>
              <div className='asst-empty-logo'>
                <svg viewBox='0 0 24 24' width='26' height='26' aria-hidden='true'>
                  <path fill='currentColor' d='M12 2.7l1.85 5.45 5.45 1.85-5.45 1.85L12 17.3l-1.85-5.45L4.7 10l5.45-1.85L12 2.7z' />
                  <path fill='currentColor' d='M18.8 15l.85 2.35L22 18.2l-2.35.85L18.8 21.4l-.85-2.35L15.6 18.2l2.35-.85L18.8 15z' />
                </svg>
              </div>
              <h3>{t('AI 助手')}</h3>
              <p>
                {t('用一句话让我查或改本机的模型转发配置。')}<br />
                {t('我会在动手前把改动列给你确认。')}
              </p>
              <div className='asst-suggests'>
                {[
                  t('看看哪些模型在多家重复了'),
                  t('测一下当前最快的模型通不通'),
                  t('CodeArts 的账号余额还剩多少？'),
                  t('把重复的模型加上家名前缀区分开'),
                ].map(text => (
                  <button key={text} type='button' onClick={() => void send(text, [])}>“{text}”</button>
                ))}
              </div>
              {config !== null && enabledCount === 0 && (
                <div className='asst-empty-warn'>
                  {t('当前一个工具都没勾选，助手只能纯聊天。点右上角「助手设置」勾上需要的工具。')}
                </div>
              )}
              {loadingFailed && (
                <div className='asst-empty-warn bad'>{status || t('助手设置读取失败，工具可能已过时。')}</div>
              )}
            </div>
          ) : (
            <div className='asst-flow'>
              <MessageList messages={ui} onConfirm={run => void resolveConfirm(run)} />
            </div>
          )}
        </div>

        <AssistantComposer
          draft={draft}
          onDraftChange={setDraft}
          images={images}
          onAddFiles={addFilesDedup}
          onRemoveImage={index => setImages(prev => prev.filter((_, i) => i !== index))}
          modelSupportsImages={modelSupportsImages}
          models={models}
          model={currentModel}
          onPickModel={pickModel}
          reasoning={config?.reasoning ?? null}
          onPickReasoning={pickReasoning}
          mode={mode}
          onModeChange={pickMode}
          busy={busy}
          awaiting={awaiting}
          onSend={() => void send(draft, images)}
          onStop={stop}
        />

        {status && !loadingFailed && (
          <div className='asst-statusline'>{status}</div>
        )}
      </div>

      <AssistantSettingsDrawer
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        config={config}
        models={models.map(item => item.name)}
        busy={busy}
        failed={loadingFailed}
        status={status}
        onSave={saveConfig}
      />
    </div>
  )
}

/* ─── 挂载：接管 index.html 里的 assistant 页 section ─── */

const PAGE_SELECTOR = '.page[data-page="assistant"]'

let root: ReturnType<typeof createRoot> | null = null

function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  root.render(<AssistantPage />)
}

if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

// 让 gatewayBase 的预热随岛加载一起发生（首轮对话少一次往返）
void gatewayBase().catch(() => {})

export { DEFAULT_SYSTEM_PROMPT }
