import { asRecord, type CodingMessage } from './assistant-coding-types'

export type CodingBlock = {
  id: string
  role: 'user' | 'assistant'
  kind: 'text' | 'thinking' | 'tool' | 'result' | 'event' | 'error'
  text: string
  title?: string
  queued?: boolean
  uncertain?: boolean
}

function text(value: unknown): string {
  if (typeof value === 'string') return value
  if (value == null) return ''
  try { return JSON.stringify(value, null, 2) } catch { return String(value) }
}

function envelope(value: unknown): Record<string, unknown> | null {
  const item = asRecord(value)
  if (!item) return null
  for (const candidate of [item, asRecord(item.message), asRecord(asRecord(item.data)?.message), asRecord(asRecord(item.payload)?.message)]) {
    if (candidate && typeof candidate.role === 'string' && 'content' in candidate) return candidate
  }
  return null
}

export function decodeCodingMessages(messages: CodingMessage[]): CodingBlock[] {
  const blocks: CodingBlock[] = []
  const streams = new Map<string, number>()
  for (const message of messages) {
    const wrapped = envelope(message.content)
    const payload = wrapped?.content
    const row = asRecord(payload)
    const push = (kind: CodingBlock['kind'], value: unknown, title?: string, role: CodingBlock['role'] = 'assistant', stream?: string) => {
      const content = text(value)
      if (!content.trim() && !title) return
      const block: CodingBlock = { id: `${message.id}:${blocks.length}`, role, kind, text: content, title,
        ...(role === 'user' ? { queued: Boolean(message.localId && !message.invokedAt), uncertain: message.deliveryState === 'indeterminate' } : {}) }
      if (stream && streams.has(stream)) { blocks[streams.get(stream)!] = { ...block, id: blocks[streams.get(stream)!].id }; return }
      if (stream) streams.set(stream, blocks.length)
      blocks.push(block)
    }
    if (!wrapped) { push('text', message.content); continue }
    if (wrapped.role === 'user') { push('text', row?.type === 'text' ? row.text : payload, undefined, 'user'); continue }
    if (wrapped.role !== 'agent' || !row) { push('text', payload); continue }
    const data = asRecord(row.data)
    if (!data) { push('text', payload); continue }
    if (row.type === 'codex') {
      const stream = typeof data.id === 'string' ? `${data.type}:${data.id}` : undefined
      switch (data.type) {
        case 'message': push('text', data.message, undefined, 'assistant', stream); break
        case 'reasoning': push('thinking', data.message, undefined, 'assistant', stream); break
        case 'error': push('error', data.message); break
        case 'tool-call': push('tool', data.input, text(data.name || data.description || data.callId)); break
        case 'tool-call-result': push(data.is_error ? 'error' : 'result', data.output, text(data.callId)); break
        case 'plan': case 'plan_update': push('event', data.entries || data.plan || data.steps || data.items); break
        case 'compact-summary': push('event', data.summary); break
      }
    } else if (row.type === 'output') {
      if (data.isMeta || data.isCompactSummary || ['rate_limit_event', 'tool_progress', 'stream_event'].includes(String(data.type))) continue
      const entry = asRecord(data.message)
      if (data.type === 'assistant' || data.type === 'user') {
        const content = entry?.content
        if (typeof content === 'string') { push('text', content); continue }
        if (!Array.isArray(content)) continue
        const realUser = data.type === 'user' && !data.isSidechain && content.every(v => asRecord(v)?.type === 'text')
        for (const raw of content) {
          const part = asRecord(raw)
          if (!part) continue
          switch (part.type) {
            case 'text': push('text', part.text, undefined, realUser ? 'user' : 'assistant'); break
            case 'thinking': push('thinking', part.thinking); break
            case 'tool_use': push('tool', part.input, text(part.name)); break
            case 'tool_result': push(part.is_error ? 'error' : 'result', data.toolUseResult ?? part.content, text(part.tool_use_id)); break
          }
        }
      } else if (data.type === 'system') {
        if (data.subtype === 'api_error') push('error', data.error)
        else if (data.subtype === 'away_summary') push('event', data.content)
      } else if (data.type === 'summary') push('event', data.summary)
      else if (data.type === 'result') { if (data.is_error) push('error', data.errors ?? data.result) }
      else push('event', data)
    } else if (row.type === 'event') {
      if (data.type === 'message' || data.type === 'error') push(data.type === 'error' ? 'error' : 'event', data.message)
      else if (data.type === 'recap') push('event', data.text)
    } else push('text', payload)
  }
  return blocks
}

export function mergeCodingMessages(current: CodingMessage[], incoming: CodingMessage[]): CodingMessage[] {
  const byId = new Map(current.map(row => [row.id, row]))
  for (const row of incoming) {
    if (row.localId) {
      for (const [id, existing] of byId) { if (id !== row.id && existing.localId === row.localId) byId.delete(id) }
    }
    byId.set(row.id, row)
  }
  return [...byId.values()].sort((a, b) => a.createdAt - b.createdAt || (a.seq ?? 0) - (b.seq ?? 0) || a.id.localeCompare(b.id))
}
