export function sortRoutingModels<T>(models: readonly T[], modelId: (model: T) => string, lastUsed?: Record<string, number>): T[] {
  const usage = new Map<string, number>()
  for (const [model, at] of Object.entries(lastUsed || {})) {
    if (!Number.isFinite(at) || at <= 0) continue
    const key = model.trim().toLowerCase()
    usage.set(key, Math.max(usage.get(key) || 0, at))
  }
  return models.map(model => {
    const key = modelId(model).trim().toLowerCase()
    return { model, key, at: usage.get(key) || 0 }
  }).sort((left, right) => right.at - left.at || (left.key < right.key ? -1 : left.key > right.key ? 1 : 0))
    .map(item => item.model)
}
