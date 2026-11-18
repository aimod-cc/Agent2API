import * as React from 'react'
import { Alert, Button, Checkbox, Label } from '@ui'
import { PluginPermissions } from './plugins-permissions'
import type { PackProvider } from './plugins'
import type { PluginTranslate } from './plugins-store'

const PROFILES = {
  monkeycode: { profile: 'monkeycode.cn.task.v1', origin: 'https://monkeycode-ai.com' },
  'monkeycode-intl': { profile: 'monkeycode.intl.task.v1', origin: 'https://monkeycode-ai.net' },
} as const
const ENTRY = 'conversation'
const CREATE = { method: 'POST', path: '/api/v1/users/tasks' } as const
const STOP = { method: 'PUT', path: '/api/v1/users/tasks/stop' } as const

type Permission = {
  pack: string
  provider: keyof typeof PROFILES
  profile: typeof PROFILES[keyof typeof PROFILES]['profile']
  entry: typeof ENTRY
  callerDigest: string
  moduleDigest: string
  declarationDigest: string
  available: boolean
  authorized: boolean
  grantPresent: boolean
  executionReady: boolean
  origin: typeof PROFILES[keyof typeof PROFILES]['origin']
  create: typeof CREATE
  stop: typeof STOP
  maxCreatesPerCall: 1
  maxStopsPerCreation: 1
  stoppingIsBestEffort: true
  remoteToolsAutoApproved: true
  questionsReturnToAgent: true
}
type Payload = { supported: boolean; managedTasks: Permission[]; saved?: boolean | null }
type Request = <T>(method: 'GET' | 'PATCH', path: string, body?: unknown) => Promise<T>
type Failure = 'load' | 'save' | 'invalid' | null
type View = {
  scope: string
  data: Payload | null
  phase: 'loading' | 'saving' | 'ready' | 'failed'
  failure: Failure
  diskWarning: boolean
}

function initialView(scope: string): View {
  return { scope, data: null, phase: 'loading', failure: null, diskWarning: false }
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function digest(value: unknown): value is string {
  return typeof value === 'string' && (value === '' || /^[a-f0-9]{64}$/.test(value))
}

// 旧主程序没有此字段时隐藏；字段存在则完整校验，只投影固定权限白名单。
// 不保留账号、任务编号、凭据、任意声明或原始错误，也不把后端地址做成链接。
function readPayload(value: unknown, packId: string): Payload | null {
  if (!record(value)) return null
  if (!Object.prototype.hasOwnProperty.call(value, 'managedTasks')) return { supported: false, managedTasks: [] }
  if (!Array.isArray(value.managedTasks) || value.managedTasks.length > 1
    || value.saved !== undefined && value.saved !== null && typeof value.saved !== 'boolean') return null
  const managedTasks: Permission[] = []
  for (const row of value.managedTasks) {
    if (!record(row) || (row.provider !== 'monkeycode' && row.provider !== 'monkeycode-intl')) return null
    const provider = row.provider
    const configuration = PROFILES[provider]
    if (row.pack !== packId || row.profile !== configuration.profile || row.entry !== ENTRY
      || !digest(row.callerDigest) || !digest(row.moduleDigest) || !digest(row.declarationDigest)
      || typeof row.available !== 'boolean' || typeof row.authorized !== 'boolean'
      || typeof row.grantPresent !== 'boolean' || typeof row.executionReady !== 'boolean'
      || row.origin !== configuration.origin || !record(row.create) || row.create.method !== CREATE.method || row.create.path !== CREATE.path
      || !record(row.stop) || row.stop.method !== STOP.method || row.stop.path !== STOP.path
      || row.maxCreatesPerCall !== 1 || row.maxStopsPerCreation !== 1 || row.stoppingIsBestEffort !== true
      || row.remoteToolsAutoApproved !== true || row.questionsReturnToAgent !== true) return null
    if ((row.available || row.authorized) && (!row.callerDigest || !row.moduleDigest || !row.declarationDigest)) return null
    if (row.authorized && (!row.available || !row.executionReady || !row.grantPresent)) return null
    managedTasks.push({
      pack: packId, provider, profile: configuration.profile, entry: ENTRY,
      callerDigest: row.callerDigest, moduleDigest: row.moduleDigest, declarationDigest: row.declarationDigest,
      available: row.available, authorized: row.authorized, grantPresent: row.grantPresent, executionReady: row.executionReady,
      origin: configuration.origin, create: CREATE, stop: STOP, maxCreatesPerCall: 1, maxStopsPerCreation: 1, stoppingIsBestEffort: true,
      remoteToolsAutoApproved: true, questionsReturnToAgent: true,
    })
  }
  return { supported: true, managedTasks, saved: value.saved }
}

function permissionKey(scope: string, item: Permission): string {
  return JSON.stringify([scope, item.pack, item.provider, item.profile, item.entry, item.callerDigest,
    item.moduleDigest, item.declarationDigest, item.available, item.authorized, item.grantPresent, item.executionReady])
}

function canAuthorize(item: Permission, enabled: boolean, providers: PackProvider[]): boolean {
  const provider = providers.find(value => value.id === item.provider)
  return enabled && item.executionReady && item.available && !item.authorized
    && provider?.enabled === true && !provider.overriddenBy
}

export function PluginManagedTaskPermissions({ packId, packVersion, scopeIdentity, providers, enabled, disabled = false,
  onBusyChange, request, t }: {
  packId: string
  packVersion: string
  scopeIdentity: string
  providers: PackProvider[]
  enabled: boolean
  disabled?: boolean
  onBusyChange?: (busy: boolean) => void
  request: Request
  t: PluginTranslate
}) {
  const scope = JSON.stringify([scopeIdentity, packId, packVersion, enabled,
    providers.map(provider => [provider.id, provider.enabled, provider.loaded, provider.overriddenBy])])
  const [view, setView] = React.useState<View>(() => initialView(scope))
  const [confirmed, setConfirmed] = React.useState<string | null>(null)
  const [busy, setBusy] = React.useState(false)
  const confirmedRef = React.useRef<string | null>(null)
  const alive = React.useRef(true)
  const generation = React.useRef(0)
  const pending = React.useRef<number | null>(null)
  const reading = React.useRef<number | null>(null)
  const latest = React.useRef({ request, packId, scope, enabled, disabled, providers, onBusyChange, view })
  latest.current = { request, packId, scope, enabled, disabled, providers, onBusyChange, view }
  const prefix = React.useId()
  // 新范围在 effect 执行前也不能展示或提交旧确认；确认只存在于本次挂载内。
  const currentView = view.scope === scope ? view : initialView(scope)
  const loading = currentView.phase === 'loading'

  const clearConfirmation = React.useCallback(() => {
    confirmedRef.current = null
    setConfirmed(null)
  }, [])

  const accept = React.useCallback((raw: unknown, target: { packId: string; scope: string },
    change?: { item: Permission; authorized: boolean }) => {
    const value = readPayload(raw, target.packId)
    clearConfirmation()
    const changed = change && value?.managedTasks.find(item => item.provider === change.item.provider && item.profile === change.item.profile)
    const acknowledged = value?.saved !== null && (!change || value?.supported && (change.authorized
      ? value.saved === true && changed?.authorized === true && changed.callerDigest === change.item.callerDigest
        && changed.moduleDigest === change.item.moduleDigest && changed.declarationDigest === change.item.declarationDigest
      : !changed?.grantPresent))
    setView(previous => value && acknowledged ? {
      scope: target.scope, data: value, phase: 'ready', failure: null,
      diskWarning: value.supported && (value.saved === false
        || value.saved === undefined && previous.scope === target.scope && previous.diskWarning),
    } : { ...initialView(target.scope), phase: 'failed', failure: value && change ? 'save' : 'invalid' })
  }, [clearConfirmation])

  const load = React.useCallback(async () => {
    if (!alive.current || pending.current !== null) return
    const target = latest.current
    const current = ++generation.current
    const isCurrent = () => alive.current && generation.current === current && latest.current.scope === target.scope
    reading.current = current
    clearConfirmation()
    setView(previous => ({ ...(previous.scope === target.scope ? previous : initialView(target.scope)), phase: 'loading', failure: null }))
    try {
      const value = await target.request<unknown>('GET', `/api/plugins/permissions?pack=${encodeURIComponent(target.packId)}`)
      if (isCurrent()) accept(value, target)
    } catch {
      if (isCurrent()) setView(previous => ({ ...previous, phase: 'failed', failure: 'load' }))
    } finally {
      if (reading.current === current) reading.current = null
    }
  }, [accept, clearConfirmation])

  React.useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      generation.current++
      confirmedRef.current = null
      latest.current.onBusyChange?.(false)
    }
  }, [])
  React.useEffect(() => {
    setView(initialView(scope))
    clearConfirmation()
    void load()
    return () => { generation.current++; confirmedRef.current = null }
  }, [scope, load, clearConfirmation, request])
  React.useEffect(() => { if (disabled) clearConfirmation() }, [disabled, clearConfirmation])

  async function save(item: Permission, authorized: boolean) {
    const target = latest.current
    const snapshot = target.view
    if (!alive.current || pending.current !== null || reading.current !== null || target.disabled || target.scope !== scope
      || snapshot.scope !== target.scope || snapshot.phase !== 'ready') return
    const key = permissionKey(target.scope, item)
    if (!snapshot.data?.managedTasks.some(value => permissionKey(target.scope, value) === key)) return
    if (authorized && (!canAuthorize(item, target.enabled, target.providers) || confirmedRef.current !== key)) return
    if (!authorized && !item.grantPresent) return
    const current = ++generation.current
    const isCurrent = () => alive.current && generation.current === current && latest.current.scope === target.scope
    // 在首个 await 之前同步占位并销毁确认，连点、认证恢复和重挂载都不能复用。
    pending.current = current
    clearConfirmation()
    setBusy(true)
    target.onBusyChange?.(true)
    setView(previous => ({ ...previous, phase: 'saving', failure: null }))
    try {
      const value = await target.request<unknown>('PATCH', '/api/plugins/permissions', {
        kind: 'managedTask', pack: target.packId, provider: item.provider, profile: item.profile,
        callerDigest: item.callerDigest, moduleDigest: item.moduleDigest, declarationDigest: item.declarationDigest,
        authorized, confirmed: authorized,
      })
      if (isCurrent()) accept(value, target, { item, authorized })
    } catch {
      // 未知结果不自动补发，也不自动 GET 后重新授权；必须由用户刷新、重新确认。
      if (isCurrent()) setView(previous => ({ ...previous, phase: 'failed', failure: 'save' }))
    } finally {
      if (pending.current === current) {
        pending.current = null
        if (alive.current) {
          setBusy(false)
          latest.current.onBusyChange?.(false)
          // 保存途中切到另一范围时，只读取新范围，永不重发原 PATCH。
          if (latest.current.scope !== target.scope) void load()
        }
      }
    }
  }

  const requested = currentView.data?.managedTasks || []
  const error = currentView.failure === 'load' ? t('读取受控任务权限失败，请手动刷新')
    : currentView.failure === 'save' ? t('权限保存结果尚未确认，请手动刷新；重新授权前必须再次勾选确认。')
      : currentView.failure === 'invalid' ? t('受控任务权限信息未通过安全校验，已阻止操作，请手动刷新。') : ''
  if (!requested.length && !error && !currentView.diskWarning) return null
  const locked = disabled || busy || currentView.phase !== 'ready'
  return <PluginPermissions t={t}>
    <div className='flex items-center justify-between gap-3'>
      <h4 className='m-0 text-[12.5px] font-medium text-foreground'>{t('受控对话任务权限')}</h4>
      <Button type='button' size='sm' variant='ghost' disabled={disabled || busy || loading} onClick={() => void load()}>
        {loading ? t('加载中…') : t('刷新')}
      </Button>
    </div>
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {currentView.diskWarning ? <Alert role='status'>{t('设置已生效，但写入磁盘失败（重启后会恢复原设置）')}</Alert> : null}
    {busy ? <p className='m-0 text-xs text-muted-foreground' role='status'>{t('保存中…')}</p> : null}
    {requested.length ? <div className='flex flex-col gap-2 text-xs leading-relaxed'>
      <p className='m-0'>{t('仅允许为当前账号本次真实对话创建任务；每次对话最多创建一次。')}</p>
      <p className='m-0'>{t('能力接通并获得授权后，主程序会在对话结束或取消时，对本次已确认创建的任务最多申请停止一次。')}</p>
      <p className='m-0'>{t('远端 Agent 的工具请求将自动批准；需要提问时返回空答案，由 Agent 继续判断。它们不代表客户端工具已执行。')}</p>
      <p className='m-0'>{t('不会枚举或停止官网、其他实例或其他对话的任务。')}</p>
      <p className='m-0'>{t('停止请求被受理，不等于计算资源已回收；断电或强行关闭程序时，无法保证清理。')}</p>
      <p className='m-0'>{t('插件包、代码或权限声明一旦改变，必须重新确认。')}</p>
    </div> : null}
    {requested.map((item, index) => {
      const key = permissionKey(scope, item)
      const ready = currentView.phase === 'ready'
      const offerAuthorization = ready && canAuthorize(item, enabled, providers)
      const checkboxId = `${prefix}-${index}`
      return <div key={`${item.provider}:${item.profile}`} className='flex min-w-0 flex-col gap-2 border-t border-hairline pt-3' aria-busy={busy || loading}>
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <Label className='text-[12.5px] text-foreground'>{item.provider === 'monkeycode-intl' ? t('MonkeyCode 国际版') : t('MonkeyCode 国内版')}</Label>
          <span className='text-xs text-muted-foreground'>{!ready ? t('权限状态待确认') : item.authorized ? t('已授权')
            : item.grantPresent ? t('已有授权记录，当前未生效') : t('未授权')}</span>
        </div>
        {!item.executionReady ? <Alert>{t('任务代码已接入，但真实执行保持关闭')}</Alert> : null}
        {ready && item.grantPresent && !item.authorized ? <p className='m-0 text-xs'>
          {t('已有记录仍可撤销；它不代表任务能力已生效。')}
        </p> : null}
        <dl className='m-0 flex min-w-0 flex-col gap-1.5 text-xs'>
          <dt className='text-muted-foreground'>{t('固定访问地址')}</dt>
          <dd className='m-0'><code className='break-all'>{item.origin}</code></dd>
          <dt className='text-muted-foreground'>{t('仅允许创建和申请停止本次任务')}</dt>
          <dd className='m-0 flex flex-col gap-1'>
            <code className='break-all'>{item.create.method} {item.create.path}</code>
            <code className='break-all'>{item.stop.method} {item.stop.path}</code>
          </dd>
          <dt className='text-muted-foreground'>{t('插件包校验值')}</dt>
          <dd className='m-0'><code className='break-all'>{item.callerDigest || '—'}</code></dd>
          <dt className='text-muted-foreground'>{t('代码校验值')}</dt>
          <dd className='m-0'><code className='break-all'>{item.moduleDigest || '—'}</code></dd>
          <dt className='text-muted-foreground'>{t('权限声明校验值')}</dt>
          <dd className='m-0'><code className='break-all'>{item.declarationDigest || '—'}</code></dd>
        </dl>
        {!enabled || !item.available ? <p className='m-0'>{t('插件未启用或暂不可用，已有授权仍可撤销。')}</p> : null}
        {offerAuthorization ? <Label htmlFor={checkboxId} className='flex items-start gap-2 text-xs leading-relaxed'>
          <Checkbox id={checkboxId} checked={confirmed === key} disabled={locked} className='mt-0.5'
            onCheckedChange={value => {
              if (pending.current !== null || reading.current !== null || latest.current.disabled || latest.current.scope !== scope) return
              const next = value === true ? key : null
              confirmedRef.current = next
              setConfirmed(next)
            }} />
          <span>{t('我已核对本插件的任务范围和停止限制，仅授权上述本次对话任务能力')}</span>
        </Label> : null}
        {offerAuthorization || item.grantPresent ? <div className='flex flex-wrap gap-2'>
          {offerAuthorization ? <Button type='button' size='sm' disabled={locked || confirmed !== key}
            onClick={() => void save(item, true)}>{t('确认授权')}</Button> : null}
          {item.grantPresent ? <Button type='button' size='sm' variant='outline' disabled={locked}
            onClick={() => void save(item, false)}>{t('撤销授权')}</Button> : null}
        </div> : null}
      </div>
    })}
  </PluginPermissions>
}
