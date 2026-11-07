import type { AssistantReadTool, Translate } from './assistant-read-types'
import { booleanArg, enumArg, number, object, page, result, safeText, stringArg, strings, text } from './assistant-read-data'
import { CAPABILITY_KEYS, capSourceOf, isTokenKey, normalizeCapabilities, type CapabilityKey, type CapabilitySource } from './model-capability'
import { modelReasoning, type ReasoningModel } from './models-reasoning'

export type ModelProviderRow = { provider?: unknown; providerLabel?: unknown }
export type AssistantModelRow = ModelProviderRow & ReasoningModel & {
  id?: unknown
  name?: unknown
  enabled?: unknown
  aliases?: unknown
  kind?: unknown
  supports_image_generation?: unknown
  supports_image_editing?: unknown
  capabilities?: unknown
  capSources?: unknown
}
export type AssistantMappingRow = ModelProviderRow & {
  alias?: unknown
  target?: unknown
  enabled?: unknown
  reasoning?: unknown
  isDefault?: unknown
  dangling?: unknown
}
export type AssistantModelView = {
  models: AssistantModelRow[]
  mappings: AssistantMappingRow[]
  providers?: ModelProviderRow[]
}

type Provider = { id: string; label: string }
const PAGE_DEFAULT = 100
const PAGE_MAX = 500

function providerDirectory(data: AssistantModelView): Provider[] {
  const providers = new Map<string, Provider>()
  for (const row of [...(data.providers || []), ...data.models, ...data.mappings]) {
    const id = text(row.provider)
    if (!id) continue
    const key = id.toLowerCase()
    const label = text(row.providerLabel) || id
    const current = providers.get(key)
    if (!current || current.label === current.id) providers.set(key, { id, label })
  }
  return [...providers.values()]
}

function resolveProvider(providers: Provider[], wanted: string, t: Translate): string {
  const key = wanted.toLowerCase()
  const byId = providers.find(provider => provider.id.toLowerCase() === key)
  if (byId) return byId.id.toLowerCase()
  const exact = providers.filter(provider => provider.label.toLowerCase() === key)
  const matches = exact.length ? exact : providers.filter(provider =>
    provider.id.toLowerCase().includes(key) || provider.label.toLowerCase().includes(key))
  if (!matches.length) throw new Error(t('没有找到提供商「{name}」，请先不填 provider 查询，或用 keyword 搜索', { name: safeText(wanted) }))
  if (matches.length > 1) throw new Error(t('提供商「{name}」匹配到多家，请使用提供商 ID：{list}', {
    name: safeText(wanted), list: matches.map(provider => `${safeText(provider.label)} (${provider.id})`).join('、'),
  }))
  return matches[0].id.toLowerCase()
}

function modelCapabilities(row: AssistantModelRow) {
  const raw = object(row.capabilities)
  // 管理接口只接受数字和布尔值；不能把 true 或字符串误读成上下文大小。
  const values: Record<string, number | boolean> = {}
  for (const key of CAPABILITY_KEYS) {
    const value = raw[key]
    if (isTokenKey(key) ? typeof value === 'number' : typeof value === 'boolean') values[key] = value as number | boolean
  }
  const capabilities = normalizeCapabilities(values)
  const capSources: Partial<Record<CapabilityKey, CapabilitySource>> = {}
  for (const key of CAPABILITY_KEYS) {
    if (!(key in capabilities)) continue
    // 自定义清单的能力值来自用户保存的覆盖，不是上游探测结果。
    const source = text(row.provider).startsWith('custom-') ? 'user' : capSourceOf(row, key)
    if (source) capSources[key] = source
  }
  const reasoning = modelReasoning(row)
  return {
    capabilities,
    capSources,
    ...(reasoning.declared ? { reasoningLevels: reasoning.levels } : {}),
    ...(reasoning.defaultLevel ? { reasoningDefaultLevel: reasoning.defaultLevel } : {}),
    ...(reasoning.fixedLevel ? { reasoningFixedLevel: reasoning.fixedLevel } : {}),
    ...(reasoning.bindingSupported !== null ? { reasoningBindingSupported: reasoning.bindingSupported } : {}),
    ...(reasoning.source ? { reasoningSource: reasoning.source } : {}),
  }
}

function modelPage<T>(rows: T[], offset: number, limit: number) {
  const start = Math.min(offset, rows.length)
  return page(rows.slice(start), start, limit, rows.length)
}

export function createModelListTool(read: () => Promise<AssistantModelView>, t: Translate): AssistantReadTool {
  return {
    id: 'list_models',
    label: t('获取模型列表'),
    kind: 'read',
    desc: '分页读取内置与自定义提供商的模型清单、展示名、启用状态、映射，以及 capabilities 中的上下文窗口、最大输出 Token、工具调用、图片输入、视频输入和推理能力。回答模型能力、上下文大小或提供商归属前先查询。capabilities 只包含已声明的值：false 是明确不支持，缺失是未知，不能当作 false 或 0；capSources 的 upstream / modelsdev / user 分别表示上游目录 / 社区数据 / 用户配置，不等于运行实测。reasoningLevels 是模型已声明的推理档位，缺失时不要猜测；固定档位、默认档位和能否配置也会随已声明数据返回。图片输入与 supports_image_generation / supports_image_editing 的生图、编辑能力不同。provider 支持 ID、展示名或唯一匹配的名称片段，重名请用 ID；keyword 可同时搜索模型与提供商名称。模型与映射各自分页，hasMore 为 true 时用 nextOffset 继续读取；不要用单页数据推算总量。',
    params: {
      type: 'object',
      properties: {
        provider: { type: 'string', description: '提供商 ID、展示名（含中文）或唯一匹配的名称片段；优先精确匹配，重名需改用 ID；不填 = 全部' },
        keyword: { type: 'string', description: '按模型 ID、模型名、映射名、提供商 ID 或展示名做子串过滤；不填 = 不过滤' },
        enabledOnly: { type: 'boolean', description: 'true = 只看已启用的模型或映射' },
        section: { type: 'string', enum: ['models', 'mappings', 'both'], description: 'models = 模型，mappings = 映射，both = 两段（默认）；两段各自分页' },
        offset: { type: 'number', description: '本页起点（从 0 开始）；翻页时传上一页的 nextOffset' },
        limit: { type: 'number', description: `每页最多条数（默认 ${PAGE_DEFAULT}，上限 ${PAGE_MAX}）；能力信息较多时会缩小本页，请以 nextOffset 为准` },
      },
    },
    run: async args => {
      const wanted = stringArg(args, 'provider', t)
      const keyword = stringArg(args, 'keyword', t).toLowerCase()
      const enabledOnly = booleanArg(args, 'enabledOnly', t) === true
      const section = enumArg(args, 'section', ['models', 'mappings', 'both'], 'both', t)
      const offset = Math.max(0, Math.round(number(args.offset) ?? 0))
      const limit = Math.max(1, Math.min(PAGE_MAX, Math.round(number(args.limit) ?? PAGE_DEFAULT)))
      const data = await read()
      const providers = providerDirectory(data)
      const provider = wanted ? resolveProvider(providers, wanted, t) : ''
      const labels = new Map(providers.map(item => [item.id.toLowerCase(), item.label]))
      const labelOf = (row: ModelProviderRow) => labels.get(text(row.provider).toLowerCase()) || text(row.providerLabel) || text(row.provider)
      const matches = (row: ModelProviderRow & { enabled?: unknown }, values: string[]) => {
        if (provider && text(row.provider).toLowerCase() !== provider) return false
        if (enabledOnly && row.enabled !== true) return false
        return !keyword || [...values, text(row.provider), labelOf(row)].join(' ').toLowerCase().includes(keyword)
      }
      const models = data.models.filter(row => matches(row, [text(row.id), text(row.name), ...strings(row.aliases)]))
      const mappings = data.mappings.filter(row => matches(row, [text(row.alias), text(row.target)]))
      const payload: Record<string, unknown> = {}
      if (section !== 'mappings') {
        payload.models = modelPage(models.map(row => ({
          provider: text(row.provider), providerLabel: labelOf(row),
          id: text(row.id), name: text(row.name), enabled: row.enabled === true,
          kind: text(row.kind) || 'chat',
          supports_image_generation: row.supports_image_generation === true,
          supports_image_editing: row.supports_image_editing === true,
          aliases: strings(row.aliases),
          ...modelCapabilities(row),
        })), offset, limit)
      }
      if (section !== 'models') {
        payload.mappings = modelPage(mappings.map(row => ({
          provider: text(row.provider), providerLabel: labelOf(row),
          alias: text(row.alias), target: text(row.target), enabled: row.enabled === true,
          isDefault: row.isDefault === true, dangling: row.dangling === true,
        })), offset, limit)
      }
      return result(payload, section === 'mappings'
        ? t('{n} 条映射', { n: mappings.length }) : t('{n} 个模型', { n: models.length }))
    },
  }
}
