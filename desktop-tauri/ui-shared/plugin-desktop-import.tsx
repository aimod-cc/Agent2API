/** 固定 ChatGLM 本机导入：准备不读凭据，确认仅发送一次，未知保存结果不补成功。 */
import * as React from 'react'
import {
  Alert, AlertDescription, AlertTitle, Button, Checkbox, Dialog, DialogBody,
  DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Label, Spinner,
} from '@ui'
import { PluginLoginActions } from './plugin-login-form'

const BASE = '/api/plugins/desktop-import/chatglm'
const PHASES = ['awaiting_confirmation', 'reading', 'normalizing', 'saving', 'saved', 'failed', 'canceled', 'expired'] as const
export type DesktopImportPhase = (typeof PHASES)[number]
export type DesktopImportAccount = { id: string; provider: 'chatglm' }
export type DesktopImportRequest = <T>(method: 'GET' | 'POST', path: string, body: unknown,
  options: { retry: 'never' }) => Promise<T>
type Translate = (key: string, vars?: Record<string, string | number>) => string
type Package = { id: string; version: string; archiveDigest: string; moduleDigest: string; declarationDigest: string }
type Scope = {
  platform: 'windows'; application: 'chatglm'; root: 'current_user_roaming'
  files: ['chatglm/Local State', 'chatglm/Network/Cookies']
  cookieHost: '.chatglm.cn'; cookieNames: ['chatglm_token', 'chatglm_refresh_token']; access: 'read_only'
  excludes: ['other_apps', 'browser_profiles', 'other_windows_users']; credentialsReturnedToUi: false
}
type Payload = {
  provider: 'chatglm'; intent: string; phase: DesktopImportPhase; saved: boolean | null
  expiresAt: number; remainingMs: number; package: Package; scope: Scope; message: string
  account: DesktopImportAccount | null
}
type Attempt = {
  owner: number; scope: string; source: unknown; request: DesktopImportRequest; managementGeneration: number
  intent: string; identity: string; deadline: number; confirmSent: boolean; cancelSent: boolean; notified: boolean
}
type View = { owner: number; data: Payload | null; action: '' | 'prepare' | 'confirm' | 'status' | 'cancel'; notice: string; unknown: boolean }
const CHANGE_EVENT = 'wb:management-session-changed'
let managementGeneration = 0
// 必须同步递增来源代次：发请求前即使事件先于React effect也不能复用旧确认。
if (typeof window !== 'undefined') window.addEventListener(CHANGE_EVENT, () => { managementGeneration++ })

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value)
const text = (value: unknown, limit: number): value is string => typeof value === 'string' && value.trim().length > 0
  && value.length <= limit && !/[\u0000-\u001f\u007f]/.test(value)
const digest = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value)
const list = (value: unknown, expected: readonly string[]): boolean => Array.isArray(value)
  && value.length === expected.length && value.every((item, index) => item === expected[index])
const emptyView = (owner: number): View => ({ owner, data: null, action: '', notice: '', unknown: false })

function readPayload(raw: unknown): Payload {
  if (record(raw) && raw.success === true) raw = raw.data
  if (!record(raw) || raw.provider !== 'chatglm' || !text(raw.intent, 512)
    || !PHASES.includes(raw.phase as DesktopImportPhase)
    || raw.saved !== null && typeof raw.saved !== 'boolean'
    || typeof raw.expiresAt !== 'number' || !Number.isSafeInteger(raw.expiresAt) || raw.expiresAt <= 0
    || typeof raw.remainingMs !== 'number' || !Number.isSafeInteger(raw.remainingMs) || raw.remainingMs < 0
    || raw.remainingMs > 2_147_483_647
    || typeof raw.message !== 'string' || raw.message.length > 8192) throw new Error('invalid_desktop_import')
  const pkg = raw.package
  const scope = raw.scope
  function readScope(scope: unknown): boolean {
    return record(scope) && scope.platform === 'windows' && scope.application === 'chatglm'
      && scope.root === 'current_user_roaming' && scope.cookieHost === '.chatglm.cn' && scope.access === 'read_only'
      && scope.credentialsReturnedToUi === false
      && list(scope.files, ['chatglm/Local State', 'chatglm/Network/Cookies'])
      && list(scope.cookieNames, ['chatglm_token', 'chatglm_refresh_token'])
      && list(scope.excludes, ['other_apps', 'browser_profiles', 'other_windows_users'])
      && Object.keys(scope).length === 9
  }
  if (!record(pkg) || !text(pkg.id, 128) || !text(pkg.version, 128)
    || !digest(pkg.archiveDigest) || !digest(pkg.moduleDigest) || !digest(pkg.declarationDigest)
    || !readScope(scope)) throw new Error('invalid_desktop_import_scope')
  let account: DesktopImportAccount | null = null
  if (raw.account !== null) {
    if (!record(raw.account) || !text(raw.account.id, 1024) || raw.account.provider !== 'chatglm'
      || raw.saved !== true || raw.phase !== 'saved'
      || Object.keys(raw.account).some(key => key !== 'id' && key !== 'provider')) throw new Error('invalid_desktop_import_account')
    account = { id: raw.account.id, provider: 'chatglm' }
  }
  if (raw.phase === 'saved' && raw.saved !== true || raw.saved === true && raw.phase !== 'saved') throw new Error('invalid_desktop_import_saved')
  return {
    provider: 'chatglm', intent: raw.intent, phase: raw.phase as DesktopImportPhase, saved: raw.saved,
    expiresAt: raw.expiresAt, remainingMs: raw.remainingMs, message: raw.message, account,
    package: { id: pkg.id, version: pkg.version, archiveDigest: pkg.archiveDigest, moduleDigest: pkg.moduleDigest, declarationDigest: pkg.declarationDigest },
    scope: { platform: 'windows', application: 'chatglm', root: 'current_user_roaming', files: ['chatglm/Local State', 'chatglm/Network/Cookies'],
      cookieHost: '.chatglm.cn', cookieNames: ['chatglm_token', 'chatglm_refresh_token'], access: 'read_only',
      excludes: ['other_apps', 'browser_profiles', 'other_windows_users'], credentialsReturnedToUi: false },
  }
}

function payloadIdentity(value: Payload): string {
  return JSON.stringify([value.provider, value.intent, value.expiresAt, value.package, value.scope])
}

function phaseText(phase: DesktopImportPhase, t: Translate): string {
  switch (phase) {
    case 'awaiting_confirmation': return t('等待本次确认，尚未读取登录凭据')
    case 'reading': return t('正在只读当前电脑的固定 ChatGLM 文件')
    case 'normalizing': return t('正在验证本次导入的 ChatGLM 账号')
    case 'saving': return t('正在保存账号，保存结果尚未确认')
    case 'saved': return t('账号已确认保存')
    case 'failed': return t('本次导入未完成')
    case 'canceled': return t('本次导入已取消')
    case 'expired': return t('本次导入确认已过期')
  }
}

/** source/current 只核对现有壳/启动来源，不从插件声明推导文件读取许可。 */
export function PluginDesktopImport({ active, disabled = false, scopeIdentity, source, current, request, onSaved, t, footerHost, preserveOnHide = false }: {
  active: boolean
  disabled?: boolean
  scopeIdentity: string
  source: unknown
  current(): boolean
  request: DesktopImportRequest
  onSaved(account: DesktopImportAccount | null): Promise<void> | void
  t: Translate
  footerHost?: HTMLElement | null
  preserveOnHide?: boolean
}): React.ReactElement {
  const [view, setView] = React.useState<View>(() => emptyView(0))
  const [open, setOpen] = React.useState(false)
  const [checked, setChecked] = React.useState(false)
  const [expired, setExpired] = React.useState(false)
  const attempt = React.useRef<Attempt | null>(null)
  const owner = React.useRef(0)
  const alive = React.useRef(false)
  const checkedRef = React.useRef(false)
  const pending = React.useRef<null | { owner: number; action: View['action'] }>(null)
  const latest = React.useRef({ active, disabled, scopeIdentity, source, current, request, onSaved, t })
  latest.current = { active, disabled, scopeIdentity, source, current, request, onSaved, t }
  const prefix = React.useId()
  const visible = view.owner === owner.current ? view : emptyView(owner.current)
  const data = visible.data
  const target = attempt.current
  const activeSource = active && current()
  const operationSource = target ? sameSource(target) : activeSource
  const busy = Boolean(visible.action) || pending.current !== null

  function sameSource(target: Attempt): boolean {
    const context = latest.current
    return context.source === target.source && context.scopeIdentity === target.scope
      && target.managementGeneration === managementGeneration && context.current()
  }
  function owned(target: Attempt): boolean {
    return alive.current && sameSource(target) && owner.current === target.owner && attempt.current === target
  }
  function live(target: Attempt): boolean { return latest.current.active && owned(target) }
  function clearConfirmation(): void { checkedRef.current = false; setChecked(false) }

  // 只在原窗口来源仍有效时取消一次；清理不准备新intent，也不读取或重发确认。
  function abandon(notify = true): void {
    const target = attempt.current
    attempt.current = null; owner.current++
    checkedRef.current = false
    if (notify && target?.intent && !target.notified && !target.cancelSent && sameSource(target)) {
      target.cancelSent = true
      void target.request<unknown>('POST', `${BASE}/cancel`, { intent: target.intent }, { retry: 'never' }).catch(() => {})
    }
  }
  function sourceLost(): void {
    const target = attempt.current
    const sent = target?.confirmSent === true && !target.notified
    abandon(false); setChecked(false); setOpen(false); setExpired(false)
    setView({ ...emptyView(owner.current), unknown: sent,
      notice: sent ? latest.current.t('管理会话已变化，原导入保存未确认；不会为新会话重发或继续查询原操作。')
        : latest.current.t('管理会话已变化，本次确认已失效；不会恢复勾选或自动重发。') })
  }

  React.useEffect(() => {
    alive.current = true
    const invalidate = () => {
      abandon(); setChecked(false); setOpen(false); setExpired(false); setView(emptyView(owner.current))
    }
    // 原intent属于旧本机会话；不能改绑新token，也不自动补发cancel/status。
    const sessionChanged = () => sourceLost()
    window.addEventListener('pagehide', invalidate)
    window.addEventListener(CHANGE_EVENT, sessionChanged)
    return () => {
      alive.current = false; abandon()
      window.removeEventListener('pagehide', invalidate)
      window.removeEventListener(CHANGE_EVENT, sessionChanged)
    }
  }, [])
  React.useEffect(() => {
    if (attempt.current && !sameSource(attempt.current)) sourceLost()
    else if (!activeSource) {
      if (!preserveOnHide) abandon()
      clearConfirmation(); setOpen(false)
      if (!preserveOnHide) { setExpired(false); setView(emptyView(owner.current)) }
    }
  }, [activeSource, source, scopeIdentity, preserveOnHide])
  React.useEffect(() => { if (disabled) clearConfirmation() }, [disabled])
  React.useEffect(() => {
    if (!data || !target || data.phase !== 'awaiting_confirmation') return
    const timer = window.setTimeout(() => {
      if (live(target)) { clearConfirmation(); setExpired(true) }
    }, Math.max(0, target.deadline - performance.now()))
    return () => window.clearTimeout(timer)
  }, [data?.intent, data?.phase, data?.remainingMs])

  async function accept(raw: unknown, target: Attempt, action: View['action']): Promise<void> {
    const receivedAt = performance.now()
    const value = readPayload(raw)
    if (target.intent && value.intent !== target.intent || target.identity && payloadIdentity(value) !== target.identity) throw new Error('desktop_import_scope_changed')
    if (action === 'prepare' && (value.phase !== 'awaiting_confirmation' || value.saved !== false || value.account !== null)) throw new Error('desktop_import_not_prepared')
    if (target.notified && value.saved !== true) throw new Error('desktop_import_saved_regressed')
    if (!live(target) && !(preserveOnHide && owned(target))) return
    target.intent = value.intent
    target.identity ||= payloadIdentity(value)
    target.deadline = Math.min(target.deadline, receivedAt + value.remainingMs)
    clearConfirmation()
    setExpired(value.remainingMs === 0)
    setView({ owner: target.owner, data: value, action: '', notice: '', unknown: false })
    if (value.saved === true && value.phase === 'saved' && !target.notified) {
      target.notified = true
      try { await latest.current.onSaved(value.account) }
      catch { if (live(target)) setView(previous => ({ ...previous, notice: latest.current.t('账号已保存，但列表刷新未完成；不会再次保存。') })) }
    }
  }

  function beginAction(target: Attempt, action: View['action']): boolean {
    if (!live(target) || pending.current || latest.current.disabled) return false
    pending.current = { owner: target.owner, action }
    clearConfirmation()
    setView(previous => ({ ...(previous.owner === target.owner ? previous : emptyView(target.owner)), action, notice: '' }))
    return true
  }
  function endAction(target: Attempt): void {
    if (pending.current?.owner !== target.owner) return
    pending.current = null
    // 迟到响应不能恢复原intent，但须解除本地等待；保留新代次的未确认提示。
    if (alive.current) setView(previous => ({ ...previous, action: '' }))
  }

  async function prepare(): Promise<void> {
    if (!alive.current || !latest.current.active || latest.current.disabled || !latest.current.current() || pending.current || attempt.current) return
    const target: Attempt = { owner: ++owner.current, scope: latest.current.scopeIdentity, source: latest.current.source, request: latest.current.request,
      managementGeneration, intent: '', identity: '', deadline: Infinity, confirmSent: false, cancelSent: false, notified: false }
    attempt.current = target
    setOpen(true); setExpired(false); setView(emptyView(target.owner))
    if (!beginAction(target, 'prepare')) return
    try {
      const raw = await target.request<unknown>('POST', `${BASE}/prepare`, {}, { retry: 'never' })
      await accept(raw, target, 'prepare')
    } catch {
      if (live(target)) setView({ ...emptyView(target.owner), notice: latest.current.t('导入准备结果未确认；不会自动重发，也不会改用旧导入入口。'), unknown: true })
    } finally { endAction(target) }
  }

  async function confirm(): Promise<void> {
    const target = attempt.current
    if (!target || !data || target.confirmSent || target.cancelSent || !checkedRef.current || visible.unknown
      || data.phase !== 'awaiting_confirmation' || data.saved !== false || expired || performance.now() >= target.deadline) return
    // 首个 await 之前销毁勾选并锁定本intent；失败、认证恢复和连点都不能再次confirm。
    if (!beginAction(target, 'confirm')) return
    target.confirmSent = true
    try {
      const raw = await target.request<unknown>('POST', `${BASE}/confirm`, { intent: target.intent, confirmed: true }, { retry: 'never' })
      if (target.cancelSent) {
        if (live(target)) setView(previous => ({ ...previous, unknown: true,
          notice: latest.current.t('取消后的最终结果未确认，请主动查询本次状态。') }))
      } else await accept(raw, target, 'confirm')
    } catch {
      if (live(target)) setView(previous => ({ ...previous, action: '', unknown: true,
        notice: latest.current.t('保存未确认；请主动查询本次状态，不要再次确认导入。') }))
    } finally { endAction(target) }
  }

  async function status(): Promise<void> {
    const target = attempt.current
    if (!target?.intent || !beginAction(target, 'status')) return
    const cancelBefore = target.cancelSent
    try {
      const raw = await target.request<unknown>('GET', `${BASE}/status?intent=${encodeURIComponent(target.intent)}`, undefined, { retry: 'never' })
      if (!cancelBefore && target.cancelSent) {
        if (live(target)) setView(previous => ({ ...previous, unknown: true,
          notice: latest.current.t('取消后的最终结果未确认，请主动查询本次状态。') }))
      } else await accept(raw, target, 'status')
    } catch {
      if (live(target)) setView(previous => ({ ...previous, action: '', unknown: true,
        notice: latest.current.t('本次状态未确认；不会自动重发任何步骤。') }))
    } finally { endAction(target) }
  }

  function cancelWhilePending(target: Attempt): void {
    // 取消可在确认/状态请求尚未返回时单独发送一次；不撤回原请求，也不重发确认。
    target.cancelSent = true; clearConfirmation()
    setView(previous => ({ ...previous, unknown: true,
      notice: latest.current.t('已申请取消，结果仍需确认；不会重发取消或开始新的读取。') }))
    void target.request<unknown>('POST', `${BASE}/cancel`, { intent: target.intent }, { retry: 'never' }).catch(() => {})
  }

  async function cancel(): Promise<void> {
    const target = attempt.current
    if (!target?.intent || target.cancelSent || target.notified || !live(target) || latest.current.disabled) return
    if (pending.current) { cancelWhilePending(target); return }
    if (!beginAction(target, 'cancel')) return
    target.cancelSent = true
    try {
      const raw = await target.request<unknown>('POST', `${BASE}/cancel`, { intent: target.intent }, { retry: 'never' })
      await accept(raw, target, 'cancel')
    } catch {
      if (live(target)) setView(previous => ({ ...previous, action: '', unknown: true,
        notice: latest.current.t('取消结果未确认；不会重发取消或开始新的读取，请主动查询本次状态。') }))
    } finally { endAction(target) }
  }

  function close(): void {
    clearConfirmation(); setOpen(false)
    if (!preserveOnHide) { abandon(); setExpired(false); setView(emptyView(owner.current)) }
  }
  const success = data?.saved === true && data.phase === 'saved' && !visible.unknown
  const unknownSaved = !success && (visible.unknown || data?.saved === null)
  const canConfirm = Boolean(data && target && !target.confirmSent && !target.cancelSent && data.phase === 'awaiting_confirmation'
    && data.saved === false && !visible.unknown && !expired && performance.now() < target.deadline
    && activeSource && operationSource && !disabled && !busy)
  const canCancel = Boolean(target?.intent && !target.cancelSent && !target.notified && activeSource && !disabled)

  return <section className='plugin-login-enrollment' hidden={!active} aria-label={t('从当前电脑导入 ChatGLM')}>
    <h3 className='text-sm font-semibold'>{t('从当前电脑导入 ChatGLM')}</h3>
    <p className='text-xs text-muted-foreground'>{data
      ? t('本次导入状态请在确认窗口查看；不会重复读取或保存。')
      : t('尚未准备。此入口仅用于当前 Windows 电脑的 ChatGLM 客户端，不会读取服务器或其他电脑的文件。')}</p>
    {!open && visible.notice ? <Alert variant='warning'><AlertDescription>{visible.notice}</AlertDescription></Alert> : null}
    <PluginLoginActions host={footerHost} active={active}>
      <Button type='button' disabled={!activeSource || disabled || busy || open}
        onClick={() => target ? setOpen(true) : void prepare()}>
        {target ? t('查看本次导入状态') : t('准备本次导入（不读取凭据）')}
      </Button>
    </PluginLoginActions>
    <Dialog open={open && activeSource && operationSource} onOpenChange={next => { if (!next) close() }}>
      <DialogContent overlayForceRender>
        <DialogHeader><DialogTitle>{t('确认本次 ChatGLM 本机导入')}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogDescription>{t('先核对固定读取范围。只有勾选并确认后，才会开始读取本次登录凭据。')}</DialogDescription>
          <Alert variant='warning'><AlertTitle>{t('仅限当前 Windows 电脑')}</AlertTitle><AlertDescription>
            {t('只读当前 Windows 用户的 ChatGLM 配置文件，不扫描其他客户端、浏览器配置、其他 Windows 用户或其他账号。')}
          </AlertDescription></Alert>
          <dl className='flex flex-col gap-1 text-xs'>
            <dt className='font-medium'>{t('固定读取范围')}</dt>
            <dd><code>chatglm/Local State</code><br /><code>chatglm/Network/Cookies</code></dd>
            <dt className='mt-2 font-medium'>{t('仅这两个登录 Cookie')}</dt>
            <dd><code>.chatglm.cn</code><br /><code>chatglm_token</code><br /><code>chatglm_refresh_token</code></dd>
          </dl>
          <p className='text-xs text-muted-foreground'>{t('本次导入仅用于验证并保存 ChatGLM 账号。凭据不会返回界面，不会自动读取其他客户端或账号。')}</p>
          {data ? <>
            <dl className='flex min-w-0 flex-col gap-1 text-xs'>
              <dt className='font-medium'>{t('本次插件包')}</dt><dd className='break-all'>{data.package.id} · {data.package.version}</dd>
              <dt className='mt-2 text-muted-foreground'>{t('插件包校验值')}</dt><dd><code className='break-all'>{data.package.archiveDigest}</code></dd>
              <dt className='text-muted-foreground'>{t('代码校验值')}</dt><dd><code className='break-all'>{data.package.moduleDigest}</code></dd>
              <dt className='text-muted-foreground'>{t('权限声明校验值')}</dt><dd><code className='break-all'>{data.package.declarationDigest}</code></dd>
            </dl>
            {!visible.unknown && visible.action !== 'confirm' ? <>
              <p role='status' className='text-xs'>{phaseText(data.phase, t)}</p>
              {data.message ? <p className='whitespace-pre-wrap text-xs text-muted-foreground'>{t(data.message)}</p> : null}
            </> : null}
            {success ? <Alert>{t('账号已确认保存；关闭界面不会再次保存。')}</Alert>
              : unknownSaved ? <Alert variant='warning'>{t('保存未确认')}</Alert>
                : data.saved === false ? <p className='text-xs'>{t('尚未确认保存成功')}</p> : null}
          </> : null}
          {visible.action ? <p role='status' className='flex items-center gap-2 text-xs'><Spinner />{t('正在处理本次操作，请勿重复点击。')}</p> : null}
          {expired && !visible.unknown && !target?.confirmSent && data?.phase === 'awaiting_confirmation' ? <Alert variant='warning'>{t('本次确认已过期，不能再次提交。')}</Alert> : null}
          {visible.notice ? <Alert variant='warning'><AlertDescription>{visible.notice}</AlertDescription></Alert> : null}
          {data?.phase === 'awaiting_confirmation' && !target?.confirmSent && !target?.cancelSent ? <Label htmlFor={`${prefix}-confirm`} className='flex items-start gap-2 text-xs leading-relaxed'>
            <Checkbox id={`${prefix}-confirm`} checked={checked} disabled={!canConfirm} className='mt-0.5'
              onCheckedChange={value => {
                if (!canConfirm || !target || !live(target)) return
                checkedRef.current = value === true; setChecked(value === true)
              }} />
            <span>{t('我已核对范围，仅同意本次读取当前电脑的上述两个 ChatGLM 登录 Cookie，用于账号验证和保存。')}</span>
          </Label> : null}
          <p className='text-xs text-muted-foreground'>{t('确认只发送一次。网络错误、重新登录或会话恢复后不会自动重发，也不会恢复确认勾选。')}</p>
          {target?.intent && !success ? <Button type='button' variant='outline' className='self-start' disabled={busy || disabled || !activeSource}
            onClick={() => void status()}>{t('查询本次状态（只读）')}</Button> : null}
        </DialogBody>
        <DialogFooter className='flex-wrap'>
          {canCancel ? <Button type='button' variant='outline' onClick={() => void cancel()}>{t('取消本次导入')}</Button> : null}
          <Button type='button' variant='outline' onClick={close}>{t('关闭')}</Button>
          {data?.phase === 'awaiting_confirmation' && !target?.confirmSent && !target?.cancelSent ? <Button type='button' disabled={!canConfirm || !checked}
            onClick={() => void confirm()}>{t('确认读取并导入一次')}</Button> : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
}
