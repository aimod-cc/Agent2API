import * as React from 'react'
import { Alert, Button, Checkbox, Label } from '@ui'
import { PluginPermissions } from './plugins-permissions'
import type { PackProvider } from './plugins'
import type { PluginTranslate } from './plugins-store'

type Permission = {
  pack: string
  provider: string
  available: boolean
  authorized: boolean
  grantPresent: boolean
  callerDigest: string
  declarationDigest: string
  secretField: string
  basePath: string
  hostSuffix: string
  routes: Array<{ method: string; path: string }>
  proxySupported: boolean
}
type Payload = { accountOrigins: Permission[]; saved?: boolean }
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

function text(value: unknown): value is string {
  return typeof value === 'string' && !/[\u0000-\u001f\u007f]/.test(value)
}

function declaredPath(value: unknown): value is string {
  return text(value) && value.startsWith('/') && !value.startsWith('//') && !/[?#\\]/.test(value)
}

// 只读取声明摘要，不保存或展示后端可能附带的账号地址、凭据或原始错误载荷。
function readPayload(value: unknown, packId: string): Payload | null {
  if (!record(value) || !Array.isArray(value.accountOrigins)
    || value.saved !== undefined && typeof value.saved !== 'boolean') return null
  const accountOrigins: Permission[] = []
  const seen = new Set<string>()
  for (const row of value.accountOrigins) {
    if (!record(row) || !text(row.pack)) return null
    if (row.pack !== packId) continue
    if (!text(row.provider) || !row.provider || seen.has(row.provider)
      || typeof row.available !== 'boolean' || typeof row.authorized !== 'boolean' || typeof row.grantPresent !== 'boolean'
      || !text(row.callerDigest) || !text(row.declarationDigest) || !text(row.secretField)
      || !text(row.basePath) || row.basePath !== '' && !declaredPath(row.basePath)
      || !text(row.hostSuffix) || row.hostSuffix !== '' && !/^\.?[a-z0-9-]+(?:\.[a-z0-9-]+)+$/i.test(row.hostSuffix)
      || !Array.isArray(row.routes) || typeof row.proxySupported !== 'boolean') return null
    const routes: Permission['routes'] = []
    for (const route of row.routes) {
      if (!record(route) || !text(route.method) || !/^[A-Z]+$/.test(route.method) || !declaredPath(route.path)) return null
      routes.push({ method: route.method, path: route.path })
    }
    if (row.available && (!row.callerDigest || !row.declarationDigest || !row.hostSuffix || !routes.length)) return null
    seen.add(row.provider)
    accountOrigins.push({
      pack: row.pack, provider: row.provider, available: row.available, authorized: row.authorized,
      grantPresent: row.grantPresent, callerDigest: row.callerDigest, declarationDigest: row.declarationDigest,
      secretField: row.secretField, basePath: row.basePath, hostSuffix: row.hostSuffix, routes, proxySupported: row.proxySupported,
    })
  }
  return { accountOrigins, saved: value.saved }
}

function permissionKey(scope: string, item: Permission): string {
  return JSON.stringify([scope, item.provider, item.callerDigest, item.declarationDigest,
    item.secretField, item.basePath, item.hostSuffix, item.routes, item.proxySupported])
}

export function PluginAccountOriginPermissions({ packId, providers, enabled, disabled = false, onBusyChange, onChanged, request, t }: {
  packId: string
  providers: PackProvider[]
  enabled: boolean
  disabled?: boolean
  onBusyChange?: (busy: boolean) => void
  onChanged?: () => void
  request: Request
  t: PluginTranslate
}) {
  const scope = JSON.stringify([packId, enabled,
    providers.map(provider => [provider.id, provider.enabled, provider.loaded, provider.overriddenBy])])
  const [view, setView] = React.useState<View>(() => initialView(scope))
  const [confirmed, setConfirmed] = React.useState<Record<string, boolean>>({})
  const [busy, setBusy] = React.useState(false)
  const alive = React.useRef(true)
  const generation = React.useRef(0)
  const pending = React.useRef<number | null>(null)
  const latest = React.useRef({ request, packId, scope, enabled, disabled })
  latest.current = { request, packId, scope, enabled, disabled }
  const prefix = React.useId()
  // 元数据变更到 effect 清理之间也不能展示或提交旧包的确认。
  const currentView = view.scope === scope ? view : initialView(scope)
  const loading = currentView.phase === 'loading'

  const accept = React.useCallback((raw: unknown, target: { packId: string; scope: string }) => {
    const value = readPayload(raw, target.packId)
    setConfirmed({})
    setView(previous => value ? {
      scope: target.scope, data: value, phase: 'ready', failure: null,
      diskWarning: value.saved === false || value.saved === undefined && previous.scope === target.scope && previous.diskWarning,
    } : { ...previous, phase: 'failed', failure: 'invalid' })
  }, [])

  const load = React.useCallback(async () => {
    if (!alive.current || pending.current !== null) return
    const target = latest.current
    const current = ++generation.current
    const isCurrent = () => alive.current && generation.current === current && latest.current.scope === target.scope
    setConfirmed({})
    setView(previous => ({ ...(previous.scope === target.scope ? previous : initialView(target.scope)), phase: 'loading', failure: null }))
    try {
      const value = await target.request<unknown>('GET', `/api/plugins/permissions?pack=${encodeURIComponent(target.packId)}`)
      if (isCurrent()) accept(value, target)
    } catch {
      if (isCurrent()) setView(previous => ({ ...previous, phase: 'failed', failure: 'load' }))
    }
  }, [accept])

  React.useEffect(() => {
    alive.current = true
    return () => { alive.current = false; generation.current++ }
  }, [])
  React.useEffect(() => {
    setView(initialView(scope))
    setConfirmed({})
    void load()
    return () => { generation.current++ }
  }, [scope, load, request])
  React.useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])
  React.useEffect(() => () => { onBusyChange?.(false) }, [onBusyChange])

  async function save(item: Permission, authorized: boolean) {
    const target = latest.current
    if (!alive.current || pending.current !== null || target.disabled || view.scope !== target.scope || view.phase !== 'ready') return
    const key = permissionKey(target.scope, item)
    if (!view.data?.accountOrigins.some(current => permissionKey(target.scope, current) === key)) return
    if (authorized && (!target.enabled || !item.available || item.authorized || !confirmed[key])) return
    if (!authorized && !item.grantPresent) return
    const current = ++generation.current
    const isCurrent = () => alive.current && generation.current === current && latest.current.scope === target.scope
    pending.current = current
    setBusy(true)
    setConfirmed({})
    setView(previous => ({ ...previous, phase: 'saving', failure: null }))
    try {
      const value = await target.request<unknown>('PATCH', '/api/plugins/permissions', {
        pack: target.packId, provider: item.provider, callerDigest: item.callerDigest,
        declarationDigest: item.declarationDigest, authorized, confirmed: authorized,
      })
      if (isCurrent()) {
        accept(value, target)
        if (readPayload(value, target.packId)) onChanged?.()
      }
    } catch {
      // 请求可能已在网关生效。先刷新状态，不能保留旧确认或自动重试授权。
      if (isCurrent()) setView(previous => ({ ...previous, phase: 'failed', failure: 'save' }))
    } finally {
      if (pending.current === current) {
        pending.current = null
        if (alive.current) {
          setBusy(false)
          // 加载状态可能在保存期间变更；先让旧写入结束，再读取新范围，避免旧 finally 解锁新请求。
          if (!isCurrent()) void load()
        }
      }
    }
  }

  const requested = currentView.data?.accountOrigins || []
  const error = currentView.failure === 'load' ? t('读取服务器访问权限失败，请重试')
    : currentView.failure === 'save' ? t('权限保存结果未确认，请刷新后重试')
      : currentView.failure === 'invalid' ? t('权限返回格式无效，请刷新后重试') : ''
  if (!requested.length && !error && !currentView.diskWarning) return null
  const locked = disabled || busy || currentView.phase !== 'ready'
  return <PluginPermissions t={t}>
    <div className='flex items-center justify-between gap-3'>
      <h4 className='m-0 text-[12.5px] font-medium text-foreground'>{t('当前账号服务器访问')}</h4>
      <Button type='button' size='sm' variant='ghost' disabled={disabled || busy || loading} onClick={() => void load()}>
        {loading ? t('加载中…') : error ? t('重试') : t('刷新')}
      </Button>
    </div>
    {requested.length ? <>
      <p className='m-0'>{t('仅访问当前账号保存的服务器，不允许选择同域下其他服务器')}</p>
      {requested.some(item => !item.proxySupported) ? <p className='m-0 text-xs text-muted-foreground'>{t('该权限暂不支持代理出口')}</p> : null}
    </> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {currentView.diskWarning ? <Alert role='status'>{t('设置已生效，但写入磁盘失败（重启后会恢复原设置）')}</Alert> : null}
    {busy ? <p className='m-0 text-xs text-muted-foreground' role='status'>{t('保存中…')}</p> : null}
    {requested.map((item, index) => {
      const key = permissionKey(scope, item)
      const label = providers.find(provider => provider.id === item.provider)?.label || item.provider
      const canAuthorize = enabled && item.available && !item.authorized
      const checkboxId = `${prefix}-${index}`
      return <div key={item.provider} className='flex min-w-0 flex-col gap-2 border-t border-hairline pt-3' aria-busy={busy || loading}>
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <Label className='text-[12.5px] text-foreground'>{t(label)}</Label>
          <span className='text-xs text-muted-foreground'>{item.authorized ? t('已授权') : t('未授权')}</span>
        </div>
        {item.grantPresent && !item.authorized ? <Alert>{t('权限已变化，需要重新确认')}</Alert> : null}
        <dl className='m-0 flex min-w-0 flex-col gap-1.5 text-xs'>
          <dt className='text-muted-foreground'>{t('允许的域名后缀')}</dt>
          <dd className='m-0'><code className='break-all'>{item.hostSuffix || '—'}</code></dd>
          <dt className='text-muted-foreground'>{t('服务器基础路径')}</dt>
          <dd className='m-0'><code className='break-all'>{item.basePath || '/'}</code></dd>
          <dt className='text-muted-foreground'>{t('允许的请求')}</dt>
          <dd className='m-0'>
            {item.routes.length ? <ul className='m-0 list-none space-y-1 p-0'>
              {item.routes.map((route, routeIndex) => <li key={`${route.method}:${route.path}:${routeIndex}`}>
                <code className='flex min-w-0 items-start gap-2'>
                  <span className='shrink-0'>{route.method}</span><span className='break-all'>{route.path}</span>
                </code>
              </li>)}
            </ul> : '—'}
          </dd>
        </dl>
        {!enabled || !item.available ? <p className='m-0'>{t('插件未启用或暂不可用，已有授权仍可撤销。')}</p> : null}
        {canAuthorize ? <Label htmlFor={checkboxId} className='flex items-start gap-2 text-xs leading-relaxed'>
          <Checkbox id={checkboxId} checked={confirmed[key] === true} disabled={locked} className='mt-0.5'
            onCheckedChange={value => setConfirmed(previous => ({ ...previous, [key]: value === true }))} />
          <span>{t('我已核对以上访问范围，并允许插件访问当前账号的服务器')}</span>
        </Label> : null}
        {canAuthorize || item.grantPresent ? <div className='flex flex-wrap gap-2'>
          {canAuthorize ? <Button type='button' size='sm' disabled={locked || !confirmed[key]}
            onClick={() => void save(item, true)}>{t('授权')}</Button> : null}
          {item.grantPresent ? <Button type='button' size='sm' variant='outline' disabled={locked}
            onClick={() => void save(item, false)}>{t('撤销授权')}</Button> : null}
        </div> : null}
      </div>
    })}
  </PluginPermissions>
}
