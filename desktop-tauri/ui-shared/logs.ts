export type LogView = 'important' | 'all' | 'debug'
export type LogEntry = {
  id?: number
  ts?: number
  level?: string
  category?: string
  message?: string
  data?: Record<string, unknown> | null
}
export type LogGroup = {
  id: number
  firstTs: number
  lastTs: number
  count: number
  entry: LogEntry
  entries: LogEntry[]
}
export type LogPage = {
  entries?: LogEntry[]
  groups?: LogGroup[]
  total?: number
  matched?: number
  grouped?: number
  offset?: number
  limit?: number
  lastId?: number
  byLevel?: Record<string, number>
  categories?: Record<string, string>
  debugEnabled?: boolean
}
export type LogQuery = {
  view?: LogView
  aggregate?: boolean
  offset?: number
  limit?: number
  untilId?: number
  sinceId?: number
  level?: string
  category?: string
  keyword?: string
  start?: number
  end?: number
  startDate?: string
  endDate?: string
}

type Translate = (text: string, params?: Record<string, string | number>) => string

export function logQueryString(query: LogQuery): string {
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== '') params.set(key, typeof value === 'boolean' ? (value ? '1' : '0') : String(value))
  }
  return params.toString()
}

export function logGroups(page: LogPage | null | undefined): LogGroup[] {
  if (Array.isArray(page?.groups)) return page.groups
  return (page?.entries || []).map((entry, index) => ({
    id: entry.id ?? index, firstTs: entry.ts ?? 0, lastTs: entry.ts ?? 0,
    count: 1, entry, entries: [entry],
  }))
}

export function logText(entry: LogEntry, key: string): string {
  const value = entry.data?.[key]
  return typeof value === 'string' || typeof value === 'number' ? String(value) : ''
}

export function logLevel(level: string | undefined, t: Translate): string {
  switch (level) {
    case 'debug': return t('调试')
    case 'warn': return t('警告')
    case 'error': return t('错误')
    default: return t('信息')
  }
}

export function logCategory(category: string | undefined, t: Translate): string {
  switch (category) {
    case 'server': return t('服务')
    case 'auth': return t('登录')
    case 'account': return t('账号')
    case 'model': return t('模型')
    case 'upstream': return t('上游')
    case 'config': return t('配置')
    case 'checkin': return t('自动签到')
    case 'maintenance': return t('凭证自动维护')
    case 'update': return t('软件版本检查')
    case 'desensitize': return t('脱敏')
    case 'plugin': return t('插件')
    default: return category || '—'
  }
}

export function logSource(entry: LogEntry, t: Translate): string {
  return logText(entry, 'pluginName') || logText(entry, 'providerLabel') || logText(entry, 'provider')
    || logText(entry, 'component') || logCategory(entry.category, t)
}

export function logSummary(entry: LogEntry, t: Translate): string {
  if (logText(entry, 'action') === 'plugin.startup.summary') {
    const ready = logNumber(entry, 'readyCount') ?? 0
    const failed = logNumber(entry, 'failedCount') ?? 0
    const blocked = logNumber(entry, 'blockedCount') ?? 0
    if (!failed && !blocked) return t('插件启动完成，{n} 个已就绪', { n: ready })
    return t('插件启动存在异常')
  }
  if (logText(entry, 'action') === 'plugin.startup') {
    switch (logText(entry, 'phase')) {
      case 'ready': return t('插件已就绪')
      case 'failed': return t('插件启动失败')
      case 'timeout': return t('插件启动超时')
      case 'blocked': return t('插件暂不可用，等待运行环境恢复')
      default: return t('插件未能就绪')
    }
  }
  switch (logText(entry, 'action')) {
    case 'plugin.failure': return t('插件暂时无法使用')
    case 'plugin.exit': return t('插件进程已退出')
    case 'plugin.recovered': return t('插件已恢复运行')
    case 'plugin.shutdown': return t('插件未能正常停止')
  }
  const summary = logText(entry, 'summary') || (entry.message || '').replace(/^(?:\[[^\]]+\]\s*)+/, '')
  switch (summary) {
    case '计费接口请求': return t('计费接口请求')
    case '上游接口请求': return t('上游接口请求')
    case '出站参数归一': return t('出站参数归一')
    case '补充系统消息': return t('补充系统消息')
    case '余额查询完成': return t('余额查询完成')
    case '余额查询未配置': return t('余额查询未配置')
    case '余额查询失败': return t('余额查询失败')
    default: return summary || '—'
  }
}

export function logNumber(entry: LogEntry, key: string): number | undefined {
  const value = entry.data?.[key]
  if (typeof value !== 'number' && (typeof value !== 'string' || !value.trim())) return undefined
  const number = Number(value)
  return Number.isFinite(number) && number >= 0 ? number : undefined
}

export function formatLogDuration(ms: number | undefined, t: Translate): string {
  if (ms === undefined) return ''
  if (ms < 1000) return t('{n} 毫秒', { n: Math.round(ms) })
  if (ms < 60_000) return t('{n} 秒', { n: Number((ms / 1000).toFixed(1)) })
  const seconds = Math.round(ms / 1000)
  return t('{minutes} 分 {seconds} 秒', { minutes: Math.floor(seconds / 60), seconds: seconds % 60 })
}

export function logContext(entry: LogEntry, t: Translate): string {
  const parts = [logText(entry, 'accountName') || logText(entry, 'account'), logText(entry, 'model')]
  const from = logText(entry, 'from')
  const to = logText(entry, 'to')
  if (from || to) parts.push([from, to].filter(Boolean).join(' → '))
  const action = logText(entry, 'action')
  if (action === 'plugin.startup.summary') {
    const ready = logNumber(entry, 'readyCount') ?? 0
    const failed = logNumber(entry, 'failedCount') ?? 0
    const blocked = logNumber(entry, 'blockedCount') ?? 0
    if (failed || blocked) parts.push(t('{n} 个已就绪', { n: ready }))
    if (failed) parts.push(t('{n} 个启动失败', { n: failed }))
    if (blocked) parts.push(t('{n} 个等待恢复', { n: blocked }))
  }
  const duration = formatLogDuration(logNumber(entry, 'durationMs'), t)
  if (duration) parts.push(t('用时 {duration}', { duration }))
  const status = logText(entry, 'status')
  if (status) parts.push(`HTTP ${status}`)
  if (action === 'plugin.startup' && logText(entry, 'phase') !== 'ready') {
    parts.push(t('相关功能暂不可用，原因见详情'))
  } else if (action === 'plugin.failure' || action === 'plugin.exit') {
    parts.push(t('相关功能暂不可用，原因见详情'))
  } else {
    const detail = logText(entry, 'detail')
    if (detail && entry.level !== 'debug' && !action.startsWith('plugin.')) parts.push(detail)
  }
  return parts.filter(Boolean).join(' · ')
}

export function logDetailText(entry: LogEntry): string {
  const detail = logText(entry, 'detail')
  const action = logText(entry, 'action')
  if (!['plugin.failure', 'plugin.exit', 'plugin.shutdown', 'plugin.recovered'].includes(action)) return detail
  const message = (entry.message || '').replace(/^(?:(?:\[[^\]]+\]|Plugin)\s*)+/, '')
  return [...new Set([message, detail].filter(Boolean))].join('\n\n')
}

export function logAdvice(entry: LogEntry, t: Translate): string {
  const action = logText(entry, 'action')
  if (action === 'plugin.startup.summary') {
    return (logNumber(entry, 'failedCount') || logNumber(entry, 'blockedCount'))
      ? t('部分插件暂不可用。请在插件页查看异常插件的加载诊断。') : ''
  }
  if (action === 'plugin.startup') {
    switch (logText(entry, 'phase')) {
      case 'ready': return ''
      case 'timeout': return t('本次启动超过等待时限。请在插件页查看加载诊断，确认后重新加载。')
      case 'blocked': return t('共享运行环境尚未就绪，不代表这个插件本身损坏。请在插件页查看加载诊断。')
      default: return t('请在插件页查看加载诊断，确认原因后重新加载。')
    }
  }
  if (['plugin.failure', 'plugin.exit', 'plugin.shutdown'].includes(action)) {
    return t('请在插件页查看加载诊断，确认原因后重新加载。')
  }
  return ''
}

export function logPhase(phase: string, t: Translate): string {
  switch (phase) {
    case 'queued': return t('等待加载')
    case 'runtime': return t('准备运行环境')
    case 'starting': return t('启动进程')
    case 'loading': return t('加载插件')
    case 'registering': return t('登记提供商')
    case 'ready': return t('已就绪')
    case 'failed': return t('启动失败')
    case 'timeout': return t('启动超时')
    case 'blocked': return t('等待恢复')
    default: return t('结果未确认')
  }
}

export function logSteps(entry: LogEntry, t: Translate): { label: string; duration: string }[] {
  const steps = entry.data?.steps
  if (!Array.isArray(steps)) return []
  return steps.flatMap(step => {
    if (!step || typeof step !== 'object' || typeof step.phase !== 'string'
      || typeof step.durationMs !== 'number' || !Number.isFinite(step.durationMs) || step.durationMs < 0) return []
    return [{ label: logPhase(step.phase, t), duration: formatLogDuration(step.durationMs, t) }]
  })
}

export function logPlugins(entry: LogEntry, t: Translate): { id: string; name: string; status: string; phase: string }[] {
  const plugins = entry.data?.plugins
  if (!Array.isArray(plugins)) return []
  return plugins.flatMap(plugin => {
    if (!plugin || typeof plugin !== 'object' || typeof plugin.pluginId !== 'string' || typeof plugin.phase !== 'string') return []
    return [{ id: plugin.pluginId, name: typeof plugin.pluginName === 'string' && plugin.pluginName ? plugin.pluginName : t('未记录插件名称'),
      status: logPhase(plugin.phase, t), phase: plugin.phase }]
  }).sort((left, right) => Number(left.phase === 'ready') - Number(right.phase === 'ready'))
}

export function logViewNote(view: LogView, t: Translate): string {
  switch (view) {
    case 'important': return t('显示启动汇总和需要关注的事件；单个插件的正常启动记录在“全部事件”中。')
    case 'all': return t('显示所有已保留的事件；技术信息可在详情中查看。')
    default: return t('仅显示调试记录，供排查问题使用。')
  }
}

export function formatLogTime(ts: number | undefined, clockOnly = false): string {
  if (!ts) return '—'
  const date = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  const clock = `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
  if (clockOnly) return clock
  const day = `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
  return date.getFullYear() === new Date().getFullYear() ? `${day} ${clock}` : `${date.getFullYear()}-${day} ${clock}`
}

export function groupTime(group: LogGroup): string {
  if (group.firstTs === group.lastTs) return formatLogTime(group.lastTs)
  const sameDay = new Date(group.firstTs).toDateString() === new Date(group.lastTs).toDateString()
  return `${formatLogTime(group.firstTs)}–${formatLogTime(group.lastTs, sameDay)}`
}

export function logRawText(entries: LogEntry[]): string {
  return entries.map(entry => JSON.stringify(entry)).join('\n')
}
