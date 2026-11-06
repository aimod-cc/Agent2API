import { callJson, requestDirect } from './assistant-api'
import { ImageGenerationPanel } from '../../../ui-shared/tools-image-config'
import { downloadToolImageBlob } from '../../../ui-shared/tools-image-history'
import type { ImageToolApi } from '../../../ui-shared/tools-image-types'

const BASE = '/api/tools/image-generation'
const api: ImageToolApi = {
  load: () => callJson('GET', `${BASE}/config`),
  save: config => callJson('PUT', `${BASE}/config`, config),
  jobs: () => callJson('GET', `${BASE}/jobs`),
  remove: id => callJson('DELETE', `${BASE}/jobs/${encodeURIComponent(id)}`),
  download: async file => {
    const bridge = (window as unknown as { workbuddyDesktop?: { saveToolImage?: (id: string) => Promise<unknown> } }).workbuddyDesktop
    if (bridge?.saveToolImage) { await bridge.saveToolImage(file.id); return }
    const response = await requestDirect(`${BASE}/files/${encodeURIComponent(file.id)}`, { method: 'GET' })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    downloadToolImageBlob(file, await response.blob())
  },
}

export function ImageGenerationConfigPanel({ onChanged }: { onChanged: () => void }) {
  return <ImageGenerationPanel api={api} onChanged={onChanged} />
}
