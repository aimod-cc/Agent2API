import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import type { FlowPath, FlowSelection } from './routing-flow-types'
import type { ReplayRow, RoutingWorkspace } from './routing-types'
import { isTraceRunning, isTraceSuccess } from './routing-trace-model'

export const clientLabel = (client: string) => !client ? t('未识别客户端') : client === 'other' ? t('其他客户端') : client
export const clientIcon = (client: string) => ({ 'Claude Code': 'claude-code.svg', Codex: 'codex.png', ZCode: 'zcode.png' } as Record<string, string>)[client]
export const sessionId = (row: ReplayRow) => row.route?.sessionId || row.route?.stickyId || ''
export const requestModelId = (row: ReplayRow) => row.clientModel || row.model || ''
export const selectedRequest = (selection: FlowSelection) => selection?.kind === 'request' ? selection.row : selection?.kind === 'session' ? selection.session.latest : null
export function pathForRequest(row: ReplayRow): FlowPath {
  return { id: JSON.stringify([row.client || '', requestModelId(row), row.provider || '', row.accountId || '', row.accountId ? '' : row.accountName || '']),
    client: row.client || '', model: requestModelId(row), provider: row.provider || '', accountId: row.accountId || '', accountName: row.accountName || '',
    requests: 1, running: Number(isTraceRunning(row)), successful: Number(isTraceSuccess(row)), failed: Number(!isTraceRunning(row) && !isTraceSuccess(row)),
    sessions: Number(Boolean(sessionId(row))), stickyHits: Number(Boolean(row.route?.stickyHit)), switched: Number((row.attempts || 1) > 1), lastSeen: row.ts }
}
export function matchesSelection(path: FlowPath, selection: FlowSelection): boolean {
  if (!selection) return true
  if (selection.kind === 'path') return path.id === selection.id
  if (selection.kind === 'edge') return path.model === selection.model && (selection.client == null || path.client === selection.client)
    && (selection.accountId == null || path.accountId === selection.accountId) && (selection.provider == null || path.provider === selection.provider)
  if (selection.kind === 'account') return path.accountId === selection.id
  if (selection.kind === 'client') return path.client === selection.id
  if (selection.kind === 'model') return path.model.toLowerCase() === selection.id.toLowerCase()
  if (selection.kind === 'provider') return path.provider === selection.id
  const row = selectedRequest(selection)
  return Boolean(row && path.id === pathForRequest(row).id)
}
export const sumPaths = (paths: FlowPath[], key: 'requests' | 'running' | 'sessions' | 'stickyHits') => paths.reduce((sum, path) => sum + path[key], 0)
export function accountParticipation(workspace: RoutingWorkspace, account: AccountRecord): boolean {
  const profile = workspace.profile, provider = providerOf(account)
  return (profile.providerScope === 'all' || profile.providers.includes(provider))
    && !profile.disabledProviders.includes(provider) && !profile.excludedAccounts.includes(account.id)
}
export function accountFlowState(workspace: RoutingWorkspace, account: AccountRecord, now: number) {
  const fact = workspace.accountStates?.[account.id]
  const entry = [...workspace.queue.ordered, ...workspace.queue.blocked].find(entry => entry.id === account.id)
  if (!workspace.model && fact?.limitedModels) return { state: 'partial', label: t('{n} 个模型限流', { n: fact.limitedModels }), reason: t('选择请求模型后查看具体限流状态'), resetAt: 0 }
  if ((fact?.resetAt || 0) > now) return { state: 'cooldown', label: t('此模型限流'), reason: fact?.model || '', resetAt: fact!.resetAt }
  if (account.enabled === false) return { state: 'disabled', label: t('已禁用'), reason: '', resetAt: 0 }
  if (account.hasCredentials === false) return { state: 'no-credentials', label: t('无凭证'), reason: '', resetAt: 0 }
  if (!accountParticipation(workspace, account)) return { state: 'excluded', label: t('当前配置已排除'), reason: '', resetAt: 0 }
  const state = entry?.state || 'unready'
  const label = state === 'usable' ? workspace.metrics.accountRunning?.[account.id] ? t('当前承载') : t('可用待命') : state === 'concurrency' ? t('并发已满') : state === 'limiter' ? t('限制器跳过') : state === 'cooldown' ? t('冷却中') : t('未就绪')
  return { state, label, reason: entry?.reason || '', resetAt: entry?.resetAt || 0 }
}
