import { useState } from 'react'
import { Alert, Badge, Button } from '@ui'
import type { CodingBlock } from './assistant-coding-messages'
import type { CodingRender, CodingTranslate } from './assistant-coding-types'
import { AssistantMessageActions } from './assistant-message-actions'

function statusLabel(status: string | undefined, t: CodingTranslate): string {
  switch (status) {
    case 'inProgress': return t('正在执行')
    case 'completed': return t('已完成')
    case 'failed': return t('失败')
    case 'declined': return t('已拒绝')
    case 'interrupted': return t('已停止')
    default: return status || ''
  }
}

function Copy({ value, t }: { value: string; t: CodingTranslate }) {
  const [copied, setCopied] = useState(false)
  if (!navigator.clipboard?.writeText) return null
  return <Button variant='ghost' size='sm' className='h-6 px-1.5 text-xs' onClick={() => {
    void navigator.clipboard.writeText(value).then(() => setCopied(true)).catch(() => setCopied(false))
  }}>{copied ? t('已复制') : t('复制')}</Button>
}

function Disclosure({ block, t }: { block: CodingBlock; t: CodingTranslate }) {
  const [expanded, setExpanded] = useState(Boolean(block.streaming))
  const title = block.kind === 'thinking' ? t('思考过程') : block.kind === 'tool' ? t('执行操作')
    : block.kind === 'result' ? t('执行结果') : t('会话事件')
  return <details open={expanded} onToggle={event => setExpanded(event.currentTarget.open)} className='rounded-md border border-border bg-muted/20 px-3 py-2'>
    <summary className='cursor-pointer break-words text-xs text-muted-foreground'>{title}{block.title ? ` · ${statusLabel(block.title, t)}` : ''}{block.streaming ? ` · ${t('更新中')}` : ''}</summary>
    {block.kind === 'thinking' ? <p className='mt-2 text-xs text-muted-foreground'>{t('只显示 Codex 实际返回的思考内容；未返回的内容不会补写。')}</p> : null}
    <div className='mt-2 flex justify-end'><Copy value={block.text} t={t} /></div>
    <pre className='max-h-80 overflow-auto whitespace-pre-wrap break-words text-xs leading-relaxed'>{block.text || t('等待输出…')}</pre>
  </details>
}

function Command({ block, t }: { block: CodingBlock; t: CodingTranslate }) {
  const [expanded, setExpanded] = useState(Boolean(block.streaming || block.status === 'inProgress'))
  const label = statusLabel(block.status, t)
  const failed = block.status === 'failed' || typeof block.exitCode === 'number' && block.exitCode !== 0
  return <div className={`overflow-hidden rounded-md border ${failed ? 'border-destructive/40' : 'border-border'}`}>
    <div className='flex flex-wrap items-center gap-2 border-b border-border bg-muted/30 px-3 py-2'>
      <span className='text-xs font-medium'>{t('命令')}</span>
      {label ? <Badge variant='outline'>{label}</Badge> : null}
      {typeof block.exitCode === 'number' ? <span className='text-xs text-muted-foreground'>{t('退出码：{code}', { code: block.exitCode })}</span> : null}
      {typeof block.durationMs === 'number' ? <span className='text-xs text-muted-foreground'>{t('耗时 {seconds} 秒', { seconds: Number((block.durationMs / 1000).toFixed(1)) })}</span> : null}
      <span className='flex-1' /><Copy value={[block.command, block.text].filter(Boolean).join('\n\n')} t={t} />
    </div>
    <pre className='overflow-x-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-xs'>{block.command || t('等待命令…')}</pre>
    {block.directory ? <p className='break-all px-3 pb-2 text-xs text-muted-foreground'>{block.directory}</p> : null}
    <details open={expanded} onToggle={event => setExpanded(event.currentTarget.open)} className='border-t border-border px-3 py-2'>
      <summary className='cursor-pointer text-xs text-muted-foreground'>{t('命令输出')}{block.streaming ? ` · ${t('更新中')}` : ''}</summary>
      <pre className='mt-2 max-h-96 overflow-auto whitespace-pre-wrap break-words font-mono text-xs leading-relaxed'>{block.text || (block.streaming ? t('等待输出…') : t('没有文本输出'))}</pre>
      {block.truncated ? <p className='mt-2 text-xs text-muted-foreground'>{t('输出过长，已截断实时预览；完整记录保留在本机 Codex。')}</p> : null}
    </details>
  </div>
}

export function CodingOutput({ block, renderText, t, onFork, forkDisabled }: {
  block: CodingBlock; renderText: CodingRender; t: CodingTranslate; onFork?: () => void; forkDisabled?: boolean
}) {
  if (block.role === 'user') return <div className='ml-auto flex max-w-[90%] flex-col items-end'>
    <div className='rounded-lg bg-primary/10 px-3 py-2'><div className='whitespace-pre-wrap break-words text-sm'>{block.text}</div></div>
    <AssistantMessageActions text={block.text} t={t} copyable />
  </div>
  if (block.kind === 'text') return <div className='min-w-0'>
    <div className='mb-1 flex items-center gap-2 text-xs text-muted-foreground'><span>{block.phase === 'commentary' ? t('执行进展') : block.phase === 'plan' ? t('计划') : t('回复')}</span>
      {block.streaming ? <span>{t('更新中')}</span> : null}</div>
    <div className='min-w-0 break-words text-sm leading-relaxed'>{renderText(block.text)}</div>
    {!block.streaming ? <AssistantMessageActions text={block.text} t={t} copyable onFork={onFork} forkDisabled={forkDisabled} forkLabel={t('从这一轮创建分支')} /> : null}
  </div>
  if (block.kind === 'error') return <Alert variant='destructive'><pre className='max-h-64 overflow-auto whitespace-pre-wrap break-words text-xs'>{block.text}</pre></Alert>
  if (block.kind === 'command') return <Command block={block} t={t} />
  if (block.title === 'contextCompaction') return <p className='text-xs text-muted-foreground'>{block.streaming ? t('Codex 正在压缩上下文…') : t('上下文已压缩')}</p>
  return <Disclosure block={block} t={t} />
}
