/**
 * AI 助手页的「一轮对话」渲染单元：用户消息 / 助手消息 / 工具折叠条 / 改动确认卡。
 *
 * ── 极简克制两条视觉口径 ────────────────────────────────────
 *   · 助手消息不套气泡、不挂头像，左对齐展开在 740px 阅读列里；
 *   · 运行中 / 已完成 / 已取消工具收成极静单行折叠文字条，只有「待确认 / 失败」
 *     才用干净细框卡片展开主体。
 */

import * as React from 'react'
import { Alert, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Spinner, Table, TableBody, TableCell, TableHead, TableHeader, TableRow, cn } from '@ui'
import { t } from '../i18n'
import { toolById, type ToolPlan } from './assistant-tools'
import { parseToolArguments, type ToolCall } from './assistant-state'
import { BoltIcon, CheckIcon, ChevronIcon, CloseIcon, FileIcon } from './assistant-icons'
import { AssistantMarkdown, renderMarkdown } from './assistant-markdown'
import { formatFileSize, type AssistantTextFile } from '../../../ui-shared/assistant-files'
import { AssistantImageResults } from '../../../ui-shared/assistant-image-results'
import type { ImageAsset } from '../../../ui-shared/assistant-image-tools'
import { loadAssistantImage } from './assistant-image-api'
import { AssistantMessageActions } from '../../../ui-shared/assistant-message-actions'

export type MessageActions = {
  disabled?: boolean
  onEdit: (id: string, text: string) => Promise<void>
  onDelete: (id: string) => Promise<void>
  onFork: (id: string) => Promise<void>
}

export type ToolCardItem = {
  kind: 'tool'
  call: ToolCall
  state: 'running' | 'confirm' | 'done' | 'failed' | 'cancelled'
  plan?: ToolPlan
  summary?: string
  detail?: string
  images?: ImageAsset[]
  error?: string
  /** 完全访问模式下自动执行 */
  auto?: boolean
}

export type UiMessage =
  | { kind: 'user'; id?: string; text: string; images?: string[]; files?: AssistantTextFile[]; createdAt?: number }
  | { kind: 'bot'; id?: string; model?: string; text: string; streaming?: boolean; createdAt?: number }
  | ToolCardItem
  | { kind: 'notice'; text: string; bad?: boolean }
  | { kind: 'images'; images: ImageAsset[] }

export { renderMarkdown }

function formatMessageTime(ts?: number): string {
  if (typeof ts !== 'number' || !Number.isFinite(ts) || ts <= 0) return ''
  try {
    return new Date(ts).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  } catch {
    return ''
  }
}

/* ─── 用户消息 ─────────────────────────────────────────────── */

type UserMessageProps = {
  messageId?: string
  text: string
  images?: string[]
  files?: AssistantTextFile[]
  createdAt?: number
  actionsDisabled?: boolean
  onEdit: (id: string, text: string) => Promise<void>
  onDelete: (id: string) => Promise<void>
}

const UserMessage = React.memo(function UserMessage({
  messageId, text, images, files, createdAt, actionsDisabled, onEdit, onDelete,
}: UserMessageProps) {
  const time = formatMessageTime(createdAt)
  const handleEdit = React.useCallback((next: string) => (
    messageId ? onEdit(messageId, next) : Promise.resolve()
  ), [messageId, onEdit])
  const handleDelete = React.useCallback(() => (
    messageId ? onDelete(messageId) : Promise.resolve()
  ), [messageId, onDelete])

  return (
    <div className='asst-msg user'>
      <div className='asst-user-col'>
        <div className='asst-user-bubble'>
          {images?.length ? (
            <div className='asst-msg-atts'>
              {images.map((url, index) => (
                <img
                  key={`${index}-${url.slice(0, 32)}`}
                  className='asst-msg-shot'
                  src={url}
                  alt={t('图片 {n}', { n: index + 1 })}
                  loading='lazy'
                  decoding='async'
                />
              ))}
            </div>
          ) : null}
          {files?.length ? (
            <div className='asst-msg-files'>
              {files.map((file, index) => (
                <span className='asst-msg-file' key={`${file.name}-${index}`} title={file.name}>
                  <FileIcon size={13} />
                  <span className='asst-msg-file-name'>{file.name}</span>
                  <span className='asst-msg-file-size'>{formatFileSize(file.size)}</span>
                </span>
              ))}
            </div>
          ) : null}
          {text ? <p>{text}</p> : null}
        </div>
        {(messageId || time) ? (
          <div className='asst-msg-meta'>
            {messageId ? (
              <div className='asst-msg-actions'>
                <AssistantMessageActions
                  copyable={Boolean(text.trim())}
                  text={text}
                  t={t}
                  disabled={actionsDisabled}
                  onEdit={handleEdit}
                  onDelete={handleDelete}
                />
              </div>
            ) : null}
            {time ? <span className='asst-msg-time'>{time}</span> : null}
          </div>
        ) : null}
      </div>
    </div>
  )
})

/* ─── 助手消息 ─────────────────────────────────────────────── */

type BotMessageProps = {
  messageId?: string
  text: string
  streaming?: boolean
  model: string
  createdAt?: number
  actionsDisabled?: boolean
  onEdit: (id: string, text: string) => Promise<void>
  onDelete: (id: string) => Promise<void>
  onFork: (id: string) => Promise<void>
}

const BotMessage = React.memo(function BotMessage({
  messageId, text, streaming, model, createdAt, actionsDisabled, onEdit, onDelete, onFork,
}: BotMessageProps) {
  const time = formatMessageTime(createdAt)
  const handleEdit = React.useCallback((next: string) => (
    messageId ? onEdit(messageId, next) : Promise.resolve()
  ), [messageId, onEdit])
  const handleDelete = React.useCallback(() => (
    messageId ? onDelete(messageId) : Promise.resolve()
  ), [messageId, onDelete])
  const handleFork = React.useCallback(() => (
    messageId ? onFork(messageId) : Promise.resolve()
  ), [messageId, onFork])

  if (!text && !streaming && !messageId) return null

  return (
    <div className='asst-msg bot'>
      <div className='asst-bot-body'>
        <div className='asst-bot-who'>
          <span className='asst-bot-model'>{model}</span>
          {time ? <span className='asst-msg-time'>{time}</span> : null}
        </div>
        {text ? (
          <AssistantMarkdown text={text} />
        ) : streaming ? (
          <Spinner className='text-muted-foreground' aria-label={t('正在思考…')} />
        ) : null}
        {streaming && text ? <span className='asst-caret' /> : null}
        {messageId && !streaming ? (
          <div className='asst-msg-meta'>
            <div className='asst-msg-actions'>
              <AssistantMessageActions
                copyable={Boolean(text.trim())}
                text={text}
                t={t}
                disabled={actionsDisabled}
                onEdit={handleEdit}
                onDelete={handleDelete}
                onFork={handleFork}
              />
            </div>
          </div>
        ) : null}
      </div>
    </div>
  )
})

/* ─── 工具条目：待确认 / 失败卡片，以及极静折叠条 ───────────── */

function planRowCount(plan?: ToolPlan): number {
  if (!plan) return 0
  return plan.table?.rows.length ?? 1
}

function quietSummary(item: ToolCardItem): string {
  switch (item.state) {
    case 'running':
      return t('执行中')
    case 'cancelled':
      return t('已取消')
    default:
      return item.summary || t('完成')
  }
}

const ToolCard = React.memo(function ToolCard({ item, onConfirm }: { item: ToolCardItem; onConfirm: (run: boolean) => void }) {
  const tool = toolById(item.call.name)
  const label = tool?.label || item.call.name
  const argsText = React.useMemo(() => {
    const args = parseToolArguments(item.call.arguments)
    const displayArgs = tool?.redactArgs ? tool.redactArgs(args) : args
    return Object.keys(displayArgs).length ? JSON.stringify(displayArgs, null, 2) : ''
  }, [item.call.arguments, tool])

  const [open, setOpen] = React.useState(item.state === 'failed')

  if (item.state === 'confirm' && item.plan) {
    return (
      <div className={cn('asst-confirm-card', tool?.kind === 'run' && 'is-danger')}>
        <div className='asst-confirm-head'>
          <span className='asst-confirm-title'>
            {item.auto ? <BoltIcon size={12} /> : null}
            {t('待确认')} · {label}
          </span>
          <span className='asst-confirm-count'>{t('{n} 条改动', { n: planRowCount(item.plan) })}</span>
        </div>

        <div className='asst-confirm-body'>
          <div className='asst-confirm-lead'>{item.plan.lead}</div>
          {item.plan.table ? (
            <div className='asst-table-wrap'>
              <Table container={false} className='asst-mini-table'>
                <TableHeader>
                  <TableRow>
                    {item.plan.table.head.map((cell, i) => <TableHead key={i}>{cell}</TableHead>)}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {item.plan.table.rows.slice(0, 20).map((row, i) => (
                    <TableRow key={i}>
                      {row.map((cell, j) => (
                        <TableCell key={j} className={j === row.length - 1 ? 'mono hl' : ''}>{cell}</TableCell>
                      ))}
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          ) : null}
          {item.plan.table && item.plan.table.rows.length > 20 ? (
            <div className='asst-confirm-hint'>{t('（仅显示前 20 条，共 {n} 条）', { n: item.plan.table.rows.length })}</div>
          ) : null}
          {item.plan.note ? <div className='asst-confirm-hint'>{item.plan.note}</div> : null}
        </div>

        {item.images?.length ? <AssistantImageResults images={item.images} load={loadAssistantImage} t={t} /> : null}

        <div className='asst-confirm-foot'>
          <span className={cn('asst-confirm-hint', tool?.kind === 'run' && 'danger')}>
            {tool?.kind === 'run' ? t('这会真实发起上游请求并消耗额度') : t('确认后立即生效')}
          </span>
          <span className='asst-confirm-actions'>
            <Button type='button' size='sm' variant='outline' onClick={() => onConfirm(false)}>{t('取消')}</Button>
            <Button type='button' size='sm' variant={tool?.kind === 'run' ? 'destructive' : 'default'} onClick={() => onConfirm(true)}>
              {t('确认执行')}
            </Button>
          </span>
        </div>
      </div>
    )
  }

  if (item.state === 'failed') {
    const hasExtra = Boolean(item.detail || argsText)
    return (
      <Collapsible open={open} onOpenChange={setOpen} className='asst-confirm-card is-failed'>
        <CollapsibleTrigger render={<Button type='button' variant='ghost' />} className='asst-confirm-head-trigger' disabled={!hasExtra}>
          <span className='asst-tool-ico-bad' aria-hidden='true'><CloseIcon size={11} /></span>
          <span className='asst-confirm-title'>{t('失败')} · {label}</span>
          {item.auto ? <span className='asst-tool-quiet-auto'>{t('已自动执行')}</span> : null}
          {hasExtra ? <ChevronIcon size={11} className={cn('asst-tool-chev ml-auto', open && 'open')} /> : null}
        </CollapsibleTrigger>
        <div className='asst-confirm-body'>
          {item.error ? <Alert variant='destructive' className='asst-tool-error'>{item.error}</Alert> : null}
          {item.images?.length ? <AssistantImageResults images={item.images} load={loadAssistantImage} t={t} /> : null}
        </div>
        {hasExtra ? (
          <CollapsibleContent>
            <div className='asst-confirm-body pt-0'>
              {item.detail ? <pre className='asst-tool-pre'>{item.detail}</pre> : null}
              {argsText ? <pre className='asst-tool-pre'>{argsText}</pre> : null}
            </div>
          </CollapsibleContent>
        ) : null}
      </Collapsible>
    )
  }

  const expandable = Boolean(argsText || item.detail)
  const summaryText = quietSummary(item)

  return (
    <Collapsible open={open} onOpenChange={setOpen} className='asst-tool-quiet'>
      <CollapsibleTrigger render={<Button type='button' variant='ghost' />} className='asst-tool-quiet-trigger' disabled={!expandable}>
        <span className={cn(
          'inline-flex shrink-0 items-center',
          item.state === 'done' && 'text-success',
          item.state !== 'done' && 'text-muted-foreground',
        )}>
          {item.state === 'running' ? <Spinner className='size-3' /> : item.state === 'cancelled' ? <CloseIcon size={11} /> : <CheckIcon size={12} />}
        </span>
        <span className='asst-tool-quiet-name'>{label}</span>
        {item.auto ? <span className='asst-tool-quiet-auto'>{t('已自动执行')}</span> : null}
        {summaryText ? (
          <>
            <span className='text-muted-foreground'>·</span>
            <span className='asst-tool-quiet-sum'>{summaryText}</span>
          </>
        ) : null}
        {expandable ? <ChevronIcon size={11} className={cn('asst-tool-chev', open && 'open')} /> : null}
      </CollapsibleTrigger>

      {item.images?.length ? <AssistantImageResults images={item.images} load={loadAssistantImage} t={t} /> : null}

      {expandable ? (
        <CollapsibleContent>
          <div className='asst-tool-quiet-detail'>
            {item.detail ? <pre className='asst-tool-pre'>{item.detail}</pre> : null}
            {argsText ? <pre className='asst-tool-pre'>{argsText}</pre> : null}
          </div>
        </CollapsibleContent>
      ) : null}
    </Collapsible>
  )
})

/* ─── 汇总渲染 ─────────────────────────────────────────────── */

export const MessageList = React.memo(function MessageList({ messages, onConfirm, model, actions }: {
  messages: UiMessage[]
  onConfirm: (run: boolean) => void
  model: string
  actions: MessageActions
}) {
  return (
    <>
      {messages.map((message, index) => {
        if (message.kind === 'user') {
          return (
            <UserMessage
              key={message.id || `user-${index}`}
              messageId={message.id}
              text={message.text}
              images={message.images}
              files={message.files}
              createdAt={message.createdAt}
              actionsDisabled={actions.disabled}
              onEdit={actions.onEdit}
              onDelete={actions.onDelete}
            />
          )
        }
        if (message.kind === 'bot') {
          return (
            <BotMessage
              key={message.id || `bot-${index}`}
              messageId={message.id}
              text={message.text}
              streaming={message.streaming}
              model={message.model || model}
              createdAt={message.createdAt}
              actionsDisabled={actions.disabled}
              onEdit={actions.onEdit}
              onDelete={actions.onDelete}
              onFork={actions.onFork}
            />
          )
        }
        if (message.kind === 'images') {
          const imgKey = message.images.map(image => image.id).join(',') || `images-${index}`
          return <AssistantImageResults key={imgKey} images={message.images} load={loadAssistantImage} t={t} />
        }
        if (message.kind === 'notice') {
          return (
            <Alert variant={message.bad ? 'destructive' : 'default'} key={`notice-${index}`} className={cn('asst-notice', message.bad && 'bad')}>
              {message.text}
            </Alert>
          )
        }
        return <ToolCard key={message.call.id || `tool-${index}`} item={message} onConfirm={onConfirm} />
      })}
    </>
  )
})
