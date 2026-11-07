import type { ReactNode } from 'react'
import { AssistantWorkspace } from '../../../ui-shared/assistant-workspace'
import type { CodingTransport } from '../../../ui-shared/assistant-coding-types'
import { ApiError, apiCall, authHeaders, refreshPanelAuth, refreshPanelAuthIfExpiring, resolveUrl } from '../api/client'
import { serverIdentity } from '../lib/servers'
import { MarkdownLite } from '../components/MarkdownLite'
import { t } from '../i18n'

const transport: CodingTransport = {
  json: (method, path, body, signal) => apiCall(path, { method, body, signal }),
  stream: async (path, signal) => {
    const identity = serverIdentity()
    const target = resolveUrl(path)
    await refreshPanelAuthIfExpiring()
    const send = () => {
      if (serverIdentity() !== identity || signal.aborted) throw new DOMException('Aborted', 'AbortError')
      return fetch(target, { headers: { ...authHeaders(), Accept: 'text/event-stream' }, signal })
    }
    let response = await send()
    if (response.status === 401 && await refreshPanelAuth()) {
      await response.body?.cancel().catch(() => {})
      response = await send()
    }
    if (!response.ok) {
      const data = await response.json().catch(() => null)
      throw new ApiError(data?.error?.message || `HTTP ${response.status}`, response.status)
    }
    if (serverIdentity() !== identity) { await response.body?.cancel(); throw new DOMException('Aborted', 'AbortError') }
    return response
  },
}

export function MobileAssistantWorkspace({ children }: { children: ReactNode }) {
  return <AssistantWorkspace mobile transport={transport} t={t} renderText={text => <MarkdownLite text={text} />}>{children}</AssistantWorkspace>
}
