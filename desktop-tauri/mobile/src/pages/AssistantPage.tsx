import { useEffect, useRef, useState } from 'react'
import { Icon } from '../components/Icon'
import { MarkdownLite } from '../components/MarkdownLite'
import { Sheet, SheetContent, SheetHeader, SheetBody, SheetTitle } from '@ui'
import { store } from '../state/store'
import { fetchManage, type ManageModel } from '../api/gateway'
import {
  fetchAssistantConfig,
  streamAssistantChat,
  type ChatMessage,
  type ToolCall,
} from '../api/assistant'
import { toolsPayload, toolById } from '../lib/assistantTools'
import { ellipsisMiddle } from '../lib/format'
import { t } from '../i18n'

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

type Bubble = {
  id: number
  role: 'user' | 'assistant'
  text: string
  /** 正在流式输出：气泡上显示光标 / 停止按钮 */
  streaming?: boolean
  /** 失败原因（assistant 气泡下方的红字） */
  error?: string
  /** 这一轮里执行过的工具（显示成一行小字，让用户知道模型查了什么） */
  tools?: string[]
}

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

let nextId = 1

export function AssistantPage() {
  const [bubbles, setBubbles] = useState<Bubble[]>([])
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  /** 默认模型（来自网关配置）；null = 还没读到 */
  const [defaultModel, setDefaultModel] = useState<string | null | undefined>(undefined)
  /** 本次会话临时选的模型；空串 = 跟随配置 */
  const [model, setModel] = useState('')
  const [models, setModels] = useState<ManageModel[]>([])
  const [pickerOpen, setPickerOpen] = useState(false)
  /** 网关里勾选过的工具表（空 = 纯聊天，后端也不会注入系统提示词） */
  const [tools, setTools] = useState<Array<Record<string, unknown>>>([])
  const abortRef = useRef<AbortController | null>(null)
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)

  // 读默认模型与工具勾选表：工具表决定后端注不注入系统提示词，必须一起读
  useEffect(() => {
    let cancelled = false
    fetchAssistantConfig()
      .then(config => {
        if (cancelled) return
        setDefaultModel(config.model || null)
        const enabled = config.tools || {}
        setTools(toolsPayload(enabled))
      })
      .catch(() => {
        if (!cancelled) setDefaultModel(null)
      })
    return () => {
      cancelled = true
    }
  }, [])

  // 模型清单只在打开选择器时拉：不进页面就不该打这个请求
  useEffect(() => {
    if (!pickerOpen || models.length) return
    let cancelled = false
    fetchManage()
      .then(view => {
        if (cancelled) return
        setModels((view.models || []).filter(item => item.enabled !== false && item.id))
      })
      .catch(() => {
        if (!cancelled) store.toast(t('读取模型清单失败'), 'err')
      })
    return () => {
      cancelled = true
    }
  }, [pickerOpen, models.length])

  // 新内容到达时贴底（用户往上翻看历史时不打扰：只有本来就在底部才跟随）
  useEffect(() => {
    const node = scrollRef.current
    if (!node) return
    const nearBottom = node.scrollHeight - node.scrollTop - node.clientHeight < 120
    if (nearBottom) node.scrollTop = node.scrollHeight
  }, [bubbles])

  const effectiveModel = model || defaultModel || ''

  /** 发一条消息（`text` 为空时取输入框） */
  const send = async (text?: string) => {
    const content = (text ?? draft).trim()
    if (!content || busy) return

    // 历史 = 已完成的对话（失败 / 空回复的气泡不进上下文）
    const history: ChatMessage[] = bubbles
      .filter(item => item.text.trim() && !item.error)
      .map(item => ({ role: item.role, content: item.text }))
    history.push({ role: 'user', content })

    const userId = nextId++
    const replyId = nextId++
    setBubbles(prev => [
      ...prev,
      { id: userId, role: 'user', text: content },
      { id: replyId, role: 'assistant', text: '', streaming: true, tools: [] },
    ])
    setDraft('')
    setBusy(true)

    const controller = new AbortController()
    abortRef.current = controller
    const patch = (next: Partial<Bubble>) =>
      setBubbles(prev => prev.map(item => (item.id === replyId ? { ...item, ...next } : item)))

    try {
      /**
       * 工具循环：模型可能连续调几轮工具，每轮都要把结果回灌再发一次。
       *
       * 上限 6 轮 —— 与桌面端同一道闸：正常任务两三轮就收敛，跑到第六轮还在
       * 调工具，多半是模型在绕圈（或者某只工具一直失败），继续烧额度没有意义。
       */
      let messages = history
      /** 本轮执行过的工具（显示在气泡下方，让用户知道模型查了什么） */
      const usedTools: string[] = []
      for (let round = 0; round < 6; round++) {
        let pendingCalls: ToolCall[] = []
        let roundText = ''

        await streamAssistantChat({
          messages,
          tools,
          model: effectiveModel || null,
          signal: controller.signal,
          onEvent: event => {
            if (event.type === 'delta') {
              roundText += event.text
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

        if (!pendingCalls.length) break

        // 把这一轮的 assistant 消息（含工具调用）与工具结果一起回灌
        messages = [
          ...messages,
          { role: 'assistant', content: roundText, toolCalls: pendingCalls },
        ]
        for (const call of pendingCalls) {
          const tool = toolById(call.name)
          let result: string
          try {
            if (!tool) throw new Error(t('未知的工具：{name}', { name: call.name }))
            const outcome = await tool.run(parseArgs(call.arguments))
            result = outcome.text
            usedTools.push(tool.label)
            patch({ tools: [...usedTools] })
          } catch (failure) {
            // 工具失败不中断对话：把错误如实回给模型，让它自己决定怎么解释
            result = JSON.stringify({
              error: failure instanceof Error ? failure.message : String(failure),
            })
          }
          messages = [
            ...messages,
            { role: 'tool', toolCallId: call.id, content: result },
          ]
        }
        if (controller.signal.aborted) break
      }

      // 流正常结束但一个字都没有：多半是上游只回了 tool_calls 或空响应
      setBubbles(prev =>
        prev.map(item => {
          if (item.id !== replyId) return item
          if (item.text.trim()) return { ...item, streaming: false }
          return { ...item, streaming: false, error: t('模型没有返回内容') }
        }),
      )
    } catch (failure) {
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
      abortRef.current = null
      setBusy(false)
      inputRef.current?.focus()
    }
  }

  const stop = () => {
    abortRef.current?.abort()
  }

  const clear = () => {
    if (busy) abortRef.current?.abort()
    setBubbles([])
    setDraft('')
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
        <div style={{ flex: 1, minWidth: 0 }}>
          <b style={{ fontSize: 15 }}>{t('AI 助手')}</b>
          <button
            type='button'
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
            </span>
            <Icon name='chev' style={{ width: 11, height: 11, flex: 'none', transform: 'rotate(90deg)' }} />
          </button>
        </div>
        <button
          type='button'
          className='m-icon-btn'
          aria-label={t('清空')}
          onClick={clear}
          disabled={!bubbles.length}
        >
          <Icon name='refresh' />
        </button>
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
                <div style={{ ...bubbleStyle(true), whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                  {bubble.text}
                </div>
              </div>
            )
          }
          return (
            <div key={bubble.id} style={{ display: 'flex', gap: 8 }}>
              <Avatar />
              <div style={{ maxWidth: '82%', minWidth: 0 }}>
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
              <button
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
              </button>
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
          <textarea
            ref={inputRef}
            value={draft}
            onChange={event => setDraft(event.target.value)}
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
          <button
            type='button'
            aria-label={busy ? t('停止') : t('发送')}
            disabled={!busy && !draft.trim()}
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
              opacity: busy || draft.trim() ? 1 : 0.45,
            }}
          >
            {busy ? (
              <span style={{ width: 10, height: 10, borderRadius: 2, background: '#fff' }} />
            ) : (
              <Icon name='up' style={{ width: 17, height: 17 }} />
            )}
          </button>
        </div>
        <div style={{ textAlign: 'center', fontSize: 10.5, color: 'var(--m-text-3)', marginTop: 7 }}>
          {busy ? t('正在生成，点右侧按钮可停止') : t('对话由网关转发到上游模型，用量计入请求日志')}
        </div>
      </div>

      <ModelPicker
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        models={models}
        current={effectiveModel}
        defaultModel={defaultModel || ''}
        onPick={value => {
          setModel(value)
          setPickerOpen(false)
        }}
      />
    </div>
  )
}

/** 模型选择抽屉：本次会话临时生效，不写回网关配置 */
function ModelPicker({
  open,
  onClose,
  models,
  current,
  defaultModel,
  onPick,
}: {
  open: boolean
  onClose: () => void
  models: ManageModel[]
  current: string
  defaultModel: string
  onPick: (value: string) => void
}) {
  return (
    <Sheet open={open} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('选择对话模型')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          <button
            type='button'
            className='m-row'
            onClick={() => onPick('')}
            style={{
              borderRadius: 'var(--m-r-md)',
              border: '1px solid var(--m-line)',
              background: current === defaultModel ? 'var(--m-primary-soft)' : 'var(--m-surface)',
            }}
          >
            <span className='m-logo'>
              <Icon name='settings' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('跟随网关设置')}</div>
              <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                {defaultModel || t('还没有配置默认模型')}
              </div>
            </div>
            {current === defaultModel ? <Icon name='check' style={{ width: 16, height: 16, color: 'var(--m-primary)' }} /> : null}
          </button>

          {models.length ? (
            models.map(item => {
              const id = String(item.id || '')
              const picked = current === id
              return (
                <button
                  key={`${item.provider || ''}:${id}`}
                  type='button'
                  className='m-row'
                  onClick={() => onPick(id)}
                  style={{
                    borderRadius: 'var(--m-r-md)',
                    border: '1px solid var(--m-line)',
                    background: picked ? 'var(--m-primary-soft)' : 'var(--m-surface)',
                  }}
                >
                  <div className='m-grow' style={{ minWidth: 0 }}>
                    <div className='m-t1 m-mono' style={{ fontSize: 12.5, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {id}
                    </div>
                    <div className='m-t2'>{item.providerLabel || item.provider || ''}</div>
                  </div>
                  {picked ? <Icon name='check' style={{ width: 16, height: 16, color: 'var(--m-primary)' }} /> : null}
                </button>
              )
            })
          ) : (
            <div style={{ fontSize: 12, color: 'var(--m-text-3)', padding: '4px 2px' }}>
              {t('没有已启用的模型，请先在「模型管理」里启用')}
            </div>
          )}

          <p style={{ margin: 0, fontSize: 11.5, lineHeight: 1.7, color: 'var(--m-text-3)' }}>
            {t('这里只改本次会话用的模型，不会改网关里保存的默认值。')}
          </p>
        </SheetBody>
      </SheetContent>
    </Sheet>
  )
}

/** 工具参数：模型偶尔给出不合法 JSON —— 给个空对象让它自己从报错里学 */
function parseArgs(raw: string): Record<string, unknown> {
  const text = String(raw || '').trim()
  if (!text) return {}
  try {
    const parsed = JSON.parse(text)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {}
  } catch {
    return {}
  }
}

function Avatar() {
  return (
    <span
      style={{
        width: 26,
        height: 26,
        borderRadius: 8,
        flex: 'none',
        display: 'grid',
        placeItems: 'center',
        background: 'var(--m-primary-soft)',
        color: 'var(--m-primary)',
      }}
    >
      <Icon name='spark' style={{ width: 15, height: 15 }} />
    </span>
  )
}

function bubbleStyle(mine: boolean): React.CSSProperties {
  return {
    maxWidth: '82%',
    padding: '10px 13px',
    borderRadius: mine ? '14px 14px 5px 14px' : '14px 14px 14px 5px',
    background: mine ? 'var(--m-primary)' : 'var(--m-surface)',
    color: mine ? '#fff' : 'var(--m-text)',
    border: mine ? 0 : '1px solid var(--m-line)',
    fontSize: 13.5,
    lineHeight: 1.62,
  }
}
