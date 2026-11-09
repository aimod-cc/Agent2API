import type { ReactNode } from 'react'

export type AssistantEngine = 'assistant' | 'codex'
export type CodingEngine = 'codex'
export type CodingTranslate = (key: string, params?: Record<string, string | number>) => string
export type CodingConfig = {
  enabled: boolean
  executable: string
  codexHome: string
  revision: string
  runtime: 'local-codex'
}
export type CodingConfigInput = Pick<CodingConfig, 'enabled' | 'executable' | 'codexHome'>
export type CodingMachine = { id: string; name: string; platform: string }
export type CodingProject = { id: string; name: string; directory: string; accessible: boolean; createdAt: number; legacy?: boolean }
export type CodingDirectories = { path: string; parent: string | null; entries: { name: string; path: string }[]; locations: { name: string; path: string }[]; limited: boolean }
export type CodingStatus = { connected: boolean; authenticated: boolean; version: string; executable: string; codexHome: string; machines: CodingMachine[] }
export type CodingPermission = { id: string; tool: string; arguments: unknown; createdAt?: number; submitted: boolean; canApprove: boolean }
export type CodingSession = {
  id: string
  engine: CodingEngine
  title: string
  directory: string
  projectId?: string | null
  projectName?: string | null
  machineId: string
  active: boolean
  thinking: boolean
  updatedAt: number
  model?: string | null
  permissionMode?: string | null
  pendingCount: number
  permissions: CodingPermission[]
  lifecycle: string
  accessible: boolean
  connected: boolean
  shared: boolean
  terminalCount: number
  terminalOpening: boolean
  terminalError?: string | null
  activeTurnId?: string | null
  modelProvider?: string | null
  reasoningEffort?: string | null
  serviceTier?: string | null
  settingsRevision: number
  settingsKnown: boolean
  tokenUsage?: CodingTokenUsage | null
  usageUpdatedAt?: number | null
  usageSource?: 'live' | 'history' | null
  forkedFromId?: string | null
}
export type CodingTokenBreakdown = { totalTokens: number; inputTokens: number; cachedInputTokens: number; outputTokens: number; reasoningOutputTokens: number }
export type CodingTokenUsage = { total: CodingTokenBreakdown; last: CodingTokenBreakdown; modelContextWindow: number | null }
export type CodingModel = {
  id: string; displayName: string; description: string; hidden: boolean; isDefault: boolean
  reasoningEfforts: { reasoningEffort: string; description: string }[]
  defaultReasoningEffort: string | null
  serviceTiers: { id: string; name: string; description: string }[]
  defaultServiceTier: string | null; supportsFast: boolean
}
export type CodingSettingsChange = { model?: string; reasoningEffort?: string; fastMode?: boolean }
export type CodingRateLimitWindow = { usedPercent: number; windowDurationMins: number | null; resetsAt: number | null }
export type CodingRateLimit = {
  limitId: string | null; limitName: string | null; planType: string | null
  primary: CodingRateLimitWindow | null; secondary: CodingRateLimitWindow | null
  credits: { hasCredits: boolean; unlimited: boolean; balance: string | null } | null
}
export type CodingAccountUsage = {
  available: boolean; reason?: 'account_type' | 'unavailable'; error?: string; stale?: boolean; updatedAt: number | null
  data?: { rateLimits: CodingRateLimit; rateLimitsByLimitId: Record<string, CodingRateLimit> | null }
}
export type CodingItem = { id: string; type: string; turnId?: string; turnStatus?: string; streaming?: boolean; clientId?: string | null; [key: string]: unknown }
export type CodingTurn = { id: string; status: string; canFork: boolean }
export type CodingSubmission = { localId: string; state: string; turnId: string | null }
export type CodingHistory = { items: CodingItem[]; turns: CodingTurn[]; submissions: CodingSubmission[] }
export type CodingTransport = {
  json: <T>(method: 'GET' | 'POST' | 'PUT', path: string, body?: unknown, signal?: AbortSignal) => Promise<T>
  stream: (path: string, signal: AbortSignal) => Promise<Response>
}
export type CodingDelivery = { localId: string; text: string; state: 'sending' | 'accepted' | 'uncertain' }
export type CodingRender = (text: string) => ReactNode
export const CODING_BASE = '/api/assistant/coding'

export function codingPath(id: string, suffix = ''): string {
  return `${CODING_BASE}/sessions/${encodeURIComponent(id)}${suffix}`
}

export function engineLabel(engine: AssistantEngine, t: CodingTranslate): string {
  return engine === 'assistant' ? t('模型对话') : 'Codex'
}

export function codingConnection(transport: CodingTransport, revision: string): CodingTransport {
  const scoped = (path: string) => `${path}${path.includes('?') ? '&' : '?'}connection=${encodeURIComponent(revision)}`
  return { json: (method, path, body, signal) => transport.json(method, scoped(path), body, signal),
    stream: (path, signal) => transport.stream(scoped(path), signal) }
}

export function codingMessageId(): string {
  if (typeof globalThis.crypto?.randomUUID === 'function') return crypto.randomUUID()
  const bytes = new Uint8Array(16)
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    crypto.getRandomValues(bytes)
    return [...bytes].map(value => value.toString(16).padStart(2, '0')).join('')
  }
  return `message-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

export function codingFailureIsDefinite(error: unknown): boolean {
  const detail = asRecord(error)
  const status = Number(detail?.status)
  return status >= 400 && status < 500 && detail?.code !== 'coding_delivery_uncertain' && detail?.type !== 'coding_delivery_uncertain'
}

export function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}
