import { useEffect, useRef, useState } from 'react'
import {
  deleteAssistantSession,
  fetchAssistantConfig,
  fetchKeys,
  fetchManageModels,
  getAssistantSession,
  listAssistantSessions,
  saveAssistantConfig,
  saveAssistantSession,
  streamAssistantChat,
  type AssistantSessionSummary,
  type ChatMessage,
  type GatewayKey,
  type ManageModel,
} from '../core/api'
import { copyText, formatRelativeTime, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonScaffold,
  EmptyBlock,
  ListGroup,
  ListItem,
  RecordCard,
  SearchField,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'

const QUICK_PROMPTS = [
  '你好，请用一句话介绍你自己并确认当前连通状态',
  '帮我写一段简洁的 TypeScript 防抖工具代码',
  '总结一下如何在不同大模型之间设计高可用故障转移',
]

export function ChatView() {
  const { server, session, revision, navTabs } = useStore()

  const [models, setModels] = useState<ManageModel[]>([])
  const [keys, setKeys] = useState<GatewayKey[]>([])
  const [selectedModel, setSelectedModel] = useState<string>('')
  const [selectedKeyId, setSelectedKeyId] = useState<string>('')

  const [sessionId, setSessionId] = useState<string>(() => `chat_${Date.now().toString(36)}`)
  const [sessionTitle, setSessionTitle] = useState<string>('新对话')
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [input, setInput] = useState('')
  const [streaming, setStreaming] = useState(false)

  const [pickerSheet, setPickerSheet] = useState<'history' | 'model' | 'key' | null>(null)
  const [historyList, setHistoryList] = useState<AssistantSessionSummary[]>([])
  const [modelSearch, setModelSearch] = useState('')

  const abortRef = useRef<AbortController | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    const id = serverIdentity()
    if (!server?.base) return

    Promise.all([
      fetchManageModels(controller.signal).catch(() => null),
      fetchKeys(controller.signal).catch(() => null),
      fetchAssistantConfig(controller.signal).catch(() => null),
    ]).then(([mView, kView, aCfg]) => {
      if (serverIdentity() !== id) return
      const mList = (mView?.models || []).filter(m => m.enabled !== false)
      const kList = (kView?.keys || []).filter(k => k.enabled !== false)
      setModels(mList)
      setKeys(kList)

      const defaultModel =
        aCfg?.model ||
        session?.defaultModel ||
        session?.lastRequestModel ||
        mList[0]?.id ||
        mList[0]?.name ||
        ''
      if (defaultModel) setSelectedModel(defaultModel)

      const defaultKeyId = aCfg?.apiKeyId || kList[0]?.id || ''
      if (defaultKeyId) setSelectedKeyId(defaultKeyId)
    })

    return () => controller.abort()
  }, [revision, server?.base, session?.defaultModel, session?.lastRequestModel])

  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight
    }
  }, [messages, streaming])

  const activeKeyObj = keys.find(k => k.id === selectedKeyId) || keys[0]

  const startNewChat = () => {
    abortRef.current?.abort()
    setStreaming(false)
    setSessionId(`chat_${Date.now().toString(36)}`)
    setSessionTitle('新对话')
    setMessages([])
  }

  const openHistory = async () => {
    setPickerSheet('history')
    try {
      const list = await listAssistantSessions()
      setHistoryList(list)
    } catch {
      setHistoryList([])
    }
  }

  const loadHistorySession = async (id: string) => {
    try {
      const detail = await getAssistantSession(id)
      setSessionId(detail.id)
      setSessionTitle(detail.title || '对话')
      setMessages(detail.messages || [])
      if (detail.model) setSelectedModel(detail.model)
      setPickerSheet(null)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '加载对话失败', 'danger')
    }
  }

  const handleSend = async (promptText?: string) => {
    const text = (promptText ?? input).trim()
    if (!text || streaming) return

    const userMsg: ChatMessage = {
      id: `u_${Date.now()}`,
      role: 'user',
      content: text,
      createdAt: Date.now(),
    }
    const assistantMsgId = `a_${Date.now() + 1}`
    const assistantPlaceholder: ChatMessage = {
      id: assistantMsgId,
      role: 'assistant',
      content: '',
      reasoning: '',
      createdAt: Date.now(),
    }

    const nextHistory = [...messages, userMsg]
    setMessages([...nextHistory, assistantPlaceholder])
    if (!promptText) setInput('')
    if (messages.length === 0) {
      setSessionTitle(text.slice(0, 22))
    }

    const controller = new AbortController()
    abortRef.current = controller
    setStreaming(true)

    let accContent = ''
    let accReasoning = ''

    try {
      await streamAssistantChat({
        apiKey: activeKeyObj?.key || '',
        messages: nextHistory,
        model: selectedModel || null,
        signal: controller.signal,
        onDelta: delta => {
          if (delta.content) accContent += delta.content
          if (delta.reasoning) accReasoning += delta.reasoning
          setMessages(prev =>
            prev.map(m =>
              m.id === assistantMsgId
                ? { ...m, content: accContent, reasoning: accReasoning }
                : m,
            ),
          )
        },
      })

      const finalMessages: ChatMessage[] = [
        ...nextHistory,
        {
          id: assistantMsgId,
          role: 'assistant',
          content: accContent || '（已完成响应）',
          reasoning: accReasoning || undefined,
          createdAt: Date.now(),
        },
      ]
      setMessages(finalMessages)
      void saveAssistantSession(sessionId, finalMessages, selectedModel, sessionTitle).catch(
        () => {},
      )
    } catch (err) {
      if (controller.signal.aborted) return
      const errMsg = err instanceof Error ? err.message : '请求失败'
      setMessages(prev =>
        prev.map(m =>
          m.id === assistantMsgId
            ? { ...m, content: accContent ? `${accContent}\n\n[错误: ${errMsg}]` : `请求失败：${errMsg}` }
            : m,
        ),
      )
    } finally {
      setStreaming(false)
      abortRef.current = null
    }
  }

  const filteredModels = models.filter(m => {
    const q = modelSearch.trim().toLowerCase()
    if (!q) return true
    const id = (m.id || m.name || '').toLowerCase()
    const prov = providerDisplayName(m.provider, m.providerLabel).toLowerCase()
    return id.includes(q) || prov.includes(q)
  })

  return (
    <CommonScaffold
      title={sessionTitle}
      onBack={navTabs.includes('chat') ? undefined : () => store.returnFromSubPage()}
      noScrollPadding
      actions={
        <>
          <TonalIconButton icon='history' onClick={openHistory} title='历史会话' />
          <TonalIconButton icon='plus' label='新对话' primary onClick={startNewChat} />
        </>
      }
    >
      {/* 顶部紧凑双胶囊：模型选择 + API Key 选择 */}
      <div
        style={{
          flex: 'none',
          padding: '0 16px 10px',
          display: 'grid',
          gridTemplateColumns: '1.35fr 1fr',
          gap: 8,
        }}
      >
        <button
          type='button'
          className='fc-btn is-pill'
          style={{
            height: 36,
            padding: '0 12px',
            background: 'var(--fc-card)',
            justifyContent: 'space-between',
            fontSize: 12.5,
          }}
          onClick={() => setPickerSheet('model')}
        >
          <span
            className='fc-mono'
            style={{
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {selectedModel || t('选择模型')}
          </span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>

        <button
          type='button'
          className='fc-btn is-pill'
          style={{
            height: 36,
            padding: '0 12px',
            background: 'var(--fc-card)',
            justifyContent: 'space-between',
            fontSize: 12.5,
          }}
          onClick={() => setPickerSheet('key')}
        >
          <span
            style={{
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {activeKeyObj?.name || (keys.length === 0 ? '免密/默认 Key' : '选择 Key')}
          </span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>
      </div>

      {/* 消息滚动流 */}
      <div
        ref={listRef}
        style={{
          flex: 1,
          overflowY: 'auto',
          WebkitOverflowScrolling: 'touch',
          padding: '6px 16px 16px',
          display: 'flex',
          flexDirection: 'column',
          gap: 12,
        }}
      >
        {messages.length === 0 ? (
          <div
            style={{
              margin: 'auto 0',
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              gap: 14,
              padding: '20px 8px',
            }}
          >
            <span className='fc-icon-badge' style={{ width: 52, height: 52 }}>
              <GlyphIcon name='spark' size={24} />
            </span>
            <div style={{ textAlign: 'center' }}>
              <div style={{ fontSize: 16, fontWeight: 700 }}>直接与网关模型对话</div>
              <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', marginTop: 4 }}>
                走真实网关转发链路，可随时测试模型响应与质量
              </div>
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, width: '100%' }}>
              {QUICK_PROMPTS.map((q, idx) => (
                <button
                  key={idx}
                  type='button'
                  className='fc-record-card is-clickable'
                  style={{
                    border: 'none',
                    textAlign: 'left',
                    padding: '12px 16px',
                    fontSize: 13,
                    fontWeight: 600,
                    color: 'var(--fc-text)',
                  }}
                  onClick={() => void handleSend(q)}
                >
                  {q}
                </button>
              ))}
            </div>
          </div>
        ) : (
          messages.map(msg => {
            const isUser = msg.role === 'user'
            return (
              <div
                key={msg.id}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: isUser ? 'flex-end' : 'flex-start',
                  gap: 4,
                }}
              >
                <div
                  style={{
                    maxWidth: '86%',
                    padding: '12px 15px',
                    borderRadius: 20,
                    borderBottomRightRadius: isUser ? 6 : 20,
                    borderBottomLeftRadius: isUser ? 20 : 6,
                    background: isUser ? 'var(--fc-primary)' : 'var(--fc-card)',
                    color: isUser ? 'var(--fc-on-primary)' : 'var(--fc-text)',
                    boxShadow: 'var(--fc-shadow-card)',
                    fontSize: 14,
                    lineHeight: 1.55,
                    whiteSpace: 'pre-wrap',
                    wordBreak: 'break-word',
                  }}
                >
                  {msg.reasoning ? (
                    <details
                      style={{
                        marginBottom: 8,
                        padding: '8px 10px',
                        borderRadius: 12,
                        background: 'var(--fc-block)',
                        color: 'var(--fc-text-secondary)',
                        fontSize: 12,
                      }}
                    >
                      <summary style={{ cursor: 'pointer', fontWeight: 600 }}>
                        思考过程
                      </summary>
                      <div style={{ marginTop: 6, whiteSpace: 'pre-wrap' }}>{msg.reasoning}</div>
                    </details>
                  ) : null}
                  {msg.content || (streaming ? '思考中…' : '')}
                </div>

                {!isUser && msg.content ? (
                  <button
                    type='button'
                    style={{
                      border: 'none',
                      background: 'transparent',
                      color: 'var(--fc-text-tertiary)',
                      fontSize: 11.5,
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 4,
                      padding: '2px 6px',
                      cursor: 'pointer',
                    }}
                    onClick={async () => {
                      await copyText(msg.content)
                      store.toast('已复制回复内容', 'success')
                    }}
                  >
                    <GlyphIcon name='copy' size={12} />
                    <span>复制</span>
                  </button>
                ) : null}
              </div>
            )
          })
        )}
      </div>

      {/* 底部 46px 紧凑胶囊输入栏（主导航激活时上移避让底栏，二级页贴底展示） */}
      <div
        className='fc-chat-composer-wrap'
        style={{
          flex: 'none',
          padding: navTabs.includes('chat')
            ? '6px 16px calc(var(--fc-safe-bottom) + 84px)'
            : '6px 16px max(var(--fc-safe-bottom), 14px)',
          background: 'var(--fc-page)',
        }}
      >
        <div
          style={{
            height: 48,
            borderRadius: 9999,
            background: 'var(--fc-card)',
            boxShadow: 'var(--fc-shadow-card)',
            padding: '4px 6px 4px 16px',
            display: 'flex',
            alignItems: 'center',
            gap: 8,
          }}
        >
          <input
            type='text'
            value={input}
            onChange={e => setInput(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                void handleSend()
              }
            }}
            placeholder='发送消息…'
            style={{
              flex: 1,
              minWidth: 0,
              border: 'none',
              outline: 'none',
              background: 'transparent',
              fontSize: 14,
              color: 'var(--fc-text)',
            }}
          />
          {streaming ? (
            <button
              type='button'
              onPointerDown={e => e.preventDefault()}
              onClick={() => abortRef.current?.abort()}
              style={{
                width: 38,
                height: 38,
                borderRadius: 9999,
                border: 'none',
                background: 'var(--fc-danger)',
                color: '#ffffff',
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                cursor: 'pointer',
                flexShrink: 0,
              }}
              aria-label='停止'
            >
              <GlyphIcon name='stop' size={16} />
            </button>
          ) : (
            <button
              type='button'
              disabled={!input.trim()}
              onPointerDown={e => e.preventDefault()}
              onClick={() => void handleSend()}
              style={{
                width: 38,
                height: 38,
                borderRadius: 9999,
                border: 'none',
                background: input.trim() ? 'var(--fc-primary)' : 'var(--fc-inset)',
                color: input.trim() ? 'var(--fc-on-primary)' : 'var(--fc-text-tertiary)',
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                cursor: input.trim() ? 'pointer' : 'not-allowed',
                flexShrink: 0,
                transition: 'background 160ms ease',
              }}
              aria-label='发送'
            >
              <GlyphIcon name='send' size={16} />
            </button>
          )}
        </div>
      </div>

      {/* 历史会话抽屉 */}
      <BottomSheet
        open={pickerSheet === 'history'}
        title='历史对话'
        onClose={() => setPickerSheet(null)}
      >
        {historyList.length === 0 ? (
          <EmptyBlock icon='history' title='暂无保存的历史对话' />
        ) : (
          historyList.map(item => (
            <RecordCard key={item.id} onClick={() => void loadHistorySession(item.id)}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                <div style={{ minWidth: 0 }}>
                  <div
                    style={{
                      fontSize: 14.5,
                      fontWeight: 700,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {item.title || '未命名对话'}
                  </div>
                  <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
                    {item.model || '默认模型'} · {formatRelativeTime(item.updatedAt)}
                  </div>
                </div>
                <button
                  type='button'
                  className='fc-btn is-danger is-sm is-pill'
                  onClick={async e => {
                    e.stopPropagation()
                    await deleteAssistantSession(item.id).catch(() => {})
                    setHistoryList(prev => prev.filter(s => s.id !== item.id))
                  }}
                >
                  <GlyphIcon name='trash' size={14} />
                </button>
              </div>
            </RecordCard>
          ))
        )}
      </BottomSheet>

      {/* 模型选择抽屉 */}
      <BottomSheet
        open={pickerSheet === 'model'}
        title='选择对话模型'
        onClose={() => setPickerSheet(null)}
        fixedHeader={
          <SearchField
            inSheet
            value={modelSearch}
            onChange={setModelSearch}
            placeholder='搜索模型 ID 或提供商…'
          />
        }
      >
        {filteredModels.length === 0 ? (
          <EmptyBlock icon='cube' title='没有找到可用模型' />
        ) : (
          <ListGroup>
            {filteredModels.slice(0, 80).map((m, idx) => {
              const id = m.id || m.name || `m-${idx}`
              const active = id === selectedModel
              return (
                <ListItem
                  key={`${m.provider}-${id}`}
                  leading={<ProviderAvatar provider={m.provider} label={m.providerLabel} size={34} />}
                  title={id}
                  subtitle={providerDisplayName(m.provider, m.providerLabel)}
                  trailing={active ? <TonalChip tone='primary'>当前</TonalChip> : null}
                  onClick={() => {
                    setSelectedModel(id)
                    void saveAssistantConfig({ model: id }).catch(() => {})
                    setPickerSheet(null)
                  }}
                />
              )
            })}
          </ListGroup>
        )}
      </BottomSheet>

      {/* API Key 选择抽屉 */}
      <BottomSheet
        open={pickerSheet === 'key'}
        title='选择调用 Key'
        onClose={() => setPickerSheet(null)}
      >
        {keys.length === 0 ? (
          <EmptyBlock
            icon='key'
            title='当前网关未配置 API Key'
            description='若网关开启了调用鉴权，请先在「转发 → Key」中创建一把 API Key'
          />
        ) : (
          <ListGroup>
            {keys.map(k => {
              const active = k.id === activeKeyObj?.id
              return (
                <ListItem
                  key={k.id}
                  icon='key'
                  title={k.name || '未命名 Key'}
                  subtitle={k.masked || k.key || ''}
                  trailing={active ? <TonalChip tone='primary'>当前</TonalChip> : null}
                  onClick={() => {
                    if (k.id) {
                      setSelectedKeyId(k.id)
                      void saveAssistantConfig({ apiKeyId: k.id }).catch(() => {})
                    }
                    setPickerSheet(null)
                  }}
                />
              )
            })}
          </ListGroup>
        )}
      </BottomSheet>
    </CommonScaffold>
  )
}
