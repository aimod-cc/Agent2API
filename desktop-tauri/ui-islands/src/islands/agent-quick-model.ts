import { t } from '../i18n'
import type { CatalogModel } from './agent-catalog'
import { blankProfile, syncClaudeTiers } from './agent-model'
import type { AgentClient, AgentGatewayKey, AgentModel, AgentProfile } from './agent-types'

export function quickProfile(client: AgentClient, gateway: string): AgentProfile {
  const profile = blankProfile(client,gateway)
  profile.id = `quick-${client.id}`
  profile.name = 'Agent2API'
  profile.updatedAt = Date.now()
  profile.default = client.supportsDefault
  return profile
}

export function quickModels(catalog: CatalogModel[], key?: AgentGatewayKey): CatalogModel[] {
  const includes = (allowed: string[] | undefined, value: string) => !allowed?.length || allowed.some(id => id.toLowerCase() === value.toLowerCase())
  return catalog.filter(model => includes(key?.allowedModels,model.id) && model.providers.some(provider => includes(key?.allowedProviders,provider)))
}

export function fillQuickProfile(client: AgentClient, profile: AgentProfile, catalog: CatalogModel[], preferred: string, previous = ''): AgentProfile {
  const known = catalog.find(model => model.id === profile.model)
  const selected = known || catalog.find(model => model.id === preferred) || catalog.find(model => model.supportsTools === true) || catalog[0]
  if (!selected) return profile
  const { label: _, provider: __, providers: ___, ...model } = selected
  const configured: AgentModel = { ...model, effort:'', reasoningLevels:client.supportsEffort ? model.reasoningLevels : [] }
  return { ...profile, model:selected.id, ...(client.supportsModels ? { models:[configured] } : {}),
    ...(client.id === 'claude-code' ? { claudeModels: previous ? syncClaudeTiers(profile.claudeModels, previous, selected.id)
      : { opus:selected.id, sonnet:selected.id, haiku:selected.id, subagent:selected.id } } : {}) }
}

export function quickBlockReason(client: AgentClient): string {
  if (!client.supportsGateway) return client.supportNote || t('这个客户端不能接入本网关')
  if (!client.writable) return client.readError || client.supportNote
  return ''
}
