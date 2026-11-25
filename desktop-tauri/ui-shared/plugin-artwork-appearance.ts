export type PluginArtworkAppearance = {
  native: boolean
  tone: 'empty' | 'neutral' | 'tint' | 'dark'
  hue?: number
}

export const EMPTY_PLUGIN_ARTWORK: PluginArtworkAppearance = { native: false, tone: 'empty' }
const MAX_RAW_KEY_LENGTH = 256
const MAX_APPEARANCE_CACHE = 1024
const INSPECT_SIZE = 32
const appearances = new Map<string, PluginArtworkAppearance>()

let sharedCanvas: HTMLCanvasElement | null = null
let sharedContext: CanvasRenderingContext2D | null = null

export function artworkCacheKey(source: string): string {
  if (!source) return ''
  if (source.length <= MAX_RAW_KEY_LENGTH) return source
  let h1 = 0x811c9dc5
  let h2 = 0x9e3779b9
  for (let index = 0; index < source.length; index++) {
    const code = source.charCodeAt(index)
    h1 = Math.imul(h1 ^ code, 0x01000193)
    h2 = Math.imul(h2 ^ (code + (index & 255)), 0x85ebca6b)
  }
  return `${source.length}:${(h1 >>> 0).toString(36)}:${(h2 >>> 0).toString(36)}`
}

function getCached(key: string): PluginArtworkAppearance | undefined {
  const entry = appearances.get(key)
  if (!entry) return undefined
  appearances.delete(key)
  appearances.set(key, entry)
  return entry
}

function remember(source: string, appearance: PluginArtworkAppearance): PluginArtworkAppearance {
  const key = artworkCacheKey(source)
  if (key) {
    if (appearances.has(key)) appearances.delete(key)
    else if (appearances.size >= MAX_APPEARANCE_CACHE) {
      const oldest = appearances.keys().next().value
      if (oldest !== undefined) appearances.delete(oldest)
    }
    appearances.set(key, appearance)
  }
  return appearance
}

export function hasCachedPluginArtwork(source: string): boolean {
  const key = artworkCacheKey(source)
  return key ? appearances.has(key) : false
}

export function cachedPluginArtwork(source: string): PluginArtworkAppearance {
  const key = artworkCacheKey(source)
  return (key && getCached(key)) || EMPTY_PLUGIN_ARTWORK
}

function getInspectContext(): CanvasRenderingContext2D | null {
  if (typeof document === 'undefined') return null
  if (!sharedCanvas) {
    sharedCanvas = document.createElement('canvas')
    sharedCanvas.width = INSPECT_SIZE
    sharedCanvas.height = INSPECT_SIZE
    sharedContext = sharedCanvas.getContext('2d', { willReadFrequently: true })
  }
  return sharedContext
}

function hueOf(red: number, green: number, blue: number): number {
  const high = Math.max(red, green, blue)
  const low = Math.min(red, green, blue)
  const range = high - low
  if (!range) return 0
  const hue = high === red ? (green - blue) / range
    : high === green ? (blue - red) / range + 2 : (red - green) / range + 4
  return Math.round((hue * 60 + 360) % 360)
}

export function inspectPluginArtwork(image: HTMLImageElement, source: string): PluginArtworkAppearance {
  const key = artworkCacheKey(source)
  const cached = key ? getCached(key) : undefined
  if (cached) return cached
  if (!image.naturalWidth || !image.naturalHeight) return EMPTY_PLUGIN_ARTWORK
  try {
    const size = INSPECT_SIZE
    const context = getInspectContext()
    if (!context) return { native: false, tone: 'neutral' }
    const scale = Math.min(size / image.naturalWidth, size / image.naturalHeight)
    const width = image.naturalWidth * scale
    const height = image.naturalHeight * scale
    context.clearRect(0, 0, size, size)
    context.drawImage(image, (size - width) / 2, (size - height) / 2, width, height)
    const pixels = context.getImageData(0, 0, size, size).data
    const alphaAt = (x: number, y: number) => pixels[(y * size + x) * 4 + 3]
    let left = size, top = size, right = -1, bottom = -1
    let opaque = 0, weight = 0, brightness = 0, colorWeight = 0, red = 0, green = 0, blue = 0
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        const offset = (y * size + x) * 4
        const alpha = pixels[offset + 3]
        if (alpha <= 32) continue
        left = Math.min(left, x); right = Math.max(right, x)
        top = Math.min(top, y); bottom = Math.max(bottom, y)
        if (alpha >= 224) opaque++
        const pixelWeight = alpha / 255
        const r = pixels[offset], g = pixels[offset + 1], b = pixels[offset + 2]
        weight += pixelWeight
        brightness += (r * 0.2126 + g * 0.7152 + b * 0.0722) / 255 * pixelWeight
        const chroma = (Math.max(r, g, b) - Math.min(r, g, b)) / 255
        if (chroma < 0.12) continue
        const tintWeight = pixelWeight * chroma
        colorWeight += tintWeight
        red += r * tintWeight; green += g * tintWeight; blue += b * tintWeight
      }
    }
    if (!weight) return remember(source, EMPTY_PLUGIN_ARTWORK)
    const boxWidth = right - left + 1, boxHeight = bottom - top + 1
    const inset = Math.min(1, Math.floor((Math.min(boxWidth, boxHeight) - 1) / 2))
    let edgeAlpha = 0
    for (let step = 0; step < 12; step++) {
      const fraction = 0.2 + step / 11 * 0.6
      const x = Math.round(left + (boxWidth - 1) * fraction)
      const y = Math.round(top + (boxHeight - 1) * fraction)
      edgeAlpha += alphaAt(x, top + inset) + alphaAt(x, bottom - inset)
        + alphaAt(left + inset, y) + alphaAt(right - inset, y)
    }
    const native = boxWidth / boxHeight >= 0.8 && boxWidth / boxHeight <= 1.25
      && opaque / (boxWidth * boxHeight) >= 0.82 && edgeAlpha / (48 * 255) >= 0.8
    if (native) return remember(source, { native: true, tone: 'neutral' })
    if (brightness / weight >= 0.76) return remember(source, { native: false, tone: 'dark' })
    if (colorWeight / weight >= 0.16) {
      return remember(source, { native: false, tone: 'tint', hue: hueOf(red / colorWeight, green / colorWeight, blue / colorWeight) })
    }
    return remember(source, { native: false, tone: 'neutral' })
  } catch {
    // 跨域图标可能禁止像素读取，仍以原图配安全浅底展示。
    return remember(source, { native: false, tone: 'neutral' })
  }
}

type PendingInspectTask = {
  image: HTMLImageElement
  source: string
  onDone: (appearance: PluginArtworkAppearance) => void
  canceled: boolean
}

const inspectQueue: PendingInspectTask[] = []
let inspectScheduled = false

function scheduleQueueFlush(): void {
  if (inspectScheduled || inspectQueue.length === 0) return
  inspectScheduled = true
  const runSlice = (deadline?: { timeRemaining(): number }) => {
    inspectScheduled = false
    const started = typeof performance !== 'undefined' ? performance.now() : Date.now()
    while (inspectQueue.length > 0) {
      const task = inspectQueue.shift()!
      if (task.canceled) continue
      if (task.image.naturalWidth > 0) {
        task.onDone(inspectPluginArtwork(task.image, task.source))
      }
      const elapsed = (typeof performance !== 'undefined' ? performance.now() : Date.now()) - started
      if (deadline ? deadline.timeRemaining() <= 1 : elapsed >= 4) break
      if (elapsed >= 6) break
    }
    if (inspectQueue.length > 0) scheduleQueueFlush()
  }
  if (typeof window !== 'undefined' && typeof window.requestIdleCallback === 'function') {
    window.requestIdleCallback(runSlice, { timeout: 120 })
  } else {
    setTimeout(() => runSlice(), 16)
  }
}

export function scheduleInspectPluginArtwork(
  image: HTMLImageElement,
  source: string,
  onDone: (appearance: PluginArtworkAppearance) => void,
): () => void {
  const key = artworkCacheKey(source)
  const cached = key ? getCached(key) : undefined
  if (cached) {
    onDone(cached)
    return () => {}
  }
  const task: PendingInspectTask = { image, source, onDone, canceled: false }
  inspectQueue.push(task)
  scheduleQueueFlush()
  return () => {
    task.canceled = true
  }
}
