export type ModelBindingModel = { id?: unknown; enabled?: unknown }
export type ModelBinding = { alias?: unknown; target?: unknown; enabled?: unknown }

export const sameModelId = (left: unknown, right: unknown): boolean =>
  String(left ?? '').trim().toLowerCase() === String(right ?? '').trim().toLowerCase()

export function activeModelBindings(
  models: readonly ModelBindingModel[], mappings: readonly ModelBinding[], alias: string, target: string,
): Array<{ alias: string; target: string }> {
  return [
    ...models.filter(model => model.enabled !== false && sameModelId(model.id, alias) && !sameModelId(model.id, target))
      .map(model => ({ alias: String(model.id), target: String(model.id) })),
    ...mappings.filter(mapping => mapping.enabled !== false && sameModelId(mapping.alias, alias)
      && !sameModelId(mapping.target, target) && !sameModelId(mapping.alias, mapping.target))
      .map(mapping => ({ alias: String(mapping.alias), target: String(mapping.target) })),
  ]
}

export function disableOtherModelBindings(
  models: ModelBindingModel[], mappings: ModelBinding[], alias: string, target: string,
): void {
  for (const model of models) {
    if (sameModelId(model.id, alias) && !sameModelId(model.id, target)) model.enabled = false
  }
  for (const mapping of mappings) {
    if (sameModelId(mapping.alias, alias) && !sameModelId(mapping.target, target)) mapping.enabled = false
  }
}

export function normalizeModelBindingStates(models: readonly ModelBindingModel[], mappings: ModelBinding[]): void {
  const claimed = new Set(models.filter(model => model.enabled !== false).map(model => String(model.id).trim().toLowerCase()))
  for (const carried of [true, false]) {
    for (const mapping of mappings.filter(mapping => models.some(model => sameModelId(model.id, mapping.target)) === carried)) {
      if (mapping.enabled === false) continue
      const alias = String(mapping.alias ?? '').trim().toLowerCase()
      if (claimed.has(alias)) mapping.enabled = false
      else claimed.add(alias)
    }
  }
}
