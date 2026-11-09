export type UsageQueryStatus = 'running' | 'success' | 'failed' | 'timeout' | 'cancelled' | 'superseded' | 'config_changed'

export type UsageQueryTask = {
  id: string
  requestId: string
  status: UsageQueryStatus
  error?: string | null
  startedAt: number
  deadlineAt: number
}

export type UsageQueryStart = { queries: UsageQueryTask[]; skipped?: number }

export const USAGE_QUERY_POLL_MS = 800

export function usageTaskText(task: UsageQueryTask | undefined, t: (key: string) => string): string {
  switch (task?.status) {
    case 'running': return t('查询中…')
    case 'cancelled': return t('已取消查询')
    case 'config_changed': return t('配置已变更，请重新查询')
    case 'failed': return task.error ? t(task.error) : t('查询失败')
    case 'timeout': return t('查询超时，可重新查询')
    default: return ''
  }
}

export function usageQueryDeadline<T>(promise: Promise<T>, milliseconds = 15_000): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('等待余额查询响应超时，请重新查询')), milliseconds)
    promise.then(value => { clearTimeout(timer); resolve(value) }, error => { clearTimeout(timer); reject(error) })
  })
}
