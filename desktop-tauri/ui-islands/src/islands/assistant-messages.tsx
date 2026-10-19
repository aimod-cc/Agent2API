/**
 * AI 助手页的「一轮对话」渲染单元：用户消息 / 助手消息 / 工具卡 / 确认卡。
 *
 * ── 为什么从 page 里拆出来 ──────────────────────────────────
 * assistant-page.tsx 已经同时装着 agent loop、会话持久化、输入区与设置抽屉；
 * 再把四种卡片的外形塞进去就过千行了。这里只负责**怎么画**，不知道 agent loop
 * 的存在 —— 所有状态都由调用方以 props 传进来（见 `assistant-page` 顶部的
 * UiMessage 说明）。
 *
 * ── 两条视觉口径 ────────────────────────────────────────────
 *   · 助手消息不套气泡：占满内容列（里面要放表格与工具卡，气泡会把它们压窄）；
 *   · 工具卡默认收成**一行摘要**，只有「待确认 / 失败」才展开主体 ——
 *     一次任务会连续调好几次工具，每张都摊开会把对话读成日志。
 */

import * as React from 'react'
import { Button, cn } from '@ui'
import { t } from '../i18n'
import { toolById, type ToolPlan } from './assistant-tools'
import { parseToolArguments, type ToolCall } from './assistant-state'
import {
  BoltIcon,
  CheckIcon,
  ChevronIcon,
  LayersIcon,
} from './assistant-icons'

export type ToolCardItem = {
  kind: 'tool'
  call: ToolCall
  state: 'running' | 'confirm' | 'done' | 'failed' | 'cancelled'
  plan?: ToolPlan
  summary?: string
  detail?: string
  error?: string
  /** 完全访问模式下自动执行的（卡头换成「已自动执行」） */
  auto?: boolean
}

export type UiMessage =
  | { kind: 'user'; text: string; images?: string[] }
  | { kind: 'bot'; text: string; streaming?: boolean }
  | ToolCardItem
  | { kind: 'notice'; text: string; bad?: boolean }

/* ─── 助手正文的 Markdown（走全局 wbMarkdown，与更新日志同一份实现）─── */

export function renderMarkdown(text: string): string {
  const api = (window as unknown as { wbMarkdown?: { render?: (text: string) => string } }).wbMarkdown
  if (api?.render) return api.render(text)
  return text.replace(/[&<>]/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch] || ch))
}

/** 助手的头像：一枚主色圆底 + spark。内联画，不依赖 icons.js 的注入时序。 */
function Avatar() {
  return (
    <span className='asst-avatar' aria-hidden='true'>
      <svg viewBox='0 0 24 24' width='14' height='14'>
        <path fill='currentColor' d='M12 3.6l1.7 4.9 4.9 1.7-4.9 1.7L12 16.8l-1.7-4.9-4.9-1.7 4.9-1.7L12 3.6z' />
        <path fill='currentColor' d='M18.3 15.2l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8.8-2.2z' />
      </svg>
    </span>
  )
}

function timeLabel(): string {
  try {
    return new Date().toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  } catch {
    return ''
  }
}

/* ─── 用户消息 ─────────────────────────────────────────────── */

function UserMessage({ text, images }: { text: string; images?: string[] }) {
  return (
    <div className='asst-msg user'>
      <div className='bubble'>
        {images?.length ? (
          <div className='asst-msg-atts'>
            {images.map((url, index) => (
              <img key={index} className='asst-msg-shot' src={url} alt={t('图片 {n}', { n: index + 1 })} />
            ))}
          </div>
        ) : null}
        {text ? <p>{text}</p> : null}
      </div>
    </div>
  )
}

/* ─── 助手消息 ─────────────────────────────────────────────── */

function BotMessage({ text, streaming }: { text: string; streaming?: boolean }) {
  // 空正文且这一轮已经结束 = 这条消息没有任何可显示的内容（模型没说话、直接
  // 调工具，这是最常见的一种形态）。早先这里只看 `text`，于是这种消息会永久
  // 挂着三个点的思考动画 —— 工具卡都执行完了、下一轮回答也出来了，那一行还在
  // 转，看起来像「加载永远不结束」。
  // `streaming` 期间仍然显示三个点：那是真的在等上游第一个 token。
  if (!text && !streaming) return null
  return (
    <div className='asst-msg bot'>
      <Avatar />
      <div className='body'>
        <div className='who'>
          <b>{t('助手')}</b>
          <span>{timeLabel()}</span>
        </div>
        {text ? (
          <div className='md' dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }} />
        ) : (
          <span className='asst-typing' aria-label={t('正在思考…')}>
            <i /><i /><i />
          </span>
        )}
        {streaming && text ? <span className='asst-caret' /> : null}
      </div>
    </div>
  )
}

/* ─── 工具卡 ───────────────────────────────────────────────── */

/** 状态对应的文案与色调（done 以外的状态都会让卡片默认展开） */
function toolStateText(item: ToolCardItem): { text: string; tone: 'ok' | 'bad' | 'muted' } {
  switch (item.state) {
    case 'running':
      return { text: t('执行中'), tone: 'muted' }
    case 'done':
      return { text: item.summary || t('完成'), tone: 'ok' }
    case 'failed':
      return { text: t('失败'), tone: 'bad' }
    case 'cancelled':
      return { text: t('已取消'), tone: 'muted' }
    default:
      return { text: t('待确认'), tone: 'muted' }
  }
}

/**
 * 工具卡。
 *
 * 形态分三层：
 *   · 卡片头（图标 + 工具名 + 状态）永远在；
 *   · **待确认**才展开计划（用户要看清楚将要改什么）；
 *   · 其余状态收成一行，点一下才展开参数 / 结果。
 */
function ToolCard({ item, onConfirm }: { item: ToolCardItem; onConfirm: (run: boolean) => void }) {
  const tool = toolById(item.call.name)
  const label = tool?.label || item.call.name
  const args = parseToolArguments(item.call.arguments)
  const argsText = Object.keys(args).length ? JSON.stringify(args, null, 2) : ''
  const [open, setOpen] = React.useState(false)
  const state = toolStateText(item)
  const expandable = item.state !== 'confirm' && Boolean(argsText || item.detail || item.error)
  const expanded = item.state === 'confirm' || open

  return (
    <div className={cn('asst-tool',
      item.state === 'confirm' && 'confirm',
      item.state === 'confirm' && tool?.kind === 'run' && 'danger',
      item.state === 'failed' && 'failed',
      item.auto && 'auto')}>
      <button
        type='button'
        className='asst-tool-head'
        onClick={() => { if (expandable) setOpen(prev => !prev) }}
        aria-expanded={expandable ? expanded : undefined}
        disabled={!expandable}
      >
        <span className={cn('asst-tool-ico', item.state === 'failed' && 'bad', item.state === 'confirm' && 'warn')}>
          {item.state === 'running'
            ? <span className='spin' />
            : item.state === 'failed'
              ? '✕'
              : item.auto
                ? <BoltIcon size={12} />
                : <LayersIcon size={13} />}
        </span>
        <span className='asst-tool-name'>{label}</span>
        {item.state === 'confirm' && <span className='asst-tool-sum'>{t('{n} 条改动', { n: planRowCount(item.plan) })}</span>}
        {item.state === 'done' && item.detail && !open && <span className='asst-tool-sum'>{t('查看结果')}</span>}
        <span className={cn('asst-tool-state', state.tone)}>
          {item.state === 'done' && <CheckIcon size={12} />}
          {state.text}
        </span>
        {expandable && <ChevronIcon size={12} className={cn('asst-tool-chev', open && 'open')} />}
      </button>

      {expanded && (
        <>
          {item.state === 'confirm' && item.plan && (
            <div className='asst-tool-plan'>
              <div className='lead'>{item.plan.lead}</div>
              {item.plan.table && (
                <table className='asst-mini-table'>
                  <thead><tr>{item.plan.table.head.map((cell, i) => <th key={i}>{cell}</th>)}</tr></thead>
                  <tbody>
                    {item.plan.table.rows.slice(0, 20).map((row, i) => (
                      <tr key={i}>
                        {row.map((cell, j) => (
                          <td key={j} className={j === row.length - 1 ? 'mono hl' : ''}>{cell}</td>
                        ))}
                      </tr>
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

          {item.state === 'failed' && item.error && (
            <div className='asst-tool-error'>{item.error}</div>
          )}
          {item.state === 'done' && item.detail && (
            <pre className='asst-tool-json'>{item.detail}</pre>
          )}
          {item.state !== 'confirm' && argsText && (
            <pre className='asst-tool-json args'>{argsText}</pre>
          )}

          {item.state === 'confirm' && (
            <div className='asst-tool-foot'>
              <span className={cn('hint', tool?.kind === 'run' && 'danger')}>
                {tool?.kind === 'run' ? t('这会真实发起上游请求并消耗额度') : t('确认后立即生效')}
              </span>
              <span className='spacer' />
              <Button size='sm' variant={tool?.kind === 'run' ? 'destructive' : 'default'}
                onClick={() => onConfirm(true)}>{t('执行')}</Button>
              <Button size='sm' variant='outline' onClick={() => onConfirm(false)}>{t('取消')}</Button>
            </div>
          )}
        </>
      )}
    </div>
  )
}

function planRowCount(plan?: ToolPlan): number {
  if (!plan) return 0
  return plan.table?.rows.length ?? 1
}

/* ─── 汇总渲染 ─────────────────────────────────────────────── */

export function MessageList({ messages, onConfirm }: {
  messages: UiMessage[]
  onConfirm: (run: boolean) => void
}) {
  return (
    <>
      {messages.map((message, index) => {
        if (message.kind === 'user') {
          return <UserMessage key={index} text={message.text} images={message.images} />
        }
        if (message.kind === 'bot') {
          return <BotMessage key={index} text={message.text} streaming={message.streaming} />
        }
        if (message.kind === 'notice') {
          return (
            <div key={index} className={cn('asst-notice', message.bad && 'bad')}>
              {message.text}
            </div>
          )
        }
        return <ToolCard key={index} item={message} onConfirm={onConfirm} />
      })}
    </>
  )
}

