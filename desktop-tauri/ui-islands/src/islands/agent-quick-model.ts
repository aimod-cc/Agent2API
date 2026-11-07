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
  const candidates = (client.quickProfiles || client.live.filter(item => item.kind === 'gateway'
    && (item.sourceId === `agent2api-${profile.id}` || ['claude-code', 'cline'].includes(client.id))))
  const existing = candidates.find(item => item.protocol === profile.protocol)
  const byId = new Map(candidates.filter(item => item.protocol === profile.protocol).flatMap(item => item.models).map(model => [model.id, model]))
  profile.models = structuredClone([...byId.values()])
  if (existing) {
    profile.model = existing.model || existing.models[0]?.id || ''
    profile.effort = existing.effort
    if (client.id === 'claude-code') profile.claudeModels = structuredClone(existing.claudeModels)
  }
  return profile
}

export function selectProfileModels(profile: AgentProfile, models: AgentModel[]): Partial<AgentProfile> {
  const selected = models.find(model => model.id === profile.model)?.id || models[0]?.id || ''
  return { models, model: selected, effort: selected === profile.model ? profile.effort : '',
    ...(Object.values(profile.claudeModels).some(Boolean) ? { claudeModels: syncClaudeTiers(profile.claudeModels, profile.model, selected) } : {}) }
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
