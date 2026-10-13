/**
 * Agent2API · 自定义提供商的模型清单数据源（模型管理页选中自定义家时的取数与写入）。
 *
 * 从 ui/models-custom-source.js 逐字搬进 TS，只把 window 接口收窄成显式类型。
 * 三件事：
 *
 *   1. **取数**：把该家记录（`models` / `mappings` 两个数组）适配成与
 *      `GET /api/models/manage` 同形的 `{models, mappings}` —— 于是表格渲染、搜索、
 *      映射 chip、思考等级、能力位两列、列设置全部照用，一行都不用为自定义家另写；
 *   2. **写入**：该家没有逐条接口，只有「整表替换」（`POST /api/custom-providers/models`）。
 *      每次操作都「读当前记录 → 应用这一处改动 → 提交全量 → 刷新目录缓存」，于是界面上
 *      的每一步（开关 / 别名 / 思考等级 / 能力位 / 移除）都**立即生效**，没有保存按钮、没有草稿态；
 *   3. **拉取**：`fetch-models` 由服务端代拉上游清单，新模型并入后同样走整表提交。
 *
 * ── 为什么在模型管理页里做适配，而不是把自定义家并进后端 manage_view ──
 * 后端 `core/custom_providers.rs` 写明了「modelRules 的 disabled/hidden **不适用于**
 * 自定义家」：内置家的启停规则挂在全局 modelRules 上（映射是全局表、同一个别名可在多家
 * 各建一条做主备），自定义家的清单本身就是用户逐条登记、存在提供商记录里的数组。两套
 * 存储语义不同，合并要重构存储，而收益只是「少一层前端适配」。所以这里做的是**适配**。
 *
 * ── 同名映射（alias == target）是早期数据形态 ──────────────
 * 早期版本会把「原始 ID 的默认绑定」也写进 mappings 数组。表格里默认绑定由模型行自己
 * 生成（开关 = `models[].enabled`、等级 = `models[].reasoning`），所以：读取时把它们
 * **合并进默认绑定**，不在映射列另占一格；提交时**不再写回**（见 draftOf 的剔除）——
 * 否则用户改了默认绑定的开关，下一次读取会被那条旧条目覆盖回去。
 *
 * 目录缓存（`wbProviders.customList`）是唯一的取数来源：它由 providers.js 统一拉取与
 * 刷新，本模块不自己发 GET，只在写入成功后 `refreshCustom()` 一次。
 */

import { CAPABILITY_KEYS, normalizeCapabilities } from './model-capability'
import { t } from '../i18n'

/* ─── 对外数据类型（表格同形数据，models-page.tsx 也读这几个类型）────── */

/** 一行模型（后端 `catalog::manage_view` 的 models[] 与本模块的适配结果同形） */
export type ManageModel = {
  id: string
  name: string
  provider: string
  providerLabel: string
  /** 'remote' / 'builtin' / 'manual'；自定义家留空串（那两列按视图隐藏） */
  source: string
  enabled: boolean
  aliases: string[]
  /** 家级字段：这家清单的最近拉取时刻（毫秒，0 = 未知）；自定义家没有 */
  refreshedAt?: unknown
  credits?: unknown
  /**
   * 对下游声明的能力位（生效值：清单原值 + 用户覆盖）。内置家由后端给
   * 「五键齐全、null = 未声明」的形状；自定义家由本模块适配成**稀疏表**
   * （没填的键不出现）—— 读侧统一走 `model-capability` 的归一/判定，
   * 两种形状不会分叉。
   */
  capabilities?: unknown
  /** 被用户覆盖过的能力键（只有内置家给；自定义家的一切都是用户填的，恒空） */
  capOverrides?: unknown
  /**
   * 模型**自己能配哪些思考档位**（可选键：只有给过依据的家才有 —— 目前是
   * ZCode 的 GLM-5.3 家族，数据来自该家官方目录）。**只读**，不参与保存：
   * 用户能改的是「某条映射用哪一档」（`ManageMapping.reasoning`），
   * 改不了上游认哪几个档位。
   */
  reasoningLevels?: unknown
  /** 默认思考档位（与 `reasoningLevels` 同源；缺失 = 未声明） */
  reasoningDefaultLevel?: unknown
}

/** 一条映射（含表格现造的默认绑定：alias == target） */
export type ManageMapping = {
  alias: string
  target: string
  provider: string
  enabled: boolean
  reasoning: string
  isDefault?: boolean
  dangling?: boolean
  carried?: boolean
}

/** 一份同形数据（内置家来自 /api/models/manage，自定义家由 buildView 适配） */
export type ManageView = {
  models?: ManageModel[]
  mappings?: ManageMapping[]
  /** 候选思考等级（只有内置家那份响应带；缺了走 models-reasoning.ts 的兜底表） */
  reasoningLevels?: string[]
}

/** 目录缓存里的一条自定义提供商记录（providers.js 的 customList()） */
export type CustomProviderRecord = {
  id?: string
  name?: string
  createdAt?: unknown
  /** `capabilities` 是该条模型的能力位覆盖（可选稀疏表，键名见 `model-capability`） */
  models?: Array<{ id?: unknown; enabled?: unknown; reasoning?: unknown; capabilities?: unknown }>
  mappings?: Array<{ alias?: unknown; target?: unknown; enabled?: unknown; reasoning?: unknown }>
}

/** 目录模块（providers.js）里本模块用到的那几个方法 */
type ProvidersBridge = {
  customList?: () => CustomProviderRecord[]
  refreshCustom?: () => Promise<unknown>
  customRequest?: (method: string, path: string, body?: unknown) => Promise<{ models?: unknown } | null | undefined>
}

/** 提交入口的入参：三态语义与后端一致（字段不给 = 不改） */
type BindingPatch = { enabled?: boolean; reasoning?: string }

/* ─── 运行期读 window ─────────────────────────── */

/**
 * 目录模块（providers.js）在 index.html 里排在本岛**之后**加载（它要给账号页的几个
 * 模块共用，位置靠后），所以只能**运行期**取 —— 加载期解构会拿到 undefined，表现是
 * 自定义家整块静默失效（左栏永远「还没有自定义提供商」）。
 */
type SharedWindow = {
  wbProviders?: ProvidersBridge
  /** 本模块注册的对外契约；models-fetch-modal.tsx 的批量导入读它的 addModels */
  wbModelsCustom?: CustomSourceApi
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 纯函数 ─────────────────────────────────── */

/** 归一化判重口径：去空白 + 大小写不敏感（与后端一致） */
const norm = (value: unknown): string => String(value ?? '').trim().toLowerCase()
const same = (left: unknown, right: unknown): boolean => norm(left) === norm(right)

/** `custom-` 前缀判据（与后端 `is_custom_provider_id` 一致） */
export function isCustom(id: unknown): boolean {
  return typeof id === 'string' && id.startsWith('custom-')
}

/** 全部自定义提供商（目录缓存的原始记录，按 createdAt 升序） */
export function list(): CustomProviderRecord[] {
  return (shared().wbProviders?.customList?.() || []).filter(provider => isCustom(provider?.id))
}

/** 取一家（不存在返回 null —— 可能刚被别处删掉） */
export function record(id: string): CustomProviderRecord | null {
  return list().find(provider => provider.id === id) || null
}

/**
 * 记录 → 可提交的草稿（`{models, mappings}`）。
 *
 * 字段归一化与当年的 custom-models-modal 那份深拷贝同口径：缺省 enabled 视作开、
 * reasoning 视作空串；同名映射并入默认绑定后从 mappings 里剔除（见文件头）。
 * 每次提交都从**目录缓存的当前值**重建，所以本函数是幂等的：连点两次开关，第二次读到
 * 的就是第一次提交后的值。
 */
type DraftModel = { id: string; enabled: boolean; reasoning: string; capabilities?: Record<string, number | boolean> }
type DraftMapping = { alias: string; target: string; enabled: boolean; reasoning: string }

/**
 * 记录条目的能力位 → 可提交的稀疏对象（空表给 `undefined`：整表提交里不带
 * 这个键，与后端「空表不落键」的口径一致）。
 */
function sparseCapabilities(value: unknown): Record<string, number | boolean> | undefined {
  const normalized = normalizeCapabilities(value)
  const result: Record<string, number | boolean> = {}
  for (const key of CAPABILITY_KEYS) {
    const item = normalized[key]
    if (typeof item === 'number' || typeof item === 'boolean') result[key] = item
  }
  return Object.keys(result).length ? result : undefined
}

function draftOf(provider: CustomProviderRecord | null): { models: DraftModel[]; mappings: DraftMapping[] } {
  const models = (Array.isArray(provider?.models) ? provider.models : [])
    .map(model => {
      const draft: DraftModel = {
        id: String(model?.id ?? '').trim(),
        enabled: model?.enabled !== false,
        reasoning: typeof model?.reasoning === 'string' ? model.reasoning : '',
      }
      // 能力位覆盖**必须原样带回**：整表替换的语义下，草稿漏了它，用户填过的
      // 能力就会被一次「切开关」的提交顺手清掉
      draft.capabilities = sparseCapabilities(model?.capabilities)
      return draft
    })
    .filter(model => model.id)
  const mappings = (Array.isArray(provider?.mappings) ? provider.mappings : [])
    .map(mapping => ({
      alias: String(mapping?.alias ?? '').trim(),
      target: String(mapping?.target ?? '').trim(),
      enabled: mapping?.enabled !== false,
      reasoning: typeof mapping?.reasoning === 'string' ? mapping.reasoning : '',
    }))
    .filter(mapping => mapping.alias && mapping.target)

  for (const model of models) {
    const legacy = mappings.find(mapping => same(mapping.alias, model.id) && same(mapping.target, model.id))
    if (!legacy) continue
    model.enabled = model.enabled && legacy.enabled
    model.reasoning = legacy.reasoning || model.reasoning
  }
  const cleaned = mappings.filter(mapping =>
    !(same(mapping.alias, mapping.target) && models.some(model => same(model.id, mapping.target))))
  return { models, mappings: cleaned }
}

/**
 * 目录记录 → 表格同形数据。返回 `null` = 这家已不存在（被别处删掉了）。
 *
 * 形状与后端 `catalog::manage_view` 逐字对齐，模型管理页的渲染只认这些字段：
 *   · models: `{id, name, provider, providerLabel, source, enabled, aliases,
 *     capabilities, capOverrides}`
 *     —— 自定义家没有倍率与来源概念，`source` 留空串（选中自定义家时那两列本来就按
 *     视图隐藏，见 models-page 的 visibleColumns）；能力位是**记录自带**的稀疏表，
 *     `capOverrides` 恒空（没有「上游原值」这回事，见 `setCapabilities`）；
 *   · mappings: `{alias, target, provider, enabled, reasoning, isDefault, dangling, carried}`
 *     —— 默认绑定（alias == target）由模型行现造，与内置家一致。
 */
export function buildView(id: string): { models: ManageModel[]; mappings: ManageMapping[] } | null {
  const provider = record(id)
  if (!provider) return null
  const draft = draftOf(provider)
  const name = provider.name || id
  const models: ManageModel[] = []
  const mappings: ManageMapping[] = []
  for (const model of draft.models) {
    models.push({
      id: model.id,
      name: '',
      provider: id,
      providerLabel: name,
      source: '',
      enabled: model.enabled,
      aliases: [],
      // 能力位是**记录自带的**（用户填的），没有「上游原值」这回事 ——
      // 覆盖标记恒空，弹窗按「未声明 / 已填」两态呈现
      capabilities: model.capabilities || {},
      capOverrides: [],
    })
    mappings.push({
      alias: model.id,
      target: model.id,
      provider: id,
      enabled: model.enabled,
      reasoning: model.reasoning,
      isDefault: true,
      dangling: false,
      carried: true,
    })
  }
  for (const mapping of draft.mappings) {
    const row = models.find(model => same(model.id, mapping.target))
    mappings.push({
      alias: mapping.alias,
      target: mapping.target,
      provider: id,
      enabled: mapping.enabled,
      reasoning: mapping.reasoning,
      isDefault: false,
      // 目标不在清单里 = 这条映射挂不到任何一行（表格底部的「未挂载」分组）。
      // 自定义家不该出现这种条目（移除模型会连带删映射），但手改过的数据文件或早期
      // 版本可能留下它 —— 照实列出来，比让用户找不到它强。
      dangling: !row,
      carried: Boolean(row),
    })
    if (row) row.aliases.push(mapping.alias)
  }
  return { models, mappings }
}

/* ─── 写入 ───────────────────────────────────── */

/**
 * 提交入口：读当前记录 → 交给 `mutate` 改草稿 → 整表提交 → 刷新目录缓存。
 * `mutate` 的返回值原样透传给调用方（用于拼 toast 文案）。
 */
async function submit<T>(id: string, mutate: (draft: ReturnType<typeof draftOf>) => T): Promise<T> {
  const providers = shared().wbProviders
  if (!providers?.customRequest) throw new Error(t('目录模块未就绪'))
  const provider = record(id)
  if (!provider) throw new Error(t('该自定义提供商已不存在（可能已被删除），请刷新后重试'))
  const draft = draftOf(provider)
  const result = mutate(draft)
  await providers.customRequest('POST', '/api/custom-providers/models', {
    providerId: id,
    models: draft.models,
    mappings: draft.mappings,
  })
  // 目录缓存先刷（左栏计数、账号页弹窗的「N 个模型」都读它），再返回
  await providers.refreshCustom?.()
  return result
}

/**
 * 一条绑定的开关 / 思考等级 —— 内置家那两个调用点（chip 开关、等级弹窗）的自定义家实现，
 * 语义逐条对齐：
 *   · `alias == target` 且清单里有这个模型 = **默认绑定**：改模型自己的字段；
 *   · 否则按 (alias, target) 找已有映射，找到就改、找不到就**新增**（添加映射）；
 *   · `patch.enabled === false` 且映射不存在 = 调用方在关一条不存在的映射，报错。
 *
 * `patch` 里没给的字段保持现值（与后端「三态」协议同一取向：不带 = 不改）。
 */
export async function setBinding(id: string, alias: string, target: string, patch: BindingPatch = {}): Promise<void> {
  const { enabled, reasoning } = patch
  await submit(id, draft => {
    if (same(alias, target)) {
      const model = draft.models.find(item => same(item.id, target))
      if (!model) throw new Error(t('该提供商的清单里没有模型「{name}」', { name: target }))
      if (enabled !== undefined) model.enabled = Boolean(enabled)
      if (reasoning !== undefined) model.reasoning = String(reasoning ?? '')
      return null
    }
    const existing = draft.mappings.find(item => same(item.alias, alias) && same(item.target, target))
    if (existing) {
      if (enabled !== undefined) existing.enabled = Boolean(enabled)
      if (reasoning !== undefined) existing.reasoning = String(reasoning ?? '')
      return null
    }
    if (enabled === false) throw new Error(t('映射「{alias} → {target}」不存在', { alias, target }))
    draft.mappings.push({
      alias,
      target,
      // 走到这里 enabled 只剩 true / undefined（false 上面已经抛错），缺省即启用
      enabled: enabled ?? true,
      reasoning: String(reasoning ?? ''),
    })
    return null
  })
}

/** 删除一条映射（默认绑定不可删 —— 界面上它没有删除按钮） */
export async function removeMapping(id: string, alias: string, target: string): Promise<void> {
  await submit(id, draft => {
    const before = draft.mappings.length
    draft.mappings = draft.mappings.filter(item => !(same(item.alias, alias) && same(item.target, target)))
    if (draft.mappings.length === before) throw new Error(t('映射「{alias} → {target}」不存在', { alias, target }))
    return null
  })
}

/** 登记一个模型（手动添加走它，判重口径与内置家一致） */
export async function addModel(id: string, modelId: string): Promise<void> {
  const value = String(modelId ?? '').trim()
  if (!value) throw new Error(t('请填写模型 ID'))
  await submit(id, draft => {
    if (draft.models.some(model => same(model.id, value))) {
      throw new Error(t('模型「{name}」已存在（忽略大小写判重）', { name: value }))
    }
    draft.models.push({ id: value, enabled: true, reasoning: '' })
    return null
  })
}

/**
 * 批量登记（「获取模型」弹窗的导入按钮）：已存在的跳过，返回实际新增条数。
 * 与 `fetchModels` 的差别只在**来源** —— 那个是服务端代拉上游后全量并入，这个是用户
 * 在弹窗里逐个勾出来的子集。
 */
export async function addModels(id: string, modelIds: readonly unknown[]): Promise<number> {
  const values = (Array.isArray(modelIds) ? modelIds : [])
    .map(value => String(value ?? '').trim())
    .filter(Boolean)
  if (!values.length) throw new Error(t('没有选中任何模型'))
  return submit(id, draft => {
    let added = 0
    for (const value of values) {
      if (draft.models.some(model => same(model.id, value))) continue
      draft.models.push({ id: value, enabled: true, reasoning: '' })
      added++
    }
    return added
  })
}

/**
 * 覆盖某条模型的能力位（自定义家的实现；内置家走 `writeCapabilities` 的
 * `POST /api/models/capabilities`）。
 *
 * 自定义家的能力存在**提供商记录的 models[] 条目**里（与启停 / 思考等级同一处
 * 存储），于是写入照那条既有路径：读当前记录 → 改这一条的字段 → 整表提交。
 * `patch` 的三态与后端一致：键缺失 = 不改、`null` = 清除这一项、有值 = 覆盖；
 * 清到一项不剩时整份 capabilities 键一并摘掉（与后端「空表不落键」同一口径）。
 */
export async function setCapabilities(
  id: string,
  modelId: string,
  patch: Record<string, number | boolean | null>,
): Promise<void> {
  await submit(id, draft => {
    const model = draft.models.find(item => same(item.id, modelId))
    if (!model) throw new Error(t('该提供商的清单里没有模型「{name}」', { name: modelId }))
    const next: Record<string, number | boolean> = { ...(model.capabilities || {}) }
    for (const [key, value] of Object.entries(patch)) {
      if (value === null || value === undefined) delete next[key]
      else next[key] = value
    }
    model.capabilities = Object.keys(next).length ? next : undefined
    return null
  })
}

/** 移除一个模型：连带删掉 target 指向它的映射，返回删了几条（用于 toast） */
export async function removeModel(id: string, modelId: string): Promise<{ removedMappings: number }> {
  return submit(id, draft => {
    const before = draft.mappings.length
    draft.mappings = draft.mappings.filter(mapping => !same(mapping.target, modelId))
    const removedMappings = before - draft.mappings.length
    draft.models = draft.models.filter(model => !same(model.id, modelId))
    return { removedMappings }
  })
}

/**
 * 从上游拉清单并并入（对应内置家的「刷新模型清单」）：服务端代拉、不落盘，已存在的 id
 * 跳过（忽略大小写），只提交新增的那些。
 */
export async function fetchModels(id: string): Promise<{ total: number; added: number }> {
  const providers = shared().wbProviders
  if (!providers?.customRequest) throw new Error(t('目录模块未就绪'))
  const data = await providers.customRequest('POST', '/api/custom-providers/fetch-models', { providerId: id })
  const ids = (Array.isArray(data?.models) ? data.models : [])
    .map(value => String(value ?? '').trim())
    .filter(Boolean)
  const added = await submit(id, draft => {
    let count = 0
    for (const value of ids) {
      if (draft.models.some(model => same(model.id, value))) continue
      draft.models.push({ id: value, enabled: true, reasoning: '' })
      count++
    }
    return count
  })
  return { total: ids.length, added: Number(added) || 0 }
}

/* ─── 对外契约：window.wbModelsCustom ───────────── */

export type CustomSourceApi = {
  isCustom: typeof isCustom
  list: typeof list
  record: typeof record
  buildView: typeof buildView
  setBinding: typeof setBinding
  removeMapping: typeof removeMapping
  addModel: typeof addModel
  addModels: typeof addModels
  removeModel: typeof removeModel
  setCapabilities: typeof setCapabilities
  fetchModels: typeof fetchModels
}

/**
 * 仍然挂到 window 上：**不是**只被本页用 ——
 * `ui-islands/src/islands/models-fetch-modal.tsx` 的「导入选中的 N 个模型」直接调
 * `wbModelsCustom.addModels(providerId, values)`（grep 结果见交付说明）。整份接口逐字
 * 保留（含本页当前没调到的 fetchModels），免得别处再引用时静默失效。
 *
 * 刻意不写 `declare global`：这些是多个岛共用的桥，各岛各 declare 一份会因同名属性
 * 类型不一致直接报 TS2717（并行迁移时必然互相撞车）。
 */
shared().wbModelsCustom = {
  isCustom,
  list,
  record,
  buildView,
  setBinding,
  removeMapping,
  addModel,
  addModels,
  removeModel,
  setCapabilities,
  fetchModels,
}
