import { useEffect, useRef, useState } from 'react'
import { Icon } from '../components/Icon'
import { MarkdownLite } from '../components/MarkdownLite'
import { Button, Textarea, Sheet, SheetContent, SheetHeader, SheetTitle, SheetBody, confirmDialog } from '@ui'
import { store } from '../state/store'
import { AssistantModelPicker } from '../components/AssistantModelPicker'
import { useAssistantModels, unavailableModelMessage } from '../lib/assistant-models'
import { selectedModel } from '../../../ui-shared/assistant-models'
import {
  fetchAssistantConfig,
  saveAssistantConfig,
  streamAssistantChat,
  saveAssistantSession, listAssistantSessions, getAssistantSession, forkAssistantSession, summarizeAssistantSession, saveAssistantTitleConfig, deleteAssistantSession,
  type AssistantConfig, type SessionSummary, type SessionDetail,
  type ChatMessage,
  type ToolCall,
} from '../api/assistant'
import { toolsPayload, toolById } from '../lib/assistantTools'
import { ellipsisMiddle } from '../lib/format'
import { t } from '../i18n'
import { imageContext, type ImageAsset } from '../../../ui-shared/assistant-image-tools'
import { assistantFileContext, formatFileSize, type AssistantTextFile } from '../../../ui-shared/assistant-files'
import { AssistantImageResults } from '../../../ui-shared/assistant-image-results'
import { loadAssistantImage } from '../api/assistant-images'
import { saveMobileAssistantImage } from '../lib/assistant-image-save'
import { readMobileAttachments } from '../lib/assistant-attachments'
import { serverIdentity } from '../lib/servers'
import { AssistantMessageActions } from '../../../ui-shared/assistant-message-actions'
import { AssistantTitleSettings } from '../../../ui-shared/assistant-title-settings'
import { parseAssistantMessages, removeAssistantMessage } from '../../../ui-shared/assistant-history'
import { replayBubbles, type Bubble } from './assistant/history'
import { AssistantAvatar as Avatar, assistantBubbleStyle as bubbleStyle, parseAssistantArgs as parseArgs } from './assistant/display'
import { AssistantHistory } from './assistant/AssistantHistory'
import { apiCall } from '../api/client'
import { useModelApiKeys } from '../../../ui-shared/use-model-api-keys'
import { ModelApiKeyField } from '../../../ui-shared/model-api-key-field'
import { modelApiKeyAllows, modelApiKeyLabel, type ModelApiKey } from '../../../ui-shared/model-api-key'

/**
 * AI 助手（手机端）。
 *
 * ── 请求走网关，不在手机上直连模型 ────────────────────────────
 * 对话发到 `POST /api/assistant/chat`，由**网关**转发到上游：模型路由、账号
 * 选取、429 换号、协议翻译、用量记账全在服务器那一侧（见 `api::assistant`
 * 的模块头）。手机端只做两件事：把消息发出去、把流回来的 delta 画出来。
 * 手机不持有任何上游凭证，也没有直连任何模型服务的代码。
 *
 * ── 为什么不再有「快捷查询」那一套 ────────────────────────────
 * 上一版用四个本地写死的查询冒充对话（直接打管理 API，不经过模型），输入框
 * 是个点了只弹提示的假按钮。那套东西的问题是：用户以为在跟模型说话，实际上
 * 只能问那四件事。现在四个快捷入口保留为**提问模板** —— 点一下把话填进输入框
 * 并发出去，走的是真正的对话链路。
 *
 * ── 模型从哪来 ────────────────────────────────────────────────
 * 默认用桌面端「助手设置」里选的那个（与桌面端共用一份配置）。手机端可以在
 * 顶部临时换一个已启用的模型，只影响本次会话，不写回配置 —— 手机上改全局
 * 设置容易误触，而换模型是个高频的临时动作。
 */

type QuickAsk = {
  label: string
  icon: Parameters<typeof Icon>[0]['name']
}

const QUICK_ASKS: QuickAsk[] = [
  { label: t('各账号余额还剩多少？'), icon: 'wallet' },
  { label: t('哪些模型在多家重复了？'), icon: 'model' },
  { label: t('今天用了多少 Token？'), icon: 'chart' },
  { label: t('现在有哪些账号被限流？'), icon: 'alert' },
]

export function AssistantPage() {
  const [bubbles, setBubbles] = useState<Bubble[]>([])
  const [draft, setDraft] = useState('')
  const [attachments, setAttachments] = useState<string[]>([])
  const [textFiles, setTextFiles] = useState<AssistantTextFile[]>([])
  const [readingAttachments, setReadingAttachments] = useState(false)
  const [busy, setBusy] = useState(false)
  const [historyBusy, setHistoryBusy] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [titleSettingsOpen, setTitleSettingsOpen] = useState(false)
  const [config, setConfig] = useState<AssistantConfig | null>(null)
  const keyState = useModelApiKeys(() => apiCall('/api/model-access/keys'))
  const selectedKey = keyState.keys.find(key => key.id === config?.apiKeyId)
  const [keyPickerOpen, setKeyPickerOpen] = useState(false)
  const [savingKey, setSavingKey] = useState(false)
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [title, setTitle] = useState('')
  const titleRef = useRef('')
  /** 默认模型（来自网关配置）；null = 还没读到 */
  const [defaultModel, setDefaultModel] = useState<string | null | undefined>(undefined)
  /** 本次会话临时选的模型；空串 = 跟随配置 */
  const [model, setModel] = useState('')
  const { models, ready: modelsReady, loading: modelsLoading, error: modelsError, refresh: refreshModels } = useAssistantModels(selectedKey?.key)
  const [pickerOpen, setPickerOpen] = useState(false)
  const checkingModelRef = useRef(false)
  const [checkingModel, setCheckingModel] = useState(false)
  /** 网关里勾选过的工具表（空 = 纯聊天，后端也不会注入系统提示词） */
  const [tools, setTools] = useState<Array<Record<string, unknown>>>([])
  const enabledToolsRef = useRef<Record<string, boolean>>({})
  const modeRef = useRef<'confirm' | 'full'>('confirm')
  const sessionRef = useRef<string>(crypto.randomUUID())
  const mountedRef = useRef(true)
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve())
  const historyRef = useRef<ChatMessage[]>([])
  const fileRef = useRef<HTMLInputElement | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)

  const persist = (session: string, messages: ChatMessage[], selected: string, identity: string): Promise<void> => {
    const snapshot = parseAssistantMessages(messages)
    const snapshotTitle = titleRef.current
    const pending = saveQueueRef.current.catch(() => {}).then(async () => {
      if (serverIdentity() !== identity) return
      const saved = await saveAssistantSession(session, snapshot, selected, snapshotTitle || undefined)
      if (mountedRef.current && serverIdentity() === identity) {
        if (sessionRef.current === session) { titleRef.current = saved.title; setTitle(saved.title) }
        setSessions(previous => [saved, ...previous.filter(item => item.id !== saved.id)])
      }
    })
    saveQueueRef.current = pending.catch(failure => {
      if (mountedRef.current && serverIdentity() === identity) store.toast(t('这条会话没能存进历史：{message}', { message: failure instanceof Error ? failure.message : String(failure) }), 'err')
    })
    return pending
  }

  useEffect(() => {
    mountedRef.current = true
    return () => { mountedRef.current = false; abortRef.current?.abort() }
  }, [])

  // 读默认模型与工具勾选表：工具表决定后端注不注入系统提示词，必须一起读
  useEffect(() => {
    let cancelled = false
    fetchAssistantConfig()
      .then(config => {
        if (cancelled) return
        setDefaultModel(config.model || null)
        setConfig(config)
        const enabled = config.tools || {}
        enabledToolsRef.current = enabled
        modeRef.current = config.mode === 'full' ? 'full' : 'confirm'
        setTools(toolsPayload(enabled))
      })
      .catch(() => {
        if (!cancelled) setDefaultModel(null)
      })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void listAssistantSessions().then(items => { if (!cancelled) setSessions(items) }).catch(() => {})
    return () => { cancelled = true }
  }, [])

  useEffect(() => {
    if (pickerOpen) void refreshModels().catch(() => {})
  }, [pickerOpen, refreshModels])

  // 新内容到达时贴底（用户往上翻看历史时不打扰：只有本来就在底部才跟随）
  useEffect(() => {
    const node = scrollRef.current
    if (!node) return
    const nearBottom = node.scrollHeight - node.scrollTop - node.clientHeight < 120
    if (nearBottom) node.scrollTop = node.scrollHeight
  }, [bubbles])

  const effectiveModel = model || defaultModel || ''
  const currentOption = selectedModel(models, effectiveModel)
  const modelWarning = modelsError
    ? t('读取模型清单失败：{message}', { message: modelsError })
    : !modelsReady ? t('正在读取可用模型…')
    : effectiveModel && !currentOption ? unavailableModelMessage(effectiveModel) : ''

  /** 发一条消息（`text` 为空时取输入框） */
  const send = async (text?: string) => {
    const content = (text ?? draft).trim()
    if ((!content && !attachments.length && !textFiles.length) || busy || readingAttachments || checkingModelRef.current) return
    if (!effectiveModel) {
      setPickerOpen(true)
      return
    }
    const session = sessionRef.current
    const identity = serverIdentity()
    checkingModelRef.current = true
    setCheckingModel(true)
    let requestModel: string
    let requestKey: ModelApiKey
    try {
      const [options, keys] = await Promise.all([refreshModels(), keyState.refresh()])
      if (!mountedRef.current || sessionRef.current !== session || serverIdentity() !== identity) return
      const key = keys.find(key => key.id === config?.apiKeyId)
      if (!key) {
        setKeyPickerOpen(true)
        store.toast(t('请先选择一把 API Key'), 'err')
        return
      }
      if (!modelApiKeyAllows(key, effectiveModel)) {
        store.toast(t('所选 API Key 无权调用此模型或提供商'), 'err')
        return
      }
      requestKey = key
      const option = selectedModel(options, effectiveModel)
      if (!option) {
        store.toast(unavailableModelMessage(effectiveModel), 'err')
        setPickerOpen(true)
        return
      }
      requestModel = option.name
    } catch (failure) {
      if (!(failure instanceof DOMException && failure.name === 'AbortError') && sessionRef.current === session && serverIdentity() === identity) {
        store.toast(t('读取模型清单失败：{message}', { message: failure instanceof Error ? failure.message : String(failure) }), 'err')
      }
      return
    } finally {
      checkingModelRef.current = false
      if (serverIdentity() === identity) setCheckingModel(false)
    }
    const attached = [...attachments]
    const attachedFiles = [...textFiles]
    const userId = crypto.randomUUID()
    const replyId = crypto.randomUUID()
    const history: ChatMessage[] = [...historyRef.current, {
      id: userId, role: 'user', content,
      ...(attached.length ? { images: attached } : {}),
      ...(attachedFiles.length ? { files: attachedFiles } : {}),
    }]
    setBubbles(prev => [
      ...prev,
      { id: userId, messageIds: [userId], role: 'user', text: content, attachments: attached, files: attachedFiles },
      { id: replyId, messageIds: [replyId], model: requestModel, role: 'assistant', text: '', streaming: true, tools: [] },
    ])
    setDraft('')
    setAttachments([])
    setTextFiles([])
    setBusy(true)

    const controller = new AbortController()
    abortRef.current = controller
    const patch = (next: Partial<Bubble>) =>
      setBubbles(prev => prev.map(item => (item.id === replyId ? { ...item, ...next } : item)))

    let messages = history
    let unfinishedText = ''
    let unfinishedId = replyId
    try {
      await persist(session, messages, requestModel, identity).catch(() => {})
      /** 本轮执行过的工具（显示在气泡下方，让用户知道模型查了什么） */
      const usedTools: string[] = []
      const generatedImages: ImageAsset[] = []
      for (let round = 0; round < 6; round++) {
        if (controller.signal.aborted || !mountedRef.current || serverIdentity() !== identity) break
        const roundId = round === 0 ? replyId : crypto.randomUUID()
        unfinishedId = roundId
        setBubbles(previous => previous.map(item => item.id === replyId ? { ...item, messageIds: [...new Set([...(item.messageIds || []), roundId])] } : item))
        let pendingCalls: ToolCall[] = []
        let roundText = ''
        unfinishedText = ''

        await streamAssistantChat({
          apiKey: requestKey.key,
          messages,
          tools,
          model: requestModel,
          signal: controller.signal,
          onEvent: event => {
            if (!mountedRef.current || sessionRef.current !== session || serverIdentity() !== identity) return
            if (event.type === 'delta') {
              roundText += event.text
              unfinishedText = roundText
              setBubbles(prev =>
                prev.map(item =>
                  item.id === replyId ? { ...item, text: item.text + event.text } : item,
                ),
              )
            } else if (event.type === 'toolCalls') {
              pendingCalls = event.calls
            }
          },
        })
        if (!mountedRef.current || sessionRef.current !== session || serverIdentity() !== identity) return
        unfinishedText = ''

        if (!pendingCalls.length) {
          messages = [...messages, { id: roundId, role: 'assistant', model: requestModel, content: roundText }]
          break
        }

        // 把这一轮的 assistant 消息（含工具调用）与工具结果一起回灌
        messages = [
          ...messages,
          { id: roundId, role: 'assistant', model: requestModel, content: roundText, toolCalls: pendingCalls },
        ]
        for (const call of pendingCalls) {
          if (!mountedRef.current || sessionRef.current !== session || serverIdentity() !== identity) break
          if (controller.signal.aborted) {
            messages = [...messages, { role: 'tool', toolCallId: call.id, content: '用户已停止，未执行这次操作。' }]
            continue
          }
          const tool = toolById(call.name)
          let result: string
          let resultImages: ImageAsset[] | undefined
          try {
            if (!tool) throw new Error(t('未知的工具：{name}', { name: call.name }))
            if (enabledToolsRef.current[tool.id] !== true) throw new Error(t('工具「{name}」未获授权，未执行。', { name: tool.label }))
            const args = parseArgs(call.arguments)
            const refreshingBalance = tool.id === 'list_account_balance' && args.refresh === true
            if ((tool.kind !== 'read' || refreshingBalance) && modeRef.current !== 'full') {
              const plan = tool.plan?.(args)
              const description = [plan?.lead || t('将执行 {label}。', { label: tool.label }),
                plan?.table ? plan.table.rows.map(row => row.join('：')).join('\n') : JSON.stringify(args, null, 2),
                plan?.note, refreshingBalance ? t('实时余额查询会请求上游，并可能触发已配置的自动禁用规则。')
                  : tool.kind === 'run' ? t('这会真实发起上游请求并消耗额度') : t('确认后立即生效'),
              ].filter(Boolean).join('\n')
              if (!await confirmDialog(description, { title: tool.label, confirmText: t('执行') })) {
                messages = [...messages, { role: 'tool', toolCallId: call.id, content: '用户取消了这次操作，未执行。' }]
                continue
              }
            }
            if (controller.signal.aborted || !mountedRef.current || serverIdentity() !== identity) throw new DOMException('Aborted', 'AbortError')
            if (tool.id === 'generate_images' || tool.id === 'edit_images') await persist(session, messages, requestModel, identity)
            const outcome = await tool.run(args, { session, images: imageContext(messages), files: assistantFileContext(messages), confirmed: true, signal: controller.signal, apiKey: requestKey.key, apiKeyName: requestKey.name })
            if (serverIdentity() !== identity || sessionRef.current !== session) break
            result = outcome.text
            resultImages = outcome.images
            generatedImages.push(...(outcome.images || []))
            usedTools.push(tool.label)
            patch({ tools: [...usedTools], images: [...generatedImages] })
          } catch (failure) {
            // 工具失败不中断对话：把错误如实回给模型，让它自己决定怎么解释
            result = JSON.stringify({
              error: failure instanceof Error ? failure.message : String(failure),
            })
          }
          messages = [
            ...messages,
            { role: 'tool', toolCallId: call.id, content: result, images: resultImages },
          ]
          if (sessionRef.current === session && serverIdentity() === identity) historyRef.current = messages
          if (resultImages?.length) await persist(session, messages, requestModel, identity).catch(() => {})
        }
        await persist(session, messages, requestModel, identity).catch(() => {})
        if (controller.signal.aborted || !mountedRef.current) break
        if (round === 5) patch({ error: t('工具调用已达本轮上限，已停止继续执行。') })
      }

      if (sessionRef.current !== session || serverIdentity() !== identity) return
      historyRef.current = messages
      await persist(session, messages, requestModel, identity).catch(() => {})
      if (!controller.signal.aborted && !sessions.find(item => item.id === session)?.titleGenerated) {
        void summarizeAssistantSession(session, requestKey.key).then(saved => {
          if (!mountedRef.current || serverIdentity() !== identity) return
          if (sessionRef.current === session) { titleRef.current = saved.title; setTitle(saved.title) }
          setSessions(previous => previous.map(item => item.id === saved.id ? saved : item))
        }).catch(failure => { if (mountedRef.current && serverIdentity() === identity) store.toast(t('标题总结失败：{message}', { message: failure instanceof Error ? failure.message : String(failure) }), 'warn') })
      }
      // 流正常结束但一个字都没有：多半是上游只回了 tool_calls 或空响应
      setBubbles(prev =>
        prev.map(item => {
          if (item.id !== replyId) return item
          if (item.text.trim() || item.images?.length) return { ...item, streaming: false }
          return { ...item, streaming: false, error: t('模型没有返回内容') }
        }),
      )
    } catch (failure) {
      if (sessionRef.current !== session || serverIdentity() !== identity) return
      if (unfinishedText) messages = [...messages, { id: unfinishedId, role: 'assistant', model: requestModel, content: unfinishedText }]
      historyRef.current = messages
      void persist(session, messages, requestModel, identity).catch(() => {})
      const aborted = controller.signal.aborted
      patch({
        streaming: false,
        error: aborted
          ? undefined
          : failure instanceof Error
            ? failure.message
            : String(failure),
      })
      if (aborted) store.toast(t('已停止生成'))
    } finally {
      if (abortRef.current === controller) abortRef.current = null
      if (mountedRef.current) {
        setBusy(false)
        inputRef.current?.focus()
      }
    }
  }

  const addAttachments = async (files: FileList | File[]) => {
    if (readingAttachments || busy || checkingModelRef.current) return
    const session = sessionRef.current
    setReadingAttachments(true)
    try {
      const result = await readMobileAttachments(files, { images: attachments.length, files: textFiles.length })
      if (sessionRef.current === session) {
        setAttachments(current => [...current, ...result.images])
        setTextFiles(current => [...current, ...result.files])
      }
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally { setReadingAttachments(false) }
  }

  const stop = () => {
    abortRef.current?.abort()
  }

  const clear = () => {
    if (busy || checkingModelRef.current) return
    setBubbles([])
    setDraft('')
    setAttachments([])
    setTextFiles([])
    historyRef.current = []
    titleRef.current = ''
    setTitle('')
    sessionRef.current = crypto.randomUUID()
    setHistoryOpen(false)
  }

  const showSession = (detail: SessionDetail) => {
    historyRef.current = detail.messages
    sessionRef.current = detail.id
    titleRef.current = detail.title
    setTitle(detail.title)
    setModel(detail.model || '')
    setBubbles(replayBubbles(detail.messages, detail.model))
    setAttachments([])
    setTextFiles([])
    setDraft('')
    setHistoryOpen(false)
  }

  const openSession = async (id: string) => {
    if (busy || historyBusy || checkingModelRef.current) return
    setHistoryBusy(true)
    try { await saveQueueRef.current; showSession(await getAssistantSession(id)) }
    catch (failure) { store.toast(failure instanceof Error ? failure.message : String(failure), 'err') }
    finally { setHistoryBusy(false) }
  }

  const operateMessage = async (bubble: Bubble, action: 'edit' | 'delete' | 'fork', text = '') => {
    if (busy || historyBusy || checkingModelRef.current) return
    setHistoryBusy(true)
    const identity = serverIdentity()
    try {
      await saveQueueRef.current
      let messages = parseAssistantMessages(historyRef.current)
      if (action === 'fork') {
        await persist(sessionRef.current, messages, effectiveModel, identity)
        const detail = await forkAssistantSession(sessionRef.current, bubble.messageIds?.at(-1) || bubble.id)
        setSessions(previous => [detail, ...previous])
        showSession(detail)
        return
      }
      const ids = new Set(bubble.messageIds || [bubble.id])
      if (action === 'delete') { for (const id of ids) messages = removeAssistantMessage(messages, id) }
      else {
        let first = true
        messages = messages.map(message => {
          if (!ids.has(message.id!)) return message
          const content = first ? text : ''
          first = false
          return { ...message, content }
        })
      }
      historyRef.current = messages
      setBubbles(replayBubbles(messages, effectiveModel))
      await persist(sessionRef.current, messages, effectiveModel, identity)
    } catch (failure) { store.toast(failure instanceof Error ? failure.message : String(failure), 'err') }
    finally { setHistoryBusy(false) }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      {/*
        页头必须自己吃下安全区：其他一级页用 `.m-head`，那里带了
        `calc(var(--m-safe-t) + …)`；这一页是手写的紧凑头（对话页要省纵向空间），
        漏了 safe-t 就会被刘海 / 状态栏压住。离线时顶部那条通知自己占安全区，
        这里再让一次会多出一段空白 —— 与 `.m-app[data-offline] .m-head` 同口径。
      */}
      <div
        className='m-assistant-head'
        style={{ display: 'flex', alignItems: 'center', gap: 9, flex: 'none' }}
      >
        <Button variant='ghost' size='icon-sm' aria-label={t('历史对话')} onClick={() => setHistoryOpen(true)}><Icon name='log' /></Button>
        <div style={{ flex: 1, minWidth: 0 }}>
          <b style={{ fontSize: 15 }}>{title || t('新对话')}</b>
          <Button size='default' variant='ghost'
            type='button'
            disabled={busy || checkingModel}
            onClick={() => setPickerOpen(true)}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 4,
              marginTop: 1,
              padding: 0,
              border: 0,
              background: 'none',
              color: 'var(--m-text-3)',
              fontSize: 11,
              maxWidth: '100%',
            }}
          >
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {effectiveModel ? ellipsisMiddle(effectiveModel, 32) : t('选择对话模型')}
              {effectiveModel && modelsReady && !currentOption ? ` · ${t('不可用')}` : ''}
            </span>
            <Icon name='chev' style={{ width: 11, height: 11, flex: 'none', transform: 'rotate(90deg)' }} />
          </Button>
          <Button variant='ghost' size='xs' className='max-w-full truncate p-0 text-xs text-muted-foreground' disabled={busy || checkingModel || savingKey}
            onClick={() => { setKeyPickerOpen(true); void keyState.refresh().catch(() => {}) }}>
            {selectedKey ? modelApiKeyLabel(selectedKey) : t('选择 API Key…')}
          </Button>
        </div>
        <Button size='icon-touch' variant='outline'
          type='button'
          className='m-icon-btn'
          aria-label={t('清空')}
          onClick={clear}
          disabled={!bubbles.length || busy || checkingModel}
        >
          <Icon name='refresh' />
        </Button>
      </div>

      <div
        ref={scrollRef}
        style={{
          flex: 1,
          minHeight: 0,
          overflowY: 'auto',
          padding: '4px 14px 8px',
          display: 'flex',
          flexDirection: 'column',
          gap: 12,
          scrollbarWidth: 'none',
        }}
      >
        {bubbles.length === 0 ? (
          <div style={{ display: 'flex', gap: 8 }}>
            <Avatar />
            <div style={bubbleStyle(false)}>
              {t('有什么可以帮你？可以问配置、查余额、让我改模型设置。')}
            </div>
          </div>
        ) : null}

        {bubbles.map(bubble => {
          if (bubble.role === 'user') {
            return (
              <div key={bubble.id} style={{ display: 'flex', justifyContent: 'flex-end' }}>
                <div className='flex max-w-[82%] min-w-0 flex-col items-end'>
                <div style={{ ...bubbleStyle(true), maxWidth: '100%', whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                  {bubble.attachments?.map((url, index) => <img key={index} src={url} alt={t('图片 {n}', { n: index + 1 })} style={{ maxWidth: '100%', maxHeight: 160, borderRadius: 8, marginBottom: 6 }} />)}
                  {bubble.files?.length ? <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 6 }}>
                    {bubble.files.map((file, index) => <span key={`${file.name}-${index}`} style={{ display: 'inline-flex', alignItems: 'center', gap: 5, maxWidth: '100%', padding: '3px 7px', borderRadius: 6, border: '1px solid var(--m-line-strong)', background: 'var(--m-surface-2)', fontSize: 11.5, color: 'var(--m-text-2)' }}>
                      <span style={{ maxWidth: 170, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: 'var(--m-text)' }}>{file.name}</span>
                      <span style={{ color: 'var(--m-text-3)', flex: 'none' }}>{formatFileSize(file.size)}</span>
                    </span>)}
                  </div> : null}
                  {bubble.text}
                </div>
                <AssistantMessageActions text={bubble.text} t={t} disabled={busy || checkingModel || historyBusy}
                  onEdit={text => operateMessage(bubble, 'edit', text)} onDelete={() => operateMessage(bubble, 'delete')} />
                </div>
              </div>
            )
          }
          return (
            <div key={bubble.id} style={{ display: 'flex', gap: 8 }}>
              <Avatar />
              <div style={{ maxWidth: '82%', minWidth: 0 }}>
                <div className='mb-1 break-all text-xs font-medium text-muted-foreground'>{bubble.model || effectiveModel}</div>
                <div style={{ ...bubbleStyle(false), maxWidth: '100%', wordBreak: 'break-word' }}>
                  {bubble.text ? <MarkdownLite text={bubble.text} /> : bubble.streaming ? t('正在思考…') : null}
                  {bubble.streaming ? (
                    <span
                      aria-hidden='true'
                      style={{
                        display: 'inline-block',
                        width: 6,
                        height: 13,
                        marginLeft: 2,
                        verticalAlign: '-2px',
                        background: 'var(--m-primary)',
                        animation: 'm-caret 1s steps(2) infinite',
                      }}
                    />
                  ) : null}
                </div>
                {!bubble.streaming ? <AssistantMessageActions text={bubble.text} t={t} disabled={busy || checkingModel || historyBusy}
                  onEdit={text => operateMessage(bubble, 'edit', text)} onDelete={() => operateMessage(bubble, 'delete')} onFork={() => operateMessage(bubble, 'fork')} /> : null}
                {bubble.images?.length ? <AssistantImageResults images={bubble.images} load={loadAssistantImage} save={saveMobileAssistantImage} t={t} /> : null}
                {bubble.error ? (
                  <div style={{ marginTop: 6, fontSize: 11.5, color: 'var(--m-danger)', lineHeight: 1.5 }}>
                    {bubble.error}
                  </div>
                ) : null}
                {bubble.tools?.length ? (
                  <div
                    style={{
                      marginTop: 6,
                      display: 'flex',
                      flexWrap: 'wrap',
                      gap: 5,
                      fontSize: 10.5,
                      color: 'var(--m-text-3)',
                    }}
                  >
                    <span>{t('已调用')}</span>
                    {bubble.tools.map((label, index) => (
                      <span
                        key={`${label}-${index}`}
                        style={{
                          padding: '1px 6px',
                          borderRadius: 6,
                          background: 'var(--m-surface-2)',
                          border: '1px solid var(--m-line)',
                        }}
                      >
                        {label}
                      </span>
                    ))}
                  </div>
                ) : null}
              </div>
            </div>
          )
        })}

        {bubbles.length === 0 ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 7, paddingTop: 2 }}>
            {QUICK_ASKS.map(item => (
              <Button size='default' variant='ghost'
                key={item.label}
                type='button'
                onClick={() => void send(item.label)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  padding: '10px 12px',
                  borderRadius: 11,
                  border: '1px solid var(--m-line)',
                  background: 'var(--m-surface)',
                  fontSize: 12.5,
                  color: 'var(--m-text-2)',
                  textAlign: 'left',
                }}
              >
                <Icon name={item.icon} style={{ width: 14, height: 14, color: 'var(--m-text-3)', flex: 'none' }} />
                {item.label}
              </Button>
            ))}
          </div>
        ) : null}
      </div>

      <div
        style={{
          flex: 'none',
          // 不再加 safe-b：底部标签栏（.m-tabbar）是它的兄弟节点，已经用
          // padding-bottom 吃掉了底部安全区。这里再加一次会多出一条空白。
          padding: '8px 12px 12px',
          borderTop: '1px solid var(--m-line)',
          background: 'color-mix(in srgb, var(--m-bg) 88%, transparent)',
          backdropFilter: 'blur(12px)',
        }}
      >
        {attachments.length ? <div style={{ display: 'flex', gap: 6, paddingBottom: 8 }}>
          {attachments.map((url, index) => <Button key={index} variant='ghost' disabled={checkingModel} onClick={() => setAttachments(current => current.filter((_, i) => i !== index))}
            aria-label={t('移除图片 {n}', { n: index + 1 })} style={{ padding: 0, height: 56 }}>
            <img src={url} alt={t('图片 {n}', { n: index + 1 })} style={{ height: 50, maxWidth: 70, objectFit: 'cover', borderRadius: 6 }} />
          </Button>)}
        </div> : null}
        {textFiles.length ? <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, paddingBottom: 8 }}>
          {textFiles.map((file, index) => <Button key={`${file.name}-${index}`} variant='ghost' disabled={checkingModel}
            onClick={() => setTextFiles(current => current.filter((_, i) => i !== index))}
            aria-label={t('移除文件「{name}」', { name: file.name })}
            style={{ display: 'flex', alignItems: 'center', gap: 5, height: 30, padding: '0 8px', borderRadius: 8, border: '1px solid var(--m-line-strong)', background: 'var(--m-surface-2)', fontSize: 11.5, color: 'var(--m-text-2)' }}>
            <span style={{ maxWidth: 160, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: 'var(--m-text)' }}>{file.name}</span>
            <span style={{ color: 'var(--m-text-3)', flex: 'none' }}>{formatFileSize(file.size)}</span>
            <span style={{ color: 'var(--m-text-3)', flex: 'none' }}>✕</span>
          </Button>)}
        </div> : null}
        {textFiles.length ? <div style={{ paddingBottom: 8, fontSize: 10.5, color: 'var(--m-warn)', lineHeight: 1.5 }}>{t('JSON 文件会以文本发送给当前对话模型，其中的凭证也会保存在这条会话里。')}</div> : null}
        <input ref={fileRef} type='file' hidden multiple accept='image/png,image/jpeg,image/webp,.json,.info,application/json' onChange={event => {
          const files = event.currentTarget.files
          if (files) void addAttachments(files)
          event.currentTarget.value = ''
        }} />
        <div
          style={{
            display: 'flex',
            alignItems: 'flex-end',
            gap: 8,
            padding: '7px 7px 7px 13px',
            borderRadius: 20,
            background: 'var(--m-surface)',
            border: '1px solid var(--m-line-strong)',
          }}
        >
          <Button variant='ghost' size='icon-touch' aria-label={t('添加图片或文件')} disabled={busy || readingAttachments || checkingModel} onClick={() => fileRef.current?.click()}><Icon name='plus' /></Button>
          <Textarea
            ref={inputRef}
            disabled={checkingModel}
            value={draft}
            onChange={event => setDraft(event.target.value)}
            onPaste={event => {
              const files = Array.from(event.clipboardData.files)
              if (files.length) { event.preventDefault(); void addAttachments(files) }
            }}
            onKeyDown={event => {
              // 回车发送、Shift+回车换行（与桌面端同一手感）；输入法组合中的回车
              // 不算（中文输入时那是在选词，发出去就糟了）
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault()
                void send()
              }
            }}
            placeholder={t('问点什么…')}
            rows={1}
            style={{
              flex: 1,
              minWidth: 0,
              maxHeight: 120,
              padding: '6px 0',
              border: 0,
              outline: 'none',
              resize: 'none',
              background: 'transparent',
              color: 'var(--m-text)',
              fontSize: 16,
              lineHeight: 1.5,
              fontFamily: 'inherit',
            }}
          />
          <Button size='default' variant='ghost'
            type='button'
            aria-label={busy ? t('停止') : t('发送')}
            disabled={readingAttachments || checkingModel || (!busy && (!modelsReady || !currentOption || (!draft.trim() && !attachments.length && !textFiles.length)))}
            onClick={() => (busy ? stop() : void send())}
            style={{
              width: 34,
              height: 34,
              borderRadius: '50%',
              border: 0,
              background: 'var(--m-primary)',
              color: '#fff',
              display: 'grid',
              placeItems: 'center',
              flex: 'none',
              opacity: busy || draft.trim() || attachments.length || textFiles.length ? 1 : 0.45,
            }}
          >
            {busy ? (
              <span style={{ width: 10, height: 10, borderRadius: 2, background: '#fff' }} />
            ) : (
              <Icon name='up' style={{ width: 17, height: 17 }} />
            )}
          </Button>
        </div>
        {modelWarning ? <p role='status' className='mb-2 text-[11.5px] leading-[1.6] text-subtle'>{modelWarning}</p> : null}
        <div style={{ textAlign: 'center', fontSize: 10.5, color: 'var(--m-text-3)', marginTop: 7 }}>
          {busy ? t('正在生成，点右侧按钮可停止') : t('对话由网关转发到上游模型，用量计入请求日志')}
        </div>
      </div>

      <AssistantHistory open={historyOpen} onClose={() => setHistoryOpen(false)} sessions={sessions} activeId={sessionRef.current} busy={busy || historyBusy || checkingModel}
        onOpen={id => void openSession(id)} onNew={clear} onSettings={() => setTitleSettingsOpen(true)}
        onDelete={id => { void confirmDialog(t('删除这条对话？'), { title: t('删除这条对话') }).then(async confirmed => {
          if (!confirmed || busy || historyBusy) return
          setHistoryBusy(true)
          try { await saveQueueRef.current; await deleteAssistantSession(id); setSessions(previous => previous.filter(item => item.id !== id)); if (sessionRef.current === id) clear() }
          catch (failure) { store.toast(failure instanceof Error ? failure.message : String(failure), 'err') } finally { setHistoryBusy(false) }
        }) }} />
      <AssistantTitleSettings open={titleSettingsOpen} onOpenChange={setTitleSettingsOpen} model={config?.titleModel ?? null} prompt={config?.titlePrompt || ''}
        models={models.map(item => item.name)} disabled={!config} t={t} onSave={async (titleModel, titlePrompt) => {
          if (!config) return
          setConfig(await saveAssistantTitleConfig(config, titleModel, titlePrompt))
        }} />
      <Sheet open={keyPickerOpen} onOpenChange={setKeyPickerOpen}>
        <SheetContent side='bottom'>
          <SheetHeader><SheetTitle>{t('选择 API Key')}</SheetTitle></SheetHeader>
          <SheetBody>
            <ModelApiKeyField {...keyState} value={config?.apiKeyId ?? null} disabled={!config || busy || savingKey} rememberSelection
              onRefresh={() => { void keyState.refresh().catch(() => {}) }} t={t}
              onChange={async apiKeyId => {
                if (!config || savingKey) return
                setSavingKey(true)
                try {
                  setConfig(await saveAssistantConfig({ ...config, apiKeyId }))
                  setKeyPickerOpen(false)
                } catch (failure) { store.toast(failure instanceof Error ? failure.message : String(failure), 'err') }
                finally { setSavingKey(false) }
              }} />
          </SheetBody>
        </SheetContent>
      </Sheet>
      <AssistantModelPicker
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        models={models}
        current={effectiveModel}
        defaultModel={defaultModel || ''}
        followingDefault={!model}
        ready={modelsReady}
        loading={modelsLoading}
        warning={modelWarning}
        onRefresh={() => { void refreshModels().catch(() => {}) }}
        onPick={value => {
          setModel(value)
          setPickerOpen(false)
        }}
      />
    </div>
  )
}
