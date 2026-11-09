import { asRecord, type CodingItem } from './assistant-coding-types'

export type CodingBlock = {
  id: string
  turnId?: string
  role: 'user' | 'assistant'
  kind: 'text' | 'thinking' | 'command' | 'tool' | 'result' | 'event' | 'error'
  text: string
  title?: string
  phase?: string
  streaming?: boolean
  command?: string
  directory?: string
  status?: string
  exitCode?: number
  durationMs?: number
  truncated?: boolean
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
    const push = (kind: CodingBlock['kind'], value: unknown, title?: string, role: CodingBlock['role'] = 'assistant', suffix = '', extra: Partial<CodingBlock> = {}) => {
      const content = text(value)
      if (!content.trim() && !title && !item.streaming) return
      blocks.push({ id: `${item.id}:${suffix || kind}`, turnId: item.turnId, kind, text: content, title, role,
        streaming: item.streaming, ...extra })
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
      case 'agentMessage': push('text', item.text, undefined, 'assistant', '', { phase: text(item.phase) }); break
      case 'plan': push('text', item.text, undefined, 'assistant', '', { phase: 'plan' }); break
      case 'reasoning': {
        const summary = parts(item.summary)
        const content = parts(item.content)
        push('thinking', summary || content, undefined, 'assistant', 'summary')
        if (summary && content && summary !== content) push('thinking', content, undefined, 'assistant', 'content')
        break
      }
      case 'commandExecution':
        push('command', item.aggregatedOutput, text(item.command) || 'command', 'assistant', '', {
          command: text(item.command), directory: text(item.cwd), status: text(item.status),
          exitCode: typeof item.exitCode === 'number' ? item.exitCode : undefined,
          durationMs: typeof item.durationMs === 'number' ? item.durationMs : undefined,
          truncated: item.outputTruncated === true,
        })
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
        if (item.progressMessage) push('event', item.progressMessage)
        break
      case 'dynamicToolCall':
        push('tool', item.arguments, text(item.tool))
        if (item.contentItems) push(item.success === false ? 'error' : 'result', item.contentItems)
        break
      case 'planUpdate': push('event', [text(item.explanation), text(item.plan)].filter(Boolean).join('\n')); break
      case 'error': push('error', item.message); break
      case 'webSearch': push('tool', item.action || item.query, 'Web'); break
      case 'contextCompaction': push('event', '', 'contextCompaction'); break
      default: push('event', item, item.type)
    }
  }
  return blocks
}
