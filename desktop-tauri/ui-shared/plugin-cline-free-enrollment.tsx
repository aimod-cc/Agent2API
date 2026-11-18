/** Free 仅新账号首次登记：原文准备、单独确认一次，未知结果只由用户查询。 */
import * as React from 'react'
import {
  Alert, AlertDescription, Button, Checkbox, Dialog, DialogBody, DialogContent,
  DialogDescription, DialogFooter, DialogHeader, DialogTitle, Label, Spinner,
} from '@ui'
import { PluginLoginActions, SecretTextarea } from './plugin-login-form'
import { MANAGEMENT_SESSION_CHANGE } from './plugin-login-managed'
import {
  CLINE_FREE_CREDENTIAL_LIMIT, CLINE_FREE_ENROLLMENT_BASE as BASE, captureClineFreeEnrollmentSource,
  clineFreeCredentialFits, clineFreeEnrollmentAvailable, clineFreeEnrollmentIdentity,
  readClineFreeEnrollmentPayload, subscribeClineFreeEnrollmentSource,
  type ClineFreeEnrollmentAccount, type ClineFreeEnrollmentPayload, type ClineFreeEnrollmentSource,
} from './plugin-cline-free-enrollment-wire'

type Translate = (key: string, vars?: Record<string, string | number>) => string
type Action = '' | 'prepare' | 'confirm' | 'status' | 'cancel'
type Attempt = {
  owner: number; source: ClineFreeEnrollmentSource; intent: string; identity: string; deadline: number
  confirmSent: boolean; cancelSent: boolean; cancelPending: boolean; blocked: boolean; unknown: boolean
  waitVersion: number; notified: boolean; last: ClineFreeEnrollmentPayload | null
}
type View = { owner: number; data: ClineFreeEnrollmentPayload | null; action: Action; unknown: boolean; notice: string }
const emptyView = (owner: number): View => ({ owner, data: null, action: '', unknown: false, notice: '' })
const terminalUnstored = (data: ClineFreeEnrollmentPayload | null) => data?.saved === false
  && ['failed', 'canceled', 'expired'].includes(data.phase)

export function PluginClineFreeEnrollment({ active, disabled = false, current, onSaved, t, footerHost }: {
  active: boolean
  disabled?: boolean
  current(): boolean
  onSaved(account: ClineFreeEnrollmentAccount | null): void | Promise<void>
  t: Translate
  footerHost?: HTMLElement | null
}): React.ReactElement {
  const [view, setView] = React.useState<View>(() => emptyView(0))
  const [open, setOpen] = React.useState(false)
  const [checked, setChecked] = React.useState(false)
  const [hasCredential, setHasCredential] = React.useState(false)
  const [invalidCredential, setInvalidCredential] = React.useState(false)
  const [expired, setExpired] = React.useState(false)
  const input = React.useRef<HTMLTextAreaElement>(null)
  const draftSource = React.useRef<ClineFreeEnrollmentSource | null>(null)
  const attempt = React.useRef<Attempt | null>(null)
  const owner = React.useRef(0)
  const alive = React.useRef(false)
  const dialogOpen = React.useRef(false)
  const checkedRef = React.useRef(false)
  const pending = React.useRef<null | { owner: number; action: Action }>(null)
  const latest = React.useRef({ active, disabled, current, onSaved, t })
  latest.current = { active, disabled, current, onSaved, t }
  const prefix = React.useId()
  const visible = view.owner === owner.current ? view : emptyView(owner.current)
  const target = attempt.current
  const data = visible.data
  const available = active && current() && clineFreeEnrollmentAvailable()
  const busy = Boolean(pending.current || target?.cancelPending || visible.action)
  const committed = data?.saved === true
  const unknown = !committed && (data?.saved === null || data?.phase === 'saving'
    || visible.action === 'confirm' || visible.unknown && target?.confirmSent === true)

  function sourceEnabled(): boolean { return latest.current.current() && clineFreeEnrollmentAvailable() }
  function owned(target: Attempt): boolean {
    return alive.current && owner.current === target.owner && attempt.current === target && target.source.current()
  }
  function live(target: Attempt): boolean { return latest.current.active && owned(target) }
  function clearConfirmation(): void { checkedRef.current = false; if (alive.current) setChecked(false) }
  function clearInput(): void {
    if (input.current) input.current.value = ''
    draftSource.current = null
    if (alive.current) { setHasCredential(false); setInvalidCredential(false) }
  }
  function sourceLost(): void {
    const original = attempt.current
    const wasSaved = original?.last?.saved === true
    const mayHaveSaved = original?.confirmSent === true && !wasSaved && !terminalUnstored(original.last)
    if (original) { original.blocked = true; original.waitVersion++ }
    attempt.current = null; owner.current++
    clearInput(); clearConfirmation()
    if (!alive.current) return
    dialogOpen.current = false; setOpen(false); setExpired(false)
    const notice = wasSaved ? latest.current.t('管理会话或窗口来源已变化；原登记已确认提交，但当前界面不再接续旧操作，也不会重新保存。')
      : mayHaveSaved ? latest.current.t('管理会话或窗口来源已变化，原登记保存未确认；新会话不会接续、查询、取消或重新提交旧操作。')
        : latest.current.t('管理会话或窗口来源已变化，本次确认已失效；不会恢复勾选，也不会查询或取消旧操作。')
    setView({ ...emptyView(owner.current), unknown: mayHaveSaved, notice })
  }
  function markUnknown(original: Attempt, notice: string): void {
    if (!owned(original)) return
    // 已经接受的真实 COMMIT 不能被迟到的取消/查询失败改成未知或未保存。
    if (original.last?.saved === true) return
    original.blocked = true; original.unknown = true; clearConfirmation()
    setView(previous => ({ ...(previous.owner === original.owner ? previous : emptyView(original.owner)),
      data: original.last, unknown: true, notice }))
  }
  function close(): void {
    const original = attempt.current, wasOpen = dialogOpen.current
    dialogOpen.current = false
    clearInput(); clearConfirmation(); setOpen(false)
    if (!original || !wasOpen) return
    original.blocked = true; original.waitVersion++
    if (!owned(original)) { sourceLost(); return }
    if (original.confirmSent && original.last?.saved !== true && !terminalUnstored(original.last)) {
      markUnknown(original, latest.current.t('已停止本次等待；保存可能已经提交，请主动查询原操作，不要再次确认。'))
    } else if (original.last?.saved !== true && !original.unknown) {
      setView(previous => ({ ...previous,
        notice: latest.current.t('本次确认已关闭，不会恢复勾选或再次确认；未发送换证请求。') }))
    }
    // 关闭只影响本地等待。不得自动发 cancel/status，亦不恢复原确认机会。
  }
  function guard(): void {
    if (attempt.current && !attempt.current.source.current() || draftSource.current && !draftSource.current.current()) sourceLost()
    else if (!latest.current.active && (attempt.current || draftSource.current)) {
      close(); clearInput(); clearConfirmation()
    }
  }

  React.useEffect(() => {
    alive.current = true
    const unsubscribe = subscribeClineFreeEnrollmentSource(guard)
    window.addEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost)
    window.addEventListener('focus', guard)
    window.addEventListener('pagehide', close)
    return () => {
      alive.current = false
      if (attempt.current) { attempt.current.blocked = true; attempt.current.waitVersion++ }
      clearInput(); checkedRef.current = false
      unsubscribe()
      window.removeEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost)
      window.removeEventListener('focus', guard)
      window.removeEventListener('pagehide', close)
      // 卸载不出网，也不把旧意图搬到另一个来源或恢复勾选。
    }
  }, [])
  React.useEffect(() => { guard() })
  React.useEffect(() => { if (disabled) clearConfirmation() }, [disabled])
  React.useEffect(() => {
    if (!target || data?.phase !== 'awaiting_confirmation' || target.confirmSent || target.blocked) return
    const timer = window.setTimeout(() => {
      if (!owned(target)) return
      target.blocked = true; clearConfirmation(); setExpired(true)
      // 过期仅封本地确认，不自动取消，不因此否认可能迟到的真实提交。
    }, Math.max(0, target.deadline - performance.now()))
    return () => window.clearTimeout(timer)
  }, [data?.intent, data?.phase, data?.remainingMs])

  function canPrepare(): boolean {
    const original = attempt.current
    if (!latest.current.active || !sourceEnabled() || latest.current.disabled || pending.current || original?.cancelPending) return false
    if (!original) return true
    // 保存未确认时不得重收材料或开新意图；只允许用户主动查询原 intent。
    return (!original.confirmSent && original.blocked && !original.intent)
      || terminalUnstored(original.last) && !original.unknown
  }
  function begin(original: Attempt, action: Action): boolean {
    if (!live(original) || pending.current || original.cancelPending || latest.current.disabled) return false
    pending.current = { owner: original.owner, action }
    clearConfirmation()
    original.unknown = action === 'confirm' || original.unknown
    setView(previous => ({ ...(previous.owner === original.owner ? previous : emptyView(original.owner)),
      action, notice: '', unknown: original.unknown }))
    return true
  }
  function end(original: Attempt): void {
    if (pending.current?.owner !== original.owner) return
    pending.current = null
    if (alive.current) setView(previous => ({ ...previous, action: original.cancelPending && attempt.current === original ? 'cancel' : '' }))
  }
  async function accept(raw: unknown, original: Attempt, action: Action, startedAt: number, version: number): Promise<void> {
    const value = readClineFreeEnrollmentPayload(raw)
    if (original.intent && original.intent !== value.intent
      || original.identity && original.identity !== clineFreeEnrollmentIdentity(value)) throw new Error('cline_free_enrollment_scope_changed')
    if (action === 'prepare' && (value.phase !== 'awaiting_confirmation' || value.saved !== false
      || value.readyRegistered === true || value.account !== null)) throw new Error('cline_free_enrollment_not_prepared')
    if (value.saved === true && !original.confirmSent) throw new Error('cline_free_enrollment_unsent_save')
    if (!owned(original)) return
    // 关闭或 cancel 可淘汰旧等待版本，但不能遮掉同源、同 intent 的真实保存回执。
    if (value.saved !== true && (original.last?.saved === true || original.waitVersion !== version || !latest.current.active)) return
    if (value.saved !== true && action !== 'cancel' && (original.cancelPending || original.cancelSent && ['prepare', 'confirm'].includes(action))) {
      markUnknown(original, latest.current.t('已申请取消本次等待；保存可能已经提交，最终结果尚未确认。'))
      return
    }
    original.intent = value.intent; original.identity ||= clineFreeEnrollmentIdentity(value)
    original.deadline = Math.min(original.deadline, startedAt + value.remainingMs)
    original.last = value; original.unknown = value.saved === null || value.phase === 'saving'
    if (value.phase !== 'awaiting_confirmation' || value.saved !== false || value.remainingMs === 0) original.blocked = true
    clearConfirmation(); setExpired(performance.now() >= original.deadline)
    setView({ owner: original.owner, data: value, action: original.cancelPending ? 'cancel' : '', unknown: original.unknown, notice: '' })
    if (value.saved === true && !original.notified) {
      original.notified = true
      try { await latest.current.onSaved(value.account) }
      catch { if (owned(original)) setView(previous => ({ ...previous,
        notice: latest.current.t('本次登记已确认提交，但账号列表刷新未完成；不会再次保存。') })) }
    }
  }

  async function prepare(): Promise<void> {
    if (!alive.current || !latest.current.active) return
    if (!canPrepare()) {
      if (attempt.current && !attempt.current.source.current() || draftSource.current && !draftSource.current.current()) sourceLost()
      return
    }
    let credentialJson = input.current?.value || ''
    if (!clineFreeCredentialFits(credentialJson)) { setInvalidCredential(true); return }
    const source = draftSource.current || captureClineFreeEnrollmentSource(sourceEnabled)
    if (!source?.current()) { credentialJson = ''; sourceLost(); return }
    const original: Attempt = { owner: ++owner.current, source, intent: '', identity: '', deadline: Infinity,
      confirmSent: false, cancelSent: false, cancelPending: false, blocked: false, unknown: false,
      waitVersion: 0, notified: false, last: null }
    attempt.current = original
    dialogOpen.current = true
    setView(emptyView(original.owner)); setOpen(true); setExpired(false)
    if (!begin(original, 'prepare')) { credentialJson = ''; clearInput(); original.blocked = true; return }
    // 凭据只在 DOM 草稿与这次原文请求中；IPC 调用前同步清空 DOM，不进 React 状态。
    clearInput()
    const startedAt = performance.now(), version = original.waitVersion
    try {
      const request = original.source.request<unknown>('POST', `${BASE}/prepare`, { credentialJson }, { retry: 'never' })
      credentialJson = ''
      await accept(await request, original, 'prepare', startedAt, version)
    } catch {
      if (original.waitVersion === version) markUnknown(original,
        latest.current.t('准备结果未确认；原文已清空，不会自动重发，也不会转入手工导入。'))
    } finally { credentialJson = ''; end(original) }
  }
  async function confirm(): Promise<void> {
    const original = attempt.current, currentData = original?.last
    if (!original || !currentData || !dialogOpen.current || original.confirmSent || original.cancelSent || original.blocked
      || !checkedRef.current || original.unknown || currentData.phase !== 'awaiting_confirmation' || currentData.saved !== false
      || expired || performance.now() >= original.deadline || !begin(original, 'confirm')) return
    original.confirmSent = true; original.blocked = true // 在首个 await 前占用唯一确认机会。
    const startedAt = performance.now(), version = original.waitVersion
    try {
      await accept(await original.source.request<unknown>('POST', `${BASE}/confirm`, {
        intent: original.intent, confirmation: { schema: 2, newAccountOnly: true, oneOpportunity: true, autoRefresh: true },
      }, { retry: 'never' }), original, 'confirm', startedAt, version)
    } catch {
      if (original.waitVersion === version) markUnknown(original,
        latest.current.t('保存未确认；请主动查询原操作，不要再次确认或重新保存。'))
    } finally { end(original) }
  }
  async function status(): Promise<void> {
    const original = attempt.current
    if (!original?.intent || !begin(original, 'status')) return
    const startedAt = performance.now(), version = original.waitVersion
    try {
      await accept(await original.source.request<unknown>('GET', `${BASE}/status?intent=${encodeURIComponent(original.intent)}`,
        undefined, { retry: 'never' }), original, 'status', startedAt, version)
    } catch {
      if (original.waitVersion === version) markUnknown(original,
        latest.current.t('本次状态未确认；不会重发确认、取消或准备请求。'))
    } finally { end(original) }
  }
  async function cancel(): Promise<void> {
    const original = attempt.current
    if (!original?.intent || original.cancelSent || original.confirmSent || original.unknown || original.last?.saved !== false
      || !live(original) || latest.current.disabled || pending.current) return
    original.cancelSent = true; original.cancelPending = true; original.blocked = true; original.waitVersion++
    clearConfirmation()
    setView(previous => ({ ...previous, action: 'cancel',
      notice: latest.current.t('已申请取消本次等待；保存可能已经提交，最终结果尚未确认。') }))
    const startedAt = performance.now(), version = original.waitVersion
    try {
      await accept(await original.source.request<unknown>('POST', `${BASE}/cancel`, { intent: original.intent }, { retry: 'never' }),
        original, 'cancel', startedAt, version)
    } catch {
      if (original.waitVersion === version) markUnknown(original,
        latest.current.t('取消结果未确认；若保存已经开始，可能已经提交，请主动查询原操作。'))
    } finally {
      original.cancelPending = false
      if (owned(original)) setView(previous => ({ ...previous,
        action: pending.current?.owner === original.owner ? pending.current.action : '' }))
    }
  }

  const canConfirm = Boolean(target && data && !target.blocked && !target.confirmSent && !target.cancelSent && !target.unknown
    && !unknown && data.phase === 'awaiting_confirmation' && data.saved === false && !expired && dialogOpen.current
    && performance.now() < target.deadline && available && target.source.current() && !disabled && !busy)
  const canQuery = Boolean(target?.intent && available && target.source.current() && !disabled && !busy)
  const canCancel = Boolean(target?.intent && !target.cancelSent && !target.confirmSent && !target.unknown && !committed
    && data?.saved === false && available && target.source.current() && !disabled && !busy)
  let resultText = ''
  if (committed) resultText = data?.storageClosed === false
    ? t('本次登记已提交，但存储收尾未完成，暂不可用；不要重新保存。')
    : data?.storageClosed !== true || data?.readyRegistered !== true
      ? t('本次登记已提交，但一次机会或存储收尾尚未确认，暂不可用；不要重新保存。')
      : t('Free 新账号与受控续期已登记；本次不换证，之后主程序仅在临期或主动刷新时单次续期，结果不明不重发。')
  else if (unknown) resultText = t('保存未确认；请主动查询原操作，不要再次确认或重新保存。')
  else if (data?.phase === 'awaiting_confirmation' && !visible.unknown) resultText = t('等待本次确认，尚未登记续期机会')
  else if (data?.phase === 'failed') resultText = t('本次登记未完成')
  else if (data?.phase === 'canceled') resultText = t('本次登记等待已取消')
  else if (data?.phase === 'expired') resultText = t('本次登记确认已过期')

  return <section className='plugin-login-enrollment' hidden={!active} aria-label={t('Cline Free 新账号首次登记')}>
    <div><h3 className='text-sm font-semibold'>{t('Cline Free 新账号首次登记')}</h3>
      <p className='mt-1 text-xs text-muted-foreground'>{t('新建 Free 账号并登记受控续期；本次不换证，之后只对原账号单次续期，结果不明不重发')}</p></div>
    <Alert variant='warning'><AlertDescription>{t('只接受真正的新 Free 账号。匹配已有账号或旧登记会拒绝；不读取或配对 Pass。本次不验证或换证；确认后允许主程序按原账号受控续期。')}</AlertDescription></Alert>
    {canPrepare() ? <>
      <SecretTextarea id={`${prefix}-credential`} label={t('完整 Cline Free JSON 原文')} resetKey={active ? 'active' : 'hidden'}
        ref={input} rows={5} maxLength={CLINE_FREE_CREDENTIAL_LIMIT} defaultValue=''
        autoComplete='off' autoCapitalize='none' spellCheck={false} disabled={disabled || busy || open}
        placeholder={t('粘贴本次完整 Free 单账号 JSON，须含访问令牌和刷新令牌；如需备注名，将 name 写在原文内。')}
        onChange={event => {
          if (!draftSource.current) draftSource.current = captureClineFreeEnrollmentSource(sourceEnabled)
          if (!draftSource.current?.current()) { sourceLost(); return }
          const value = event.currentTarget.value
          setHasCredential(value.length > 0); setInvalidCredential(value.length > 0 && !clineFreeCredentialFits(value))
        }} />
      <p className='text-xs text-muted-foreground'>{t('最多 128 KiB；原文不会在界面解析或重新拼接，发送准备请求前会立即清空。此入口不另配代理。')}</p>
      {invalidCredential ? <Alert variant='warning'>{t('请粘贴非空的完整 Cline Free JSON 原文，且 UTF-8 大小不超过 128 KiB。')}</Alert> : null}
      <PluginLoginActions host={footerHost} active={active}>
        <Button type='button' disabled={!hasCredential || invalidCredential || disabled || busy || open}
          onClick={() => void prepare()}>{t('准备本次登记（不保存、不换证）')}</Button>
      </PluginLoginActions>
    </> : null}
    {!open && resultText ? <Alert variant={committed && data?.storageClosed === true && data?.readyRegistered === true ? 'default' : 'warning'}>{resultText}</Alert> : null}
    {!open && visible.notice ? <Alert variant='warning'><AlertDescription>{visible.notice}</AlertDescription></Alert> : null}
    {!open && target?.intent ? <Button type='button' variant='outline' className='self-start' disabled={!available || !target.source.current()}
      onClick={() => { if (!target || !live(target)) return; clearConfirmation(); dialogOpen.current = true; setOpen(true) }}>{t('打开本次登记状态')}</Button> : null}
    <Dialog open={open && available && Boolean(target?.source.current())} onOpenChange={next => { if (!next) close() }}>
      <DialogContent overlayForceRender>
        <DialogHeader><DialogTitle>{t('确认本次 Free 新账号首次登记')}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogDescription>{t('新建 Free 账号并登记受控续期；本次不换证，之后只对原账号单次续期，结果不明不重发')}</DialogDescription>
          {!target?.confirmSent && !visible.unknown ? <p className='text-xs text-muted-foreground'>{t('准备仅接收本次原文，不查询账号库、不保存；请核对当前插件包，再单独勾选并确认。')}</p> : null}
          {data ? <dl className='flex min-w-0 flex-col gap-1 text-xs'>
            <dt className='font-medium'>{t('本次插件包')}</dt><dd className='break-all'>{data.package.id} · {data.package.version}</dd>
            <dt className='mt-2 text-muted-foreground'>{t('插件包校验值')}</dt><dd><code className='break-all'>{data.package.archiveDigest}</code></dd>
            <dt className='text-muted-foreground'>{t('代码校验值')}</dt><dd><code className='break-all'>{data.package.moduleDigest}</code></dd>
            <dt className='text-muted-foreground'>{t('权限声明校验值')}</dt><dd><code className='break-all'>{data.package.declarationDigest}</code></dd>
          </dl> : null}
          {resultText ? <Alert variant={committed && data?.storageClosed === true && data?.readyRegistered === true ? 'default' : 'warning'}>{resultText}</Alert> : null}
          {data?.message && !unknown && !visible.unknown && !target?.confirmSent && !target?.cancelSent ? <p className='whitespace-pre-wrap break-words text-xs text-muted-foreground'>{t(data.message)}</p> : null}
          {visible.action ? <p role='status' className='flex items-center gap-2 text-xs'><Spinner />{t('正在处理本次登记，请勿重复点击。')}</p> : null}
          {expired && !unknown && !visible.unknown && !target?.confirmSent && data?.phase === 'awaiting_confirmation' ? <Alert variant='warning'>{t('本次确认已过期，不能再次提交。')}</Alert> : null}
          {visible.notice ? <Alert variant='warning'><AlertDescription>{visible.notice}</AlertDescription></Alert> : null}
          {canConfirm ? <Label htmlFor={`${prefix}-confirm`} className='flex items-start gap-2 text-xs leading-relaxed'>
            <Checkbox id={`${prefix}-confirm`} className='mt-0.5' checked={checked} disabled={!canConfirm} onCheckedChange={value => {
              if (!target || !live(target) || !canConfirm) return
              checkedRef.current = value === true; setChecked(value === true)
            }} />
            <span>{t('我同意用本次完整材料新建 Free 账号并登记受控续期；之后只对原账号单次换证，结果不明不重发。不更新存量账号、不解封旧登记、不配对 Pass，本次不验证或换证。')}</span>
          </Label> : null}
          <p className='text-xs text-muted-foreground'>{t('确认只提交一次。请求失败或会话恢复后不会重发，也不会恢复勾选；状态只由你主动查询。')}</p>
          <p className='text-xs text-muted-foreground'>{t('关闭和取消只停止本次等待，不能撤回已经开始的保存；最终结果可能仍需主动查询。')}</p>
          {target?.intent ? <Button type='button' variant='outline' className='self-start' disabled={!canQuery}
            onClick={() => void status()}>{t('查询本次状态（只读）')}</Button> : null}
        </DialogBody>
        <DialogFooter className='flex-wrap'>
          {canCancel ? <Button type='button' variant='outline' onClick={() => void cancel()}>{t('取消本次登记等待')}</Button> : null}
          <Button type='button' variant='outline' onClick={close}>{t('关闭')}</Button>
          {canConfirm ? <Button type='button' disabled={!checked || !canConfirm} onClick={() => void confirm()}>{t('确认登记一次（不换证）')}</Button> : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
}
