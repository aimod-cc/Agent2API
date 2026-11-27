export type ReasoningModel = {
  reasoningLevels?: unknown
  reasoningDefaultLevel?: unknown
  reasoningFixedLevel?: unknown
  reasoningBindingSupported?: unknown
  reasoningSource?: unknown
  supportsReasoning?: unknown
  capabilities?: unknown
}

export const POSITIVE_REASONING_LEVELS = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const

function supportsReasoningOf(model?: ReasoningModel | null): boolean | null {
  const caps = model?.capabilities && typeof model.capabilities === 'object'
    ? model.capabilities as { supportsReasoning?: unknown } : null
  if (typeof caps?.supportsReasoning === 'boolean') return caps.supportsReasoning
  if (typeof model?.supportsReasoning === 'boolean') return model.supportsReasoning
  return null
}

export function modelReasoning(model?: ReasoningModel | null): {
  levels: string[]
  defaultLevel: string
  fixedLevel: string
  bindingSupported: boolean | null
  source: string
  declared: boolean
} {
  const text = (value: unknown): string => typeof value === 'string' ? value.trim() : ''
  const raw = model?.reasoningLevels
  const levels = Array.isArray(raw)
    ? [...new Set(raw.filter((value): value is string => typeof value === 'string')
      .map(value => value.trim()).filter(Boolean))]
    : []
  const fixedLevel = text(model?.reasoningFixedLevel)
  const bindingSupported = typeof model?.reasoningBindingSupported === 'boolean' ? model.reasoningBindingSupported : null
  const source = text(model?.reasoningSource)
  const resolvedLevels = fixedLevel ? [fixedLevel] : levels
  if (supportsReasoningOf(model) === false && !(source === 'upstream' && resolvedLevels.length > 0)) {
    return {
      levels: [],
      defaultLevel: '',
      fixedLevel: '',
      bindingSupported,
      source,
      declared: true,
    }
  }
  return {
    levels: resolvedLevels,
    defaultLevel: text(model?.reasoningDefaultLevel),
    fixedLevel,
    bindingSupported,
    source,
    declared: Array.isArray(raw) || Boolean(fixedLevel),
  }
}

export function bindingReasoningLevel(requested: string, model?: ReasoningModel | null): string {
  const info = modelReasoning(model)
  if (!requested || info.bindingSupported === false || ['off', 'none'].includes(requested.trim().toLowerCase())) return ''
  const rank = POSITIVE_REASONING_LEVELS.findIndex(value => value === requested.trim().toLowerCase())
  const exact = info.levels.find(value => value.toLowerCase() === requested.trim().toLowerCase())
  if (exact) return exact
  if (rank < 0) return ''
  if (info.fixedLevel) return info.fixedLevel
  const ranked = info.levels.map(level => ({
    level, rank: POSITIVE_REASONING_LEVELS.findIndex(value => value === level.toLowerCase()),
  })).filter(value => value.rank >= 0).sort((a, b) => a.rank - b.rank)
  const lower = ranked.filter(value => value.rank <= rank)
  return lower[lower.length - 1]?.level || ranked[0]?.level
    || (info.declared ? info.defaultLevel : '')
}
