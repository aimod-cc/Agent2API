/**
 * 「工具服务」页的数据层：类型、网关请求与少量展示映射。
 *
 * 所有管理请求都走 `callJson`（与其它岛同一套网关基址 / 凭证 / 错误信封），
 * 运行时地址只在展示与安装配置里出现，不在前端直接调用。
 */

import { callJson, gatewayBase } from './assistant-state'
import { t } from '../i18n'

export type ToolProtocol = 'skill' | 'mcp' | 'api'
export type ToolServiceId = 'web-search' | 'vision'

export type SkillVariantInfo = {
  id: 'node' | 'python'
  label: string
  runtime: string
  recommended: boolean
  version: string
  entrypoint: string
}

export type ToolService = {
  id: ToolServiceId
  protocol: ToolProtocol
  key: string
  enabled: boolean
  capability_count: number
  endpoint: string
  package_name: string
  version: string
  timeout_seconds: number
  recommended_variant?: string
  variants?: SkillVariantInfo[]
}

export type ProviderRow = {
  provider: string
  display_name: string
  auth_type?: 'api_key' | 'account'
  supports_search?: boolean
  supports_fetch?: boolean
  available?: boolean
  configured: boolean
  key_hint: string | null
  enabled: boolean
  updated_at: number | null
  account_id?: string
  account_name?: string
  accounts?: Array<{ id: string; name: string; enabled: boolean }>
  max_query_chars?: number | null
  unavailable_reason?: string | null
}

export type ProviderSelection = { search_providers: string[]; fetch_provider: string }

export type ProviderTestResult = {
  success: boolean
  provider?: string
  code?: string
  error?: string
  result_count?: number
  duration_ms?: number
}

export type VisionConfig = {
  provider: string
  provider_label: string
  upstream_model: string
  downstream_model: string
  reasoning: string
  api_key_id: string
  api_key_name: string
  configured: boolean
  key_available: boolean
}

export type VisionKeyOption = { id: string; name: string; key_prefix: string }

export type InstallTarget = {
  key: string
  label: string
  path: string
  detected: boolean
  installed: boolean
}

export type InstallOutcome = {
  clientId: string
  label: string
  ok: boolean
  path: string
  error: string | null
}

export type ApiKeyOption = { id: string; name: string; masked: string }

export type InstallInfo = {
  base_url: string
  service: ToolServiceId
  name: string
  version: string
  timeout_seconds: number
  mcp: { server_name: string; url: string; tools: string[]; bridge_url: string }
  skill?: { name: string; version: string; recommended_variant: string; variants: SkillVariantInfo[] }
}

export const PROTOCOL_LABELS: Record<ToolProtocol, string> = { skill: 'Skill', mcp: 'MCP', api: 'API' }

/**
 * 服务 id → 名称 / 描述（后端只回固定元数据，文案由前端映射）。
 *
 * 在**模块加载期**就取好译文：与侧边栏菜单表同一做法 —— `t('中文')` 的字面量
 * 调用同时是 i18n 扫描器的登记点，写成运行期变量查表会让五份词典永远缺这几条。
 */
export const SERVICE_NAME: Record<string, string> = {
  'web-search': t('网络搜索'),
  vision: t('视觉分析'),
}

export const SERVICE_DESC: Record<string, string> = {
  'web-search': t('聚合标准搜索服务及已有 WorkBuddy、Kiro 账号的搜索能力；网页抓取仍使用标准服务。'),
  vision: t('把图片与提示词交给后台配置的视觉模型分析，并把模型回复返回给调用方的大模型。'),
}

/** 接入协议说明（列表页与详情页共用同一份文案） */
export const PROTOCOL_DESC: Record<ToolProtocol, string> = {
  skill: t('打包为 Agent Skill，由 Agent2API 面板安装到本机客户端目录。'),
  mcp: t('作为远程 MCP 服务，在支持 MCP 的客户端配置里填写地址即可接入。'),
  api: t('提供 REST API，任何能发 HTTP 请求的程序都可以携带网关 Key 直接调用。'),
}

/** 各提供商官方 API Key 页面（新标签页打开） */
export const PROVIDER_KEY_PAGES: Record<string, string> = {
  tavily: 'https://app.tavily.com/home',
  querit: 'https://app.querit.ai/en/dashboard/api-keys',
  firecrawl: 'https://www.firecrawl.dev/app/api-keys',
  ollama: 'https://ollama.com/settings/keys',
}

/** 是否运行在桌面壳里（浏览器 / 网关托管的网页端不显示「本机接入」） */
export function isDesktopShell(): boolean {
  const bridge = (window as unknown as { workbuddyDesktop?: { platform?: string } }).workbuddyDesktop
  return bridge ? bridge.platform !== 'web' : false
}

export function formatTime(value: number | null): string {
  if (!value || !Number.isFinite(value)) return '-'
  try {
    return new Date(value).toLocaleString()
  } catch {
    return '-'
  }
}

/* ─── 管理 API ─────────────────────────────────────────────── */

export const fetchServices = async (protocol?: ToolProtocol): Promise<ToolService[]> =>
  callJson<ToolService[]>('GET', `/api/tools/servers${protocol ? `?protocol=${protocol}` : ''}`)

export const patchServer = (id: string, body: Record<string, unknown>): Promise<ToolService> =>
  callJson<ToolService>('PATCH', `/api/tools/servers/${encodeURIComponent(id)}`, body)

export const fetchProviders = async (): Promise<ProviderRow[]> => {
  const providers = await callJson<ProviderRow[]>('GET', '/api/tools/providers')
  return providers.map(provider => ({ ...provider, display_name: t(provider.display_name) }))
}

export const fetchSelection = (): Promise<ProviderSelection> => callJson('GET', '/api/tools/provider-selection')

export const saveSelection = (value: ProviderSelection): Promise<ProviderSelection> =>
  callJson('PUT', '/api/tools/provider-selection', value)

export const saveProvider = (provider: string, body: { api_key?: string; account_id?: string | null; enabled?: boolean }) =>
  callJson('PUT', `/api/tools/providers/${encodeURIComponent(provider)}`, body)

export const clearProviderKey = (provider: string) =>
  callJson('DELETE', `/api/tools/providers/${encodeURIComponent(provider)}/key`)

export const testProvider = (provider: string): Promise<ProviderTestResult> =>
  callJson('POST', `/api/tools/providers/${encodeURIComponent(provider)}/test`)

export const fetchVision = (): Promise<{ config: VisionConfig | null; keys: VisionKeyOption[] }> =>
  callJson('GET', '/api/tools/vision/config')

export const saveVision = (body: Record<string, unknown>): Promise<VisionConfig> =>
  callJson('PUT', '/api/tools/vision/config', body)

export const clearVision = () => callJson('DELETE', '/api/tools/vision/config')

export const testVision = (): Promise<{ success: boolean; duration_ms?: number; reply?: string; error?: string }> =>
  callJson('POST', '/api/tools/vision/config/test')

export const fetchInstallInfo = (service: ToolServiceId): Promise<InstallInfo> =>
  callJson('GET', `/api/tools/install-info?service=${encodeURIComponent(service)}`)

export const fetchInstallTargets = (service: ToolServiceId, protocol: 'skill' | 'mcp'): Promise<{ targets: InstallTarget[] }> =>
  callJson('GET', `/api/tools/install-targets?service=${encodeURIComponent(service)}&protocol=${protocol}`)

export const fetchApiKeys = (): Promise<{ keys: ApiKeyOption[] }> => callJson('GET', '/api/tools/api-keys')

export const installSkill = (body: { variant: string; api_key_id: string; clients: string[] }) =>
  callJson<{ results: InstallOutcome[] }>('POST', '/api/tools/install-skill', body)

export const installMcp = (body: { service_id: string; mode: string; api_key_id: string; clients: string[] }) =>
  callJson<{ results: InstallOutcome[] }>('POST', '/api/tools/install-mcp', body)

export const uninstallSkill = (body: { service_id: string; clients: string[] }) =>
  callJson<{ results: InstallOutcome[] }>('POST', '/api/tools/uninstall-skill', body)

export const uninstallMcp = (body: { service_id: string; clients: string[] }) =>
  callJson<{ results: InstallOutcome[] }>('POST', '/api/tools/uninstall-mcp', body)

/** 网关基址（MCP 配置示例与 API 文档使用） */
export { gatewayBase }
