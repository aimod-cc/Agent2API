import * as React from 'react'
import type { PluginMutation, PluginOperation, PluginsPayload, ScriptOperation } from './plugins'
import { isRecommendationInstalled, reinstallRequest } from './plugins-script-display'
import { refreshPluginStartup } from './plugin-startup'
import type { PluginBatchAction, PluginBatchItem } from './plugins-batch-items'
import type { PluginTranslate } from './plugins-store'

export type PluginBatchPhase = 'ready' | 'running' | 'done'
export type PluginBatchResult = {
  item: PluginBatchItem
  state: 'waiting' | 'working' | 'succeeded' | 'failed' | 'warning' | 'unconfirmed'
  message?: string
  percent?: number | null
}
export type PluginBatchTask = { action: PluginBatchAction; phase: PluginBatchPhase; running: boolean; results: PluginBatchResult[] }
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
  const [open, setOpen] = React.useState(false)
  const [task, setTask] = React.useState<PluginBatchTask | null>(null)
  const taskRef = React.useRef<PluginBatchTask | null>(null)
  const running = React.useRef(false)
  const sessionRef = React.useRef(0)
  const alive = React.useRef(true)
  const scopeRef = React.useRef(scope)
  const disabledRef = React.useRef(disabled)
  disabledRef.current = disabled
  const targetRef = React.useRef(target)
  targetRef.current = target
  const requestRef = React.useRef(request)
  requestRef.current = request
  const refreshRef = React.useRef(refresh)
  refreshRef.current = refresh
  const synchronizeRef = React.useRef(synchronize)
  synchronizeRef.current = synchronize
  const completedRef = React.useRef(completed)
  completedRef.current = completed
  const tRef = React.useRef(t)
  tRef.current = t

  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  React.useEffect(() => {
    if (scopeRef.current === scope) return
    scopeRef.current = scope
    if (!running.current) {
      sessionRef.current += 1
      taskRef.current = null
      setTask(null)
      setOpen(false)
    }
  }, [scope])

  const commit = React.useCallback((next: PluginBatchTask | null) => {
    taskRef.current = next
    if (alive.current && scopeRef.current === scope) setTask(next)
  }, [scope])

  const prepare = React.useCallback((action: PluginBatchAction, items: PluginBatchItem[]) => {
    if (disabledRef.current || running.current || !items.length) return
    sessionRef.current += 1
    commit({
      action,
      phase: 'ready',
      running: false,
      results: items.map(item => ({ item, state: 'waiting' })),
    })
    setOpen(true)
  }, [commit])

  const remove = React.useCallback((key: string) => {
    if (running.current) return
    const current = taskRef.current
    if (!current || current.phase !== 'ready') return
    commit({ ...current, results: current.results.filter(row => row.item.key !== key) })
  }, [commit])

  const runBatch = React.useCallback(async (
    action: PluginBatchAction,
    initialResults: PluginBatchResult[],
    indices: number[],
  ): Promise<void> => {
    if (disabledRef.current || running.current || !indices.length) return
    const session = ++sessionRef.current
    running.current = true
    const results: PluginBatchResult[] = initialResults.map(row => ({ ...row }))
    for (const index of indices) results[index] = { item: results[index].item, state: 'waiting' }
    const publish = (phase: PluginBatchPhase) => {
      if (alive.current && scopeRef.current === scope && sessionRef.current === session) {
        const next: PluginBatchTask = { action, phase, running: phase === 'running', results: [...results] }
        taskRef.current = next
        setTask(next)
      }
    }
    const finish = (index: number, patch: Partial<PluginBatchResult>) => {
      results[index] = { ...results[index], ...patch }
      publish('running')
    }
    const successful: PluginBatchItem[] = []
    const translate = tRef.current
    const req = requestRef.current
    publish('running')
    // 任一结果未确认时，不提交下一项，以免覆盖脚本宿主的单任务进度。
    try {
      for (let pos = 0; pos < indices.length; pos++) {
        const index = indices[pos]
        const item = results[index].item
        if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
        finish(index, { state: 'working', message: translate('正在处理…'), percent: null })
        try {
          let notice = ''
          if (item.kind === 'pack') {
            if (action === 'install' || action === 'update') {
              let response: { operation: PluginOperation }
              try {
                response = await req(
                  'POST',
                  action === 'update' ? '/api/plugins/update' : '/api/plugins/install-from-release',
                  { id: item.id, target: targetRef.current },
                  { retry: 'never' },
                )
              } catch (cause) {
                if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(translate))
                throw cause
              }
              if (!response?.operation) throw new PluginOperationUnconfirmed(defaultMessage(translate))
              const operation = await waitPackOperation(response.operation, req, translate, op => {
                finish(index, { message: packStage(op, translate), percent: op.stage === 'downloading' ? op.percent : null })
              })
              if (operation.error) throw new Error(operation.error)
              if (operation.hostError) notice = operation.hostError
              else if (!operation.done) throw new Error(translate('插件安装未完成'))
              notice ||= operation.result?.installNotice || ''
            } else {
              let result: PluginMutation
              try {
                result = await req(
                  'POST',
                  action === 'uninstall' ? '/api/plugins/uninstall' : '/api/plugins/enable',
                  action === 'uninstall' ? { id: item.id } : { id: item.id, enabled: action === 'enable' },
                  { retry: 'never' },
                )
              } catch (cause) {
                if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(translate))
                throw cause
              }
              notice = result.hostError || result.installNotice || ''
            }
          } else {
            const body = action === 'install'
              ? { name: item.source, source: item.source, version: item.selector, trusted: true }
              : action === 'update' && item.script
                ? { ...reinstallRequest(item.script), trusted: true }
                : { id: item.id, action: action === 'uninstall' ? 'remove' : 'enable', enabled: action === 'enable' }
            let response: { operation: ScriptOperation }
            try {
              response = await req(
                'POST',
                action === 'install' || action === 'update' ? '/api/plugins/scripts/install' : '/api/plugins/scripts/change',
                body,
                { retry: 'never' },
              )
            } catch (cause) {
              if (uncertain(cause)) throw new PluginOperationUnconfirmed(defaultMessage(translate))
              throw cause
            }
            if (!response?.operation) throw new PluginOperationUnconfirmed(defaultMessage(translate))
            const operation = await waitScriptOperation(response.operation, req, translate, op => (
              finish(index, { message: op.message || translate('正在处理…') })
            ))
            if (operation.error || operation.stage === 'failed') throw new Error(operation.error || translate('插件操作失败'))
          }
          if (action === 'install' && item.required) {
            let state: PluginsPayload
            try { state = await req('GET', '/api/plugins') }
            catch { throw new PluginOperationUnconfirmed(defaultMessage(translate)) }
            const installed = item.kind === 'pack'
              ? state.packs.find(pack => pack.id === item.id)
              : state.scripts?.packages.find(pack => (item.id && pack.id === item.id)
                || (item.recommendation && isRecommendationInstalled(item.recommendation, [pack])))
            if (!installed) throw new PluginOperationUnconfirmed(defaultMessage(translate))
            if (!installed.enabled) notice ||= translate('插件已安装，原有停用状态已保留')
            else if (installed.startup && installed.startup.phase !== 'ready') notice ||= translate('插件已安装，但仍未就绪，请查看加载状态')
            else if (state.hiddenAccounts?.providers.some(provider => item.providers.includes(provider.id))) notice ||= translate('插件已安装，部分账号仍不可用，请查看账号状态')
          }
          finish(index, { state: notice ? 'warning' : 'succeeded', message: notice || undefined, percent: 100 })
          successful.push(item)
          refreshPluginStartup()
        } catch (cause) {
          const unconfirmed = cause instanceof PluginOperationUnconfirmed
          finish(index, { state: unconfirmed ? 'unconfirmed' : 'failed', message: cause instanceof Error ? cause.message : String(cause), percent: null })
          if (unconfirmed) {
            for (let nextPos = pos + 1; nextPos < indices.length; nextPos++) {
              finish(indices[nextPos], { message: translate('本批已暂停，刷新状态后可继续剩余项') })
            }
            break
          }
        }
        try { await refreshRef.current() } catch {}
      }
      if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
      completedRef.current?.(successful)
      try { await synchronizeRef.current(); await refreshRef.current() }
      catch (cause) {
        if (alive.current && sessionRef.current === session) {
          const index = results.findIndex(result => result.state === 'succeeded')
          if (index >= 0) finish(index, { state: 'warning', message: translate('插件已变更，但刷新界面失败：{error}', { error: cause instanceof Error ? cause.message : String(cause) }) })
        }
      }
    } finally {
      if (sessionRef.current === session) {
        running.current = false
        if (scopeRef.current !== scope && alive.current) {
          taskRef.current = null
          setTask(null)
          setOpen(false)
        } else {
          publish('done')
        }
      }
    }
  }, [scope])

  const start = React.useCallback(async (retry = false): Promise<void> => {
    const current = taskRef.current
    if (!current || disabledRef.current || running.current || current.results.some(row => row.state === 'unconfirmed')) return
    const targetState = retry ? 'failed' : 'waiting'
    const indices = current.results.flatMap((row, index) => (row.state === targetState ? [index] : []))
    await runBatch(current.action, current.results, indices)
  }, [runBatch])

  const execute = React.useCallback(async (action: PluginBatchAction, items: PluginBatchItem[]): Promise<void> => {
    if (disabledRef.current || running.current || !items.length) return
    const initialResults: PluginBatchResult[] = items.map(item => ({ item, state: 'waiting' }))
    setOpen(true)
    await runBatch(action, initialResults, items.map((_, index) => index))
  }, [runBatch])

  const clear = React.useCallback(() => {
    if (running.current) return
    sessionRef.current += 1
    taskRef.current = null
    setTask(null)
    setOpen(false)
  }, [])

  const close = React.useCallback(() => {
    // 批量执行中允许收起到后台进度条；其余阶段关闭直接清理本轮记录，避免页面残留。
    if (running.current) {
      setOpen(false)
      return
    }
    clear()
  }, [clear])

  const backgroundVisible = !open && task !== null && (task.phase === 'running' || task.phase === 'done') && task.results.length > 0

  return {
    task,
    open,
    setOpen,
    close,
    clear,
    prepare,
    remove,
    start,
    execute,
    running: task?.running === true,
    backgroundVisible,
  }
}

export type PluginBatchModel = ReturnType<typeof usePluginBatch>
