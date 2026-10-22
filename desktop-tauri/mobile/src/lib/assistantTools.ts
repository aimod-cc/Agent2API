/**
 * AI 助手的工具集（手机端）。
 *
 * ── 为什么手机端也要有工具表 ──────────────────────────────────
 * 后端的 `/api/assistant/chat` 有一条产品口径（见 `config::AssistantConfig::
 * system_prompt`）：**一只工具都没勾 → 不注入系统提示词**。而系统提示词正是
 * 告诉模型「你是 Agent2API 的内置助手，管的是本机模型转发配置」的那段话。
 * 手机端早先只发 messages、不带 tools，于是模型拿到的是一个没有身份设定的
 * 通用对话 —— 用户问「各账号余额」，它答成了银行账户余额。
 *
 * 所以这里移植桌面端的九只工具。执行全部走**网关的管理 API**（手机端本来就
 * 在用那些接口），不经过桌面壳的桥。
 *
 * ── 与桌面端的差异 ────────────────────────────────────────────
 *   · 执行层从 `window.workbuddyDesktop` 换成 `api/gateway` 的 HTTP 调用；
 *   · 自定义提供商（`custom-` 前缀）的读写走 `/api/custom-providers`；
 *   · 去掉「确认卡」：手机屏幕小，一次确认卡要占掉半屏。这里的做法是**只让
 *     模型看到用户在设置里勾选过的工具**，并把写操作的结果如实回报 ——
 *     与桌面端「勾选表即权限」同一套语义（未勾选的工具根本不在请求里，
 *     模型看不见也就调不到）。
 */

import {
  fetchAccounts,
  fetchAccountUsage,
  fetchManage,
  fetchUsageSnapshot,
  setAccountEnabled,
  setModelEnabled,
  type Account,
  type ManageModel,
} from '../api/gateway'
import { apiCall } from '../api/client'
import { accountName } from '../lib/format'
import { usageIndex, usageText } from '../lib/usage'
import { t } from '../i18n'

export type ToolResult = { text: string; summary: string }

export type AssistantTool = {
  id: string
  label: string
  kind: 'read' | 'write' | 'run'
  desc: string
  params: Record<string, unknown>
  run: (args: Record<string, unknown>) => Promise<ToolResult>
}

/* ─── 入参读取（模型给的东西不一定守规矩，坏值一律当缺）── */

function str(value: unknown, max = 256): string {
  return typeof value === 'string' ? value.trim().slice(0, max) : ''
}
function bool(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined
}
function num(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}
function rows(value: unknown): Array<Record<string, unknown>> {
  return Array.isArray(value)
    ? value.filter((item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object')
    : []
}

/** 分页：模型与映射共用（一页塞太多会撑爆上下文窗口） */
const PAGE_DEFAULT = 100
const PAGE_MAX = 500

function paginate<T>(items: T[], offset: number, limit: number) {
  const start = Math.min(offset, items.length)
  const end = Math.min(start + limit, items.length)
  return {
    meta: {
      total: items.length,
      offset: start,
      limit,
      returned: end - start,
      hasMore: end < items.length,
      ...(end < items.length ? { nextOffset: end } : {}),
    },
    items: items.slice(start, end),
  }
}

/** 账号点名：id 精确 > 名字精确 > 名字子串（唯一命中才算） */
function findAccount(accounts: Account[], ref: string): Account {
  const needle = ref.trim()
  if (!needle) throw new Error(t('没给账号名'))
  const byId = accounts.find(row => row.id === needle)
  if (byId) return byId
  const exact = accounts.filter(row => accountName(row) === needle)
  if (exact.length === 1) return exact[0]
  const fuzzy = accounts.filter(row => accountName(row).includes(needle))
  if (fuzzy.length === 1) return fuzzy[0]
  const candidates = (exact.length ? exact : fuzzy).map(accountName).slice(0, 6)
  if (!candidates.length) throw new Error(t('没有找到账号「{name}」', { name: needle }))
  throw new Error(
    t('「{name}」匹配到多个账号：{list}，请用完整账号名重试', {
      name: needle,
      list: candidates.join('、'),
    }),
  )
}

async function readAccounts(): Promise<Account[]> {
  const data = await fetchAccounts()
  // 全局队列 = priority 升序（后端 priority 越小越靠前）
  return (data.accounts || [])
    .slice()
    .sort((a, b) => Number(a.priority ?? 0) - Number(b.priority ?? 0))
}

/** 模型行 + 映射（自定义提供商由 `/api/models/manage` 一并给出，见后端说明） */
async function readModels(): Promise<ManageModel[]> {
  const view = await fetchManage()
  return Array.isArray(view.models) ? view.models : []
}

/* ─── 九只工具 ─────────────────────────────── */

/** 1 · 获取模型列表（只读，分页） */
const listModels: AssistantTool = {
  id: 'list_models',
  label: t('获取模型列表'),
  kind: 'read',
  desc: '分页读取各提供商的模型清单（模型 ID、所属提供商、是否启用、已挂的映射名）。回答「有哪些模型」「哪些模型在多家重复」「某个名字挂在哪」这类问题之前先调用它。\n\n结果分页：响应里带 total / offset / limit / returned / hasMore / nextOffset。hasMore 为 true 时用 nextOffset 继续取，直到取完；不要只看第一页就下结论。',
  params: {
    type: 'object',
    properties: {
      provider: { type: 'string', description: '只看某一家（提供商 id）；不填 = 全部' },
      keyword: { type: 'string', description: '按模型 ID 或映射名做子串过滤；不填 = 不过滤' },
      enabledOnly: { type: 'boolean', description: 'true = 只看已启用的模型' },
      offset: { type: 'number', description: '本页起点（从 0 开始）；翻页时传上一页的 nextOffset' },
      limit: { type: 'number', description: `每页条数（默认 ${PAGE_DEFAULT}，最多 ${PAGE_MAX}）` },
    },
  },
  run: async args => {
    const provider = str(args.provider, 64).toLowerCase()
    const keyword = str(args.keyword, 128).toLowerCase()
    const enabledOnly = bool(args.enabledOnly) === true
    const offset = Math.max(0, Math.round(num(args.offset) ?? 0))
    const limit = Math.max(1, Math.min(PAGE_MAX, Math.round(num(args.limit) ?? PAGE_DEFAULT)))

    const models = await readModels()
    const filtered = models.filter(row => {
      if (provider && str(row.provider).toLowerCase() !== provider) return false
      if (enabledOnly && row.enabled !== true) return false
      if (!keyword) return true
      const hay = [str(row.id), str(row.name), ...(row.aliases || [])].join(' ').toLowerCase()
      return hay.includes(keyword)
    })
    const page = paginate(
      filtered.map(row => ({
        provider: str(row.provider),
        providerLabel: str(row.providerLabel) || str(row.provider),
        id: str(row.id),
        enabled: row.enabled === true,
        aliases: Array.isArray(row.aliases) ? row.aliases : [],
      })),
      offset,
      limit,
    )
    return {
      text: JSON.stringify({ models: { ...page.meta, items: page.items } }),
      summary: t('{n} 个模型', { n: page.meta.total }),
    }
  },
}

/** 2 · 查询账号余额（只读） */
const listAccountBalance: AssistantTool = {
  id: 'list_account_balance',
  label: t('查询账号余额'),
  kind: 'read',
  desc: '查询各账号的余额 / 剩余额度。回答「还剩多少」「哪个账号快用完了」这类问题之前先调用它。读的是网关的余额快照（不逐个打上游）；要给某个账号做实时查询，用户需要在账号详情里点「查询余额」。',
  params: {
    type: 'object',
    properties: {
      account: { type: 'string', description: '只查某个账号（账号名或 id）；不填 = 全部账号' },
      refresh: { type: 'boolean', description: 'true = 顺带向上游做一次实时查询（较慢，会打上游）' },
    },
  },
  run: async args => {
    const ref = str(args.account, 128)
    const live = bool(args.refresh) === true

    if (live && ref) {
      const target = findAccount(await readAccounts(), ref)
      const payload = await fetchAccountUsage(target.id)
      const row = payload.results?.[0]
      const view = usageText(row)
      return {
        text: JSON.stringify({ account: accountName(target), usage: view.text, detail: view.detail }),
        summary: view.text,
      }
    }

    const snapshot = await fetchUsageSnapshot()
    const index = usageIndex(snapshot)
    const accounts = await readAccounts()
    const picked = ref ? [findAccount(accounts, ref)] : accounts
    const out = picked.map(row => {
      const view = usageText(index.get(row.id))
      return {
        account: accountName(row),
        provider: row.provider || '',
        enabled: row.enabled === true,
        usage: view.text,
        detail: view.detail,
      }
    })
    return {
      text: JSON.stringify({ accounts: out.slice(0, 60) }).slice(0, 12000),
      summary: t('{n} 个账号', { n: out.length }),
    }
  },
}

/** 3 · 启用 / 禁用模型（改动） */
const setModelEnabledTool: AssistantTool = {
  id: 'set_model_enabled',
  label: t('启用 / 禁用模型'),
  kind: 'write',
  desc: '开启或关闭某几个模型在网关上的可用性（粒度是「提供商 × 模型」）。禁用后下游请求不再路由到它。一次可以改多个。',
  params: {
    type: 'object',
    properties: {
      items: {
        type: 'array',
        description: '要改的模型列表',
        items: {
          type: 'object',
          properties: {
            provider: { type: 'string', description: '提供商 id' },
            id: { type: 'string', description: '模型 ID' },
            enabled: { type: 'boolean', description: 'true = 启用，false = 禁用' },
          },
          required: ['provider', 'id', 'enabled'],
        },
      },
    },
    required: ['items'],
  },
  run: async args => {
    const items = rows(args.items)
    const done: string[] = []
    for (const item of items) {
      const provider = str(item.provider)
      const id = str(item.id)
      const enabled = bool(item.enabled)
      if (!provider || !id || enabled === undefined) {
        throw new Error(t('参数不完整（需要 provider / id / enabled）'))
      }
      await setModelEnabled({ provider, id, enabled })
      done.push(`${enabled ? '启用' : '禁用'} ${provider}/${id}`)
    }
    return {
      text: JSON.stringify({ ok: true, changed: done }),
      summary: t('已修改 {n} 个模型', { n: done.length }),
    }
  },
}

/** 4 · 设置模型映射 ID（改动） */
const setModelMapping: AssistantTool = {
  id: 'set_model_mapping',
  label: t('设置模型映射 ID'),
  kind: 'write',
  desc: '给模型新增或删除映射名（对外名）。同一对外名可以在多个提供商各挂一条：下游用同一个名字请求，网关按账号优先级主备切换；也可以给不同家的同名模型各加前缀来精确指定某一家。一次可以改多条。删掉的映射名只是不再对外可用，不删任何模型。',
  params: {
    type: 'object',
    properties: {
      items: {
        type: 'array',
        description: '要改的映射列表',
        items: {
          type: 'object',
          properties: {
            provider: { type: 'string', description: '提供商 id' },
            id: { type: 'string', description: '上游模型 ID' },
            alias: { type: 'string', description: '要新增 / 删除的映射名（对外名）' },
            action: { type: 'string', enum: ['add', 'remove'], description: 'add = 新增，remove = 删除' },
          },
          required: ['provider', 'id', 'alias', 'action'],
        },
      },
    },
    required: ['items'],
  },
  run: async args => {
    const items = rows(args.items)
    const done: string[] = []
    for (const item of items) {
      const provider = str(item.provider)
      const target = str(item.id)
      const alias = str(item.alias, 128)
      const action = str(item.action) === 'remove' ? 'remove' : 'add'
      if (!provider || !target || !alias) {
        throw new Error(t('参数不完整（需要 provider / id / alias）'))
      }
      if (action === 'remove') {
        await apiCall('/api/models/mappings/remove', {
          method: 'POST',
          body: { provider, target, alias },
        })
        done.push(`删除 ${alias} → ${provider}/${target}`)
      } else {
        await apiCall('/api/models/mappings', {
          method: 'POST',
          body: { provider, target, alias },
        })
        done.push(`新增 ${alias} → ${provider}/${target}`)
      }
    }
    return {
      text: JSON.stringify({ ok: true, changed: done }),
      summary: t('已改动 {n} 条映射', { n: done.length }),
    }
  },
}

/** 5 · 修改模型能力（改动） */
const CAPABILITY_KEYS = [
  'maxInputTokens',
  'maxOutputTokens',
  'supportsToolCall',
  'supportsImages',
  'supportsVideo',
  'supportsReasoning',
] as const

const CAPABILITY_LABELS: Record<string, string> = {
  maxInputTokens: t('上下文窗口'),
  maxOutputTokens: t('最大输出 Token'),
  supportsToolCall: t('工具调用'),
  supportsImages: t('图片识别'),
  supportsVideo: t('视频识别'),
  supportsReasoning: t('深度推理'),
}

const setModelCapability: AssistantTool = {
  id: 'set_model_capability',
  label: t('修改模型能力'),
  kind: 'write',
  desc: '修改某个模型对下游声明的能力位：上下文窗口、最大输出 Token、工具调用 / 图片 / 视频 / 深度推理四个布尔位。只传要改的键；布尔位给 null 表示恢复上游原值。',
  params: {
    type: 'object',
    properties: {
      provider: { type: 'string', description: '提供商 id' },
      id: { type: 'string', description: '模型 ID' },
      capabilities: {
        type: 'object',
        description: '要改的能力位（只写要改的键）',
        properties: {
          maxInputTokens: { type: ['number', 'null'], description: '上下文窗口（token 数）' },
          maxOutputTokens: { type: ['number', 'null'], description: '最大输出 token 数' },
          supportsToolCall: { type: ['boolean', 'null'], description: '是否支持工具调用' },
          supportsImages: { type: ['boolean', 'null'], description: '是否支持图片识别' },
          supportsVideo: { type: ['boolean', 'null'], description: '是否支持视频识别' },
          supportsReasoning: { type: ['boolean', 'null'], description: '是否支持深度推理' },
        },
      },
    },
    required: ['provider', 'id', 'capabilities'],
  },
  run: async args => {
    const provider = str(args.provider)
    const id = str(args.id)
    const raw = (args.capabilities && typeof args.capabilities === 'object'
      ? args.capabilities
      : {}) as Record<string, unknown>
    const caps: Record<string, unknown> = {}
    for (const key of CAPABILITY_KEYS) {
      if (!(key in raw)) continue
      const value = raw[key]
      if (value === null) {
        caps[key] = null
        continue
      }
      if (key === 'maxInputTokens' || key === 'maxOutputTokens') {
        const value2 = num(value)
        if (value2 === undefined) throw new Error(t('{key} 需要数字', { key: CAPABILITY_LABELS[key] }))
        caps[key] = Math.round(value2)
      } else {
        const value2 = bool(value)
        if (value2 === undefined) {
          throw new Error(t('{key} 需要 true / false / null', { key: CAPABILITY_LABELS[key] }))
        }
        caps[key] = value2
      }
    }
    if (!provider || !id || !Object.keys(caps).length) {
      throw new Error(t('参数不完整（需要 provider / id / capabilities）'))
    }
    await apiCall('/api/models/capabilities', {
      method: 'POST',
      body: { provider, id, capabilities: caps },
    })
    return {
      text: JSON.stringify({ ok: true, provider, id, capabilities: caps }),
      summary: t('已修改 {n} 项能力', { n: Object.keys(caps).length }),
    }
  },
}

/** 6 · 发起模型测试（消耗额度） */
const testModel: AssistantTool = {
  id: 'test_model',
  label: t('发起模型测试'),
  kind: 'run',
  desc: '对某个模型发一次真实的对话请求，回报连通性、耗时与首字延迟。会消耗该账号的额度。回答「这个模型通不通 / 快不快」时用它。',
  params: {
    type: 'object',
    properties: {
      provider: { type: 'string', description: '提供商 id' },
      id: { type: 'string', description: '模型 ID' },
      prompt: { type: 'string', description: '测试用的用户提示词；不填 = 默认一句问候' },
    },
    required: ['provider', 'id'],
  },
  run: async args => {
    const payload: Record<string, unknown> = {
      provider: str(args.provider),
      model: str(args.id),
    }
    const prompt = str(args.prompt, 4000)
    if (prompt) payload.prompt = prompt
    const result = (await apiCall('/api/models/test', { method: 'POST', body: payload })) as
      | Record<string, unknown>
      | undefined
    const status = String(result?.status ?? '')
    const ok = result?.success === true
    const slim = {
      ok,
      status,
      error: result?.error ?? null,
      reply: String(result?.reply ?? '').slice(0, 1500),
      duration_ms: result?.duration_ms ?? null,
      ttfb_ms: result?.ttfb_ms ?? null,
      account: result?.account_name ?? null,
    }
    return {
      text: JSON.stringify(slim).slice(0, 6000),
      summary: ok
        ? t('连通 · {ms}ms', { ms: String(slim.duration_ms ?? '') })
        : t('失败 · {status}', { status: status || '—' }),
    }
  },
}

/** 7 · 调整提供商顺位（改动）—— 整组移动 */
const setProviderOrder: AssistantTool = {
  id: 'set_provider_order',
  label: t('调整提供商顺位'),
  kind: 'write',
  desc: '调整某个提供商在账号全局队列里的**整组顺位**（第几组），决定路由时先试哪一家。这一家的所有账号会作为一整块移动。位置从 1 开始数。',
  params: {
    type: 'object',
    properties: {
      provider: { type: 'string', description: '提供商 id' },
      position: { type: 'number', description: '目标组顺位（1 = 最前）' },
    },
    required: ['provider', 'position'],
  },
  run: async args => {
    const provider = str(args.provider).toLowerCase()
    const position = num(args.position)
    if (!provider || position === undefined) {
      throw new Error(t('参数不完整（需要 provider / position）'))
    }
    const accounts = await readAccounts()
    // 按家分组，保持组内原有相对顺序
    const groups: Array<{ provider: string; ids: string[] }> = []
    for (const row of accounts) {
      const id = str(row.id)
      if (!id) continue
      const key = str(row.provider).toLowerCase()
      const last = groups[groups.length - 1]
      if (last && last.provider === key) last.ids.push(id)
      else groups.push({ provider: key, ids: [id] })
    }
    const from = groups.findIndex(group => group.provider === provider)
    if (from < 0) throw new Error(t('没有找到提供商「{provider}」的账号', { provider }))
    const to = Math.max(0, Math.min(groups.length - 1, Math.round(position) - 1))
    const [moved] = groups.splice(from, 1)
    groups.splice(to, 0, moved)
    await apiCall('/api/accounts/reorder', {
      method: 'POST',
      body: { queue: groups.flatMap(group => group.ids) },
    })
    return {
      text: JSON.stringify({ ok: true, provider, from: from + 1, to: to + 1, moved: moved.ids.length }),
      summary: t('已移到第 {n} 顺位', { n: to + 1 }),
    }
  },
}

/** 8 · 调整账号优先级（改动） */
const setAccountOrder: AssistantTool = {
  id: 'set_account_order',
  label: t('调整账号优先级'),
  kind: 'write',
  desc: '把某个账号移动到账号全局队列的第 N 顺位（1 = 最前）。位置决定主备与切换顺序：越靠前越先被使用。',
  params: {
    type: 'object',
    properties: {
      account: { type: 'string', description: '账号名或账号 id' },
      position: { type: 'number', description: '目标顺位（1 = 最前）' },
    },
    required: ['account', 'position'],
  },
  run: async args => {
    const ref = str(args.account, 128)
    const position = num(args.position)
    if (!ref || position === undefined) {
      throw new Error(t('参数不完整（需要 account / position）'))
    }
    const accounts = await readAccounts()
    const target = findAccount(accounts, ref)
    const ids = accounts.map(row => row.id).filter(Boolean)
    const from = ids.indexOf(target.id)
    if (from < 0) throw new Error(t('账号不在当前队列里'))
    ids.splice(from, 1)
    const to = Math.max(0, Math.min(ids.length, Math.round(position) - 1))
    ids.splice(to, 0, target.id)
    await apiCall('/api/accounts/reorder', { method: 'POST', body: { queue: ids } })
    return {
      text: JSON.stringify({ ok: true, account: accountName(target), from: from + 1, to: to + 1 }),
      summary: t('已移到第 {n} 顺位', { n: to + 1 }),
    }
  },
}

/** 9 · 启用 / 禁用账号（高危） */
const setAccountEnabledTool: AssistantTool = {
  id: 'set_account_enabled',
  label: t('启用 / 禁用账号'),
  kind: 'write',
  desc: '开启或关闭某几个账号的转发资格。禁用只是把它移出队列（余额仍可查），但如果某条线路只剩它，禁用会直接断供。一次可以改多个。',
  params: {
    type: 'object',
    properties: {
      items: {
        type: 'array',
        description: '要改的账号列表',
        items: {
          type: 'object',
          properties: {
            account: { type: 'string', description: '账号名或账号 id' },
            enabled: { type: 'boolean', description: 'true = 启用，false = 禁用' },
          },
          required: ['account', 'enabled'],
        },
      },
    },
    required: ['items'],
  },
  run: async args => {
    const items = rows(args.items)
    const accounts = await readAccounts()
    const done: string[] = []
    for (const item of items) {
      const enabled = bool(item.enabled)
      if (enabled === undefined) throw new Error(t('参数不完整（需要 account / enabled）'))
      const target = findAccount(accounts, str(item.account, 128))
      await setAccountEnabled(target.id, enabled)
      done.push(`${enabled ? '启用' : '禁用'} ${accountName(target)}`)
    }
    return {
      text: JSON.stringify({ ok: true, changed: done }),
      summary: t('已修改 {n} 个账号', { n: done.length }),
    }
  },
}

/** 全部工具（顺序 = 设置页清单顺序：只读在前，改动在后，高危最后） */
export const ASSISTANT_TOOLS: AssistantTool[] = [
  listModels,
  listAccountBalance,
  setModelEnabledTool,
  setModelMapping,
  setModelCapability,
  testModel,
  setProviderOrder,
  setAccountOrder,
  setAccountEnabledTool,
]

export function toolById(id: string): AssistantTool | undefined {
  return ASSISTANT_TOOLS.find(tool => tool.id === id)
}

/** 按勾选表组装 OpenAI tools 数组（顺序固定 = ASSISTANT_TOOLS 的顺序） */
export function toolsPayload(enabled: Record<string, boolean>): Array<Record<string, unknown>> {
  return ASSISTANT_TOOLS.filter(tool => enabled[tool.id]).map(tool => ({
    type: 'function',
    function: { name: tool.id, description: tool.desc, parameters: tool.params },
  }))
}
