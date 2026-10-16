/**
 * AI 助手页（侧边栏「智能」组的那个一级菜单）。
 *
 * ── 一句话结构 ──────────────────────────────────────────────
 * 消息流（用户 / 助手 / 工具卡）+ 输入区；助手每轮的回答来自
 * `POST /api/assistant/chat`（SSE，见 assistant-state），工具的执行与「确认卡」
 * 全在这一层（见 assistant-tools 的模块头：只读直接跑，改动类先确认）。
 *
 * ── agent loop 的形状（本文件里最要紧的一段）────────────────
 * 一轮 = 一次上游调用。上游回的 `tool_calls` 按**顺序逐个处理**：
 *   · 只读工具 → 立即执行，结果攒着，继续下一个；
 *   · 改动 / 消耗类 → **停下来**渲染确认卡（`suspendedRef`），等用户点
 *     「执行」或「取消」；点完从下一个继续（`pump` 的续跑）；
 *   · 全部处理完 → 把攒下的结果作为 `role:"tool"` 消息回灌，再开下一轮。
 * 循环上限 `MAX_ROUNDS` 是硬保险：模型陷进「反复调工具」时用户不该看着它
 * 无限烧额度。
 *
 * ── 状态为什么用 ref + setState 双份 ─────────────────────────
 * 流式回调与异步续跑都是**闭包**，拿不到 setState 之后的新值；所以权威副本
 * 放在 `uiRef` / `wireRef` 两个 ref 里（UI 消息 / 上游协议消息），改完调
 * `syncUI()` 把 ref 的浅拷贝塞进 state 触发渲染。这是本项目多处在用的老办法
 * （见 accounts-page 的渲染范式），比在每处回调里拼 prev 更不容易出错。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Textarea, cn } from '@ui'
import { t } from '../i18n'
import {
  DEFAULT_SYSTEM_PROMPT,
  gatewayBase,
  loadAssistantConfig,
  loadModelNames,
  parseToolArguments,
  saveAssistantConfig,
  streamChat,
  type AssistantConfig,
  type ChatMessage,
  type ToolCall,
} from './assistant-state'
import {
  ASSISTANT_TOOLS,
  errorText,
  toolById,
  toolsPayload,
  type ToolPlan,
  type ToolResult,
} from './assistant-tools'

/** 一轮对话里最多允许的「上游调用 → 工具 → 上游调用」循环次数 */
const MAX_ROUNDS = 12

/* ─── UI 模型 ─────────────────────────────────────────────── */

type ToolCardItem = {
  kind: 'tool'
  call: ToolCall
  state: 'running' | 'confirm' | 'done' | 'failed' | 'cancelled'
  plan?: ToolPlan
  summary?: string
  detail?: string
  error?: string
}

type UiMessage =
  | { kind: 'user'; text: string }
  | { kind: 'bot'; text: string; streaming?: boolean }
  | ToolCardItem
  | { kind: 'notice'; text: string; bad?: boolean }

/** 挂起的工具链：确认卡点完之后从 `index` 继续跑 */
type Suspended = {
  calls: ToolCall[]
  index: number
  collected: Array<{ toolCallId: string; content: string }>
  cardIndex: number
}

/** 图标（icons.js 的 spark；拿不到时留空，不影响排版） */
function iconHtml(name: string, size: number): string {
  const api = (window as unknown as { wbIcons?: { icon?: (name: string, size: number) => string } }).wbIcons
  return api?.icon?.(name, size) || ''
}

/** 助手正文的渲染：走全局 wbMarkdown（与更新日志 / 对话预览同一份实现） */
function renderMarkdown(text: string): string {
  const api = (window as unknown as { wbMarkdown?: { render?: (text: string) => string } }).wbMarkdown
  if (api?.render) return api.render(text)
  return text.replace(/[&<>]/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch] || ch))
}

/* ─── 小组件 ─────────────────────────────────────────────── */

/** 工具卡的图标位：只读 / 执行用 spark（品牌的同一枚），待确认与失败用字符
 *  （icons.js 里没有通用的警告 / 叉号，而这里只需要一个一眼可辨的提示符） */
function ToolIcon({ tone }: { tone: 'plain' | 'warn' | 'bad' }) {
  if (tone === 'plain') {
    return <span className='asst-tc-ico' dangerouslySetInnerHTML={{ __html: iconHtml('spark', 13) }} />
  }
  return <span className={cn('asst-tc-ico', tone)}>{tone === 'warn' ? '!' : '✕'}</span>
}

/** 工具卡：五种形态共用一套骨架（头 / 参数或计划 / 结果或错误 / 脚） */
function ToolCard({ item, onConfirm }: { item: ToolCardItem; onConfirm: (run: boolean) => void }) {
  const tool = toolById(item.call.name)
  const label = tool?.label || item.call.name
  const args = parseToolArguments(item.call.arguments)
  const argsText = Object.keys(args).length ? JSON.stringify(args, null, 2) : ''
  return (
    <div className={cn('asst-tool-card',
      item.state === 'confirm' && 'confirm',
      item.state === 'confirm' && tool?.kind === 'run' && 'danger',
      item.state === 'failed' && 'failed',
      item.state === 'running' && 'running')}>
      <div className='asst-tc-head'>
        {item.state === 'running'
          ? <span className='asst-tc-ico'><span className='spin' /></span>
          : <ToolIcon tone={item.state === 'failed' ? 'bad' : item.state === 'confirm' ? 'warn' : 'plain'} />}
        <span className='asst-tc-name'>{label}</span>
        {item.state === 'confirm' && <span className='badge tag warn'>{t('待确认')}</span>}
        {item.state === 'done' && (
          <span className='asst-tc-state'>✓ {item.summary || t('完成')}</span>
        )}
        {item.state === 'failed' && <span className='asst-tc-state bad'>{t('失败')}</span>}
        {item.state === 'cancelled' && <span className='asst-tc-state plain'>{t('已取消')}</span>}
      </div>

      {item.state === 'confirm' && item.plan && (
        <div className='asst-tc-plan'>
          {item.plan.lead}
          {item.plan.table && (
            <table className='asst-mini-table'>
              <thead><tr>{item.plan.table.head.map((cell, i) => <th key={i}>{cell}</th>)}</tr></thead>
              <tbody>
                {item.plan.table.rows.slice(0, 20).map((row, i) => (
                  <tr key={i}>{row.map((cell, j) => <td key={j} className={j === row.length - 1 ? 'mono hl' : ''}>{cell}</td>)}</tr>
                ))}
              </tbody>
            </table>
          )}
          {item.plan.table && item.plan.table.rows.length > 20 && (
            <div className='hint'>{t('（仅显示前 20 条，共 {n} 条）', { n: item.plan.table.rows.length })}</div>
          )}
          {item.plan.note && <div className='hint'>{item.plan.note}</div>}
        </div>
      )}

      {item.state === 'failed' && item.error && <div className='asst-tc-error'>{item.error}</div>}
      {item.state === 'done' && item.detail && (
        <details>
          <summary className='asst-tc-result' style={{ cursor: 'pointer' }}>{t('查看结果')}</summary>
          <pre className='asst-tc-json'>{item.detail}</pre>
        </details>
      )}
      {item.state !== 'confirm' && argsText && (
        <details>
          <summary className='asst-tc-result' style={{ cursor: 'pointer' }}>{t('查看参数')}</summary>
          <pre className='asst-tc-json'>{argsText}</pre>
        </details>
      )}

      {item.state === 'confirm' && (
        <div className='asst-tc-foot'>
          <span className={cn('hint', tool?.kind === 'run' && 'danger')}>
            {tool?.kind === 'run' ? t('这会真实发起上游请求并消耗额度') : t('确认后立即生效')}
          </span>
          <Button size='sm' variant={tool?.kind === 'run' ? 'destructive' : 'default'}
            onClick={() => onConfirm(true)}>{t('执行')}</Button>
          <Button size='sm' variant='outline' onClick={() => onConfirm(false)}>{t('取消')}</Button>
        </div>
      )}
    </div>
  )
}

/* ─── 主组件 ─────────────────────────────────────────────── */



function AssistantPage() {
  const [config, setConfig] = React.useState<AssistantConfig | null>(null)
  const [models, setModels] = React.useState<string[]>([])
  const [ui, setUi] = React.useState<UiMessage[]>([])
  const [busy, setBusy] = React.useState(false)
  const [awaiting, setAwaiting] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  const [status, setStatus] = React.useState('')
  const [loadingFailed, setLoadingFailed] = React.useState(false)

  const uiRef = React.useRef<UiMessage[]>([])
  const wireRef = React.useRef<ChatMessage[]>([])
  const suspendedRef = React.useRef<Suspended | null>(null)
  const controllerRef = React.useRef<AbortController | null>(null)
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const loadedRef = React.useRef(false)
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

  /* 贴底：新消息进来时滚到底（用户往上翻时不打扰 —— 只在接近底部时贴） */
  React.useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < 160
    if (near) el.scrollTop = el.scrollHeight
  }, [ui])

  /* 数据加载：配置 + 模型清单（activate 时触发；重复调用只加载一次）
   *
   * 两件事**各自独立**，刻意不放进同一个 try：配置走的是直连 HTTP
   * （`/api/assistant/config`），模型清单走的是壳的桥 —— 后端的助手接口缺失
   * 或版本偏旧时，不该把「模型选择器」一起打成空的（那会让用户以为
   * 「我一个启用的模型都没有」，把两件事混成一件去排查）。
   *
   * 配置失败还会**自动重试**（有限次）：dev 刚启动时窗口可能比网关的 HTTP
   * 监听先就绪，那一刻的失败只代表「还早」，不代表「坏了」。 */
  const activate = React.useCallback(async () => {
    if (loadedRef.current) return
    loadedRef.current = true
    // ① 助手配置（对话模型 / 系统提示词 / 工具勾选）
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
    // ② 模型清单（助手自己只能选「可请求的名字」：启用的模型 ID + 启用的映射名）
    try {
      setModels(await loadModelNames())
    } catch (error) {
      setStatus(prev => prev || t('读取模型清单失败：{message}', { message: errorText(error) }))
    }
  }, [])

  React.useEffect(() => {
    const api = window as unknown as { wbAssistant?: { activate?: () => void } }
    api.wbAssistant = { activate: () => { void activate() } }
    // 挂载点被显示出来（首屏就停在助手页）时也要加载
    void activate()
    return () => { delete api.wbAssistant }
  }, [activate])

  const toolEnabled = React.useMemo(() => {
    const map: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) map[tool.id] = config?.tools?.[tool.id] ?? false
    return map
  }, [config])
  const enabledTools = React.useMemo(
    () => toolsPayload(toolEnabled),
    [toolEnabled],
  )
  const enabledCount = Object.values(toolEnabled).filter(Boolean).length

  const currentModel = config?.model ?? null

  /* ─── agent loop ───────────────────────────────────────── */

  /** 处理一批 tool_calls（从 `index` 起；改动类会挂起，等确认卡点完再续跑） */
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
      // 改动 / 消耗类：挂起等确认
      const plan = tool.plan?.(args) || { lead: t('将执行 {label}。', { label: tool.label }) }
      const cardIndex = uiRef.current.push({ kind: 'tool', call, state: 'confirm', plan }) - 1
      suspendedRef.current = { calls, index: i, collected, cardIndex }
      setAwaiting(true)
      setBusy(false)
      syncUI()
      return
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
    // 循环上限：模型陷进「反复调工具」时，用户不该看着它无限烧额度
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
    if (!calls.length) {
      setBusy(false)
      return
    }
    await pump(calls, 0, [])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabledTools, pump, syncUI])
  // 把最新的 loop 交给 ref（每次渲染都刷新，pump 因此总能调到当前闭包）
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

  /** 发送一条用户消息 */
  const send = React.useCallback(async (text: string) => {
    const value = text.trim()
    if (!value || busy || awaiting) return
    roundRef.current = 0
    uiRef.current.push({ kind: 'user', text: value })
    wireRef.current.push({ role: 'user', content: value })
    setDraft('')
    syncUI()
    if (!config) {
      // 设置没读出来（后端版本旧 / 接口不通）：说清「现在不能发」，别让下面
      // 那句「还没选模型」把人引到去设置里翻配置
      uiRef.current.push({
        kind: 'notice',
        text: t('助手设置没读出来，现在还不能发消息 —— 看输入框下方的提示。'),
        bad: true,
      })
      syncUI()
      return
    }
    if (!config.model) {
      uiRef.current.push({
        kind: 'notice',
        text: t('还没有选择对话模型：先在下方选一个（或去「设置 → AI 助手」里选）。'),
        bad: true,
      })
      syncUI()
      return
    }
    await loop()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awaiting, busy, config, loop, syncUI])

  const stop = React.useCallback(() => {
    controllerRef.current?.abort()
  }, [])

  const clearChat = React.useCallback(() => {
    controllerRef.current?.abort()
    suspendedRef.current = null
    uiRef.current = []
    wireRef.current = []
    setAwaiting(false)
    setBusy(false)
    setStatus('')
    syncUI()
  }, [syncUI])

  /** 换对话模型（顶栏选择器）：存的是对外模型名 */
  const pickModel = React.useCallback(async (name: string) => {
    if (!name) return
    if (!config) {
      // 设置没读出来 = 也存不回去（同一条接口）。给一句明确的话，别静默吞掉点击
      setStatus(t('助手设置没读出来，暂时存不了对话模型 —— 看输入框下方的提示。'))
      return
    }
    const next: AssistantConfig = { ...config, model: name }
    setConfig(next)
    try {
      await saveAssistantConfig(next)
    } catch (error) {
      setStatus(t('保存对话模型失败：{message}', { message: errorText(error) }))
    }
  }, [config])

  /* ─── 渲染 ─── */

  const composerDisabled = busy || awaiting
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null)

  return (
    <div className='asst-layout'>
      <div className='asst-top'>
        <Select value={currentModel ?? ''}
          onValueChange={value => void pickModel(String(value))}>
          <SelectTrigger className='asst-model-pick' aria-label={t('对话模型')}>
            <SelectValue>
              {currentModel
                ? <span className='flex items-center gap-2'>
                    <span className={cn('dot', 'ok')} />
                    <span className='mono'>{currentModel}</span>
                  </span>
                : t('选择模型…')}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {models.length === 0 && <SelectItem value='none' disabled>{t('没有可用的模型')}</SelectItem>}
            {models.map(name => (
              <SelectItem key={name} value={name}>
                {name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className='asst-top-hint'>{t('助手走本地网关，消耗所选账号的额度')}</span>
        <span className='spacer' />
        <Button variant='ghost' size='sm' onClick={clearChat} disabled={busy || awaiting || !ui.length}>
          {t('新对话')}
        </Button>
      </div>

      <div className='asst-scroll' ref={scrollRef}>
        {ui.length === 0 && (
          <div className='asst-empty'>
            <div className='empty-logo' dangerouslySetInnerHTML={{ __html: iconHtml('spark', 26) || '' }} />
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
                <button key={text} type='button' onClick={() => void send(text)}>“{text}”</button>
              ))}
            </div>
            {config !== null && enabledCount === 0 && (
              <div className='asst-empty-warn'>
                {t('当前一个工具都没勾选，助手只能纯聊天。去「设置 → AI 助手 → 工具」里勾上需要的工具。')}
              </div>
            )}
            {loadingFailed && (
              <div className='asst-empty-warn bad'>{status || t('助手设置读取失败，工具可能已过时。')}</div>
            )}
          </div>
        )}

        {ui.map((message, index) => {
          if (message.kind === 'user') {
            return (
              <div className='asst-msg user' key={index}>
                <div className='bubble'>{message.text}</div>
              </div>
            )
          }
          if (message.kind === 'bot') {
            if (!message.text && message.streaming) {
              return (
                <div className='asst-msg bot' key={index}>
                  <div className='who'><span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml('spark', 14) || '' }} /> {t('助手')}</div>
                  <div className='body'><span className='spin' /></div>
                </div>
              )
            }
            return (
              <div className='asst-msg bot' key={index}>
                <div className='who'><span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml('spark', 14) || '' }} /> {t('助手')}</div>
                <div className='body'>
                  <span dangerouslySetInnerHTML={{ __html: renderMarkdown(message.text) }} />
                  {message.streaming && <span className='asst-caret' />}
                </div>
              </div>
            )
          }
          if (message.kind === 'notice') {
            return (
              <div className='asst-msg bot' key={index}>
                <div className={cn('asst-empty-warn', message.bad && 'bad')} style={{ margin: 0 }}>
                  {message.text}
                </div>
              </div>
            )
          }
          return <ToolCard key={index} item={message} onConfirm={run => void resolveConfirm(run)} />
        })}
      </div>

      <div className='asst-composer'>
        <div className='asst-ta-wrap'>
          <Textarea
            ref={textareaRef}
            rows={1}
            value={draft}
            placeholder={composerDisabled ? t('等这一轮跑完…') : t('直接说要改什么 / 查什么…（Enter 发送，Shift+Enter 换行）')}
            disabled={composerDisabled}
            onChange={event => {
              setDraft(event.currentTarget.value)
              // 自增高（上限 = CSS 的 max-height）：text 区不滚动时也不出现内部滚动条
              const el = event.currentTarget
              el.style.height = 'auto'
              el.style.height = `${Math.min(el.scrollHeight, 132)}px`
            }}
            onKeyDown={event => {
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault()
                void send(draft)
              }
            }}
          />
          {busy
            ? <button type='button' className='asst-send stop' title={t('停止')} onClick={stop}>■</button>
            : <button type='button' className='asst-send' title={t('发送')} disabled={composerDisabled || !draft.trim()}
                onClick={() => void send(draft)}>➤</button>}
        </div>
        <div className='asst-composer-foot'>
          <span className='tools-state'>
            <span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml('settings', 12) || '' }} />
            {t('已注入 {n} / {total} 个工具 · 系统提示词：{state}', {
              n: enabledCount,
              total: ASSISTANT_TOOLS.length,
              state: config?.systemPromptEnabled ? t('开') : t('关'),
            })}
          </span>
          <span className='spacer' />
          {status
            ? <span className='hint'>
                {status}
                {loadingFailed && <button type='button' className='asst-tc-more' onClick={() => void activate()}>{t('重试')}</button>}
              </span>
            : currentModel ? <span className='hint'>{t('回复由 {model} 生成 · 走本地网关', { model: currentModel })}</span> : null}
        </div>
      </div>
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
