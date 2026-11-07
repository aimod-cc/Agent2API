import { asRecord, type CodingItem } from './assistant-coding-types'

export type CodingBlock = {
  id: string
  role: 'user' | 'assistant'
  kind: 'text' | 'thinking' | 'tool' | 'result' | 'event' | 'error'
  text: string
  title?: string
}

function text(value: unknown): string {
  if (typeof value === 'string') return value
  if (value == null) return ''
  try { return JSON.stringify(value, null, 2) } catch { return String(value) }
}

function parts(value: unknown): string {
  return Array.isArray(value) ? value.map(text).filter(Boolean).join('\n\n') : text(value)
}

export function decodeCodingMessages(items: CodingItem[]): CodingBlock[] {
  const blocks: CodingBlock[] = []
  for (const item of items) {
    const push = (kind: CodingBlock['kind'], value: unknown, title?: string, role: CodingBlock['role'] = 'assistant', suffix = '') => {
      const content = text(value)
      if (!content.trim() && !title) return
      blocks.push({ id: `${item.id}:${suffix || kind}`, kind, text: content, title, role })
    }
    switch (item.type) {
      case 'userMessage': {
        const content = Array.isArray(item.content) ? item.content : []
        const body = content.map(raw => {
          const part = asRecord(raw)
          if (part?.type === 'text') return text(part.text)
          if (part?.type === 'localImage') return text(part.path)
          if (part?.type === 'image') return '[image]'
          return text(part?.name || part?.path)
        }).filter(Boolean).join('\n')
        push('text', body, undefined, 'user'); break
      }
      case 'agentMessage': push('text', item.text); break
      case 'plan': push('event', item.text); break
      case 'reasoning': push('thinking', parts(item.summary) || parts(item.content)); break
      case 'commandExecution':
        push('tool', [text(item.command), text(item.aggregatedOutput)].filter(Boolean).join('\n\n'),
          `${item.status || ''}${typeof item.exitCode === 'number' ? ` · exit ${item.exitCode}` : ''}`)
        break
      case 'fileChange': {
        const changes = Array.isArray(item.changes) ? item.changes : []
        push('tool', changes.map(raw => {
          const change = asRecord(raw)
          return [text(change?.path), text(change?.diff)].filter(Boolean).join('\n')
        }).join('\n\n'), text(item.status))
        break
      }
      case 'mcpToolCall':
        push('tool', item.arguments, [item.server, item.tool, item.status].filter(Boolean).join(' · '))
        if (item.error) push('error', item.error)
        else if (item.result) push('result', item.result)
        break
      case 'dynamicToolCall':
        push('tool', item.arguments, text(item.tool))
        if (item.contentItems) push(item.success === false ? 'error' : 'result', item.contentItems)
        break
      case 'planUpdate': push('event', [text(item.explanation), text(item.plan)].filter(Boolean).join('\n')); break
      case 'error': push('error', item.message); break
      case 'webSearch': push('tool', item.action || item.query, 'Web'); break
      case 'contextCompaction': break
      default: push('event', item, item.type)
    }
  }
  return blocks
}
