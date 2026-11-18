import * as React from 'react'
import type { PluginMutation, PluginOperation, PluginsPayload, ScriptOperation } from './plugins'
import { isRecommendationInstalled, reinstallRequest } from './plugins-script-display'
import { refreshPluginStartup } from './plugin-startup'
import type { PluginBatchAction, PluginBatchItem } from './plugins-batch-items'
import type { PluginTranslate } from './plugins-store'

export type PluginBatchResult = {
  item: PluginBatchItem
  state: 'waiting' | 'working' | 'succeeded' | 'failed' | 'warning' | 'unconfirmed'
  message?: string
  percent?: number | null
}
export type PluginBatchTask = { action: PluginBatchAction; running: boolean; results: PluginBatchResult[] }
export type PluginRequest = <T>(method: string, path: string, body?: unknown, options?: { retry: 'never' }) => Promise<T>

export class PluginOperationUnconfirmed extends Error {}

const uncertain = (cause: unknown) => /超时|timeout|timed out|network|fetch|连接|网络/i.test(cause instanceof Error ? cause.message : String(cause))
const pause = () => new Promise(resolve => window.setTimeout(resolve, 1000))
const defaultMessage = (t: PluginTranslate) => t('结果尚未确认，请刷新插件状态后再操作')

export async function waitPackOperation(initial: PluginOperation, request: PluginRequest, t: PluginTranslate,
  changed: (operation: PluginOperation) => void = () => {}): Promise<PluginOperation> {
  let operation = initial
  const deadline = performance.now() + 15 * 60_000
  changed(operation)
  while (operation.active) {
    if (performance.now() >= deadline) throw new PluginOperationUnconfirmed(defaultMessage(t))
    await pause()
    let progress: { operations: PluginOperation[] }
    try { progress = await request('GET', '/api/plugins/progress') }
    catch { throw new PluginOperationUnconfirmed(defaultMessage(t)) }
    const next = progress.operations?.find(item => item.id === initial.id && (initial.operationId
      ? item.operationId === initial.operationId : item.updatedAt >= initial.updatedAt))
    if (!next) throw new PluginOperationUnconfirmed(defaultMessage(t))
    operation = next
    changed(operation)
  }
  return operation
}

export async function waitScriptOperation(initial: ScriptOperation, request: PluginRequest, t: PluginTranslate,
  changed: (operation: ScriptOperation) => void = () => {}): Promise<ScriptOperation> {
  let operation = initial
  const deadline = performance.now() + 15 * 60_000
  changed(operation)
  while (operation.active) {
    if (performance.now() >= deadline) throw new PluginOperationUnconfirmed(defaultMessage(t))
    await pause()
    let payload: PluginsPayload
    try { payload = await request('GET', '/api/plugins') }
    catch { throw new PluginOperationUnconfirmed(defaultMessage(t)) }
    const next = payload.scripts?.operation
    // 旧网关没有任务编号，至少保留开始时刻或来源与动作的一致性。
    const same = next && (initial.operationId ? next.operationId === initial.operationId : initial.startedAt
      ? next.startedAt === initial.startedAt : next.action === initial.action && next.target?.source === initial.target?.source && next.target?.id === initial.target?.id)
    if (!next || !same) throw new PluginOperationUnconfirmed(defaultMessage(t))
    operation = next
    changed(operation)
  }
  return operation
}

function packStage(operation: PluginOperation, t: PluginTranslate): string {
  if (operation.stage === 'downloading') return t('正在下载…')
  if (operation.stage === 'verifying') return t('正在校验包内容…')
  if (operation.stage === 'waiting') return t('等待当前任务结束，插件继续服务…')
  if (operation.stage === 'loading') return t('正在加载新插件…')
  if (operation.stage === 'restoring') return t('正在恢复旧版本插件…')
  return t('正在安装…')
}

export function usePluginBatch({ request, refresh, synchronize, t, disabled, target, completed, scope = '' }: {
  request: PluginRequest
  refresh: () => Promise<void>
  synchronize: () => Promise<void>
  t: PluginTranslate
  disabled: boolean
  target: string
  scope?: string
  completed?: (items: PluginBatchItem[]) => void
}) {
  const [task, setTask] = React.useState<PluginBatchTask | null>(null)
  const running = React.useRef(false)
  const alive = React.useRef(true)
  const scopeRef = React.useRef(scope)
  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  React.useEffect(() => {
    if (scopeRef.current === scope) return
    scopeRef.current = scope
    if (!running.current) setTask(null)
  }, [scope])

  const execute = async (action: PluginBatchAction, items: PluginBatchItem[]): Promise<void> => {
    if (disabled || running.current || !items.length) return
    running.current = true
    const results: PluginBatchResult[] = items.map(item => ({ item, state: 'waiting' }))
    const publish = () => { if (alive.current && scopeRef.current === scope) setTask({ action, running: running.current, results: [...results] }) }
    const finish = (index: number, patch: Partial<PluginBatchResult>) => { results[index] = { ...results[index], ...patch }; publish() }
    const successful: PluginBatchItem[] = []
    publish()
    // 任一结果未确认时，不提交下一项，以免覆盖脚本宿主的单任务进度。
    try {
      for (const [index, item] of items.entries()) {
        if (!alive.current || scopeRef.current !== scope) return
        finish(index, { state: 'working', message: t('正在处理…') })
        try {
          let notice = ''
          if (item.kind === 'pack') {
            if (action === 'install' || action === 'update') {
              let response: { operation: PluginOperation }
              try { response = await request('POST', action === 'update' ? '/api/plugins/update' : '/api/plugins/install-from-release', { id: item.id, target }, { retry: 'never' }) }
              catch (cause) { if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(t)); throw cause }
              if (!response?.operation) throw new PluginOperationUnconfirmed(defaultMessage(t))
              const operation = await waitPackOperation(response.operation, request, t, operation => {
                finish(index, { message: packStage(operation, t), percent: operation.stage === 'downloading' ? operation.percent : null })
              })
              if (operation.error) throw new Error(operation.error)
              if (operation.hostError) notice = operation.hostError
              else if (!operation.done) throw new Error(t('插件安装未完成'))
              notice ||= operation.result?.installNotice || ''
            } else {
              let result: PluginMutation
              try { result = await request('POST', action === 'uninstall' ? '/api/plugins/uninstall' : '/api/plugins/enable',
                action === 'uninstall' ? { id: item.id } : { id: item.id, enabled: action === 'enable' }, { retry: 'never' }) }
              catch (cause) { if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(t)); throw cause }
              notice = result.hostError || result.installNotice || ''
            }
          } else {
            const body = action === 'install' ? { name: item.source, source: item.source, version: item.selector, trusted: true }
              : action === 'update' && item.script ? { ...reinstallRequest(item.script), trusted: true }
                : { id: item.id, action: action === 'uninstall' ? 'remove' : 'enable', enabled: action === 'enable' }
            let response: { operation: ScriptOperation }
            try { response = await request('POST', action === 'install' || action === 'update' ? '/api/plugins/scripts/install' : '/api/plugins/scripts/change', body, { retry: 'never' }) }
            catch (cause) { if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(t)); throw cause }
            if (!response?.operation) throw new PluginOperationUnconfirmed(defaultMessage(t))
            const operation = await waitScriptOperation(response.operation, request, t, operation => finish(index, { message: operation.message || t('正在处理…') }))
            if (operation.error || operation.stage === 'failed') throw new Error(operation.error || t('插件操作失败'))
          }
          if (action === 'install' && item.required) {
            let state: PluginsPayload
            try { state = await request('GET', '/api/plugins') }
            catch { throw new PluginOperationUnconfirmed(defaultMessage(t)) }
            const installed = item.kind === 'pack' ? state.packs.find(pack => pack.id === item.id)
              : state.scripts?.packages.find(pack => item.recommendation && isRecommendationInstalled(item.recommendation, [pack]))
            if (!installed) throw new PluginOperationUnconfirmed(defaultMessage(t))
            if (!installed.enabled) notice ||= t('插件已安装，原有停用状态已保留')
            else if (installed.startup && installed.startup.phase !== 'ready') notice ||= t('插件已安装，但仍未就绪，请查看加载状态')
            else if (state.hiddenAccounts?.providers.some(provider => item.providers.includes(provider.id))) notice ||= t('插件已安装，部分账号仍不可用，请查看账号状态')
          }
          finish(index, { state: notice ? 'warning' : 'succeeded', message: notice || undefined, percent: 100 })
          successful.push(item)
          refreshPluginStartup()
        } catch (cause) {
          const unconfirmed = cause instanceof PluginOperationUnconfirmed
          finish(index, { state: unconfirmed ? 'unconfirmed' : 'failed', message: cause instanceof Error ? cause.message : String(cause), percent: null })
          if (unconfirmed) {
            for (let pending = index + 1; pending < results.length; pending++) finish(pending, { message: t('本批已暂停，刷新状态后可继续剩余项') })
            break
          }
        }
        try { await refresh() } catch {}
      }
      if (!alive.current || scopeRef.current !== scope) return
      completed?.(successful)
      try { await synchronize(); await refresh() }
      catch (cause) {
        if (alive.current) {
          const index = results.findIndex(result => result.state === 'succeeded')
          if (index >= 0) finish(index, { state: 'warning', message: t('插件已变更，但刷新界面失败：{error}', { error: cause instanceof Error ? cause.message : String(cause) }) })
        }
      }
    } finally { running.current = false; if (scopeRef.current !== scope && alive.current) setTask(null); else publish() }
  }
  return { task, running: task?.running === true, execute, clear: () => { if (!running.current) setTask(null) } }
}
