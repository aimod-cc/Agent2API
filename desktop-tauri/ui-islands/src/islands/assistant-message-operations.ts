import { useCallback, useMemo, useRef } from 'react'
import { removeAssistantMessage, type ChatMessage } from '../../../ui-shared/assistant-history'
import { forkSession, toast, type SessionDetail } from './assistant-state'
import { errorText } from './assistant-tools'
import { replayMessages } from './assistant-replay'
import type { UiMessage } from './assistant-messages'
import { t } from '../i18n'

type Options = {
  messages: { current: ChatMessage[] }
  ui: { current: UiMessage[] }
  activeId: { current: string }
  busy: () => boolean
  setBusy: (value: boolean) => void
  model: () => string | null
  sync: () => void
  persist: () => Promise<void>
  open: (detail: SessionDetail) => void
}

export function useAssistantMessageOperations(options: Options) {
  const optionsRef = useRef(options)
  optionsRef.current = options

  const edit = useCallback(async (id: string, text: string) => {
    const current = optionsRef.current
    if (current.busy()) return
    current.setBusy(true)
    try {
      current.messages.current = current.messages.current.map(message => message.id === id ? { ...message, content: text } : message)
      current.ui.current = replayMessages(current.messages.current, current.model())
      current.sync()
      await current.persist()
    } finally { current.setBusy(false) }
  }, [])

  const remove = useCallback(async (id: string) => {
    const current = optionsRef.current
    if (current.busy()) return
    current.setBusy(true)
    try {
      current.messages.current = removeAssistantMessage(current.messages.current, id)
      current.ui.current = replayMessages(current.messages.current, current.model())
      current.sync()
      await current.persist()
    } finally { current.setBusy(false) }
  }, [])

  const fork = useCallback(async (id: string) => {
    const current = optionsRef.current
    if (current.busy()) return
    current.setBusy(true)
    try {
      await current.persist()
      const detail = await forkSession(current.activeId.current, id)
      current.open(detail)
      toast(t('已从这条消息分叉为新对话'))
    } catch (failure) {
      toast(t('分叉会话失败：{message}', { message: errorText(failure) }), 'err')
    } finally { current.setBusy(false) }
  }, [])

  return useMemo(() => ({ onEdit: edit, onDelete: remove, onFork: fork }), [edit, remove, fork])
}
