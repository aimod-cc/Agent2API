import * as React from 'react'
import { Button, HtmlContent } from '@ui'
import { t } from '../i18n'
import { CheckIcon, CopyIcon } from './assistant-icons'

type MarkdownSegment =
  | { kind: 'md'; content: string }
  | { kind: 'code'; lang: string; content: string }

const TOP_LEVEL_FENCE_RE = /^ {0,3}(`{3,}|~{3,})\s*([A-Za-z0-9_+#.-]*)\s*$/

/** 助手正文的 Markdown（走全局 wbMarkdown，与更新日志同一份实现） */
export function renderMarkdown(text: string): string {
  const api = (window as unknown as { wbMarkdown?: { render?: (text: string) => string } }).wbMarkdown
  if (api?.render) return api.render(text)
  return text.replace(/[&<>]/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch] || ch))
}

/** 仅拆分顶层围栏代码块：正文块保留完整换行语境，已完成分段在流式阶段可稳定命中记忆化。 */
function splitMarkdownBlocks(source: string): MarkdownSegment[] {
  const normalized = source.replace(/\r\n?/g, '\n')
  if (!normalized) return []
  const lines = normalized.split('\n')
  const segments: MarkdownSegment[] = []
  let mdBuffer: string[] = []
  let i = 0

  const flushMd = () => {
    if (!mdBuffer.length) return
    const text = mdBuffer.join('\n')
    mdBuffer = []
    if (text.trim()) segments.push({ kind: 'md', content: text })
  }

  while (i < lines.length) {
    const line = lines[i]
    const open = TOP_LEVEL_FENCE_RE.exec(line)
    if (!open) {
      mdBuffer.push(line)
      i += 1
      continue
    }
    flushMd()
    const marker = open[1]
    const lang = open[2] || ''
    const codeLines: string[] = []
    i += 1
    while (i < lines.length) {
      const close = TOP_LEVEL_FENCE_RE.exec(lines[i])
      if (close && close[1][0] === marker[0] && close[1].length >= marker.length) {
        i += 1
        break
      }
      codeLines.push(lines[i])
      i += 1
    }
    segments.push({ kind: 'code', lang, content: codeLines.join('\n') })
  }
  flushMd()
  return segments
}

const MarkdownProse = React.memo(function MarkdownProse({ content }: { content: string }) {
  const html = React.useMemo(() => renderMarkdown(content), [content])
  return <HtmlContent className='md' html={html} />
})

const MarkdownCodeBlock = React.memo(function MarkdownCodeBlock({ lang, content }: { lang: string; content: string }) {
  const [copied, setCopied] = React.useState(false)
  const timerRef = React.useRef<number | null>(null)

  React.useEffect(() => () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current)
  }, [])

  const copy = () => {
    if (!navigator.clipboard?.writeText) return
    void navigator.clipboard.writeText(content).then(() => {
      setCopied(true)
      if (timerRef.current !== null) window.clearTimeout(timerRef.current)
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null
        setCopied(false)
      }, 1800)
    }).catch(() => setCopied(false))
  }

  const canCopy = typeof navigator !== 'undefined' && typeof (navigator.clipboard as { writeText?: unknown } | undefined)?.writeText === 'function'

  return (
    <div className='asst-code-card'>
      <div className='asst-code-head'>
        <span>{lang || t('代码')}</span>
        {canCopy ? (
          <Button type='button' variant='ghost' size='xs' className='asst-code-copy' onClick={copy}>
            {copied ? <CheckIcon size={11} /> : <CopyIcon size={11} />}
            <span>{copied ? t('已复制') : t('复制')}</span>
          </Button>
        ) : null}
      </div>
      <pre className='asst-code-pre'><code>{content}</code></pre>
    </div>
  )
})

export const AssistantMarkdown = React.memo(function AssistantMarkdown({ text }: { text: string }) {
  const segments = React.useMemo(() => splitMarkdownBlocks(text), [text])
  if (!segments.length) return null
  return (
    <div className='asst-md-flow'>
      {segments.map((segment, index) => (
        segment.kind === 'code'
          ? <MarkdownCodeBlock key={`code-${index}`} lang={segment.lang} content={segment.content} />
          : <MarkdownProse key={`md-${index}`} content={segment.content} />
      ))}
    </div>
  )
})
