import * as React from 'react'
import { t } from '../i18n'
import { clearSessions, deleteSession, saveSession, summarizeSession, toast, type AssistantConfig, type ChatMessage, type SessionDetail, type SessionSummary } from './assistant-state'
import { parseAssistantMessages } from '../../../ui-shared/assistant-history'
import { equalSavedContent } from '../../../ui-shared/saved-content'
import type { ToolCardItem, UiMessage } from './assistant-messages'
import type { ImageAsset } from '../../../ui-shared/assistant-image-tools'
import type { ModelApiKey } from '../../../ui-shared/model-api-key'

export type CollectedTool = { toolCallId: string; content: string; images?: ImageAsset[] }

type SessionRefs = {
  config: { current: AssistantConfig | null }
  apiKey: { current: ModelApiKey | undefined }
  messages: { current: ChatMessage[] }
  activeId: { current: string }
  title: { current: string }
  ui: { current: UiMessage[] }
}

export function titleFrom(messages: ChatMessage[]): string {
  const first = messages.find(message => message.role === 'user')
  const content = first?.content.trim().replace(/\s+/g, ' ') || ''
  const text = content || first?.files?.[0]?.name || ''
  return text.length > 40 ? `${text.slice(0, 40)}…` : text
}

export function useAssistantSessions(refs: SessionRefs) {
  const [sessions, setSessions] = React.useState<SessionSummary[]>([])
  const sessionsRef = React.useRef<SessionSummary[]>(sessions)
  sessionsRef.current = sessions
  const queueRef = React.useRef<Promise<void>>(Promise.resolve())
  const deletedRef = React.useRef(new Set<string>())
  const clearingRef = React.useRef(false)
  const generationRef = React.useRef(0)
  const lastSavedRef = React.useRef<{ id: string; generation: number; payload: Parameters<typeof saveSession>[0] } | null>(null)

  const persist = React.useCallback((bump = true, collected: CollectedTool[] = []): Promise<void> => {
    const id = refs.activeId.current
    if (!id || deletedRef.current.has(id) || clearingRef.current) return Promise.resolve()
    const toolCards = new Map<string, ToolCardItem>()
    for (const item of refs.ui.current) {
      if (item.kind === 'tool' && item.call.id) toolCards.set(item.call.id, item)
    }
    const messages = parseAssistantMessages([...refs.messages.current, ...collected.map(item => ({ role: 'tool' as const, ...item }))]).map(message => {
      if (message.role !== 'tool') return message
      const card = toolCards.get(message.toolCallId)
      if (!card || card.state === 'running' || card.state === 'confirm') return message
      return { ...message, outcome: { state: card.state, summary: card.summary, error: card.error, auto: card.auto } }
    })
    if (!messages.length && !sessionsRef.current.some(session => session.id === id)) return Promise.resolve()
    const generation = generationRef.current
    const payload = {
      id, messages, title: refs.title.current || titleFrom(messages),
      model: refs.config.current?.model ?? null, reasoning: refs.config.current?.reasoning ?? null,
    }
    queueRef.current = queueRef.current.catch(() => {}).then(async () => {
      if (generation !== generationRef.current || deletedRef.current.has(id)) return
      const last = lastSavedRef.current
      if (last?.id === id && last.generation === generation && equalSavedContent(last.payload, payload)) return
      try {
        const saved = await saveSession(payload)
        if (generation !== generationRef.current || deletedRef.current.has(id)) return
        lastSavedRef.current = { id, generation, payload: { ...payload, title: saved.title } }
        if (refs.activeId.current === id) refs.title.current = saved.title
        setSessions(previous => {
          const index = previous.findIndex(item => item.id === id)
          if (index < 0 || bump) return [saved, ...previous.filter(item => item.id !== id)]
          return previous.map(item => item.id === id ? saved : item)
        })
      } catch (failure) {
        lastSavedRef.current = null
        toast(t('这条会话没能存进历史：{message}', { message: failure instanceof Error ? failure.message : String(failure) }), 'err')
      }
    })
    return queueRef.current
  }, [refs.activeId, refs.config, refs.messages, refs.title, refs.ui])

  const remove = React.useCallback(async (id: string) => {
    deletedRef.current.add(id)
    const pending = queueRef.current.catch(() => {}).then(() => deleteSession(id))
    queueRef.current = pending.catch(() => {})
    try {
      await pending
      if (lastSavedRef.current?.id === id) lastSavedRef.current = null
      setSessions(previous => previous.filter(item => item.id !== id))
    } catch (failure) {
      deletedRef.current.delete(id)
      throw failure
    }
  }, [])

  const clear = React.useCallback(async () => {
    clearingRef.current = true
    generationRef.current += 1
    lastSavedRef.current = null
    const id = refs.activeId.current
    const pending = queueRef.current.catch(() => {}).then(() => clearSessions())
    queueRef.current = pending.then(() => {}, () => {})
    try {
      await pending
      deletedRef.current.add(id)
      setSessions([])
    } finally {
      clearingRef.current = false
    }
  }, [refs.activeId])

  const summarizingRef = React.useRef(new Set<string>())
  const summarize = React.useCallback(async (id: string) => {
    if (summarizingRef.current.has(id) || deletedRef.current.has(id)) return
    summarizingRef.current.add(id)
    const generation = generationRef.current
    const apiKey = refs.apiKey.current?.key
    if (!apiKey) { summarizingRef.current.delete(id); return }
    try {
      await queueRef.current
      if (generation !== generationRef.current || deletedRef.current.has(id)) return
      const saved = await summarizeSession(id, apiKey)
      if (generation !== generationRef.current || deletedRef.current.has(id)) return
      const last = lastSavedRef.current
      if (last?.id === id && last.generation === generation) last.payload = { ...last.payload, title: saved.title }
      if (refs.activeId.current === id) refs.title.current = saved.title
      setSessions(previous => previous.map(item => item.id === id ? saved : item))
    } catch (failure) {
      toast(t('标题总结失败：{message}', { message: failure instanceof Error ? failure.message : String(failure) }), 'warn')
    } finally { summarizingRef.current.delete(id) }
  }, [refs.activeId, refs.title])

  const acceptLoaded = React.useCallback((detail: SessionDetail) => {
    lastSavedRef.current = {
      id: detail.id, generation: generationRef.current,
      payload: { id: detail.id, messages: parseAssistantMessages(detail.messages), title: detail.title,
        model: detail.model, reasoning: detail.reasoning },
    }
  }, [])

  return { sessions, setSessions, persist, remove, clear, summarize, acceptLoaded }
}
