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
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { t } from '../i18n'
import {
  AssistantRequestError,
  DEFAULT_SYSTEM_PROMPT,
  gatewayBase,
  loadSession,
  loadSessions,
  parseToolArguments,
  newSessionId,
  streamChat,
  toast,
  type AssistantConfig,
  type ChatMessage,
  type PermissionMode,
  type ToolCall,
} from './assistant-state'
import {
  ASSISTANT_TOOLS,
  errorText,
  toolById,
  toolsPayload,
  type ToolResult,
} from './assistant-tools'
import { selectedModel, unavailableModelMessage } from './assistant-models'
import { useAssistantModels } from './assistant-model-state'
import { useAssistantConfig } from './assistant-config-state'
import { titleFrom, useAssistantSessions } from './assistant-session-state'
import { AssistantComposer } from './assistant-composer'
import { useAssistantAttachments } from './assistant-attachments'
import { MessageList, type UiMessage } from './assistant-messages'
import { AssistantSettingsDrawer } from './assistant-settings-drawer'
import { AssistantEmpty, AssistantSidebar, AssistantTopbar } from './assistant-shell'
import { useAssistantMessageOperations } from './assistant-message-operations'
import { AssistantTitleSettings } from '../../../ui-shared/assistant-title-settings'
import { replayMessages } from './assistant-replay'
import { imageContext, type ImageAsset } from '../../../ui-shared/assistant-image-tools'
import { assistantFileContext, type AssistantTextFile } from '../../../ui-shared/assistant-files'
import { useModelApiKeys } from '../../../ui-shared/use-model-api-keys'
import { ModelApiKeyField } from '../../../ui-shared/model-api-key-field'
import { modelApiKeyAllows, modelApiKeyLabel, type ModelApiKey } from '../../../ui-shared/model-api-key'
import { callJson } from './assistant-state'

/** 一轮对话里最多允许的「上游调用 → 工具 → 上游调用」循环次数 */
const MAX_ROUNDS = 12

/** 挂起的工具链：确认卡点完之后从 `index` 继续跑 */
type Suspended = {
  calls: ToolCall[]
  index: number
  collected: Array<{ toolCallId: string; content: string; images?: ImageAsset[] }>
  cardIndex: number
}

/* ─── 主组件 ─────────────────────────────────────────────── */

function AssistantPage() {
  const { config, configRef, status, failed: loadingFailed, load: loadConfig, save: saveConfig } = useAssistantConfig()
  const keyState = useModelApiKeys(() => callJson('GET', '/api/model-access/keys'))
  const selectedKey = keyState.keys.find(key => key.id === config?.apiKeyId)
  const requestKeyRef = React.useRef<ModelApiKey | undefined>(undefined)
  const { models, ready: modelsReady, loading: modelsLoading, error: modelsError, refresh: refreshModels } = useAssistantModels(selectedKey?.key)
  const [ui, setUi] = React.useState<UiMessage[]>([])
  const [busy, setBusy] = React.useState(false)
  const [awaiting, setAwaiting] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  /** 历史会话列表（侧边栏）与当前打开的那一条 */
  const [activeId, setActiveId] = React.useState<string>('')
  /** 侧边栏在窄屏下是否展开（宽屏常驻，见 CSS 的 .asst-layout） */
  const [sidebarOpen, setSidebarOpen] = React.useState(false)
  /** 右上角「助手设置」抽屉 */
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [titleSettingsOpen, setTitleSettingsOpen] = React.useState(false)

  const uiRef = React.useRef<UiMessage[]>([])
  const wireRef = React.useRef<ChatMessage[]>([])
  const suspendedRef = React.useRef<Suspended | null>(null)
  const controllerRef = React.useRef<AbortController | null>(null)
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const loadedRef = React.useRef(false)
  /** 当前会话 id（`uiRef` 那串消息归属哪一条；新建时换一个） */
  const activeIdRef = React.useRef<string>('')
  const { images, setImages, files, setFiles, addFiles, reading: readingAttachments } = useAssistantAttachments(activeIdRef)
  /** 会话标题（用户没改过就一直取第一条用户消息，见 `titleFrom`） */
  const titleRef = React.useRef<string>('')
  const { sessions, setSessions, persist, summarize, remove: removeSavedSession, clear: clearSavedSessions } = useAssistantSessions({
    config: configRef, messages: wireRef, ui: uiRef, activeId: activeIdRef, title: titleRef, apiKey: requestKeyRef,
  })
  const stoppedRef = React.useRef(false)
  const historyBusyRef = React.useRef(false)
  const [historyBusy, setHistoryBusy] = React.useState(false)
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
  const sendingRef = React.useRef(false)
  const checkingModelRef = React.useRef(false)
  const preflightRef = React.useRef(0)
  const requestModelRef = React.useRef<string>('')
  const roundRef = React.useRef(0)

  const refreshModelOptions = React.useCallback(() => { void refreshModels().catch(() => {}) }, [refreshModels])

  const syncUI = React.useCallback(() => { setUi(uiRef.current.slice()) }, [])

  /** `persist` 的稳定引用（`loop` 的 deps 里放它会陷入循环，见 loopRef 的说明） */
  const persistRef = React.useRef(persist)
  persistRef.current = persist
  const summarizeRef = React.useRef(summarize)
  summarizeRef.current = summarize

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
    void loadConfig()
    void refreshModels().catch(() => {})
    if (loadedRef.current) return
    loadedRef.current = true
    try {
      setSessions(await loadSessions())
    } catch (error) {
      toast(t('读取历史会话失败：{message}', { message: errorText(error) }), 'err')
    }
    // 首次进入开一条空白会话（不等用户点「新对话」：直接就能打字）
    if (!activeIdRef.current) {
      const id = newSessionId()
      activeIdRef.current = id
      titleRef.current = ''
      setActiveId(id)
    }
  }, [loadConfig, refreshModels])

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
  const currentOption = selectedModel(models, currentModel)
  const modelSupportsImages = currentOption?.supportsImages === true
  const modelWarning = modelsError
    ? t('读取模型清单失败：{message}', { message: modelsError })
    : !modelsReady ? t('正在读取可用模型…')
    : currentModel && !currentOption ? unavailableModelMessage(currentModel) : ''
  const mode: PermissionMode = config?.mode ?? 'confirm'

  const runTool = React.useCallback(async (tool: NonNullable<ReturnType<typeof toolById>>, args: Record<string, unknown>, collected: Suspended['collected']): Promise<ToolResult> => {
    const controller = new AbortController()
    controllerRef.current = controller
    const session = activeIdRef.current
    try {
      if (tool.id === 'generate_images' || tool.id === 'edit_images') await persistRef.current(true, collected)
      if (controller.signal.aborted) throw new DOMException('Aborted', 'AbortError')
      const references = imageContext([...wireRef.current, ...collected.map(item => ({ role: 'tool', images: item.images }))])
      const attachmentFiles = assistantFileContext(wireRef.current)
      const key = requestKeyRef.current
      const result = await tool.run(args, { session, images: references, files: attachmentFiles, confirmed: true, signal: controller.signal, apiKey: key?.key, apiKeyName: key?.name })
      if (activeIdRef.current !== session) throw new DOMException('Aborted', 'AbortError')
      return result
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null
    }
  }, [])

  /* ─── agent loop ───────────────────────────────────────── */

  /** 处理一批 tool_calls（从 `index` 起；confirm 模式遇到改动类会挂起） */
  const pump = React.useCallback(async (calls: ToolCall[], index: number, collected: Suspended['collected']) => {
    setBusy(true)
    const session = activeIdRef.current
    for (let i = index; i < calls.length; i += 1) {
      if (activeIdRef.current !== session) return
      if (stoppedRef.current) {
        for (const call of calls.slice(i)) {
          uiRef.current.push({ kind: 'tool', call, state: 'cancelled' })
          collected.push({ toolCallId: call.id, content: '用户已停止，未执行这次操作。' })
        }
        break
      }
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
      if (configRef.current?.tools?.[tool.id] !== true) {
        const message = t('工具「{name}」未获授权，未执行。', { name: tool.label })
        uiRef.current.push({ kind: 'tool', call, state: 'failed', error: message })
        collected.push({ toolCallId: call.id, content: message })
        syncUI()
        continue
      }
      const refreshingBalance = tool.id === 'list_account_balance' && args.refresh === true
      if (tool.kind === 'read' && !refreshingBalance) {
        const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'running' }) - 1
        syncUI()
        try {
          const result = await runTool(tool, args, collected)
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'done', summary: result.summary, detail: result.text, images: result.images }
          collected.push({ toolCallId: call.id, content: result.text, images: result.images })
          if (result.images?.length) void persistRef.current(true, collected)
        } catch (error) {
          if (error instanceof DOMException && error.name === 'AbortError') {
            if (activeIdRef.current !== session) return
            stoppedRef.current = true
            uiRef.current[cardIndex] = { kind: 'tool', call, state: 'cancelled' }
            collected.push({ toolCallId: call.id, content: '用户已停止，未继续执行这次操作。' })
            syncUI()
            continue
          }
          const message = errorText(error)
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
          collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
        }
        syncUI()
        continue
      }

      // 改动 / 消耗类：看权限模式决定「先问」还是「直接做」
      const plan = tool.plan?.(args) || {
        lead: t('将执行 {label}。', { label: tool.label }),
        ...(refreshingBalance ? { note: t('实时余额查询会请求上游，并可能触发已配置的自动禁用规则。') } : {}),
      }
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
        const result = await runTool(tool, args, collected)
        uiRef.current[cardIndex] = {
          kind: 'tool', call, state: 'done', auto: true,
          summary: result.summary, detail: result.text, images: result.images,
        }
        collected.push({ toolCallId: call.id, content: result.text, images: result.images })
        if (result.images?.length) void persistRef.current(true, collected)
      } catch (error) {
        if (error instanceof DOMException && error.name === 'AbortError') {
          if (activeIdRef.current !== session) return
          stoppedRef.current = true
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'cancelled' }
          collected.push({ toolCallId: call.id, content: '用户已停止，未继续执行这次操作。' })
          syncUI()
          continue
        }
        const message = errorText(error)
        uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', auto: true, error: message }
        collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      syncUI()
    }
    // 一批处理完：回灌工具结果，进入下一轮（经 ref 拿最新的 loop，见它的说明）
    for (const item of collected) {
      wireRef.current.push({ role: 'tool', toolCallId: item.toolCallId, content: item.content, images: item.images })
    }
    syncUI()
    await persistRef.current()
    if (stoppedRef.current) { setBusy(false); return }
    await loopRef.current?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** 开一轮上游调用（流式收 delta / tool_calls） */
  const loop = React.useCallback(async () => {
    if (stoppedRef.current) { setBusy(false); return }
    const session = activeIdRef.current
    if (roundRef.current >= MAX_ROUNDS) {
      uiRef.current.push({
        kind: 'notice',
        text: t('这一轮来回已达 {n} 次上限，我先停下等你指示（可以换个说法，或拆成几步让我做）。', { n: MAX_ROUNDS }),
        bad: true,
      })
      syncUI()
      setBusy(false)
      return
    }
    roundRef.current += 1
    setBusy(true)
    const controller = new AbortController()
    controllerRef.current = controller
    const messageId = newSessionId()
    const botIndex = uiRef.current.push({ kind: 'bot', id: messageId, model: requestModelRef.current, text: '', streaming: true }) - 1
    syncUI()
    let calls: ToolCall[] = []
    let failed = false
    try {
      if (!requestKeyRef.current) throw new Error(t('请先选择一把 API Key'))
      await streamChat({
        model: requestModelRef.current,
        apiKey: requestKeyRef.current.key,
        messages: wireRef.current,
        tools: enabledTools,
        reasoning: configRef.current?.reasoning ?? null,
        signal: controller.signal,
        onEvent: event => {
          if (activeIdRef.current !== session) return
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
      if (activeIdRef.current !== session) return
      failed = true
      if (error instanceof AssistantRequestError && error.code === 'model_not_found') {
        void refreshModels().catch(() => {})
      }
      const message = errorText(error)
      const current = uiRef.current[botIndex]
      const partial = current && current.kind === 'bot' ? current.text : ''
      if (!partial) uiRef.current.splice(botIndex, 1)
      else {
        uiRef.current[botIndex] = { kind: 'bot', id: messageId, model: requestModelRef.current, text: partial }
        wireRef.current.push({ role: 'assistant', id: messageId, model: requestModelRef.current, content: partial })
      }
      void persistRef.current()
      const aborted = controller.signal.aborted
      uiRef.current.push({
        kind: 'notice',
        text: aborted ? t('已停止这次回答。') : t('请求失败：{message}', { message }),
        bad: !aborted,
      })
      syncUI()
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null
    }
    if (activeIdRef.current !== session) return
    if (failed) {
      setBusy(false)
      return
    }
    const current = uiRef.current[botIndex]
    const text = current && current.kind === 'bot' ? current.text : ''
    if (current && current.kind === 'bot') {
      if (!text && !calls.length) uiRef.current.splice(botIndex, 1)
      else uiRef.current[botIndex] = { ...current, streaming: false }
    }
    wireRef.current.push({ role: 'assistant', id: messageId, model: requestModelRef.current, content: text, toolCalls: calls.length ? calls : undefined })
    syncUI()
    // 每轮落一次库（串行，见 persist 的说明）：工具链中途也存，崩了不丢已答的部分
    await persistRef.current()
    if (!calls.length) {
      setBusy(false)
      if (!stoppedRef.current) void summarizeRef.current(session)
      return
    }
    await pump(calls, 0, [])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabledTools, pump, syncUI, refreshModels])
  loopRef.current = loop

  /** 确认卡上的一次决策：执行或取消，然后继续跑剩下的调用 */
  const resolveConfirm = React.useCallback(async (run: boolean) => {
    const session = activeIdRef.current
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
        if (configRef.current?.tools?.[tool.id] !== true) throw new Error(t('工具「{name}」未获授权，未执行。', { name: tool.label }))
        const result: ToolResult = await runTool(tool, args, suspended.collected)
        uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'done', summary: result.summary, detail: result.text, images: result.images }
        suspended.collected.push({ toolCallId: call.id, content: result.text, images: result.images })
        if (result.images?.length) void persistRef.current(true, suspended.collected)
      } catch (error) {
        if (error instanceof DOMException && error.name === 'AbortError') {
          if (activeIdRef.current !== session) return
          stoppedRef.current = true
          uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'cancelled' }
          suspended.collected.push({ toolCallId: call.id, content: '用户已停止，未继续执行这次操作。' })
          syncUI()
          await pump(suspended.calls, suspended.index + 1, suspended.collected)
          return
        }
        const message = errorText(error)
        uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
        suspended.collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      syncUI()
    }
    await pump(suspended.calls, suspended.index + 1, suspended.collected)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pump, syncUI])

  /** 发送前校验当前模型，校验失败不追加历史也不清空草稿。 */
  const send = React.useCallback(async (text: string, attached: string[], textFiles: AssistantTextFile[]) => {
    const value = text.trim()
    if ((!value && attached.length === 0 && textFiles.length === 0) || busy || awaiting || readingAttachments || sendingRef.current || historyBusyRef.current) return
    const current = configRef.current
    if (!current?.model) {
      toast(current
        ? t('还没有选择对话模型：在输入框右下角选一个，或点右上角「助手设置」。')
        : t('助手设置没读出来，现在还不能发消息 —— 点右上角「助手设置」看看。'), 'err')
      return
    }
    const attempt = ++preflightRef.current
    sendingRef.current = true
    checkingModelRef.current = true
    setBusy(true)
    const model = current.model
    const session = activeIdRef.current
    try {
      const [latest, keys] = await Promise.all([refreshModels(), keyState.refresh()])
      if (attempt !== preflightRef.current || activeIdRef.current !== session || configRef.current?.model !== model) return
      const key = keys.find(key => key.id === current.apiKeyId)
      if (!key) {
        toast(t('请先选择一把 API Key'), 'err')
        return
      }
      if (!modelApiKeyAllows(key, model)) {
        toast(t('所选 API Key 无权调用此模型或提供商'), 'err')
        return
      }
      const option = selectedModel(latest, model)
      if (!option) {
        toast(unavailableModelMessage(model), 'err')
        return
      }
      checkingModelRef.current = false
      requestModelRef.current = option.name
      requestKeyRef.current = key
      if (!activeIdRef.current) {
        const id = newSessionId()
        activeIdRef.current = id
        setActiveId(id)
      }
      roundRef.current = 0
      stoppedRef.current = false
      const messageId = newSessionId()
      uiRef.current.push({
        kind: 'user', id: messageId, text: value,
        ...(attached.length ? { images: attached } : {}),
        ...(textFiles.length ? { files: textFiles } : {}),
      })
      wireRef.current.push({
        role: 'user', id: messageId, content: value,
        ...(attached.length ? { images: attached } : {}),
        ...(textFiles.length ? { files: textFiles } : {}),
      })
      setDraft('')
      setImages([])
      setFiles([])
      syncUI()
      await loop()
      await persistRef.current()
    } catch (error) {
      if (attempt === preflightRef.current) {
        toast(checkingModelRef.current
          ? t('读取模型清单失败：{message}', { message: errorText(error) })
          : t('请求失败：{message}', { message: errorText(error) }), 'err')
      }
    } finally {
      if (attempt === preflightRef.current) {
        checkingModelRef.current = false
        sendingRef.current = false
        setBusy(false)
      }
    }
  }, [awaiting, busy, readingAttachments, loop, syncUI, refreshModels, configRef, keyState.refresh, setFiles])

  const stop = React.useCallback(() => {
    if (checkingModelRef.current) {
      preflightRef.current += 1
      checkingModelRef.current = false
      sendingRef.current = false
      setBusy(false)
      return
    }
    stoppedRef.current = true
    controllerRef.current?.abort()
  }, [])

  /** 重置成一条空白会话（点「新对话」/ 新建时用） */
  const resetToBlank = React.useCallback((id: string) => {
    preflightRef.current += 1
    checkingModelRef.current = false
    sendingRef.current = false
    controllerRef.current?.abort()
    suspendedRef.current = null
    uiRef.current = []
    wireRef.current = []
    activeIdRef.current = id
    titleRef.current = ''
    setActiveId(id)
    setAwaiting(false)
    setBusy(false)
    setImages([])
    setFiles([])
    syncUI()
  }, [syncUI, setImages, setFiles])

  const newChat = React.useCallback(() => {
    if (historyBusyRef.current || sendingRef.current) return
    // 与 openSession 同一理由：开新对话前保存旧的，但不算「有新内容」，
    // 不该把刚才那条顶到列表第一位。
    void persistRef.current(false)
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
    if (busy || awaiting || sendingRef.current || historyBusyRef.current) return
    if (id === activeIdRef.current) { setSidebarOpen(false); return }
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await persistRef.current(false)
      const detail = await loadSession(id)
      uiRef.current = replayMessages(detail.messages, detail.model)
      wireRef.current = detail.messages
      activeIdRef.current = detail.id
      titleRef.current = detail.title
      setActiveId(detail.id)
      setImages([])
      setFiles([])
      setDraft('')
      suspendedRef.current = null
      setAwaiting(false)
      setBusy(false)
      syncUI()
      // 会话自己记了模型与等级：打开就恢复（用户在聊天框里改的也是这条会话的）
      if (config && (detail.model || detail.reasoning)) {
        const next: AssistantConfig = {
          ...config,
          model: detail.model ?? config.model,
          reasoning: detail.reasoning,
        }
        saveConfig(next)
        refreshModelOptions()
      }
    } catch (error) {
      toast(t('打开会话失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
    setSidebarOpen(false)
  }, [awaiting, busy, config, syncUI, saveConfig, refreshModelOptions])

  const removeSession = React.useCallback(async (id: string) => {
    if (busy || awaiting || sendingRef.current || historyBusyRef.current) return
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await removeSavedSession(id)
      if (id === activeIdRef.current) resetToBlank(newSessionId())
    } catch (error) {
      toast(t('删除会话失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
  }, [busy, awaiting, resetToBlank, removeSavedSession])

  const clearHistory = React.useCallback(async () => {
    if (busy || awaiting || sendingRef.current || historyBusyRef.current) return
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await clearSavedSessions()
      resetToBlank(newSessionId())
    } catch (error) {
      toast(t('清空历史失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
  }, [busy, awaiting, resetToBlank, clearSavedSessions])

  const pickModel = React.useCallback((name: string) => {
    if (!name) return
    if (!config) {
      toast(t('助手设置没读出来，暂时存不了对话模型。'), 'err')
      return
    }
    saveConfig({ ...config, model: name })
  }, [config, saveConfig])

  const pickReasoning = React.useCallback((level: string | null) => {
    if (!config) {
      toast(t('助手设置没读出来，暂时存不了思考等级。'), 'err')
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

  const messageActions = useAssistantMessageOperations({
    messages: wireRef, ui: uiRef, activeId: activeIdRef,
    busy: () => busy || awaiting || sendingRef.current || historyBusyRef.current,
    setBusy: value => { historyBusyRef.current = value; setHistoryBusy(value) },
    model: () => configRef.current?.model ?? null, sync: syncUI, persist: () => persistRef.current(),
    open: detail => {
      wireRef.current = detail.messages
      uiRef.current = replayMessages(detail.messages, detail.model)
      activeIdRef.current = detail.id
      titleRef.current = detail.title
      setActiveId(detail.id)
      setSessions(previous => [detail, ...previous])
      setImages([])
      setFiles([])
      setDraft('')
      syncUI()
      setSidebarOpen(false)
    },
  })

  /* ─── 渲染 ─── */

  const activeTitle = titleRef.current || titleFrom(wireRef.current) || t('新对话')
  const empty = ui.length === 0

  return (
    <div className='asst-layout'>
      <AssistantSidebar open={sidebarOpen} onOpenChange={setSidebarOpen} sessions={sessions} activeId={activeId}
        disabled={busy || awaiting || historyBusy} settingsDisabled={!config} onNew={newChat}
        onOpen={id => { void openSession(id) }} onRemove={id => { void removeSession(id) }} onClear={() => { void clearHistory() }}
        onSettings={() => { setTitleSettingsOpen(true); refreshModelOptions() }} />

      <div className='asst-main'>
        <AssistantTopbar title={activeTitle} keyLabel={selectedKey ? modelApiKeyLabel(selectedKey) : t('请先选择一把 API Key')}
          onHistory={() => setSidebarOpen(open => !open)} onSettings={() => setSettingsOpen(true)} />

        <div className='asst-scroll' ref={scrollRef}>
          {empty ? (
            <AssistantEmpty noTools={config !== null && enabledCount === 0} failed={loadingFailed} status={status}
              onSend={text => { void send(text, [], []) }} />
          ) : (
            <div className='asst-flow'>
              <MessageList messages={ui} model={currentModel || ''} actions={{ ...messageActions, disabled: busy || awaiting || historyBusy }} onConfirm={run => void resolveConfirm(run)} />
            </div>
          )}
        </div>

        <AssistantComposer
          draft={draft}
          onDraftChange={setDraft}
          images={images}
          files={files}
          onAddFiles={addFiles}
          onRemoveImage={index => setImages(prev => prev.filter((_, i) => i !== index))}
          onRemoveFile={index => setFiles(prev => prev.filter((_, i) => i !== index))}
          modelSupportsImages={modelSupportsImages}
          imageToolsEnabled={toolEnabled.edit_images === true}
          modelAvailable={modelsReady && Boolean(currentOption) && keyState.ready && Boolean(selectedKey)}
          modelsReady={modelsReady}
          modelsLoading={modelsLoading}
          modelWarning={modelWarning}
          onRefreshModels={refreshModelOptions}
          models={models}
          model={currentModel}
          onPickModel={pickModel}
          reasoning={config?.reasoning ?? null}
          onPickReasoning={pickReasoning}
          mode={mode}
          onModeChange={pickMode}
          busy={busy || readingAttachments}
          awaiting={awaiting || historyBusy}
          onSend={() => void send(draft, images, files)}
          onStop={stop}
        />
      </div>

      <AssistantTitleSettings open={titleSettingsOpen} onOpenChange={setTitleSettingsOpen}
        model={config?.titleModel ?? null} prompt={config?.titlePrompt ?? ''} models={models.map(item => item.name)} t={t} disabled={!config}
        onSave={(titleModel, titlePrompt) => { if (config) saveConfig({ ...config, titleModel, titlePrompt }, t('标题总结设置已保存')) }} />
      <AssistantSettingsDrawer
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        config={config}
        models={models.map(item => item.name)}
        modelAvailable={modelsReady && Boolean(currentOption)}
        modelsReady={modelsReady}
        modelWarning={modelWarning}
        onRefreshModels={refreshModelOptions}
        busy={busy}
        failed={loadingFailed}
        status={status}
        onSave={saveConfig}
        apiKeyField={<ModelApiKeyField {...keyState} value={config?.apiKeyId ?? null} disabled={!config || busy || awaiting} rememberSelection
          onChange={apiKeyId => { if (config) saveConfig({ ...config, apiKeyId }) }}
          onRefresh={() => { void keyState.refresh().catch(() => {}) }} t={t} />}
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
  // 首轮对话少一次往返：只在用户第一次打开助手页时预热网关地址。
  void gatewayBase().catch(() => {})
}

mountOnPage('assistant', PAGE_SELECTOR, mount)


export { DEFAULT_SYSTEM_PROMPT }
