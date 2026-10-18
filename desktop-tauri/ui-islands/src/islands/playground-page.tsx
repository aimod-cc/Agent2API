/**
 * 试炼场（Playground）—— 侧边栏「智能」组的第二个一级菜单，React 岛。
 *
 * ── 这一页解决什么问题 ──────────────────────────────────────
 * 接客户端之前先在这里自检：模型通不通、认证对不对、首字节多久、这一轮烧了
 * 多少 token。以前要验证这些只能开第三方客户端，或者对着账号页那个「测试」
 * 弹窗猜（它只回一句通不通）。
 *
 * ── 与 AI 助手页的分工（最要紧的一条）──────────────────────
 * 两页都是「对话 + 模型下拉」，但**能力面完全不同**：
 *   · AI 助手 → 注入管理助手人设 + 带 9 个工具（能改模型开关/映射/能力、
 *     调顺序、查余额），是「**能动手的运维入口**」；
 *   · 试炼场 → 不注入任何提示词、不带工具、可选采样参数、回传耗时与用量，
 *     是「**只读的连通性验证台**」。
 *
 * 两者都走**面板面**（`/api/*`），与 `/v1/chat/completions` 共用同一条
 * `UpstreamService::forward` 转发链路 —— 所以试炼场的结论依然代表真实客户端的
 * 体验。为什么不能直连 `/v1/*`：网关面默认不应答 CORS，页面直连会
 * `Failed to fetch`（踩过，详见 playground-state 的模块头）。
 *
 * ── 挂载：面板岛模式（与 docs-page / assistant-page 同款）─────
 * root 直接建在 index.html 既有的 `<section class="page" data-page="playground">`
 * 上，先 replaceChildren 清掉静态骨架。app.js 的 showPage 只在这个 section 上
 * 切 `active` 类、从不碰子节点，两边不抢同一个 DOM。
 *
 * ── 状态为什么用 ref + setState 双份 ─────────────────────────
 * 流式回调是闭包，拿不到 setState 之后的新值；所以正在生成的那条消息的正文
 * 放在 `streamingRef` 里，每帧改完调 `bump()` 触发渲染。与本项目 assistant-page /
 * accounts-page 同一套老办法。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Textarea } from '@ui'
import { t } from '../i18n'
import { gatewayBase, loadModelNames, requestDirect } from './assistant-state'
import { formatMs, formatTokens, runPlayground, type PlayMessage, type PlayStats } from './playground-state'

/* ─── 常量 ─────────────────────────────────────────────── */

const PAGE_SELECTOR = '.page[data-page="playground"]'

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 用「局部窄类型 + 转型」而不是 declare global —— 与 keys-page / accounts-page
 * 同款理由：`wbApp` 是多个岛共用的桥，各岛各 declare 一份会因同名属性类型
 * 不一致直接报 TS2717（见 keys-page 的 SharedWindow 注释）。
 */
type SharedWindow = {
  wbApp?: { toast?: (message: string, kind?: 'err' | 'ok') => void }
}

function sharedWindow(): SharedWindow {
  return window as unknown as SharedWindow
}

/** 轻提示：桥在就转交，不在就静默（与 keys-page 的 toast 同一口径） */
function toast(message: string, kind?: 'err' | 'ok'): void {
  sharedWindow().wbApp?.toast?.(message, kind)
}


/** 温度滑杆的取值范围（0~2 是 OpenAI 规范，步进 0.1） */
const TEMPERATURE_MIN = 0
const TEMPERATURE_MAX = 2

/** 历史消息回灌上限：试炼场是**手动**多轮，不做长上下文管理，超过就丢最早的
 *  （丢的是 user/assistant 对，system 永远保留）。防止一轮轮聊下去把上游的
 *  上下文窗口打满、拿到一个「与真实客户端不一致」的失败结论。 */
const MAX_HISTORY = 20

/* ─── 类型 ─────────────────────────────────────────────── */

/** 页面上的一条消息（带本地 id，React 列表用；与协议消息分开，见 toWire） */
type UiMessage = {
  id: number
  role: 'user' | 'assistant'
  content: string
  /** 助手消息的思维链（DeepSeek 系推理模型会回；折叠展示） */
  reasoning?: string
  /** 这一轮的统计（只有助手消息有） */
  stats?: PlayStats
  /** 失败时把错误摆在这条消息位置上（而不是 toast 完就没了） */
  error?: string
}

/* ─── 小组件 ───────────────────────────────────────────── */

/** 一枚「标签：值」的统计小格 */
function StatChip({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <span className='pg-chip' title={title}>
      <span className='pg-chip-k'>{label}</span>
      <span className='pg-chip-v'>{value}</span>
    </span>
  )
}

/** 助手消息的收尾统计行（首字节 / 总耗时 / token / 结束原因） */
function StatsRow({ stats }: { stats: PlayStats }) {
  const usage = stats.usage
  return (
    <div className='pg-stats'>
      <StatChip label={t('首字节')} value={formatMs(stats.ttfbMs)} title={t('流式：第一个内容片段到达的耗时')} />
      <StatChip label={t('总耗时')} value={formatMs(stats.totalMs)} />
      <StatChip label={t('输入')} value={formatTokens(usage?.promptTokens ?? null)} title={t('提示词 token')} />
      <StatChip label={t('输出')} value={formatTokens(usage?.completionTokens ?? null)} title={t('生成 token')} />
      <StatChip label={t('合计')} value={formatTokens(usage?.totalTokens ?? null)} />
      {usage?.cachedTokens !== null && usage?.cachedTokens !== undefined && (
        <StatChip label={t('命中缓存')} value={formatTokens(usage.cachedTokens)} title={t('上游提示词缓存命中的 token 数')} />
      )}
      {stats.finishReason && <StatChip label={t('结束原因')} value={stats.finishReason} />}
    </div>
  )
}

/** 思维链折叠块：默认收起（它很长，且多数时候用户只关心最终答案） */
function ReasoningBlock({ text }: { text: string }) {
  const [open, setOpen] = React.useState(false)
  return (
    <div className='pg-think'>
      <button type='button' className='pg-think-head' onClick={() => setOpen(!open)}
        aria-expanded={open}>
        <span className='pg-think-ico' aria-hidden='true'>{open ? '▾' : '▸'}</span>
        {t('思考过程')}
        <span className='pg-think-len'>{t('{n} 字', { n: text.length })}</span>
      </button>
      {open && <div className='pg-think-body'>{text}</div>}
    </div>
  )
}

/* ─── 主页面 ───────────────────────────────────────────── */

function PlaygroundPage() {
  const [models, setModels] = React.useState<string[]>([])
  const [model, setModel] = React.useState('')
  const [systemPrompt, setSystemPrompt] = React.useState('')
  const [temperature, setTemperature] = React.useState(1)
  /** 关掉 = 不传 temperature（用上游默认）；与「传 1」不是一回事 */
  const [useTemperature, setUseTemperature] = React.useState(false)
  const [maxTokens, setMaxTokens] = React.useState('')
  /**
   * 是否向请求要 usage 读数（`stream_options.include_usage`）。
   *
   * **默认关**，与真实客户端一致：多传这个字段有些上游会直接 400 —— 若默认打开，
   * 试炼场会报一个「客户端根本不会遇到」的错，这一页就不可信了。
   * 勾上才传，风险写在勾选项旁边（见渲染处）。
   */
  const [wantsUsage, setWantsUsage] = React.useState(false)
  const [input, setInput] = React.useState('')
  const [messages, setMessages] = React.useState<UiMessage[]>([])
  const [busy, setBusy] = React.useState(false)
  const [base, setBase] = React.useState('')

  /* 流式在写的正文：权威副本放 ref（闭包拿不到新 state） */
  const streamingRef = React.useRef<{ id: number; content: string; reasoning: string } | null>(null)
  const abortRef = React.useRef<AbortController | null>(null)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  const idRef = React.useRef(1)

  /** 把 ref 里的流式正文合并进 state（每帧调一次） */
  const flushStreaming = React.useCallback(() => {
    const snapshot = streamingRef.current
    if (!snapshot) return
    setMessages(prev => prev.map(item =>
      item.id === snapshot.id
        ? { ...item, content: snapshot.content, reasoning: snapshot.reasoning }
        : item,
    ))
  }, [])

  /* 首屏：模型清单与网关基址 */
  React.useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const names = await loadModelNames()
        if (!alive) return
        setModels(names)
        setModel(current => current || names[0] || '')
      } catch (error) {
        if (alive) console.warn('试炼场：读取模型清单失败', error)
      }
      try {
        const value = await gatewayBase()
        if (alive) setBase(value)
      } catch { /* 壳没就绪：地址栏留空，不影响发请求 */ }
    })()
    return () => { alive = false }
  }, [])

  /* 新消息自动滚到底 */
  React.useEffect(() => {
    const node = listRef.current
    if (node) node.scrollTop = node.scrollHeight
  }, [messages])

  /** 组装发给网关的消息数组：system（若填了）+ 历史 + 本轮 */
  const toWire = React.useCallback((history: UiMessage[], nextUser: string): PlayMessage[] => {
    const wire: PlayMessage[] = []
    const trimmedSystem = systemPrompt.trim()
    if (trimmedSystem) wire.push({ role: 'system', content: trimmedSystem })
    // 只取最近 MAX_HISTORY 条（失败的助手轮次也要带 —— 上游看不到我方的错误标注，
    // 但把它当空回答回灌会让模型以为「我刚才没说话」，所以失败的轮次整轮丢掉）
    const usable = history.filter(item => !item.error).slice(-MAX_HISTORY)
    for (const item of usable) wire.push({ role: item.role, content: item.content })
    wire.push({ role: 'user', content: nextUser })
    return wire
  }, [systemPrompt])

  /** 发一轮 */
  const send = React.useCallback(async () => {
    const text = input.trim()
    if (!text || busy) return
    if (!model) { toast(t('请先选择一个模型'), 'err'); return }

    const userMsg: UiMessage = { id: idRef.current++, role: 'user', content: text }
    const history = messages
    const assistantId = idRef.current++
    setMessages([...history, userMsg, { id: assistantId, role: 'assistant', content: '' }])
    setInput('')
    setBusy(true)
    streamingRef.current = { id: assistantId, content: '', reasoning: '' }
    const controller = new AbortController()
    abortRef.current = controller

    const maxTokensValue = maxTokens.trim() ? Number(maxTokens.trim()) : undefined
    try {
      const result = await runPlayground(
        {
          model,
          messages: toWire(history, text),
          stream: true,
          temperature: useTemperature ? temperature : undefined,
          maxTokens: Number.isFinite(maxTokensValue) ? maxTokensValue : undefined,
          wantsUsage,
          signal: controller.signal,
        },
        {
          onDelta: piece => {
            const snapshot = streamingRef.current
            if (!snapshot) return
            snapshot.content += piece
            flushStreaming()
          },
          onReasoning: piece => {
            const snapshot = streamingRef.current
            if (!snapshot) return
            snapshot.reasoning += piece
            flushStreaming()
          },
        },
      )
      // 收尾：把最终文本与统计一次性落到这条消息上（流式期间已逐帧渲染）
      setMessages(prev => prev.map(item =>
        item.id === assistantId
          ? { ...item, content: result.text, reasoning: result.reasoning, stats: result.stats }
          : item,
      ))
    } catch (error) {
      const aborted = controller.signal.aborted
      const message = aborted
        ? t('已停止')
        : (error instanceof Error ? error.message : String(error))
      setMessages(prev => prev.map(item =>
        item.id === assistantId
          ? { ...item, content: streamingRef.current?.content ?? '', reasoning: streamingRef.current?.reasoning, error: message }
          : item,
      ))
    } finally {
      streamingRef.current = null
      abortRef.current = null
      setBusy(false)
    }
  }, [input, busy, model, messages, maxTokens, toWire, temperature, useTemperature, wantsUsage, flushStreaming])

  /** 停止当前这一轮（abort 后 reader 循环以 AbortError 结束，不是错误） */
  const stop = React.useCallback(() => {
    abortRef.current?.abort()
  }, [])

  /** 清空对话（不动模型与参数设置 —— 那是「换一次实验条件」不是「重开实验」） */
  const clear = React.useCallback(() => {
    setMessages([])
    streamingRef.current = null
  }, [])

  /** Enter 发送、Shift+Enter 换行（与助手页同款手感） */
  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      void send()
    }
  }

  return (
    <div className='pg'>
      {/* ── 顶部：模型与参数（一次实验的条件，改它不影响已有对话）── */}
      <div className='pg-bar'>
        <Select value={model} onValueChange={value => setModel(String(value))}>
          <SelectTrigger className='pg-model-pick' aria-label={t('模型')}>
            <SelectValue>
              {model || t('选择模型…')}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {models.length === 0 && <SelectItem value='__none' disabled>{t('没有可用的模型')}</SelectItem>}
            {models.map(name => (
              <SelectItem key={name} value={name}>{name}</SelectItem>
            ))}
          </SelectContent>
        </Select>

        <label className='pg-param'>
          <input type='checkbox' checked={useTemperature}
            onChange={event => setUseTemperature(event.target.checked)} />
          {t('温度')}
        </label>
        <input type='range' className='pg-range' min={TEMPERATURE_MIN} max={TEMPERATURE_MAX} step={0.1}
          value={temperature} disabled={!useTemperature}
          onChange={event => setTemperature(Number(event.target.value))}
          aria-label={t('温度')} />
        <span className='pg-range-v'>{temperature.toFixed(1)}</span>

        <label className='pg-param' htmlFor='pg-max-tokens'>{t('最大输出')}</label>
        <input id='pg-max-tokens' type='number' className='pg-num' min={1} placeholder={t('不限')}
          value={maxTokens} onChange={event => setMaxTokens(event.target.value)} />

        {/* 用量读数：默认不勾 —— 把风险写在旁边，比事后解释「为什么报错」有用 */}
        <label className='pg-param'
          title={t('向请求带上 stream_options.include_usage 才能拿到 token 用量。真实客户端默认不带；部分上游不认这个字段会直接报错，所以默认关。')}>
          <input type='checkbox' checked={wantsUsage}
            onChange={event => setWantsUsage(event.target.checked)} />
          {t('要用量读数')}
        </label>

        <span className='pg-spacer' />
        <span className='pg-base' title={base || t('网关地址')}>{base || t('网关地址：读取中…')}</span>
        <Button variant='ghost' size='sm' onClick={clear} disabled={busy || messages.length === 0}>
          {t('清空')}
        </Button>
      </div>

      {/* ── 主体：对话流 ── */}
      <div className='pg-scroll' ref={listRef}>
        {messages.length === 0
          ? (
            <div className='pg-empty'>
              <p className='pg-empty-title'>{t('试炼场')}</p>
              <p>{t('这里与客户端的转发链路是同一条（同样的模型路由、账号轮换与协议翻译），结论可以代表真实客户端。')}</p>
              <p>{t('选好模型、写一句话发出去：能看到回答、思考过程、首字节耗时。token 用量要勾上「要用量读数」才有。')}</p>
            </div>
          )
          : messages.map(item => (
            <div key={item.id} className={`pg-msg pg-${item.role}`}>
              <div className='pg-msg-role'>{item.role === 'user' ? t('我') : t('模型')}</div>
              <div className='pg-msg-body'>
                {item.reasoning ? <ReasoningBlock text={item.reasoning} /> : null}
                {item.content
                  ? <div className='pg-msg-text'>{item.content}</div>
                  : (busy && !item.error ? <div className='pg-msg-wait'>{t('等待上游响应…')}</div> : null)}
                {item.error && <div className='pg-msg-err'>{item.error}</div>}
                {item.stats && <StatsRow stats={item.stats} />}
              </div>
            </div>
          ))}
      </div>

      {/* ── 底部：系统提示词（可折叠）+ 输入区 ── */}
      <div className='pg-foot'>
        <details className='pg-sys'>
          <summary>{t('系统提示词')}{systemPrompt.trim() ? t('（已设置）') : ''}</summary>
          <Textarea className='pg-sys-ta' rows={3} value={systemPrompt}
            placeholder={t('留空则不发送 system 消息（与多数客户端默认一致）')}
            onChange={event => setSystemPrompt(event.target.value)} />
        </details>
        <div className='pg-composer'>
          <Textarea className='pg-ta' rows={2} value={input}
            placeholder={t('输入一句话，Enter 发送，Shift+Enter 换行')}
            onChange={event => setInput(event.target.value)}
            onKeyDown={onKeyDown} />
          {busy
            ? <Button variant='outline' size='sm' onClick={stop}>{t('停止')}</Button>
            : <Button size='sm' onClick={() => void send()} disabled={!input.trim() || !model}>{t('发送')}</Button>}
        </div>
      </div>
    </div>
  )
}

/* ─── 挂载 ─────────────────────────────────────────────── */

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<PlaygroundPage />)
}

if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
