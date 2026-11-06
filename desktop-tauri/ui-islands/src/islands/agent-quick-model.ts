import { t } from '../i18n'
import { catalogModel, type CatalogModel } from './agent-catalog'
import { blankProfile, syncClaudeTiers } from './agent-model'
import type { AgentClient, AgentModel, AgentProfile } from './agent-types'

export function quickProfile(client: AgentClient, gateway: string): AgentProfile {
  const profile = blankProfile(client, gateway)
  profile.id = `quick-${client.id}`
  profile.name = 'Agent2API'
  profile.updatedAt = Date.now()
  profile.default = client.supportsDefault
  const existing = client.configMode === 'providers'
    ? client.live.find(item => item.sourceId === `agent2api-${profile.id}` && item.kind === 'gateway' && item.protocol === profile.protocol)
    : undefined
  profile.models = existing ? structuredClone(existing.models) : []
  if (existing) profile.model = existing.model || existing.models[0]?.id || ''
  return profile
}

export function selectProfileModels(profile: AgentProfile, models: AgentModel[]): Partial<AgentProfile> {
  const selected = models.find(model => model.id === profile.model)?.id || models[0]?.id || ''
  return { models, model: selected, effort: selected === profile.model ? profile.effort : '' }
}

export function fillQuickProfile(client: AgentClient, profile: AgentProfile, catalog: CatalogModel[], preferred: string): AgentProfile {
  if (client.supportsModels && profile.models.length) return { ...profile, models: profile.models.map(model => {
    const known = catalog.find(item => item.id === model.id)
    if (!known) return model
    const inferred = catalogModel(client, known)
    return { ...model, name: model.name === model.id ? inferred.name : model.name,
      contextWindow: model.contextWindow ?? inferred.contextWindow, maxOutputTokens: model.maxOutputTokens ?? inferred.maxOutputTokens,
      supportsTools: model.supportsTools ?? inferred.supportsTools, supportsImages: model.supportsImages ?? inferred.supportsImages,
      supportsVideo: model.supportsVideo ?? inferred.supportsVideo, supportsReasoning: model.supportsReasoning ?? inferred.supportsReasoning,
      reasoningLevels: [...new Set([...inferred.reasoningLevels, ...model.reasoningLevels])] }
  }) }
  const selected = catalog.find(model => model.id === profile.model) || catalog.find(model => model.id === preferred)
    || catalog.find(model => model.supportsTools === true) || catalog[0]
  if (!selected) return profile
  return { ...profile, model: selected.id, ...(client.supportsModels ? { models: [catalogModel(client, selected)] } : {}),
    ...(client.id === 'claude-code' ? { claudeModels: syncClaudeTiers(profile.claudeModels, profile.model, selected.id) } : {}) }
}

export function quickBlockReason(client: AgentClient): string {
  if (!client.supportsGateway) return client.supportNote || t('这个客户端不能接入本网关')
  if (!client.writable) return client.readError || client.supportNote
  return ''
}
