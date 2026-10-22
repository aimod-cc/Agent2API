/**
 * 「Agent」页的**数据层**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 与 proxies-state / models-panel-state 同一套结构：模块级快照 +
 * useSyncExternalStore。对外契约 `window.wbAgentPanel = { load }`
 * （app.js 切到本页时调），从 React 之外调用、必须驱动界面刷新。
 *
 * ── 两种客户端形态（这是这一页的核心，视图层只按它分叉）──────────
 *   single  一个文件里只有一套生效的键值；多套配置存在我们这边
 *           （后端的 kv `agentProfiles`），切换 = 整段覆写 + 备份原文件。
 *   multi   客户端自己管理多套配置（ZCode 的 provider_config.json），
 *           切换只改 enabled / 默认项，不动它自己的条目。
 * 两种形态的开关语义因此不同：single 是单选（打开一套就关掉其它），
 * multi 是各自独立。
 *
 * ── 保存与写入是两件事（刻意的）──────────────────────────
 * 「保存」只改我们的配置列表（不碰客户端文件），「写入」才动磁盘。
 * 分开的理由：用户可能先把几套配置都配好、再决定用哪套；也可能只想改个
 * 备注。合并成一个动作会让「点一下开关就改了别人的配置文件」变得不可预期。
 */

import { t } from '../i18n'

/* ─── 后端形态 ───────────────────────────────── */

export type AgentConfigMode = 'single' | 'multi'

export type AgentProfile = {
  id: string
  name: string
  /** gateway / official / relay / local */
  kind: string
  /** anthropic / responses / openai_chat / gemini */
  protocol: string
  baseUrl: string
  model: string
  provider: string
  effort: string
  enabled: boolean
  default: boolean
  note: string
}

export type AgentClient = {
  id: string
  name: string
  vendor: string
  hue: number
  letter: string
  /** 品牌图标路径（`assets/agents/<file>`）；空串 = 用 letter 兜底 */
  icon: string
  /** 图标自带整块方形底（铺满格子）；false = 透明底标识，要留内边距 */
  iconFull: boolean
  configPath: string
  resolvedPath: string
  /** json / jsonc / toml / json+env */
  format: string
  configMode: AgentConfigMode
  protocol: string
  installed: boolean
  needsRestart: boolean
  caps: { prompt: number; skills: number; mcp: number }
  /** 展示用的配置列表：我们自己存的优先，没有时是 live（客户端文件里的） */
  profiles: AgentProfile[]
  /** 从客户端文件里读出来的（只读事实），与 profiles 分开给 */
  live: AgentProfile[]
  /** profiles 是否来自客户端文件（= 我们还没存过任何条目） */
  profilesAreLive: boolean
  /** 读客户端文件失败的原因（解析不了 / 权限不足），null = 没出错 */
  readError: string | null
  /** 有 gateway 类配置且生效（= 原来的「已接管」） */
  viaGateway: boolean
  activeId: string | null
}

export type AgentPayload = {
  clients?: AgentClient[]
  gateway?: { baseUrl?: string; hint?: string }
  applied?: {
    clientId?: string
    profileId?: string
    path?: string
    backup?: string | null
    needsRestart?: boolean
  }
  restored?: { clientId?: string; path?: string; from?: string }
  imported?: { clientId?: string; count?: number }
  /** import 不带 adopt 时返回的只读条目（与 clients 二选一） */
  profiles?: AgentProfile[]
  readError?: string | null
}

export type AgentPreview = {
  path?: string
  text?: string
  /** 改动行（1 起） */
  changed?: number[]
  /** 新增行（1 起）：旧文件里没有这一行 */
  added?: number[]
  /** 写入的副作用提示（如「原文件的注释会被清掉」） */
  warnings?: string[]
}

/* ─── 桥（只声明本页用到的成员；不 declare global，理由见 accounts-shared）─── */

export type AgentBridge = {
  getAgents?: () => Promise<AgentPayload | null | undefined>
  saveAgentProfiles?: (payload: Record<string, unknown>) => Promise<AgentPayload | null | undefined>
  applyAgentProfile?: (payload: Record<string, unknown>) => Promise<AgentPayload | null | undefined>
  previewAgentProfile?: (payload: Record<string, unknown>) => Promise<AgentPreview | null | undefined>
  restoreAgentClient?: (payload: Record<string, unknown>) => Promise<AgentPayload | null | undefined>
  importAgentClient?: (payload: Record<string, unknown>) => Promise<AgentPayload | null | undefined>
}

export type AgentSharedWindow = {
  workbuddyDesktop?: AgentBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    readonly currentPage?: string
  }
  wbConfirm?: {
    ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean>
  }
}

export function wb(): AgentSharedWindow {
  return window as unknown as AgentSharedWindow
}

export function toast(message: string, kind?: 'err' | 'ok'): void {
  wb().wbApp?.toast?.(message, kind)
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/* ─── 状态 ───────────────────────────────────── */

export type AgentStore = {
  clients: AgentClient[]
  /** 本机网关地址（后端按实际监听端口给出，不写死） */
  gateway: string
  loading: boolean
  error: string
  /** 当前选中的客户端 id */
  selected: string
  /** 正在编辑的配置 id（空串 = 跟随当前生效那套） */
  editing: string
  /** 页内筛选：all / via / direct / missing */
  filter: string
  /** 搜索词 */
  query: string
  /** 写入预览（按客户端缓存） */
  preview: AgentPreview | null
  previewing: boolean
  /** 正在写入 / 还原的客户端 id */
  busy: string
}

const initial: AgentStore = {
  clients: [],
  gateway: '',
  loading: true,
  error: '',
  selected: '',
  editing: '',
  filter: 'all',
  query: '',
  preview: null,
  previewing: false,
  busy: '',
}

let state: AgentStore = initial
const listeners = new Set<() => void>()

export function getAgentStore(): AgentStore {
  return state
}

export function subscribeAgentStore(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function patch(next: Partial<AgentStore>): void {
  state = { ...state, ...next }
  for (const listener of listeners) listener()
}

/* ─── 派生 ───────────────────────────────────── */

export function clientById(id: string): AgentClient | undefined {
  return state.clients.find(item => item.id === id)
}

export function selectedClient(): AgentClient | undefined {
  return clientById(state.selected)
}

/** 正在编辑的那套（没有显式编辑 = 当前生效那套 = 第一套） */
export function editingProfile(client: AgentClient | undefined): AgentProfile | undefined {
  if (!client || !client.profiles.length) return undefined
  return (
    client.profiles.find(item => item.id === state.editing) ||
    client.profiles.find(item => item.id === client.activeId) ||
    client.profiles[0]
  )
}

export function enabledCount(client: AgentClient): number {
  return client.profiles.filter(item => item.enabled).length
}

/** 筛选后的客户端列表（左栏用） */
export function visibleClients(store: AgentStore = state): AgentClient[] {
  const query = store.query.trim().toLowerCase()
  return store.clients.filter(client => {
    if (store.filter === 'via' && !(client.installed && client.viaGateway)) return false
    if (store.filter === 'direct' && !(client.installed && !client.viaGateway)) return false
    if (store.filter === 'missing' && client.installed) return false
    if (!query) return true
    return `${client.name} ${client.vendor} ${client.configPath}`.toLowerCase().includes(query)
  })
}

export function counts(store: AgentStore = state): {
  all: number
  via: number
  direct: number
  missing: number
  installed: number
} {
  const installed = store.clients.filter(item => item.installed)
  const via = installed.filter(item => item.viaGateway).length
  return {
    all: store.clients.length,
    via,
    direct: installed.length - via,
    missing: store.clients.length - installed.length,
    installed: installed.length,
  }
}

/* ─── 取数 ───────────────────────────────────── */

/** 把后端返回的整份快照吃进来（写操作也返回同一份，就地替换）。 */
function accept(payload: AgentPayload | null | undefined): void {
  if (!payload || !Array.isArray(payload.clients)) return
  const clients = payload.clients
  // 选中项失效（第一次进页、或客户端列表变了）时回落到第一个已安装的
  const stillThere = clients.some(item => item.id === state.selected)
  const selected = stillThere
    ? state.selected
    : (clients.find(item => item.installed)?.id || clients[0]?.id || '')
  patch({ clients, selected, gateway: payload.gateway?.baseUrl || state.gateway, error: '' })
}

export async function loadPanel(silent = false): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.getAgents !== 'function') {
    patch({ loading: false, error: t('当前环境不支持客户端接管（桥接方法缺失）') })
    return
  }
  if (!silent) patch({ loading: true })
  try {
    accept(await bridge.getAgents())
  } catch (error) {
    patch({ error: errorMessage(error) })
    if (!silent) toast(t('客户端列表加载失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ loading: false })
  }
}

/* ─── 界面状态（不需要请求）───────────────────── */

export function selectClient(id: string): void {
  // 重复点已选中的客户端必须原样返回：下面会把 preview 清成 null，
  // 而重新拉预览靠的是 ClientDetail 里 [client.id, editing?.id, installed]
  // 这个 effect —— selected 没变、editing 本来就是空串，依赖全不变，
  // effect 不重跑，预览就再也回不来（表现为「点一下出现、再点一下消失」）。
  if (id === state.selected) return
  patch({ selected: id, editing: '', preview: null })
}

export function setFilter(filter: string): void {
  patch({ filter })
}

export function setQuery(query: string): void {
  patch({ query })
}

export function startEditing(profileId: string): void {
  patch({ editing: profileId })
}

export function setPreview(preview: AgentPreview | null): void {
  patch({ preview, previewing: false })
}

/* ─── 保存（只改我们的配置列表）───────────────── */

/**
 * 保存一个客户端的配置列表。
 *
 * 单配置客户端在这里做「单选」归一：只留一条 enabled，其余关掉 ——
 * 后端还会再兜一层（防手写请求），但界面上的开关行为要在这里就对。
 */
export async function saveProfiles(clientId: string, profiles: AgentProfile[]): Promise<boolean> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.saveAgentProfiles !== 'function') {
    toast(t('当前环境不支持保存（桥接方法缺失）'), 'err')
    return false
  }
  const client = clientById(clientId)
  const payload = profiles.map(item => ({ ...item }))
  if (client && client.configMode === 'single') {
    let kept = false
    for (const item of payload) {
      if (item.enabled && !kept) kept = true
      else if (item.enabled) item.enabled = false
    }
    if (!kept && payload.length) payload[0].enabled = true
    const active = payload.find(item => item.enabled)
    for (const item of payload) item.default = item === active
  }
  try {
    accept(await bridge.saveAgentProfiles({ clientId, profiles: payload }))
    return true
  } catch (error) {
    toast(t('保存失败：{reason}', { reason: errorMessage(error) }), 'err')
    return false
  }
}

/* ─── 写入客户端文件 ──────────────────────────── */

export async function applyProfile(clientId: string, profileId?: string): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.applyAgentProfile !== 'function') {
    toast(t('当前环境不支持写入（桥接方法缺失）'), 'err')
    return
  }
  patch({ busy: clientId })
  try {
    const data = await bridge.applyAgentProfile({ clientId, profileId: profileId || '' })
    accept(data)
    const client = clientById(clientId)
    const applied = data?.applied
    if (applied?.needsRestart) {
      toast(
        t('已写入 {name}；这个客户端只在启动时读配置，重开一个会话才生效', { name: client?.name || clientId }),
        'ok',
      )
    } else {
      toast(t('已写入 {name}', { name: client?.name || clientId }), 'ok')
    }
  } catch (error) {
    toast(t('写入失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ busy: '' })
  }
}

/* ─── 写入预览 ───────────────────────────────── */

export async function refreshPreview(clientId: string, profileId?: string): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.previewAgentProfile !== 'function') {
    patch({ preview: null })
    return
  }
  patch({ previewing: true })
  try {
    const data = await bridge.previewAgentProfile({ clientId, profileId: profileId || '' })
    patch({ preview: data || null, previewing: false })
  } catch (error) {
    patch({ preview: null, previewing: false })
    // 预览失败不弹 toast：未安装的客户端本来就没有文件可预览
    void error
  }
}

/* ─── 还原 / 导入 ────────────────────────────── */

export async function restoreClient(clientId: string): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.restoreAgentClient !== 'function') {
    toast(t('当前环境不支持还原（桥接方法缺失）'), 'err')
    return
  }
  const client = clientById(clientId)
  const ok = await wb().wbConfirm?.ask?.({
    title: t('按备份还原 {name}？', { name: client?.name || clientId }),
    html: t('会用最近一次备份覆盖它的配置文件。这一步不会改动你在本页存的配置列表。'),
    okText: t('还原'),
  })
  if (!ok) return
  patch({ busy: clientId })
  try {
    const data = await bridge.restoreAgentClient({ clientId })
    accept(data)
    toast(t('已按备份还原 {name}', { name: client?.name || clientId }), 'ok')
  } catch (error) {
    toast(t('还原失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ busy: '' })
  }
}

/** 从客户端现有文件读回一套配置（用户以前手动配过网关时省得重打） */
/**
 * 把客户端文件里的现有配置收进「我的配置」（写 kv），收完刷新整份快照。
 *
 * 与「读一下看看」区分开：不传 adopt 只读不落盘；adopt = true 才写。
 * 收进来之后这些条目就能编辑 / 切换 / 写回了。
 */
export async function importFromClient(clientId: string, adopt = false): Promise<number> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.importAgentClient !== 'function') {
    toast(t('当前环境不支持导入（桥接方法缺失）'), 'err')
    return 0
  }
  try {
    const data = await bridge.importAgentClient({ clientId, adopt })
    if (adopt) {
      accept(data as AgentPayload | null | undefined)
      const count = data?.imported?.count ?? 0
      if (count > 0) {
        toast(t('已收进我的配置：{n} 条', { n: count }), 'ok')
      } else {
        toast(t('没有新的条目可收（都已在我的配置里）'), 'ok')
      }
      return count
    }
    return data?.profiles?.length ?? 0
  } catch (error) {
    toast(t('导入失败：{reason}', { reason: errorMessage(error) }), 'err')
    return 0
  }
}
