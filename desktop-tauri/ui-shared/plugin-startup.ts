import type { HiddenAccountProvider, HiddenAccountsSummary } from './plugins'

export type PluginPhase = 'queued' | 'runtime' | 'starting' | 'loading' | 'waiting' | 'registering' | 'ready' | 'failed' | 'timeout' | 'blocked' | 'unsupported' | 'disabled' | 'unknown'
export type PluginLoadStatus = {
  phase: PluginPhase
  attempt: number
  startedAt: number | null
  updatedAt: number
  finishedAt: number | null
  deadlineAt: number | null
  error: string | null
  previousError: string | null
  steps?: { phase: PluginPhase; durationMs: number }[]
}
export type PluginProviderState = {
  id: string; label: string; packId: string; packEnabled: boolean; enabled: boolean; available: boolean; startup: PluginLoadStatus
}
export type PluginStartupSnapshot = {
  bootId: string
  revision: string
  availabilityRevision: string
  pending: boolean
  booting?: boolean
  packs: { id: string; name: string; script: boolean; startup: PluginLoadStatus }[]
  providers: Record<string, PluginProviderState>
  error: string | null
  scriptOperation?: { active: boolean; target?: { id?: string }; error?: string | null; updatedAt?: number }
  now: number
}
type Translate = (key: string, params?: Record<string, string | number>) => string

export function pluginPending(status?: PluginLoadStatus | null): boolean {
  return Boolean(status && ['queued', 'runtime', 'starting', 'loading', 'waiting', 'registering'].includes(status.phase))
}
export function pluginFailed(status?: PluginLoadStatus | null): boolean {
  return status?.phase === 'failed' || status?.phase === 'timeout'
}
export function pluginPhaseLabel(status: Pick<PluginLoadStatus, 'phase'> | null | undefined, t: Translate): string {
  switch (status?.phase) {
    case 'queued': return t('等待加载')
    case 'runtime': return t('准备运行环境')
    case 'starting': return t('正在启动')
    case 'loading': return t('正在加载')
    case 'waiting': return t('等待其他插件')
    case 'registering': return t('即将就绪')
    case 'ready': return t('已就绪')
    case 'failed': return t('加载失败')
    case 'timeout': return t('初始化超时')
    case 'blocked': return t('等待运行环境恢复')
    case 'unsupported': return t('旧插件已停止支持')
    case 'disabled': return t('已停用')
    default: return t('状态待确认')
  }
}
export function pluginLoadSteps(status: PluginLoadStatus | null | undefined, now: number): { phase: PluginPhase; durationMs: number }[] {
  if (!status) return []
  const steps = [...(status.steps || [])]
  if (pluginPending(status) && status.finishedAt == null) steps.push({ phase: status.phase, durationMs: Math.max(0, now - status.updatedAt) })
  return steps
}

export function pluginStepLabel(phase: PluginPhase, t: Translate): string {
  return phase === 'loading' ? t('自身初始化') : pluginPhaseLabel({ phase }, t)
}

export function pluginPhaseDetail(status: PluginLoadStatus | null | undefined, t: Translate, now = Date.now()): string {
  if (status?.error && pluginFailed(status)) return status.error
  switch (status?.phase) {
    case 'queued': return t('等待空闲的加载位置，不代表加载失败。')
    case 'runtime': return t('正在检查或准备脚本运行环境。')
    case 'starting': return t('正在启动插件，账号将在就绪后自动恢复。')
    case 'loading': return now - status.updatedAt > 10_000
      ? t('启动较慢，仍在加载；尚未收到失败结果。') : t('正在初始化插件，尚未收到失败结果。')
    case 'waiting': return t('本插件初始化已完成，正在等待其他插件完成后统一校验和登记。')
    case 'registering': return t('初始化已完成，正在等待校验和登记。')
    case 'ready': return t('插件已就绪，账号列表将自动同步。')
    case 'timeout': return t('本次初始化超过等待时限，已停止；可查看诊断后重试。')
    case 'blocked': return t('共享运行环境尚未就绪，本插件没有明确的加载失败结果。')
    case 'unsupported': return t('旧原生提供商插件已停止支持，请安装对应的 WASM 插件；已有账号和配置保留。')
    case 'failed': return t('本次加载未完成，请查看诊断。')
    default: return t('暂未取得明确的加载结果，不能据此判断启动失败。')
  }
}

type View = { value: PluginStartupSnapshot | null; error: string; receivedAt: number; unavailable: boolean }
let view: View = { value: null, error: '', receivedAt: 0, unavailable: false }
let packStatusMap = new Map<string, PluginLoadStatus>()
const listeners = new Set<() => void>()
export const subscribePluginStartup = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener) } }
export const getPluginStartup = () => view
function publish(patch: Partial<View>) {
  view = { ...view, ...patch }
  if ('value' in patch) {
    packStatusMap = new Map((view.value?.packs || []).map(pack => [pack.id, pack.startup]))
  }
  listeners.forEach(listener => listener())
}
let wake: ((reusePending?: boolean) => void) | undefined
export function refreshPluginStartup() { wake?.() }
export function refreshPluginStartupIfIdle() { wake?.(true) }

export function startPluginStartupPolling(options: {
  read(signal: AbortSignal): Promise<PluginStartupSnapshot>
  refresh(): Promise<unknown>
  session(): PluginStartupSnapshot | null | undefined
  onChange?(): void
}) {
  let disposed = false
  let pending = false
  let again = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let controller: AbortController | undefined
  let syncing: Promise<void> | null = null
  let syncAgain = false
  let lastAvailable = ''
  let failures = 0
  publish({ value: null, error: '', receivedAt: 0, unavailable: false })
  const sync = () => {
    if (syncing) { syncAgain = true; return }
    syncing = (async () => {
      do {
        syncAgain = false
        try { await options.refresh() } catch { /* 会话读取错误由账号页展示 */ }
      } while (!disposed && syncAgain)
    })().finally(() => { syncing = null })
  }
  const poll = async () => {
    if (disposed || document.hidden) return
    if (pending) { again = true; return }
    pending = true
    controller = new AbortController()
    const timeout = setTimeout(() => controller?.abort(), 8_000)
    try {
      const next = await options.read(controller.signal)
      if (disposed || controller.signal.aborted) return
      if (!next || typeof next.bootId !== 'string' || typeof next.revision !== 'string'
        || !next.providers || !Array.isArray(next.packs)) throw new Error('Invalid plugin startup status')
      failures = 0
      const prev = view.value
      const changed = !prev || prev.bootId !== next.bootId || prev.revision !== next.revision
      const metaChanged = !prev
        || prev.availabilityRevision !== next.availabilityRevision
        || Boolean(prev.pending) !== Boolean(next.pending)
        || Boolean(prev.booting) !== Boolean(next.booting)
        || (prev.error || '') !== (next.error || '')
        || view.error !== (next.error || '')
        || view.unavailable
        || prev.scriptOperation?.active !== next.scriptOperation?.active
        || prev.scriptOperation?.target?.id !== next.scriptOperation?.target?.id
        || (prev.scriptOperation?.error || '') !== (next.scriptOperation?.error || '')
        || prev.scriptOperation?.updatedAt !== next.scriptOperation?.updatedAt
      if (changed || metaChanged || next.pending) {
        publish({ value: next, error: next.error || '', receivedAt: Date.now(), unavailable: false })
      }
      if (prev && (prev.bootId !== next.bootId || prev.revision !== next.revision)) options.onChange?.()
      const availability = `${next.bootId}:${next.availabilityRevision}:${Boolean(next.booting)}`
      const session = options.session()
      if ((lastAvailable && lastAvailable !== availability || !session
        || session.bootId !== next.bootId || session.availabilityRevision !== next.availabilityRevision
        || Boolean(session.booting) !== Boolean(next.booting)) && (!next.booting || !session)) sync()
      lastAvailable = availability
    } catch (error) {
      if (disposed) return
      failures += 1
      const status = (error as { status?: number })?.status
      publish({ error: error instanceof Error ? error.message : String(error), unavailable: status === 404 })
    } finally {
      clearTimeout(timeout)
      pending = false
      controller = undefined
      if (!disposed) {
        clearTimeout(timer)
        const delay = again ? 0 : view.unavailable ? 60_000 : failures ? Math.min(30_000, 2_000 * failures)
          : view.value?.pending || view.value?.scriptOperation?.active ? 1500 : 20_000
        again = false
        timer = setTimeout(() => { void poll() }, delay)
      }
    }
  }
  const refresh = (reusePending = false) => {
    if (reusePending && pending) return
    clearTimeout(timer)
    void poll()
  }
  const visibility = () => { if (!document.hidden) refresh(); else clearTimeout(timer) }
  wake = refresh
  document.addEventListener('visibilitychange', visibility)
  refresh()
  return () => {
    disposed = true
    clearTimeout(timer)
    controller?.abort()
    if (wake === refresh) wake = undefined
    document.removeEventListener('visibilitychange', visibility)
  }
}

export function startupForPack(id: string, fallback?: PluginLoadStatus | null): PluginLoadStatus | null | undefined {
  const live = packStatusMap.get(id)
  return fallback && (!live || fallback.updatedAt > live.updatedAt) ? fallback : live || fallback
}
export function hiddenWithStartup(hidden: HiddenAccountsSummary): HiddenAccountsSummary {
  const providers = hidden.providers.map((item): HiddenAccountProvider => {
    if (item.reason === 'replaced_by_template') return item
    const live = view.value?.providers[item.id]
    if (!live || item.startup && item.startup.updatedAt > live.startup.updatedAt) return item
    if (live.startup.phase === 'unsupported') return { ...item, packId: live.packId, startup: live.startup, reason: 'legacy_unsupported' }
    if (!live.enabled) return { ...item, packId: live.packId, startup: live.startup,
      reason: live.packEnabled ? 'provider_disabled' : 'pack_disabled' }
    const startup = live.startup
    const reason = pluginFailed(startup) ? startup.phase === 'timeout' ? 'load_timeout' : 'load_failed'
      : pluginPending(startup) || live.available ? 'loading' : 'not_loaded'
    return { ...item, packId: live.packId, startup, reason }
  })
  return { ...hidden, providers }
}
