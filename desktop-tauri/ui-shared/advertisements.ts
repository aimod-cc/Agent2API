import * as React from 'react'

export type AdvertisementCopy = { title: string; description: string; linkLabel: string; tags: string[]; note: string }
export type Advertisement = {
  id: string
  category: 'provider' | 'proxy'
  presetKey: string
  order: number
  startsAt: number | null
  endsAt: number | null
  url: string
  imageUrl: string
  imageDarkUrl: string
  tone: 'gold' | 'blue' | 'neutral'
  translations: Record<string, AdvertisementCopy>
}

type Snapshot = {
  schemaVersion: number; revision: number; enabled: boolean; serverTime?: number; expiresInSeconds: number; items: Advertisement[]
}
type Reader = () => Promise<unknown>
const EMPTY: readonly Advertisement[] = []
const listeners = new Set<() => void>()
let reader: Reader | null = null
let identity = () => ''
let sourceIdentity = ''
let current: readonly Advertisement[] = EMPTY
let expiresAt = 0
let requestGeneration = 0
let inFlight = false
let timer: ReturnType<typeof setInterval> | null = null
let expiryTimer: ReturnType<typeof setTimeout> | null = null

export function configureAdvertisements(read: Reader, source: () => string): void {
  reader = read
  identity = source
  resetAdvertisements()
}

export function resetAdvertisements(): void {
  requestGeneration++
  inFlight = false
  if (expiryTimer) { clearTimeout(expiryTimer); expiryTimer = null }
  sourceIdentity = identity()
  expiresAt = 0
  publish(EMPTY)
  if (listeners.size) void refresh()
}

function publish(items: readonly Advertisement[]): void {
  current = items
  for (const listener of listeners) listener()
}

function expire(): void {
  if (expiresAt && performance.now() >= expiresAt) { expiresAt = 0; publish(EMPTY) }
}

async function refresh(): Promise<void> {
  if (sourceIdentity !== identity()) { resetAdvertisements(); return }
  expire()
  if (!reader || inFlight) return
  inFlight = true
  const generation = requestGeneration
  const source = sourceIdentity
  const started = performance.now()
  try {
    const result = await reader() as Snapshot
    if (generation !== requestGeneration || source !== identity()) return
    if (!result || result.schemaVersion !== 1 || typeof result.enabled !== 'boolean'
      || typeof result.expiresInSeconds !== 'number' || !Number.isFinite(result.expiresInSeconds)
      || result.expiresInSeconds < 0 || result.expiresInSeconds > 3600 || !Array.isArray(result.items) || result.items.length > 100) {
      expiresAt = 0; publish(EMPTY); return
    }
    expiresAt = started + result.expiresInSeconds * 1000
    const serverTime = result.serverTime
    const items = result.enabled && expiresAt > performance.now() ? result.items.filter(validAdvertisement)
      .filter(ad => typeof serverTime === 'number' && (ad.startsAt === null || ad.startsAt <= serverTime)
        && (ad.endsAt === null || ad.endsAt > serverTime))
      .sort((left, right) => left.order - right.order || left.id.localeCompare(right.id)) : EMPTY
    publish(items)
    if (expiryTimer) { clearTimeout(expiryTimer); expiryTimer = null }
    if (result.expiresInSeconds > 0 && items.length) {
      const nextEnd = typeof serverTime === 'number' ? Math.min(expiresAt,
        ...items.filter(ad => ad.endsAt !== null).map(ad => started + ad.endsAt! - serverTime)) : expiresAt
      expiryTimer = setTimeout(() => {
        expiresAt = 0; publish(EMPTY); if (listeners.size) void refresh()
      }, Math.max(0, nextEnd - performance.now()) + 1)
    }
  } catch {
    expire()
  } finally { if (generation === requestGeneration) inFlight = false }
}

function validAdvertisement(ad: Advertisement): boolean {
  if (!ad || typeof ad.id !== 'string' || typeof ad.presetKey !== 'string'
    || !['provider', 'proxy'].includes(ad.category) || !['gold', 'blue', 'neutral'].includes(ad.tone)
    || !Number.isInteger(ad.order) || !safeLink(ad.url) || !safeImage(ad.imageUrl) || !safeImage(ad.imageDarkUrl)
    || !ad.translations || typeof ad.translations !== 'object') return false
  return Object.entries(ad.translations).every(([locale, copy]) => ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko', 'pt-BR'].includes(locale)
    && copy && typeof copy.title === 'string' && typeof copy.description === 'string' && typeof copy.linkLabel === 'string'
    && typeof copy.note === 'string' && Array.isArray(copy.tags) && copy.tags.every(tag => typeof tag === 'string'))
    && Boolean(ad.translations['zh-Hans'])
}

function safeLink(value: unknown): boolean {
  if (typeof value !== 'string') return false
  try { const url = new URL(value); return url.protocol === 'https:' && !url.username && !url.password } catch { return false }
}

function safeImage(value: unknown): boolean {
  if (value === '') return true
  if (typeof value !== 'string') return false
  try {
    const url = new URL(value)
    if (url.search || url.hash) return false
    if (url.origin === 'https://agent2api.dev') {
      return /^\/(?:ad-assets\/[\w-]+\.(?:png|jpg|jpeg|webp)|api\/media\/[a-f0-9-]+\.(?:png|jpg|webp))$/.test(url.pathname)
    }
    if (url.origin === 'https://cdn.agent2api.dev') {
      return /^\/advertisements\/[\w-]+\.(?:png|jpg|jpeg|webp)$/.test(url.pathname)
    }
    return false
  } catch { return false }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  if (!timer) timer = setInterval(() => void refresh(), 10000)
  void refresh()
  return () => {
    listeners.delete(listener)
    if (!listeners.size && timer) { clearInterval(timer); timer = null }
  }
}

export function useAdvertisements(): readonly Advertisement[] {
  return React.useSyncExternalStore(subscribe, () => current, () => EMPTY)
}

export function advertisements(): readonly Advertisement[] { return current }

export function advertisementCopy(ad: Advertisement): AdvertisementCopy {
  const api = (window as unknown as { wbI18n?: { locale?: () => string } }).wbI18n
  const locale = api?.locale?.() || 'zh-Hans'
  return ad.translations[locale] || ad.translations['zh-Hans']
}
