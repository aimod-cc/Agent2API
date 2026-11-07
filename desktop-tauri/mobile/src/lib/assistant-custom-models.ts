import { apiCall } from '../api/client'
import { t } from '../i18n'
import type { ImageToolContext } from '../../../ui-shared/assistant-image-tools'
import type { AssistantModelView } from '../../../ui-shared/assistant-read-models'
import { checkToolSignal } from '../../../ui-shared/assistant-tool-execution'

type Model = { id: string; enabled?: boolean; reasoning?: string; capabilities?: Record<string, unknown> }
type Mapping = { alias: string; target: string; enabled?: boolean; reasoning?: string }
type Provider = { id: string; name?: string; models?: Model[]; mappings?: Mapping[] }
type Draft = { models: Model[]; mappings: Mapping[] }

const same = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase()
export const isCustomModelProvider = (id: string) => id.startsWith('custom-')

function draftOf(provider: Provider): Draft {
  const models = (provider.models || []).map(model => ({ ...model, enabled: model.enabled !== false }))
  const mappings = (provider.mappings || []).map(mapping => ({ ...mapping, enabled: mapping.enabled !== false }))
  for (const model of models) {
    const legacy = mappings.find(mapping => same(mapping.alias, model.id) && same(mapping.target, model.id))
    if (!legacy) continue
    model.enabled = model.enabled && legacy.enabled
    model.reasoning = legacy.reasoning || model.reasoning
  }
  return { models, mappings: mappings.filter(mapping => !(same(mapping.alias, mapping.target) && models.some(model => same(model.id, mapping.target)))) }
}

async function providers(): Promise<Provider[]> {
  const data = await apiCall<{ providers?: Provider[] }>('/api/custom-providers')
  if (!Array.isArray(data.providers)) throw new Error(t('读取模型清单失败'))
  return data.providers
}

export async function readCustomModelView(): Promise<Required<AssistantModelView>> {
  const list = await providers()
  const models: AssistantModelView['models'] = []
  const mappings: AssistantModelView['mappings'] = []
  for (const provider of list) {
    const draft = draftOf(provider)
    for (const model of draft.models) {
      models.push({ ...model, provider: provider.id, providerLabel: provider.name || provider.id,
        kind: 'chat', aliases: draft.mappings.filter(mapping => same(mapping.target, model.id)).map(mapping => mapping.alias) })
      mappings.push({ alias: model.id, target: model.id, provider: provider.id, enabled: model.enabled,
        reasoning: model.reasoning || '', isDefault: true, dangling: false })
    }
    for (const mapping of draft.mappings) mappings.push({ ...mapping, provider: provider.id,
      isDefault: false, dangling: !draft.models.some(model => same(model.id, mapping.target)) })
  }
  return {
    models, mappings,
    providers: list.map(provider => ({ provider: provider.id, providerLabel: provider.name || provider.id })),
  }
}

async function update(id: string, mutate: (draft: Draft) => void, context?: ImageToolContext): Promise<void> {
  checkToolSignal(context)
  const provider = (await providers()).find(provider => provider.id === id)
  if (!provider) throw new Error(t('该自定义提供商已不存在（可能已被删除），请刷新后重试'))
  const draft = draftOf(provider)
  mutate(draft)
  checkToolSignal(context)
  await apiCall('/api/custom-providers/models', { method: 'POST', body: { providerId: id, ...draft } })
}

export function setCustomBinding(provider: string, alias: string, target: string, enabled: boolean, context?: ImageToolContext): Promise<void> {
  return update(provider, draft => {
    if (same(alias, target)) {
      const model = draft.models.find(model => same(model.id, target))
      if (!model) throw new Error(t('该提供商的清单里没有模型「{name}」', { name: target }))
      model.enabled = enabled
      return
    }
    const mapping = draft.mappings.find(mapping => same(mapping.alias, alias) && same(mapping.target, target))
    if (mapping) mapping.enabled = enabled
    else draft.mappings.push({ alias, target, enabled, reasoning: '' })
  }, context)
}

export function removeCustomMapping(provider: string, alias: string, target: string, context?: ImageToolContext): Promise<void> {
  return update(provider, draft => {
    if (same(alias, target)) throw new Error(t('原始 ID 的默认绑定不能删除，请关闭它的开关'))
    const index = draft.mappings.findIndex(mapping => same(mapping.alias, alias) && same(mapping.target, target))
    if (index < 0) throw new Error(t('映射「{alias} → {target}」不存在', { alias, target }))
    draft.mappings.splice(index, 1)
  }, context)
}

export function setCustomCapabilities(provider: string, id: string, patch: Record<string, unknown>, context?: ImageToolContext): Promise<void> {
  return update(provider, draft => {
    const model = draft.models.find(model => same(model.id, id))
    if (!model) throw new Error(t('该提供商的清单里没有模型「{name}」', { name: id }))
    const capabilities = { ...model.capabilities }
    for (const [key, value] of Object.entries(patch)) {
      if (value === null) delete capabilities[key]
      else capabilities[key] = value
    }
    model.capabilities = capabilities
  }, context)
}
