import { t } from '../i18n'
import type { AgentClient, AgentGatewayKey, AgentOperation, AgentPayload, AgentPreview, AgentProfile } from './agent-types'
export type { AgentClient, AgentOperation, AgentPayload, AgentPreview, AgentProfile } from './agent-types'

type Bridge = {
  platform?: string
  getAgents?: (options?: { refresh?: boolean }) => Promise<AgentPayload>
  saveAgentProfiles?: (payload: Record<string, unknown>) => Promise<AgentPayload>
  applyAgentProfile?: (payload: Record<string, unknown>) => Promise<AgentPayload>
  previewAgentProfile?: (payload: Record<string, unknown>) => Promise<AgentPreview>
  restoreAgentClient?: (payload: Record<string, unknown>) => Promise<AgentPayload>
  importAgentClient?: (payload: Record<string, unknown>) => Promise<AgentPayload>
  pickAgentConfigFile?: (defaultPath?: string) => Promise<{ canceled?: boolean; unsupported?: boolean; path?: string }>
}

const SNAPSHOT_CACHE_KEY = 'wb-agent-panel-snapshot-v1'
const SELECTED_CACHE_KEY = 'wb-agent-selected-v1'
const AUTO_REFRESH_COOLDOWN_MS = 15_000

type AgentWindow = {
  workbuddyDesktop?: Bridge
  wbApp?: { toast?: (message: string, kind?: 'err' | 'ok') => void }
  wbConfirm?: { ask?: (options: { title?: string; text?: string; okText?: string; okClass?: string }) => Promise<boolean> }
}
const host = () => window as unknown as AgentWindow
export const isDesktopEnvironment = () => {
  const platform = host().workbuddyDesktop?.platform
  return Boolean(platform && platform !== 'web')
}
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error)
export const toast = (message: string, kind?: 'err' | 'ok') => host().wbApp?.toast?.(message, kind)
export async function confirm(title: string, text: string, okText = t('确定')): Promise<boolean> {
  return await host().wbConfirm?.ask?.({ title, text, okText }) ?? false
}

export type AgentStore = {
  clients: AgentClient[]
  gateway: string
  defaultModel: string
  requiresKey: boolean
  gatewayKeys: AgentGatewayKey[]
  detectedAt: number
  hostName: string
  loading: boolean
  refreshing: boolean
  error: string
  selected: string
  filter: string
  query: string
  busy: string
}

function readStoredString(key: string): string {
  try { return window.localStorage.getItem(key) || '' } catch { return '' }
}
function writeStoredString(key: string, value: string): void {
  try { window.localStorage.setItem(key, value) } catch { /* ignore storage errors */ }
}
function readCachedPayload(): AgentPayload | null {
  try {
    const raw = window.localStorage.getItem(SNAPSHOT_CACHE_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as AgentPayload
    if (!parsed || !Array.isArray(parsed.clients) || !parsed.clients.length) return null
    if (parsed.clients.some(client => !Array.isArray(client.configurations))) return null
    if (typeof parsed.gateway?.baseUrl !== 'string') return null
    return parsed
  } catch {
    return null
  }
}
function saveCachedPayload(payload: AgentPayload): void {
  try { window.localStorage.setItem(SNAPSHOT_CACHE_KEY, JSON.stringify(payload)) } catch { /* ignore storage quota */ }
}
function defaultSelected(clients: AgentClient[], preferred: string): string {
  if (preferred && clients.some(client => client.id === preferred)) return preferred
  return clients.find(client => client.detected)?.id || clients[0]?.id || ''
}

const cachedPayload = readCachedPayload()
const cachedSelected = cachedPayload ? defaultSelected(cachedPayload.clients, readStoredString(SELECTED_CACHE_KEY)) : ''
let lastLiveLoadedAt = 0

let state: AgentStore = cachedPayload ? {
  clients: cachedPayload.clients,
  gateway: cachedPayload.gateway.baseUrl,
  defaultModel: cachedPayload.gateway.defaultModel || '',
  requiresKey: cachedPayload.gateway.requiresKey,
  gatewayKeys: cachedPayload.gateway.keys || [],
  detectedAt: cachedPayload.detectedAt || 0,
  hostName: cachedPayload.host?.name || '',
  loading: false,
  refreshing: false,
  error: '',
  selected: cachedSelected,
  filter: 'all',
  query: '',
  busy: '',
} : {
  clients: [], gateway: '', defaultModel: '', requiresKey: false, gatewayKeys: [], detectedAt: 0, hostName: '', loading: false, refreshing: false, error: '', selected: '', filter: 'all', query: '', busy: '',
}
const listeners = new Set<() => void>()
export const getAgentStore = () => state
export function subscribeAgentStore(listener: () => void): () => void { listeners.add(listener); return () => { listeners.delete(listener) } }
function patch(values: Partial<AgentStore>): void { state = { ...state, ...values }; for (const listener of listeners) listener() }
function syncVisibleSelection(next: AgentStore): string {
  const list = visibleClients(next)
  if (!list.length) return next.selected
  return list.some(client => client.id === next.selected) ? next.selected : list[0].id
}
export function selectedClient(): AgentClient | undefined { return state.clients.find(client => client.id === state.selected) }
export function selectClient(id: string): void { writeStoredString(SELECTED_CACHE_KEY, id); patch({ selected: id }) }
export function setFilter(filter: string): void {
  const selected = syncVisibleSelection({ ...state, filter })
  if (selected) writeStoredString(SELECTED_CACHE_KEY, selected)
  patch({ filter, selected })
}
export function setQuery(query: string): void {
  const selected = syncVisibleSelection({ ...state, query })
  if (selected) writeStoredString(SELECTED_CACHE_KEY, selected)
  patch({ query, selected })
}
export function clearFilters(): void {
  const selected = syncVisibleSelection({ ...state, filter: 'all', query: '' })
  if (selected) writeStoredString(SELECTED_CACHE_KEY, selected)
  patch({ filter: 'all', query: '', selected })
}

function accept(payload: AgentPayload): void {
  if (!payload || !Array.isArray(payload.clients)) throw new Error(t('客户端列表响应无效'))
  if (payload.clients.some(client => !Array.isArray(client.configurations))) throw new Error(t('配置列表格式不匹配，请重启网关后刷新页面'))
  const next: AgentStore = {
    ...state,
    clients: payload.clients,
    gateway: payload.gateway.baseUrl,
    defaultModel: payload.gateway.defaultModel || '',
    requiresKey: payload.gateway.requiresKey,
    gatewayKeys: payload.gateway.keys || [],
    detectedAt: payload.detectedAt,
    hostName: payload.host?.name || '',
    error: '',
    selected: defaultSelected(payload.clients, state.selected || readStoredString(SELECTED_CACHE_KEY)),
  }
  next.selected = syncVisibleSelection(next)
  if (next.selected) writeStoredString(SELECTED_CACHE_KEY, next.selected)
  saveCachedPayload(payload)
  lastLiveLoadedAt = Date.now()
  patch(next)
}

export function visibleClients(store = state): AgentClient[] {
  const query = store.query.trim().toLowerCase()
  return store.clients.filter(client => {
    if (store.filter === 'via' && !client.viaGateway) return false
    if (store.filter === 'direct' && (!client.detected || client.viaGateway)) return false
    if (store.filter === 'missing' && client.detected) return false
    return !query || `${client.name} ${client.vendor} ${client.configPath}`.toLowerCase().includes(query)
  })
}
export function counts(store = state) {
  const detected = store.clients.filter(client => client.detected)
  const via = detected.filter(client => client.viaGateway).length
  return { all: store.clients.length, detected: detected.length, via, direct: detected.length - via, missing: store.clients.length - detected.length }
}

export type LoadOptions = { silent?: boolean; force?: boolean }
let loadingRequest = 0
export async function loadPanel(optionsOrSilent: boolean | LoadOptions = false): Promise<void> {
  const options = typeof optionsOrSilent === 'boolean' ? { silent: optionsOrSilent, force: false } : optionsOrSilent
  const silent = Boolean(options.silent)
  const force = Boolean(options.force)
  if (state.refreshing || state.busy) return
  if (!force && lastLiveLoadedAt > 0 && Date.now() - lastLiveLoadedAt < AUTO_REFRESH_COOLDOWN_MS) return
  const call = host().workbuddyDesktop?.getAgents
  if (!call) { patch({ error: t('当前环境不支持客户端配置'), loading: false, refreshing: false }); return }
  const request = ++loadingRequest
  patch({ loading: state.clients.length === 0, refreshing: true })
  try {
    const payload = await call(force ? { refresh: true } : undefined)
    if (request === loadingRequest) accept(payload)
  }
  catch (error) { if (request === loadingRequest) { patch({ error: errorMessage(error) }); if (!silent) toast(errorMessage(error), 'err') } }
  finally { if (request === loadingRequest) patch({ loading: false, refreshing: false }) }
}

export async function pickConfigFile(defaultPath?: string) {
  const picker = host().workbuddyDesktop?.pickAgentConfigFile
  if (!picker) return { canceled: true, unsupported: true }
  return await picker(defaultPath)
}

async function mutation(clientId: string, call: () => Promise<AgentPayload>): Promise<AgentPayload> {
  if (state.busy) throw new Error(t('另一项配置操作尚未完成'))
  loadingRequest += 1
  patch({ busy: clientId, loading: false, refreshing: false })
  try { const payload = await call(); accept(payload); return payload }
  finally { patch({ busy: '' }) }
}

export async function saveProfiles(client: AgentClient, profiles: AgentProfile[]): Promise<void> {
  const call = host().workbuddyDesktop?.saveAgentProfiles
  if (!call) throw new Error(t('当前环境不支持保存'))
  await mutation(client.id, () => call({ clientId: client.id, profiles, revision: client.revision }))
}

export async function saveConfiguration(client: AgentClient, profile: AgentProfile, creating: boolean): Promise<void> {
  const call = host().workbuddyDesktop?.saveAgentProfiles
  if (!call) throw new Error(t('当前环境不支持保存'))
  await mutation(client.id, () => call({ clientId: client.id, action: 'config-save', profile,
    profileId: creating ? '' : profile.id, liveId: profile.liveId || '', revision: client.revision, nativeRevision: client.nativeRevision, configPath: client.configPath }))
}

export async function removeConfiguration(client: AgentClient, profile: AgentProfile): Promise<void> {
  const call = host().workbuddyDesktop?.saveAgentProfiles
  if (!call) throw new Error(t('当前环境不支持保存'))
  await mutation(client.id, () => call({ clientId: client.id, action: 'config-remove', profileId: profile.id,
    liveId: profile.liveId || '', revision: client.revision, nativeRevision: client.nativeRevision, configPath: client.configPath }))
}

export async function savePath(client: AgentClient, configPath: string): Promise<void> {
  const call = host().workbuddyDesktop?.saveAgentProfiles
  if (!call) throw new Error(t('当前环境不支持保存'))
  await mutation(client.id, () => call({ clientId: client.id, action: 'path', configPath }))
}

export async function importFromClient(client: AgentClient): Promise<void> {
  const call = host().workbuddyDesktop?.importAgentClient
  if (!call) throw new Error(t('当前环境不支持导入'))
  try {
    const payload = await mutation(client.id, () => call({ clientId: client.id, adopt: true, revision: client.revision }))
    toast(payload.imported?.count ? t('已导入 {n} 套方案', { n: payload.imported.count }) : t('没有新的配置可以导入'))
  } catch (error) { toast(errorMessage(error), 'err') }
}

export async function requestPreview(client: AgentClient, profile: AgentProfile | undefined, operation: AgentOperation): Promise<AgentPreview> {
  const call = host().workbuddyDesktop?.previewAgentProfile
  if (!call) throw new Error(t('当前环境不支持预览'))
  return call({ clientId: client.id, profileId: profile?.id || '', ...(profile ? { profile } : {}), operation,
    ...(operation.startsWith('native-') || operation.startsWith('config-') ? { nativeRevision: client.nativeRevision } : {}),
    ...(operation.startsWith('config-') ? { revision: client.revision, liveId: profile?.liveId || '', configPath: client.configPath } : {}) })
}

export async function applyProfile(client: AgentClient, profile: AgentProfile, operation: AgentOperation, preview: AgentPreview): Promise<void> {
  const call = host().workbuddyDesktop?.applyAgentProfile
  if (!call) throw new Error(t('当前环境不支持应用'))
  const direct = operation.startsWith('native-') || operation.startsWith('config-') || operation === 'quick-connect'
  const result = await mutation(client.id, () => call({ clientId: client.id, profileId: profile.id, operation, token: preview.token,
    ...(direct ? { profile, nativeRevision: client.nativeRevision } : {}),
    ...(operation.startsWith('config-') ? { revision: client.revision, liveId: profile.liveId || '', configPath: client.configPath } : {}) }))
  if (!result.applied?.verified) throw new Error(t('客户端配置写入后未通过校验'))
  if (operation === 'config-delete') { toast(t('配置已删除，客户端文件已同步更新')); return }
  toast(result.applied.changed ? client.id === 'dsh'
    ? t('配置文件已写入并校验；尚未验证 DSH 是否加载，请在所选客户端新建会话确认。')
    : t('配置已应用并读回校验；请重开客户端或会话') : t('客户端配置已经一致，无需写入'))
}

export async function restoreClient(client: AgentClient, operation: AgentOperation, preview: AgentPreview): Promise<void> {
  const call = host().workbuddyDesktop?.restoreAgentClient
  if (!call) throw new Error(t('当前环境不支持恢复'))
  await mutation(client.id, () => call({ clientId: client.id, operation, token: preview.token }))
  toast(t('配置已恢复并校验'))
}
