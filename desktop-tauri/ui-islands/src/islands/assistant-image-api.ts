import { requestDirect } from './assistant-api'

export async function loadAssistantImage(id: string, signal?: AbortSignal): Promise<Blob> {
  const response = await requestDirect(`/api/assistant/images/files/${encodeURIComponent(id)}`, { method: 'GET', signal })
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  return response.blob()
}
