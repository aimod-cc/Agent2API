import { HtmlContent } from '@ui'
import { AssistantWorkspace } from '../../../ui-shared/assistant-workspace'
import type { CodingTransport } from '../../../ui-shared/assistant-coding-types'
import { t } from '../i18n'
import { callJson, requestDirect, requestError } from './assistant-api'
import { renderMarkdown } from './assistant-messages'
import type { ReactNode } from 'react'

const transport: CodingTransport = {
  json: (method, path, body, signal) => callJson(method, path, body, signal),
  stream: async (path, signal) => {
    const response = await requestDirect(path, { method: 'GET', signal })
    if (!response.ok) throw requestError(await response.json().catch(() => null), response.status, 'GET', path)
    return response
  },
}

export function DesktopAssistantWorkspace({ children }: { children: ReactNode }) {
  return <AssistantWorkspace transport={transport} t={t} renderText={text => <HtmlContent className='md' html={renderMarkdown(text)} />}>{children}</AssistantWorkspace>
}
