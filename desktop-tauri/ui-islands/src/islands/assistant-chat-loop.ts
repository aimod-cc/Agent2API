/**
 * AI 助手对话状态机（agent loop）：流式接收 + 工具顺序执行 + 确认卡挂起续跑。
 *
 * ── 三项性能要点 ─────────────────────────────────────────────
 *   · delta 增量经 requestAnimationFrame 合帧后再同步 React 状态，结束/错误/工具
 *     切换时立即 flush，兼顾 60fps 流畅度与确定性；
 *   · 发送前校验优先复用 30 秒内的新鲜模型 / Key 缓存，避免每次发送阻塞两次请求；
 *   · 用户与助手消息在创建时记录时间戳，回放与重渲染不再漂移。
 */

import * as React from 'react'
import { t } from '../i18n'
import {
  AssistantRequestError,
  newSessionId,
  parseToolArguments,
  streamChat,
  toast,
  type AssistantConfig,
  type ChatMessage,
  type ToolCall,
} from './assistant-state'
import {
  errorText,
  toolById,
  type ToolResult,
} from './assistant-tools'
import { selectedModel, unavailableModelMessage, type ModelOption } from './assistant-models'
import type { UiMessage } from './assistant-messages'
import type { CollectedTool } from './assistant-session-state'
import { imageContext, type ImageAsset } from '../../../ui-shared/assistant-image-tools'
import { assistantFileContext, type AssistantTextFile } from '../../../ui-shared/assistant-files'
import { modelApiKeyAllows, type ModelApiKey } from '../../../ui-shared/model-api-key'

const MAX_ROUNDS = 12
const PREFLIGHT_FRESH_MS = 30_000

type Suspended = {
  calls: ToolCall[]
  index: number
  collected: CollectedTool[]
  cardIndex: number
}

type Params = {
  configRef: { current: AssistantConfig | null }
  keyState: {
    keysRef: { current: ModelApiKey[] }
    lastUpdatedAtRef: { current: number }
    refresh: () => Promise<ModelApiKey[]>
  }
  modelState: {
    modelsRef: { current: ModelOption[] }
    lastUpdatedAtRef: { current: number }
    refresh: (silent?: boolean) => Promise<ModelOption[]>
  }
  requestKeyRef: { current: ModelApiKey | undefined }
  requestModelRef: { current: string }
  activeIdRef: { current: string }
  uiRef: { current: UiMessage[] }
  wireRef: { current: ChatMessage[] }
  enabledTools: Array<Record<string, unknown>>
  persistRef: { current: (bump?: boolean, collected?: CollectedTool[]) => Promise<void> }
  summarizeRef: { current: (id: string) => Promise<void> }
  historyBusyRef: { current: boolean }
  busy: boolean
  awaiting: boolean
  readingAttachments: boolean
  setUi: React.Dispatch<React.SetStateAction<UiMessage[]>>
  setBusy: React.Dispatch<React.SetStateAction<boolean>>
  setAwaiting: React.Dispatch<React.SetStateAction<boolean>>
  setActiveId: React.Dispatch<React.SetStateAction<string>>
  setImages: React.Dispatch<React.SetStateAction<string[]>>
  setFiles: React.Dispatch<React.SetStateAction<AssistantTextFile[]>>
  onClearDraft: () => void
}

export function useAssistantChatLoop(params: Params) {
  const {
    configRef, keyState, modelState, requestKeyRef, requestModelRef,
    activeIdRef, uiRef, wireRef, enabledTools, persistRef, summarizeRef,
    historyBusyRef, busy, awaiting, readingAttachments,
    setUi, setBusy, setAwaiting, setActiveId, setImages, setFiles, onClearDraft,
  } = params

  const suspendedRef = React.useRef<Suspended | null>(null)
  const controllerRef = React.useRef<AbortController | null>(null)
  const stoppedRef = React.useRef(false)
  const checkingModelRef = React.useRef(false)
  const sendingRef = React.useRef(false)
  const preflightRef = React.useRef(0)
  const roundRef = React.useRef(0)
  const rafRef = React.useRef<number | null>(null)
  const loopRef = React.useRef<(() => Promise<void>) | null>(null)
  const refreshKeysRef = React.useRef(keyState.refresh)
  refreshKeysRef.current = keyState.refresh
  const refreshModelsRef = React.useRef(modelState.refresh)
  refreshModelsRef.current = modelState.refresh
  const onClearDraftRef = React.useRef(onClearDraft)
  onClearDraftRef.current = onClearDraft

  const flushSyncUI = React.useCallback(() => {
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current)
      rafRef.current = null
    }
    setUi(uiRef.current.slice())
  }, [setUi, uiRef])

  const scheduleSyncUI = React.useCallback(() => {
    if (rafRef.current !== null) return
    rafRef.current = window.requestAnimationFrame(() => {
      rafRef.current = null
      setUi(uiRef.current.slice())
    })
  }, [setUi, uiRef])

  React.useEffect(() => () => {
    if (rafRef.current !== null) window.cancelAnimationFrame(rafRef.current)
  }, [])

  const resetLoopState = React.useCallback(() => {
    preflightRef.current += 1
    checkingModelRef.current = false
    sendingRef.current = false
    stoppedRef.current = false
    controllerRef.current?.abort()
    controllerRef.current = null
    suspendedRef.current = null
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current)
      rafRef.current = null
    }
  }, [])

  const isSending = React.useCallback(() => sendingRef.current, [])

  const runTool = React.useCallback(async (
    tool: NonNullable<ReturnType<typeof toolById>>,
    args: Record<string, unknown>,
    collected: Suspended['collected'],
  ): Promise<ToolResult> => {
    const controller = new AbortController()
    controllerRef.current = controller
    const session = activeIdRef.current
    try {
      if (tool.id === 'generate_images' || tool.id === 'edit_images') await persistRef.current(true, collected)
      if (controller.signal.aborted) throw new DOMException('Aborted', 'AbortError')
      const references = imageContext([
        ...wireRef.current,
        ...collected.map(item => ({ role: 'tool' as const, images: item.images as ImageAsset[] | undefined })),
      ])
      const attachmentFiles = assistantFileContext(wireRef.current)
      const key = requestKeyRef.current
      const result = await tool.run(args, {
        session, images: references, files: attachmentFiles, confirmed: true,
        signal: controller.signal, apiKey: key?.key, apiKeyName: key?.name,
      })
      if (activeIdRef.current !== session) throw new DOMException('Aborted', 'AbortError')
      return result
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null
    }
  }, [activeIdRef, persistRef, requestKeyRef, wireRef])

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
        flushSyncUI()
        continue
      }
      if (configRef.current?.tools?.[tool.id] !== true) {
        const message = t('工具「{name}」未获授权，未执行。', { name: tool.label })
        uiRef.current.push({ kind: 'tool', call, state: 'failed', error: message })
        collected.push({ toolCallId: call.id, content: message })
        flushSyncUI()
        continue
      }
      const refreshingBalance = tool.id === 'list_account_balance' && args.refresh === true
      if (tool.kind === 'read' && !refreshingBalance) {
        const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'running' }) - 1
        flushSyncUI()
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
            flushSyncUI()
            continue
          }
          const message = errorText(error)
          uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
          collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
        }
        flushSyncUI()
        continue
      }

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
        flushSyncUI()
        return
      }
      const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'running', plan, auto: true }) - 1
      flushSyncUI()
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
          flushSyncUI()
          continue
        }
        const message = errorText(error)
        uiRef.current[cardIndex] = { kind: 'tool', call, state: 'failed', auto: true, error: message }
        collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      flushSyncUI()
    }
    for (const item of collected) {
      wireRef.current.push({ role: 'tool', toolCallId: item.toolCallId, content: item.content, images: item.images })
    }
    flushSyncUI()
    await persistRef.current()
    if (stoppedRef.current) { setBusy(false); return }
    await loopRef.current?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const loop = React.useCallback(async () => {
    if (stoppedRef.current) { setBusy(false); return }
    const session = activeIdRef.current
    if (roundRef.current >= MAX_ROUNDS) {
      uiRef.current.push({
        kind: 'notice',
        text: t('这一轮来回已达 {n} 次上限，我先停下等你指示（可以换个说法，或拆成几步让我做）。', { n: MAX_ROUNDS }),
        bad: true,
      })
      flushSyncUI()
      setBusy(false)
      return
    }
    roundRef.current += 1
    setBusy(true)
    const controller = new AbortController()
    controllerRef.current = controller
    const messageId = newSessionId()
    const createdAt = Date.now()
    const botIndex = uiRef.current.push({
      kind: 'bot', id: messageId, model: requestModelRef.current, text: '', streaming: true, createdAt,
    }) - 1
    flushSyncUI()
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
              scheduleSyncUI()
            }
          }
          if (event.type === 'toolCalls') calls = event.calls
        },
      })
    } catch (error) {
      if (activeIdRef.current !== session) return
      failed = true
      if (error instanceof AssistantRequestError && error.code === 'model_not_found') {
        void refreshModelsRef.current(true).catch(() => {})
      }
      const message = errorText(error)
      const current = uiRef.current[botIndex]
      const partial = current && current.kind === 'bot' ? current.text : ''
      if (!partial) uiRef.current.splice(botIndex, 1)
      else {
        uiRef.current[botIndex] = { kind: 'bot', id: messageId, model: requestModelRef.current, text: partial, createdAt }
        wireRef.current.push({ role: 'assistant', id: messageId, model: requestModelRef.current, content: partial, createdAt })
      }
      void persistRef.current()
      const aborted = controller.signal.aborted
      uiRef.current.push({
        kind: 'notice',
        text: aborted ? t('已停止这次回答。') : t('请求失败：{message}', { message }),
        bad: !aborted,
      })
      flushSyncUI()
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
    wireRef.current.push({
      role: 'assistant', id: messageId, model: requestModelRef.current, content: text,
      createdAt, toolCalls: calls.length ? calls : undefined,
    })
    flushSyncUI()
    await persistRef.current()
    if (!calls.length) {
      setBusy(false)
      if (!stoppedRef.current) void summarizeRef.current(session)
      return
    }
    await pump(calls, 0, [])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabledTools, pump, flushSyncUI, scheduleSyncUI])
  loopRef.current = loop

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
      flushSyncUI()
    } else {
      uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'running' }
      setBusy(true)
      flushSyncUI()
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
          flushSyncUI()
          await pump(suspended.calls, suspended.index + 1, suspended.collected)
          return
        }
        const message = errorText(error)
        uiRef.current[suspended.cardIndex] = { kind: 'tool', call, state: 'failed', error: message }
        suspended.collected.push({ toolCallId: call.id, content: `工具执行失败：${message}` })
      }
      flushSyncUI()
    }
    await pump(suspended.calls, suspended.index + 1, suspended.collected)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pump, flushSyncUI])

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
    const model = current.model
    const session = activeIdRef.current
    try {
      const now = Date.now()
      const cachedModels = modelState.modelsRef.current
      const cachedKeys = keyState.keysRef.current
      const modelsFresh = cachedModels.length > 0 && now - modelState.lastUpdatedAtRef.current < PREFLIGHT_FRESH_MS
      const keysFresh = cachedKeys.length > 0 && now - keyState.lastUpdatedAtRef.current < PREFLIGHT_FRESH_MS
      const cachedOption = selectedModel(cachedModels, model)
      const cachedKey = cachedKeys.find(item => item.id === current.apiKeyId)

      let latest = cachedModels
      let keys = cachedKeys
      if (!modelsFresh || !keysFresh || !cachedOption || !cachedKey) {
        checkingModelRef.current = true
        setBusy(true)
        const [fetchedModels, fetchedKeys] = await Promise.all([
          modelsFresh && cachedOption ? Promise.resolve(cachedModels) : refreshModelsRef.current(),
          keysFresh && cachedKey ? Promise.resolve(cachedKeys) : refreshKeysRef.current(),
        ])
        if (attempt !== preflightRef.current || activeIdRef.current !== session || configRef.current?.model !== model) return
        latest = fetchedModels
        keys = fetchedKeys
      }

      const key = keys.find(item => item.id === current.apiKeyId)
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
      const createdAt = Date.now()
      uiRef.current.push({
        kind: 'user', id: messageId, text: value, createdAt,
        ...(attached.length ? { images: attached } : {}),
        ...(textFiles.length ? { files: textFiles } : {}),
      })
      wireRef.current.push({
        role: 'user', id: messageId, content: value, createdAt,
        ...(attached.length ? { images: attached } : {}),
        ...(textFiles.length ? { files: textFiles } : {}),
      })
      onClearDraftRef.current()
      setImages([])
      setFiles([])
      setBusy(true)
      flushSyncUI()
      await loop()
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
  }, [
    awaiting, busy, readingAttachments, loop, flushSyncUI,
    configRef, keyState.keysRef, keyState.lastUpdatedAtRef,
    modelState.modelsRef, modelState.lastUpdatedAtRef,
    requestKeyRef, requestModelRef, activeIdRef, uiRef, wireRef,
    historyBusyRef, setActiveId, setBusy, setFiles, setImages,
  ])

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
  }, [setBusy])

  return { send, stop, resolveConfirm, resetLoopState, flushSyncUI, isSending }
}
