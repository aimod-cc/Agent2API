import { ImageGenerationPanel } from '../../../ui-shared/tools-image-config'
import type { ImageToolApi } from '../../../ui-shared/tools-image-types'
import { apiCall, authHeaders, refreshPanelAuthIfExpiring, resolveUrl } from '../api/client'
import { serverIdentity } from '../lib/servers'
import { saveMobileAssistantImage } from '../lib/assistant-image-save'

const BASE = '/api/tools/image-generation'
const api: ImageToolApi = {
  load: () => apiCall(`${BASE}/config`),
  save: config => apiCall(`${BASE}/config`, { method: 'PUT', body: config }),
  saveTimeout: seconds => apiCall('/api/tools/servers/image-generation', { method: 'PATCH', body: { protocol: 'mcp', timeout_seconds: seconds } }),
  jobs: () => apiCall(`${BASE}/jobs`),
  remove: id => apiCall(`${BASE}/jobs/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  download: async file => {
    const identity = serverIdentity()
    await refreshPanelAuthIfExpiring()
    if (serverIdentity() !== identity) throw new DOMException('Aborted', 'AbortError')
    const response = await fetch(resolveUrl(`${BASE}/files/${encodeURIComponent(file.id)}`), { headers: authHeaders() })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    const blob = await response.blob()
    if (serverIdentity() !== identity) throw new DOMException('Aborted', 'AbortError')
    await saveMobileAssistantImage(file, blob)
  },
}

export function ImageGenerationSection({ onChanged }: { onChanged: () => void }) {
  return <div className='m-section'><ImageGenerationPanel api={api} showTimeout onChanged={onChanged} /></div>
}
