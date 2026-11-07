export const CAPABILITY_KEYS = [
  'maxInputTokens',
  'maxOutputTokens',
  'supportsToolCall',
  'supportsImages',
  'supportsVideo',
  'supportsReasoning',
] as const

export type CapabilityKey = (typeof CAPABILITY_KEYS)[number]
export type Capabilities = Partial<Record<CapabilityKey, number | boolean | null>>
export type CapabilitySource = 'upstream' | 'modelsdev' | 'user'

export const TOKEN_KEYS: readonly CapabilityKey[] = ['maxInputTokens', 'maxOutputTokens']
export const BOOLEAN_KEYS: readonly CapabilityKey[] = [
  'supportsToolCall', 'supportsImages', 'supportsVideo', 'supportsReasoning',
]

// 与后端 core::capability::MAX_TOKEN_VALUE 保持一致。
export const MAX_TOKEN_VALUE = 100_000_000

export function isTokenKey(key: CapabilityKey): boolean {
  return TOKEN_KEYS.includes(key)
}

export function normalizeCapability(key: CapabilityKey, value: unknown): number | boolean | null {
  if (isTokenKey(key)) {
    const number = typeof value === 'number' ? value : Number(value)
    if (!Number.isFinite(number) || number < 1 || number > MAX_TOKEN_VALUE) return null
    return Math.trunc(number)
  }
  return typeof value === 'boolean' ? value : null
}

export function normalizeCapabilities(raw: unknown): Capabilities {
  const result: Capabilities = {}
  if (!raw || typeof raw !== 'object') return result
  const source = raw as Record<string, unknown>
  for (const key of CAPABILITY_KEYS) {
    const value = normalizeCapability(key, source[key])
    if (value !== null) result[key] = value
  }
  return result
}

export function normalizeOverrides(raw: unknown): CapabilityKey[] {
  if (!Array.isArray(raw)) return []
  return CAPABILITY_KEYS.filter(key => raw.some(item => item === key))
}

export function capSourceOf(
  model: { capSources?: unknown } | null | undefined,
  key: CapabilityKey,
): CapabilitySource | '' {
  const raw = (model?.capSources as Record<string, unknown> | undefined)?.[key]
  return raw === 'upstream' || raw === 'modelsdev' || raw === 'user' ? raw : ''
}
