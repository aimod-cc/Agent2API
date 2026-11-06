import type { ReactNode } from 'react'

export type AssistantEngine = 'assistant' | 'codex' | 'claude'
export type CodingEngine = Exclude<AssistantEngine, 'assistant'>
export type CodingTranslate = (key: string, params?: Record<string, string | number>) => string
export type CodingConfig = { enabled: boolean; hubUrl: string; hasToken: boolean; revision: string }
export type CodingConfigInput = { enabled: boolean; hubUrl: string; accessToken: string; clearToken?: boolean }
export type CodingMachine = { id: string; name: string; platform: string; homeDirectory?: string; workspaceRoots?: string[] }
export type CodingPermission = { id: string; tool: string; arguments: unknown; createdAt?: number; toolCallId?: string }
export type CodingSession = {
  id: string
  engine: CodingEngine
  title: string
  directory: string
  machineId?: string
  active: boolean
  thinking: boolean
  updatedAt: number
  model?: string | null
  permissionMode?: string | null
  controlledByTerminal: boolean
  concurrentClients: boolean
  pendingCount: number
  permissions: CodingPermission[]
  lifecycle?: string | null
}
export type CodingMessage = {
  id: string
  seq: number | null
  localId: string | null
  createdAt: number
  invokedAt?: number | null
  deliveryState?: 'indeterminate'
  content: unknown
}
export type CodingPage = {
  messages: CodingMessage[]
  page: {
    epoch: number; reset: boolean; hasMore: boolean; direction: string
    nextBeforeSeq: number | null; nextBeforeAt: number | null
    nextAfterSeq: number | null; nextAfterAt: number | null
    snapshotHeadSeq: number | null; snapshotHeadAt: number | null
  }
}
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
  return engine === 'assistant' ? t('AI 助手') : engine === 'codex' ? 'Codex' : 'Claude Code'
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

export function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}
