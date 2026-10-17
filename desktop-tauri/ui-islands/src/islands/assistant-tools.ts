/**
 * AI 助手的工具集：定义（给模型看的 JSON Schema）+ 执行器（调既有的管理接口）。
 *
 * ── 为什么工具全在前端 ───────────────────────────────────────
 * 每只工具做的都是界面本来就会做的事（模型启停、映射、能力位、顺序、余额、
 * 模型测试），而这些动作在**桥**上已有现成入口（`window.workbuddyDesktop`
 * 的同名方法，桌面壳与 headless 面板各有一份实现）。工具放在这里 = 加一只工具
 * 不用动 Rust；执行走既有桥 = 与界面按钮走同一条链（校验、提示、刷新口径一致）。
 * 后端只负责把 `tools` 原样转给上游、把 `tool_calls` 原样流回来（见
 * `api::assistant` 模块头）。
 *
 * ── 三类工具（决定确认流程）──────────────────────────────────
 *   read  只读：读完直接把结果回给模型，不打扰用户；
 *   write 改动：**先渲染确认卡**，用户点「执行」才落库；
 *   run   会真打上游 / 消耗额度（模型测试）：同 write，且卡上说清代价。
 * 「要不要确认」不看工具自己，看用户点的是哪张卡 —— 这里的 `kind` 是唯一的
 * 判据，别在页面里另写一套。
 *
 * ── 工具 id 是与设置页的契约 ─────────────────────────────────
 * `tools` 勾选表存的是这些 id（后端默认集见 `config::DEFAULT_ENABLED_TOOLS`，
 * 同一份字符串）。改名 = 用户存过的勾选失效，加工具 = 两边各加一处。
 */

import { t } from '../i18n'

/** 工具结果：`text` 回给模型（走 role:"tool" 的消息体），`summary` 给卡片显示 */
export type ToolResult = {
  text: string
  summary: string
}

/** 确认卡上的计划（改动类工具在用户确认前渲染它） */
export type ToolPlan = {
  /** 一句话说清将要做什么（含数量） */
  lead: string
  /** 明细表（可选；列数任意，行数由调用方截断显示） */
  table?: { head: string[]; rows: string[][] }
  /** 底部的补充说明（风险 / 生效时机 / 失败语义） */
  note?: string
}

export type AssistantTool = {
  id: string
  label: string
  kind: 'read' | 'write' | 'run'
  /** 给模型看的说明（中文 —— 上游模型对中文的理解足够，且与界面文案同源） */
  desc: string
  params: Record<string, unknown>
  /** 改动类 / 消耗类：渲染确认卡（只读工具不实现） */
  plan?: (args: Record<string, unknown>) => ToolPlan
  run: (args: Record<string, unknown>) => Promise<ToolResult>
}

/* ─── 桥（只声明用到的那些方法，缺失时按「这条链在当前形态不可用」处理）── */

type Bridge = {
  platform?: string
  getModelManage?: () => Promise<unknown>
  getState?: () => Promise<unknown>
  setModelState?: (payload: Record<string, unknown>) => Promise<unknown>
  addModelMapping?: (alias: string, target: string, provider: string, reasoning?: string, enabled?: boolean) => Promise<unknown>
  removeModelMapping?: (alias: string, target: string, provider: string) => Promise<unknown>
  setModelCapabilities?: (provider: string, id: string, capabilities: Record<string, unknown>) => Promise<unknown>
  testModel?: (payload: Record<string, unknown>) => Promise<unknown>
  updateAccount?: (id: string, patch: Record<string, unknown>) => Promise<unknown>
  reorderAccounts?: (queue: string[]) => Promise<unknown>
  getAllBalances?: (id?: string) => Promise<unknown>
}

function bridge(): Bridge {
  const api = (window as unknown as { workbuddyDesktop?: Bridge }).workbuddyDesktop
  if (!api) throw new Error(t('桌面桥不可用（本页需要在 Agent2API 客户端里打开）'))
  return api
}

/** 改动类工具落库后刷新全局状态（其它页面的数据依赖它） */
function refreshApp(): void {
  const app = (window as unknown as { wbApp?: { refresh?: () => void } }).wbApp
  void app?.refresh?.()
}

/* ─── 入参读取小工具（模型给的东西不一定守规矩，坏值一律当缺）── */

function str(value: unknown, max = 256): string {
  return typeof value === 'string' ? value.trim().slice(0, max) : ''
}
function bool(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined
}
function num(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}
function list(value: unknown): Array<Record<string, unknown>> {
  return Array.isArray(value) ? value.filter((item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object') : []
}
function errorText(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  return message.replace(/^Error:\s*/, '')
}

/* ─── 数据形状（只声明用到的字段）── */

type ModelRow = {
  id?: unknown
  name?: unknown
  provider?: unknown
  providerLabel?: unknown
  enabled?: unknown
  aliases?: unknown
}
type AccountRow = {
  id?: unknown
  provider?: unknown
  name?: unknown
  nickname?: unknown
  priority?: unknown
  enabled?: unknown
}

async function readModels(): Promise<ModelRow[]> {
  const data = (await bridge().getModelManage?.()) as { models?: ModelRow[] } | undefined
  return Array.isArray(data?.models) ? data.models : []
}

async function readAccounts(): Promise<AccountRow[]> {
  const state = (await bridge().getState?.()) as { accounts?: AccountRow[] } | undefined
  const rows = Array.isArray(state?.accounts) ? state.accounts : []
  // 全局队列 = priority 升序（后端 priority 0 起、越小越靠前）
  return rows
    .slice()
    .sort((a, b) => Number(a.priority ?? 0) - Number(b.priority ?? 0))
}

/** 模型行的可读标识（模型名一律带提供商，免得同名映射下说不清是哪一条） */
function modelLabel(row: ModelRow): string {
  const provider = str(row.providerLabel) || str(row.provider)
  return provider ? `${provider} / ${str(row.id)}` : str(row.id)
}
function accountLabel(row: AccountRow): string {
  return str(row.name) || str(row.nickname) || str(row.id) || t('未命名账号')
}

/** 账号点名：id 精确 > 名字精确 > 名字子串（唯一命中才算），命中多条时说清是哪些 */
function findAccount(accounts: AccountRow[], ref: string): AccountRow {
  const needle = ref.trim()
  if (!needle) throw new Error(t('没给账号名'))
  const byId = accounts.find(row => str(row.id) === needle)
  if (byId) return byId
  const exact = accounts.filter(row => accountLabel(row) === needle)
  if (exact.length === 1) return exact[0]
  const fuzzy = accounts.filter(row => accountLabel(row).includes(needle))
  if (fuzzy.length === 1) return fuzzy[0]
  const candidates = (exact.length ? exact : fuzzy).map(accountLabel).slice(0, 6)
  if (!candidates.length) throw new Error(t('没有找到账号「{name}」', { name: needle }))
  throw new Error(t('「{name}」匹配到多个账号：{list}，请用完整账号名重试', { name: needle, list: candidates.join('、') }))
}

/* ─── 九只工具 ─────────────────────────────────────────────── */

/** 1 · 获取模型列表（只读） */
const listModels: AssistantTool = {
  id: 'list_models',
  label: t('获取模型列表'),
  kind: 'read',
  desc: '读取各提供商的模型清单（模型 ID、所属提供商、是否启用、已挂的映射名）以及全部映射。回答「有哪些模型」「哪些模型在多家重复」「某个名字挂在哪」这类问题之前先调用它。',
  params: {
    type: 'object',
    properties: {
      provider: { type: 'string', description: '只看某一家（提供商 id，如 codearts / workbuddy）；不填 = 全部' },
      keyword: { type: 'string', description: '按模型 ID 或映射名做子串过滤；不填 = 不过滤' },
      enabledOnly: { type: 'boolean', description: 'true = 只看已启用的模型' },
    },
  },
  run: async args => {
    const provider = str(args.provider, 64).toLowerCase()
    const keyword = str(args.keyword, 128).toLowerCase()
    const enabledOnly = bool(args.enabledOnly) === true
    const data = (await bridge().getModelManage?.()) as { models?: ModelRow[]; mappings?: Array<Record<string, unknown>> } | undefined
    const rows = (Array.isArray(data?.models) ? data.models : []).filter(row => {
      if (provider && str(row.provider).toLowerCase() !== provider) return false
      if (enabledOnly && row.enabled !== true) return false
      if (keyword) {
        const hay = [str(row.id), str(row.name), ...(Array.isArray(row.aliases) ? row.aliases.map(item => str(item)) : [])].join(' ').toLowerCase()
        if (!hay.includes(keyword)) return false
      }
      return true
    })
    const slim = rows.map(row => ({
      provider: str(row.provider),
      id: str(row.id),
      enabled: row.enabled === true,
      aliases: Array.isArray(row.aliases) ? row.aliases.map(item => str(item)) : [],
    }))
    return {
      text: JSON.stringify({ total: slim.length, models: slim.slice(0, 400) }),
      summary: t('{n} 个模型', { n: slim.length }),
    }
  },
}

/** 2 · 查询账号余额（只读） */
const listAccountBalance: AssistantTool = {
  id: 'list_account_balance',
  label: t('查询账号余额'),
  kind: 'read',
  desc: '查询各账号的余额 / 剩余额度（会向各上游发起真实的余额查询）。回答「还剩多少」「哪个账号快用完了」这类问题之前先调用它。',
  params: {
    type: 'object',
    properties: {
      account: { type: 'string', description: '只查某个账号（账号名或 id）；不填 = 全部账号' },
    },
  },
  run: async args => {
    const ref = str(args.account, 128)
    // 点名某个账号时先把它解析成 id（余额接口按 id 查）
    const id = ref ? str(findAccount(await readAccounts(), ref).id) : ''
    const data = (await bridge().getAllBalances?.(id || undefined)) as unknown
    const results = Array.isArray((data as { results?: unknown[] })?.results)
      ? ((data as { results: unknown[] }).results)
      : data
    const rows = Array.isArray(results) ? results : []
    return {
      text: JSON.stringify({ accounts: rows.slice(0, 60) }).slice(0, 12000),
      summary: t('{n} 个账号', { n: rows.length }),
    }
  },
}

/** 3 · 启用 / 禁用模型（改动） */
const setModelEnabled: AssistantTool = {
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
  plan: args => {
    const items = list(args.items)
    return {
      lead: t('将修改 {n} 个模型的启停状态：', { n: items.length }),
      table: {
        head: [t('提供商'), t('模型'), t('改成')],
        rows: items.map(item => [
          str(item.provider),
          str(item.id),
          bool(item.enabled) === false ? t('禁用') : t('启用'),
        ]),
      },
      note: t('改动立即生效；禁用后指向它的请求会落到下一个候选。'),
    }
  },
  run: async args => {
    const api = bridge()
    const items = list(args.items)
    const done: string[] = []
    for (const item of items) {
      const provider = str(item.provider)
      const id = str(item.id)
      const enabled = bool(item.enabled)
      if (!provider || !id || enabled === undefined) throw new Error(t('参数不完整（需要 provider / id / enabled）'))
      await api.setModelState?.({ provider, id, enabled })
      done.push(`${enabled ? '启用' : '禁用'} ${provider}/${id}`)
    }
    refreshApp()
    return { text: JSON.stringify({ ok: true, changed: done }), summary: t('已修改 {n} 个模型', { n: done.length }) }
  },
}

/** 4 · 设置模型映射 ID（改动）—— 两类用法的公共入口：
 *   「同名合并」（多家各挂同一个新名字）与「前缀区分」（各家挂各自前缀的名字），
 *   都只是同一批 alias 的两组参数。 */
const setModelMapping: AssistantTool = {
  id: 'set_model_mapping',
  label: t('设置模型映射 ID'),
  kind: 'write',
  desc: '给模型新增或删除映射名（对外名）。同一对外名可以在多个提供商各挂一条：下游用同一个名字请求，网关按账号优先级主备切换；也可以给不同家的同名模型各加前缀（如 ca/claude-sonnet-4.5）来精确指定某一家。一次可以改多条。删掉的映射名只是不再对外可用，不删任何模型。',
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
            action: { type: 'string', enum: ['add', 'remove'], description: 'add = 新增映射，remove = 删除映射' },
          },
          required: ['provider', 'id', 'alias', 'action'],
        },
      },
    },
    required: ['items'],
  },
  plan: args => {
    const items = list(args.items)
    const adds = items.filter(item => str(item.action) !== 'remove').length
    const removes = items.length - adds
    const parts: string[] = []
    if (adds) parts.push(t('新增 {n} 条', { n: adds }))
    if (removes) parts.push(t('删除 {n} 条', { n: removes }))
    return {
      lead: t('将改动 {n} 条映射（{parts}）：', { n: items.length, parts: parts.join('、') }),
      table: {
        head: [t('提供商'), t('上游模型'), t('操作'), t('映射名')],
        rows: items.map(item => [
          str(item.provider),
          str(item.id),
          str(item.action) === 'remove' ? t('删除') : t('新增'),
          str(item.alias),
        ]),
      },
      note: t('只改映射名，不动上游模型 ID；映射立即生效（新对话即可用）。'),
    }
  },
  run: async args => {
    const api = bridge()
    const items = list(args.items)
    const done: string[] = []
    for (const item of items) {
      const provider = str(item.provider)
      const id = str(item.id)
      const alias = str(item.alias, 128)
      if (!provider || !id || !alias) throw new Error(t('参数不完整（需要 provider / id / alias）'))
      if (str(item.action) === 'remove') {
        await api.removeModelMapping?.(alias, id, provider)
        done.push(`删除 ${alias} → ${provider}/${id}`)
      } else {
        await api.addModelMapping?.(alias, id, provider)
        done.push(`新增 ${alias} → ${provider}/${id}`)
      }
    }
    refreshApp()
    return { text: JSON.stringify({ ok: true, changed: done }), summary: t('已改动 {n} 条映射', { n: done.length }) }
  },
}

/** 5 · 修改模型能力（改动） */
const CAPABILITY_KEYS = ['maxInputTokens', 'maxOutputTokens', 'supportsToolCall', 'supportsImages', 'supportsVideo', 'supportsReasoning'] as const
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
          supportsToolCall: { type: ['boolean', 'null'], description: '是否支持工具调用；null = 恢复上游原值' },
          supportsImages: { type: ['boolean', 'null'], description: '是否支持图片识别；null = 恢复上游原值' },
          supportsVideo: { type: ['boolean', 'null'], description: '是否支持视频识别；null = 恢复上游原值' },
          supportsReasoning: { type: ['boolean', 'null'], description: '是否支持深度推理；null = 恢复上游原值' },
        },
      },
    },
    required: ['provider', 'id', 'capabilities'],
  },
  plan: args => {
    const caps = (args.capabilities && typeof args.capabilities === 'object' ? args.capabilities : {}) as Record<string, unknown>
    const keys = CAPABILITY_KEYS.filter(key => key in caps)
    return {
      lead: t('将修改 {model} 的 {n} 项能力：', { model: `${str(args.provider)} / ${str(args.id)}`, n: keys.length }),
      table: {
        head: [t('能力'), t('改成')],
        rows: keys.map(key => {
          const value = caps[key]
          const text = value === null ? t('恢复上游原值') : typeof value === 'boolean' ? (value ? t('支持') : t('不支持')) : String(value)
          return [CAPABILITY_LABELS[key] || key, text]
        }),
      },
      note: t('能力位决定下游看到的声明（能不能贴图、能塞多少上下文），不改上游的真实行为。'),
    }
  },
  run: async args => {
    const provider = str(args.provider)
    const id = str(args.id)
    const raw = (args.capabilities && typeof args.capabilities === 'object' ? args.capabilities : {}) as Record<string, unknown>
    const caps: Record<string, unknown> = {}
    for (const key of CAPABILITY_KEYS) {
      if (!(key in raw)) continue
      const value = raw[key]
      if (value === null) { caps[key] = null; continue }
      if (key === 'maxInputTokens' || key === 'maxOutputTokens') {
        const n = num(value)
        if (n === undefined) throw new Error(t('{key} 需要数字', { key: CAPABILITY_LABELS[key] }))
        caps[key] = Math.round(n)
      } else {
        const b = bool(value)
        if (b === undefined) throw new Error(t('{key} 需要 true / false / null', { key: CAPABILITY_LABELS[key] }))
        caps[key] = b
      }
    }
    if (!provider || !id || !Object.keys(caps).length) throw new Error(t('参数不完整（需要 provider / id / capabilities）'))
    await bridge().setModelCapabilities?.(provider, id, caps)
    refreshApp()
    return { text: JSON.stringify({ ok: true, provider, id, capabilities: caps }), summary: t('已修改 {n} 项能力', { n: Object.keys(caps).length }) }
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
      prompt: { type: 'string', description: '测试用的用户提示词；不填 = 默认一句问候（最小请求）' },
    },
    required: ['provider', 'id'],
  },
  plan: args => ({
    lead: t('将对 {model} 发起一次真实请求：', { model: `${str(args.provider)} / ${str(args.id)}` }),
    table: {
      head: [t('项'), t('值')],
      rows: [
        [t('提示词'), str(args.prompt) || t('（默认问候）')],
        [t('代价'), t('消耗该账号额度')],
      ],
    },
    note: t('这是真实的上游请求，与生产走同一条链路（会记账，报表排除）。'),
  }),
  run: async args => {
    const payload: Record<string, unknown> = { provider: str(args.provider), model: str(args.id) }
    const prompt = str(args.prompt, 4000)
    if (prompt) payload.prompt = prompt
    const result = (await bridge().testModel?.(payload)) as Record<string, unknown> | undefined
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
      summary: ok ? t('连通 · {ms}ms', { ms: String(slim.duration_ms ?? '') }) : t('失败 · {status}', { status: status || '—' }),
    }
  },
}

/** 7 · 调整提供商顺位（改动）—— 整组移动：这一家的全部账号在全局队列里的位置 */
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
  plan: args => ({
    lead: t('将把「{provider}」整组移到第 {position} 顺位：', {
      provider: str(args.provider) || t('（未指定）'),
      position: String(num(args.position) ?? '?'),
    }),
    note: t('队列是一次性整表提交（后端按新顺序重编优先级），移动会连带调整其它组的相对次序。'),
  }),
  run: async args => {
    const provider = str(args.provider).toLowerCase()
    const position = num(args.position)
    if (!provider || position === undefined) throw new Error(t('参数不完整（需要 provider / position）'))
    const accounts = await readAccounts()
    // 按家分组（保持组内原有相对顺序 —— 组内顺位是用户自己排的，不该被这步打乱）
    const groups: Array<{ provider: string; ids: string[] }> = []
    for (const row of accounts) {
      const id = str(row.id)
      const key = str(row.provider).toLowerCase()
      if (!id) continue
      const last = groups[groups.length - 1]
      if (last && last.provider === key) last.ids.push(id)
      else groups.push({ provider: key, ids: [id] })
    }
    const from = groups.findIndex(group => group.provider === provider)
    if (from < 0) throw new Error(t('没有找到提供商「{provider}」的账号', { provider }))
    const to = Math.max(0, Math.min(groups.length - 1, Math.round(position) - 1))
    const [moved] = groups.splice(from, 1)
    groups.splice(to, 0, moved)
    await bridge().reorderAccounts?.(groups.flatMap(group => group.ids))
    refreshApp()
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
  plan: args => ({
    lead: t('将把账号「{account}」移到第 {position} 顺位：', {
      account: str(args.account) || t('（未指定）'),
      position: String(num(args.position) ?? '?'),
    }),
    note: t('队列是一次性整表提交；位置越靠前，路由时越先被选中。'),
  }),
  run: async args => {
    const ref = str(args.account, 128)
    const position = num(args.position)
    if (!ref || position === undefined) throw new Error(t('参数不完整（需要 account / position）'))
    const accounts = await readAccounts()
    const target = findAccount(accounts, ref)
    const id = str(target.id)
    const ids = accounts.map(row => str(row.id)).filter(Boolean)
    const from = ids.indexOf(id)
    if (from < 0) throw new Error(t('账号不在当前队列里'))
    ids.splice(from, 1)
    const to = Math.max(0, Math.min(ids.length, Math.round(position) - 1))
    ids.splice(to, 0, id)
    await bridge().reorderAccounts?.(ids)
    refreshApp()
    return {
      text: JSON.stringify({ ok: true, account: accountLabel(target), from: from + 1, to: to + 1 }),
      summary: t('已移到第 {n} 顺位', { n: to + 1 }),
    }
  },
}

/** 9 · 启用 / 禁用账号（高危） */
const setAccountEnabled: AssistantTool = {
  id: 'set_account_enabled',
  label: t('启用 / 禁用账号'),
  kind: 'write',
  desc: '开启或关闭某几个账号的转发资格。禁用只是把它移出队列（余额仍可查），但**如果某条线路只剩它**，禁用会直接断供。一次可以改多个。',
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
  plan: args => {
    const items = list(args.items)
    const disabling = items.filter(item => bool(item.enabled) === false).length
    return {
      lead: t('将修改 {n} 个账号的启用状态（其中禁用 {k} 个）：', { n: items.length, k: disabling }),
      table: {
        head: [t('账号'), t('改成')],
        rows: items.map(item => [str(item.account), bool(item.enabled) === false ? t('禁用') : t('启用')]),
      },
      note: t('禁用会把账号移出转发队列；若这条线路没有别的可用账号，请求会直接失败。'),
    }
  },
  run: async args => {
    const api = bridge()
    const items = list(args.items)
    const accounts = await readAccounts()
    const done: string[] = []
    for (const item of items) {
      const enabled = bool(item.enabled)
      if (enabled === undefined) throw new Error(t('参数不完整（需要 account / enabled）'))
      const target = findAccount(accounts, str(item.account, 128))
      await api.updateAccount?.(str(target.id), { enabled })
      done.push(`${enabled ? '启用' : '禁用'} ${accountLabel(target)}`)
    }
    refreshApp()
    return { text: JSON.stringify({ ok: true, changed: done }), summary: t('已修改 {n} 个账号', { n: done.length }) }
  },
}

/** 全部工具（顺序 = 设置页清单顺序：只读在前，改动在后，高危最后） */
export const ASSISTANT_TOOLS: AssistantTool[] = [
  listModels,
  listAccountBalance,
  setModelEnabled,
  setModelMapping,
  setModelCapability,
  testModel,
  setProviderOrder,
  setAccountOrder,
  setAccountEnabled,
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

/** 设置页清单与文案（工具 id → 中文名 / 一句话 / 风险标签）。
 *  文案在这里就用 `t()` 包好（定义时求值）：页面直接取 `meta.label` 即可，
 *  不会再出现「把变量喂给 t()」那种扫描器提不到键的写法。 */
export const TOOL_META: Array<{ id: string; label: string; desc: string; tag?: 'danger' | 'cost' }> = [
  { id: 'list_models', label: t('获取模型列表'), desc: t('读各提供商的模型清单、映射名与启用状态。') },
  { id: 'list_account_balance', label: t('查询账号余额'), desc: t('读各账号的额度 / 剩余用量。') },
  { id: 'set_model_enabled', label: t('启用 / 禁用模型'), desc: t('开关模型的对外可用性，不改其它属性。') },
  { id: 'set_model_mapping', label: t('设置模型映射 ID'), desc: t('新增 / 删除映射名，支持批量加前缀与同名合并到多家。') },
  { id: 'set_model_capability', label: t('修改模型能力'), desc: t('改上下文窗口 / 最大输出 Token / 工具调用等能力位。') },
  { id: 'test_model', label: t('发起模型测试'), desc: t('对指定模型发一次真实请求，回报连通性与耗时。'), tag: 'cost' },
  { id: 'set_provider_order', label: t('调整提供商顺位'), desc: t('改某一家的整组顺位（先试哪一家）。') },
  { id: 'set_account_order', label: t('调整账号优先级'), desc: t('改账号在全局队列里的名次，决定主备与切换顺序。') },
  { id: 'set_account_enabled', label: t('启用 / 禁用账号'), desc: t('禁用会把账号移出转发队列，线路可能因此断供。'), tag: 'danger' },
]

export { errorText }
