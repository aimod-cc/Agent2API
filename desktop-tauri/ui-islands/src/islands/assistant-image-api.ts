import { requestDirect } from './assistant-api'

const MAX_CACHED_IMAGES = 40
const imageCache = new Map<string, Promise<Blob>>()

function touchCache(id: string, entry: Promise<Blob>): void {
  imageCache.delete(id)
  imageCache.set(id, entry)
  while (imageCache.size > MAX_CACHED_IMAGES) {
    const oldest = imageCache.keys().next().value
    if (oldest !== undefined) imageCache.delete(oldest)
    else break
  }
}

function awaitWithSignal(promise: Promise<Blob>, signal?: AbortSignal): Promise<Blob> {
  if (!signal) return promise
  if (signal.aborted) return Promise.reject(new DOMException('Aborted', 'AbortError'))
  return new Promise<Blob>((resolve, reject) => {
    const onAbort = () => reject(new DOMException('Aborted', 'AbortError'))
    signal.addEventListener('abort', onAbort, { once: true })
    promise.then(
      blob => { signal.removeEventListener('abort', onAbort); resolve(blob) },
      error => { signal.removeEventListener('abort', onAbort); reject(error) },
    )
  })
}

/** 读取生成的图片字节流，带轻量 LRU 内存缓存与并发去重。 */
export function loadAssistantImage(id: string, signal?: AbortSignal): Promise<Blob> {
  const cached = imageCache.get(id)
  if (cached) {
    touchCache(id, cached)
    return awaitWithSignal(cached, signal)
  }
  const pending = requestDirect(`/api/assistant/images/files/${encodeURIComponent(id)}`, { method: 'GET' })
    .then(response => {
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      return response.blob()
    })
    .catch(error => {
      if (imageCache.get(id) === pending) imageCache.delete(id)
      throw error
    })
  touchCache(id, pending)
  return awaitWithSignal(pending, signal)
}
