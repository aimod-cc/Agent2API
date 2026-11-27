export type LaunchClient = { id: string; label: string; platform: 'windows' }
export type LaunchSelection = { provider: string; account: string; client: string }
export type LaunchPhase = 'awaiting_confirmation' | 'preparing' | 'closing' | 'writing' | 'reloading' | 'starting' | 'started' | 'switched' | 'committed' | 'failed' | 'canceled' | 'restored' | 'recovery_required'
export type LaunchProxy = {
  supported: boolean
  configured: boolean
  enabled: boolean
  available: boolean
  label: string
  address: string
  error: string | null
}
export type LaunchResult = {
  intent: string
  provider: string
  account: string
  client: { id: string; label: string }
  phase: LaunchPhase
  message: string
  error: boolean
  committed: boolean
  started: boolean
  restored: boolean
  running: boolean
  requiresRestart: boolean
  hotSwitch: boolean
  switchOnly: boolean
  proxy: LaunchProxy
  executable: string
  resources: Array<{ id: string; label: string; location: string }>
  backup: string | null
  pendingBackup: string | null
  remainingMs: number
  canRestore: boolean
  canRetryStart: boolean
}
export type LaunchSettings = { provider: string; client: string; path: string; args: string; switchOnly: boolean; detected: string | null; error: string | null; revision: string }
export type LaunchPendingRecovery = { backup: string; provider: string; account: string; client: string; label: string; executable: string }
export type LaunchRequest = <T>(method: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown, options?: { retry: 'never' }) => Promise<T>
export type LaunchTranslate = (key: string, vars?: Record<string, string | number>) => string

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value)
const string = (value: unknown, maximum = 4096): value is string => typeof value === 'string' && value.length <= maximum && !/[\u0000-\u001f\u007f]/.test(value)
const phases: readonly string[] = ['awaiting_confirmation', 'preparing', 'closing', 'writing', 'reloading', 'starting', 'started', 'switched', 'committed', 'failed', 'canceled', 'restored', 'recovery_required']

export function launchClients(raw: unknown, provider: string): LaunchClient[] {
  if (!object(raw) || !object(raw[provider])) return []
  const value = raw[provider]
  if (value.version !== 1 || !Array.isArray(value.clients) || value.clients.length > 8) return []
  return value.clients.flatMap(client => object(client) && string(client.id, 128) && string(client.label, 128) && client.platform === 'windows'
    ? [{ id: client.id, label: client.label, platform: 'windows' as const }] : [])
}

export function readLaunchResult(raw: unknown): LaunchResult {
  if (!object(raw) || !string(raw.intent, 48) || !/^[a-f0-9]{48}$/.test(raw.intent)
    || !string(raw.provider, 128) || !string(raw.account, 512) || !object(raw.client)
    || !string(raw.client.id, 128) || !string(raw.client.label, 128) || !phases.includes(String(raw.phase))
    || !string(raw.message) || !string(raw.executable) || !Array.isArray(raw.resources) || raw.resources.length > 8
    || raw.resources.some(resource => !object(resource) || !string(resource.id, 128) || !string(resource.label, 128) || !string(resource.location))
    || typeof raw.remainingMs !== 'number' || !Number.isFinite(raw.remainingMs) || raw.remainingMs < 0
    || ['error', 'committed', 'started', 'restored', 'running', 'requiresRestart', 'hotSwitch', 'switchOnly', 'canRestore', 'canRetryStart'].some(key => typeof raw[key] !== 'boolean')
    || ['backup', 'pendingBackup'].some(key => raw[key] !== null && !string(raw[key], 48))) throw new Error('本机启动状态返回格式无效')
  const proxy = raw.proxy
  if (!object(proxy)
    || ['supported', 'configured', 'enabled', 'available'].some(key => typeof proxy[key] !== 'boolean')
    || !string(proxy.label, 256) || !string(proxy.address, 512)
    || proxy.error !== null && !string(proxy.error)) throw new Error('本机启动状态返回格式无效')
  return raw as unknown as LaunchResult
}

export function readLaunchSettings(raw: unknown, selection: LaunchSelection): LaunchSettings {
  if (!object(raw) || raw.provider !== selection.provider || raw.client !== selection.client || !string(raw.path, 2048)
    || !string(raw.args, 1024) || typeof raw.switchOnly !== 'boolean'
    || raw.detected !== null && !string(raw.detected, 2048) || raw.error !== null && !string(raw.error)
    || !string(raw.revision, 64) || !/^[a-f0-9]{64}$/.test(raw.revision)) throw new Error('本机客户端设置返回格式无效')
  return raw as unknown as LaunchSettings
}

export function readLaunchRecoveries(raw: unknown): LaunchPendingRecovery[] {
  if (!object(raw) || !Array.isArray(raw.pending) || raw.pending.length > 64 || raw.pending.some(row => !object(row)
    || !string(row.backup, 48) || !/^[a-f0-9]{48}$/.test(row.backup) || !string(row.provider, 128)
    || !string(row.account, 512) || !string(row.client, 128) || !string(row.label, 128) || !string(row.executable))) {
    throw new Error('本机凭证恢复列表返回格式无效')
  }
  return raw.pending as LaunchPendingRecovery[]
}

export const launchPending = (phase: LaunchPhase): boolean => ['preparing', 'closing', 'writing', 'reloading', 'starting'].includes(phase)

export function launchMessage(message: string, t: LaunchTranslate): string {
  const translated = t(message)
  if (translated !== message) return translated
  if (message.startsWith('凭证已切换；')) return t('凭证已切换，客户端未启动') + '；' + launchMessage(message.slice('凭证已切换；'.length), t)
  const segments = message.split('；')
  return segments.map(segment => {
    if (segment.startsWith('恢复原凭证失败：')) return t('恢复原凭证失败：{error}', { error: launchMessage(segment.slice('恢复原凭证失败：'.length), t) })
    if (segment.startsWith('备份状态保存失败：')) return t('备份状态保存失败：{error}', { error: launchMessage(segment.slice('备份状态保存失败：'.length), t) })
    if (segment.startsWith('本机切换记录保存失败：')) return t('本机切换记录保存失败：{error}', { error: launchMessage(segment.slice('本机切换记录保存失败：'.length), t) })
    if (segment.startsWith('Windows 凭据库 · ')) return t('Windows 凭据库 · {target}', { target: segment.slice('Windows 凭据库 · '.length) })
    return t(segment)
  }).join('；')
}

export function launchPhaseText(phase: LaunchPhase, t: LaunchTranslate, switchOnly = false): string {
  switch (phase) {
    case 'awaiting_confirmation': return t('等待确认')
    case 'preparing': return t('正在准备账号凭证')
    case 'closing': return t('正在关闭客户端')
    case 'writing': return t('正在切换本机凭证')
    case 'reloading': return t('正在重载客户端服务')
    case 'starting': return t('正在启动客户端')
    case 'started': return t('客户端已启动')
    case 'switched': return switchOnly ? t('凭证已切换') : t('凭证已在线切换')
    case 'committed': return t('凭证已切换，客户端未启动')
    case 'failed': return switchOnly ? t('切换未完成') : t('启动未完成')
    case 'canceled': return switchOnly ? t('本次切换已取消') : t('本次启动已取消')
    case 'restored': return t('原凭证已恢复')
    case 'recovery_required': return t('需要恢复原凭证')
  }
}
