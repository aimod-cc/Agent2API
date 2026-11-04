import type { ChatMessage } from '../../../../ui-shared/assistant-history'
import type { ImageAsset } from '../../../../ui-shared/assistant-image-tools'
import type { AssistantTextFile } from '../../../../ui-shared/assistant-files'
import { toolById } from '../../lib/assistantTools'

export type Bubble = {
  id: string
  messageIds?: string[]
  role: 'user' | 'assistant'
  model?: string
  text: string
  streaming?: boolean
  error?: string
  tools?: string[]
  attachments?: string[]
  files?: AssistantTextFile[]
  images?: ImageAsset[]
}

export function replayBubbles(messages: ChatMessage[], model: string | null): Bubble[] {
  const bubbles: Bubble[] = []
  for (const message of messages) {
    if (message.role === 'user') {
      bubbles.push({ id: message.id!, messageIds: [message.id!], role: 'user', text: message.content, attachments: message.images, files: message.files })
      continue
    }
    if (message.role === 'tool') {
      const last = bubbles[bubbles.length - 1]
      if (last?.role === 'assistant') {
        last.images = [...(last.images || []), ...(message.images || [])]
        if (message.outcome?.state === 'failed') last.error = message.outcome.error || message.content
      }
      continue
    }
    const last = bubbles[bubbles.length - 1]
    if (last?.role === 'assistant' && last.model === (message.model || model || '')) {
      last.text += `${last.text && message.content ? '\n\n' : ''}${message.content}`
      last.messageIds?.push(message.id!)
      last.tools?.push(...(message.toolCalls || []).map(call => toolById(call.name)?.label || call.name))
    } else {
      bubbles.push({ id: message.id!, messageIds: [message.id!], role: 'assistant', model: message.model || model || '', text: message.content,
        tools: (message.toolCalls || []).map(call => toolById(call.name)?.label || call.name) })
    }
  }
  return bubbles
}
