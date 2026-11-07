import * as React from 'react'
import { t } from '../i18n'
import type { AgentClient, AgentModel } from './agent-types'
import { modelLevels } from './agent-model-config'

export type CatalogModel = AgentModel & { label: string }
type CatalogWindow = { workbuddyDesktop?: { getAgentModels?: (gatewayKeyId?: string) => Promise<{ models: AgentModel[] }> } }
const EMPTY_CATALOG: CatalogModel[] = []

export async function loadCatalog(gatewayKeyId?: string): Promise<CatalogModel[]> {
  const call = (window as unknown as CatalogWindow).workbuddyDesktop?.getAgentModels
  if (!call) throw new Error(t('当前环境无法读取网关模型目录'))
  const result = await call(gatewayKeyId)
  if (!Array.isArray(result?.models)) throw new Error(t('模型目录响应无效'))
  const seen = new Set<string>()
  return result.models.filter(model => {
    if (!model.id?.trim() || seen.has(model.id.toLowerCase())) return false
    seen.add(model.id.toLowerCase())
    return true
  }).map(model => ({ ...model, name: model.name || model.id,
    label: model.name && model.name !== model.id ? `${model.name} · ${model.id}` : model.id }))
}

export function catalogModel(client: AgentClient, model: CatalogModel): AgentModel {
  const { label: _, ...configured } = model
  return { ...configured, effort: '', reasoningLevels: client.supportsEffort || client.id === 'zcode' ? modelLevels(client, configured.reasoningLevels) : [] }
}

export function refreshCatalogModels(client: AgentClient, models: AgentModel[], previous: CatalogModel[], catalog: CatalogModel[]): AgentModel[] {
  return models.map(model => {
    const before = previous.find(item => item.id === model.id)
    const after = catalog.find(item => item.id === model.id)
    if (!before || !after) return model
    const old = catalogModel(client, before)
    const next = catalogModel(client, after)
    const updated = { ...model }
    for (const key of ['contextWindow', 'maxOutputTokens'] as const) {
      if (model[key] == null || model[key] === old[key]) updated[key] = next[key]
    }
    for (const key of ['supportsTools', 'supportsImages', 'supportsVideo', 'supportsReasoning'] as const) {
      if (model[key] == null || model[key] === old[key]) updated[key] = next[key]
    }
    if (model.name === old.name || model.name === model.id) updated.name = next.name
    if (JSON.stringify(model.reasoningLevels) === JSON.stringify(old.reasoningLevels)) updated.reasoningLevels = next.reasoningLevels
    if (updated.effort && !updated.reasoningLevels.includes(updated.effort)) updated.effort = ''
    return updated
  })
}

export function useAgentCatalog(gatewayKeyId?: string) {
  const [attempt, retry] = React.useReducer(value => value + 1, 0)
  const [result, setResult] = React.useState<{ key?: string; attempt: number; models: CatalogModel[]; error: string } | null>(null)
  const current = result && result.key === gatewayKeyId && result.attempt === attempt ? result : null
  React.useEffect(() => {
    let alive = true
    void loadCatalog(gatewayKeyId).then(models => {
      if (alive) setResult({ key: gatewayKeyId, attempt, models, error: '' })
    }).catch(error => {
      if (alive) setResult({ key: gatewayKeyId, attempt, models: EMPTY_CATALOG, error: error instanceof Error ? error.message : String(error) })
    })
    return () => { alive = false }
  }, [gatewayKeyId, attempt])
  return { catalog: current?.models || EMPTY_CATALOG, loading: !current, error: current?.error || '', retry }
}
