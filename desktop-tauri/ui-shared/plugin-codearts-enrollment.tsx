/** CodeArts 显式首次登记：只由用户提交一次，保存未知不重发、不当作令牌验证成功。 */
import * as React from 'react'
import {
  Alert, AlertDescription, Button, Checkbox, Dialog, DialogBody, DialogContent,
  DialogDescription, DialogFooter, DialogHeader, DialogTitle, Label, Spinner,
} from '@ui'
import { PluginLoginActions, SecretTextarea } from './plugin-login-form'
import { MANAGEMENT_SESSION_CHANGE } from './plugin-login-managed'
import {
  CODEARTS_CREDENTIAL_LIMIT, CODEARTS_ENROLLMENT_BASE as BASE, captureCodeArtsEnrollmentSource,
  codeArtsCredentialFits, codeArtsEnrollmentAvailable, codeArtsEnrollmentIdentity,
  readCodeArtsEnrollmentPayload, subscribeCodeArtsEnrollmentSource,
  type CodeArtsEnrollmentAccount, type CodeArtsEnrollmentPayload, type CodeArtsEnrollmentSource,
} from './plugin-codearts-enrollment-wire'

type Translate = (key: string, vars?: Record<string, string | number>) => string
type Action = '' | 'prepare' | 'confirm' | 'status' | 'cancel'
type Attempt = {
  owner: number; source: CodeArtsEnrollmentSource; intent: string; identity: string; deadline: number
  confirmSent: boolean; cancelSent: boolean; cancelPending: boolean; blocked: boolean; unknown: boolean
  waitVersion: number; notified: boolean; last: CodeArtsEnrollmentPayload | null
}
type View = { owner: number; data: CodeArtsEnrollmentPayload | null; action: Action; unknown: boolean; notice: string }
const emptyView = (owner: number): View => ({ owner, data: null, action: '', unknown: false, notice: '' })

export function PluginCodeArtsEnrollment({ active, disabled = false, current, onSaved, t, footerHost }: {
  active: boolean
  disabled?: boolean
  current(): boolean
  onSaved(account: CodeArtsEnrollmentAccount | null): void | Promise<void>
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
  const draftSource = React.useRef<CodeArtsEnrollmentSource | null>(null)
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
  const available = active && current() && codeArtsEnrollmentAvailable()
  const busy = Boolean(pending.current || target?.cancelPending || visible.action)
  const committed = data?.saved === true
  const unknown = !committed && (data?.saved === null || data?.phase === 'saving' || visible.action === 'confirm'
    || visible.unknown && target?.confirmSent === true)

  function sourceEnabled(): boolean { return latest.current.current() && codeArtsEnrollmentAvailable() }
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
    const target = attempt.current
    const wasSaved = target?.last?.saved === true
    const mayHaveSaved = target?.confirmSent === true && !wasSaved
      && !(target.last?.saved === false && ['failed', 'canceled', 'expired'].includes(target.last.phase))
    if (target) { target.blocked = true; target.waitVersion++ }
    attempt.current = null; owner.current++
    clearInput(); clearConfirmation()
    if (!alive.current) return
    dialogOpen.current = false; setOpen(false); setExpired(false)
    const notice = wasSaved ? latest.current.t('管理会话或窗口来源已变化；原登记已确认提交，但当前界面不再接续旧操作，也不会重新保存。')
      : mayHaveSaved ? latest.current.t('管理会话或窗口来源已变化，原登记保存未确认；新会话不会接续、查询、取消或重新提交旧操作。')
        : latest.current.t('管理会话或窗口来源已变化，本次确认已失效；不会恢复勾选，也不会查询或取消旧操作。')
    setView({ ...emptyView(owner.current), unknown: mayHaveSaved, notice })
  }
  function markUnknown(target: Attempt, notice: string): void {
    if (!owned(target)) return
    target.blocked = true; target.unknown = target.last?.saved !== true; clearConfirmation()
    setView(previous => ({ ...(previous.owner === target.owner ? previous : emptyView(target.owner)),
      data: target.last, unknown: target.unknown, notice }))
  }
  function close(): void {
    const target = attempt.current
    const wasOpen = dialogOpen.current
    dialogOpen.current = false
    clearInput(); clearConfirmation(); setOpen(false)
    if (!target || !wasOpen) return
    target.blocked = true; target.waitVersion++
    if (!owned(target)) { sourceLost(); return }
    if (target.confirmSent && target.last?.saved !== true && !(target.last?.saved === false && ['failed', 'canceled', 'expired'].includes(target.last.phase))) {
      markUnknown(target, latest.current.t('已停止本次等待；保存可能已经提交，请主动查询原操作，不要再次确认。'))
    } else if (target.last?.saved !== true && !target.unknown) {
      setView(previous => ({ ...previous, notice: latest.current.t('本次确认已关闭，不会恢复勾选或再次确认；未发送换证请求。') }))
    }
  }
  function guard(): void {
    if (attempt.current && !attempt.current.source.current() || draftSource.current && !draftSource.current.current()) sourceLost()
    else if (!latest.current.active && (attempt.current || draftSource.current)) close()
  }

  React.useEffect(() => {
    alive.current = true
    const unsubscribe = subscribeCodeArtsEnrollmentSource(guard)
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
      // 卸载仅停止本地等待；不对原操作或新会话补发取消/查询。
    }
  }, [])
  React.useEffect(() => { guard() })
  React.useEffect(() => { if (disabled) clearConfirmation() }, [disabled])
  React.useEffect(() => {
    if (!target || data?.phase !== 'awaiting_confirmation' || target.confirmSent || target.blocked) return
    const timer = window.setTimeout(() => {
      if (!live(target)) return
      target.blocked = true; clearConfirmation(); setExpired(true)
    }, Math.max(0, target.deadline - performance.now()))
    return () => window.clearTimeout(timer)
  }, [data?.intent, data?.phase, data?.remainingMs])

  function canPrepare(): boolean {
    const target = attempt.current
    if (!latest.current.active || !sourceEnabled() || latest.current.disabled || pending.current || target?.cancelPending) return false
    if (!target) return true
    // 已提交或保存未知绝不提示再保存；只有明确未保存的结束态才能主动准备新的材料。
    return (!target.confirmSent && target.blocked && !target.intent)
      || (target.last?.saved === false && !target.unknown && ['failed', 'canceled', 'expired'].includes(target.last.phase))
  }
  function begin(target: Attempt, action: Action): boolean {
    if (!live(target) || pending.current || target.cancelPending || latest.current.disabled) return false
    pending.current = { owner: target.owner, action }
    clearConfirmation()
    target.unknown = action === 'confirm' || target.unknown
    setView(previous => ({ ...(previous.owner === target.owner ? previous : emptyView(target.owner)), action, notice: '', unknown: target.unknown }))
    return true
  }
  function end(target: Attempt): void {
    if (pending.current?.owner !== target.owner) return
    pending.current = null
    if (alive.current) setView(previous => ({ ...previous, action: target.cancelPending ? 'cancel' : '' }))
  }
  async function accept(raw: unknown, target: Attempt, action: Action, startedAt: number, version: number): Promise<void> {
    const value = readCodeArtsEnrollmentPayload(raw)
    if (target.intent && target.intent !== value.intent || target.identity && target.identity !== codeArtsEnrollmentIdentity(value)) throw new Error('codearts_enrollment_scope_changed')
    if (action === 'prepare' && (value.phase !== 'awaiting_confirmation' || value.saved !== false || value.readyRegistered === true || value.account !== null)) throw new Error('codearts_enrollment_not_prepared')
    if (value.saved === true && !target.confirmSent || target.last?.saved === true && value.saved !== true) throw new Error('codearts_enrollment_saved_conflict')
    if (!live(target) || target.waitVersion !== version) return
    if ((action !== 'cancel' && target.cancelPending) || (target.cancelSent && value.saved !== true && (action === 'prepare' || action === 'confirm'))) {
      markUnknown(target, latest.current.t('已申请取消本次等待；保存可能已经提交，最终结果尚未确认。'))
      return
    }
    target.intent = value.intent; target.identity ||= codeArtsEnrollmentIdentity(value)
    target.deadline = Math.min(target.deadline, startedAt + value.remainingMs)
    target.last = value; target.unknown = false
    if (value.phase !== 'awaiting_confirmation' || value.saved !== false || value.remainingMs === 0) target.blocked = true
    clearConfirmation(); setExpired(performance.now() >= target.deadline)
    setView({ owner: target.owner, data: value, action: target.cancelPending ? 'cancel' : '', unknown: false, notice: '' })
    if (value.saved === true && !target.notified) {
      target.notified = true
      try { await latest.current.onSaved(value.account) }
      catch { if (live(target)) setView(previous => ({ ...previous,
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
    if (!codeArtsCredentialFits(credentialJson)) { setInvalidCredential(true); return }
    if (attempt.current && !attempt.current.source.current()) { credentialJson = ''; sourceLost(); return }
    const source = draftSource.current || captureCodeArtsEnrollmentSource(sourceEnabled)
    if (!source?.current()) { credentialJson = ''; sourceLost(); return }
    const target: Attempt = { owner: ++owner.current, source, intent: '', identity: '', deadline: Infinity,
      confirmSent: false, cancelSent: false, cancelPending: false, blocked: false, unknown: false, waitVersion: 0, notified: false, last: null }
    attempt.current = target
    dialogOpen.current = true
    setView(emptyView(target.owner)); setOpen(true); setExpired(false)
    if (!begin(target, 'prepare')) {
      credentialJson = ''; clearInput(); target.blocked = true
      return
    }
    // 凭据仅存在于本次局部原文和具名请求，发送前同步清空 textarea，不保留 React 草稿。
    clearInput()
    const startedAt = performance.now(), version = target.waitVersion
    try {
      const request = target.source.request<unknown>('POST', `${BASE}/prepare`, { credentialJson }, { retry: 'never' })
      credentialJson = ''
      await accept(await request, target, 'prepare', startedAt, version)
    } catch {
      if (target.waitVersion === version) markUnknown(target, latest.current.t('准备结果未确认；原文已清空，不会自动重发，也不会转入手工导入。'))
    } finally { credentialJson = ''; end(target) }
  }
  async function confirm(): Promise<void> {
    const target = attempt.current, currentData = target?.last
    if (!target || !currentData || target.confirmSent || target.cancelSent || target.blocked || !checkedRef.current
      || target.unknown || currentData.phase !== 'awaiting_confirmation' || currentData.saved !== false
      || expired || performance.now() >= target.deadline || !begin(target, 'confirm')) return
    // 首个 await 前永久占用本意图的确认机会；状态回查或会话恢复不会解锁。
    target.confirmSent = true; target.blocked = true
    const startedAt = performance.now(), version = target.waitVersion
    try {
      await accept(await target.source.request<unknown>('POST', `${BASE}/confirm`, {
        intent: target.intent, confirmation: { schema: 1, initialRegistration: true, oneOpportunity: true },
      }, { retry: 'never' }), target, 'confirm', startedAt, version)
    } catch {
      if (target.waitVersion === version) markUnknown(target, latest.current.t('保存未确认；请主动查询原操作，不要再次确认或重新保存。'))
    } finally { end(target) }
  }
  async function status(): Promise<void> {
    const target = attempt.current
    if (!target?.intent || !begin(target, 'status')) return
    const startedAt = performance.now(), version = target.waitVersion
    try {
      await accept(await target.source.request<unknown>('GET', `${BASE}/status?intent=${encodeURIComponent(target.intent)}`,
        undefined, { retry: 'never' }), target, 'status', startedAt, version)
    } catch {
      if (target.waitVersion === version) markUnknown(target, latest.current.t('本次状态未确认；不会重发确认、取消或准备请求。'))
    } finally { end(target) }
  }
  async function cancel(): Promise<void> {
    const target = attempt.current
    if (!target?.intent || target.cancelSent || target.last?.saved === true || !live(target) || latest.current.disabled) return
    // cancel 与原确认可以各发送一次；取消不撤回 worker，也不把原确认材料放回。
    target.cancelSent = true; target.cancelPending = true; target.blocked = true; target.waitVersion++
    target.unknown = target.confirmSent || target.unknown
    clearConfirmation()
    setView(previous => ({ ...previous, action: 'cancel', unknown: target.unknown,
      notice: latest.current.t('已申请取消本次等待；保存可能已经提交，最终结果尚未确认。') }))
    const startedAt = performance.now(), version = target.waitVersion
    try {
      await accept(await target.source.request<unknown>('POST', `${BASE}/cancel`, { intent: target.intent }, { retry: 'never' }),
        target, 'cancel', startedAt, version)
    } catch {
      if (target.waitVersion === version) markUnknown(target, latest.current.t('取消结果未确认；若保存已经开始，可能已经提交，请主动查询原操作。'))
    } finally {
      target.cancelPending = false
      if (live(target)) setView(previous => ({ ...previous, action: pending.current?.owner === target.owner ? pending.current.action : '' }))
    }
  }

  const canConfirm = Boolean(target && data && !target.blocked && !target.confirmSent && !target.cancelSent
    && !unknown && !target.unknown && data.phase === 'awaiting_confirmation' && data.saved === false && !expired
    && performance.now() < target.deadline && available && target.source.current() && !disabled && !busy)
  const canQuery = Boolean(target?.intent && available && target.source.current() && !disabled && !busy)
  const canCancel = Boolean(target?.intent && !target.cancelSent && !committed && available && target.source.current() && !disabled)
  const showPrepare = canPrepare()
  let resultText = ''
  if (committed) resultText = data?.storageClosed === false
    ? t('本次登记已提交，但存储收尾未完成，暂不可用；不要重新保存。')
    : data?.storageClosed !== true || data?.readyRegistered !== true
      ? t('本次登记已提交，但一次机会或存储收尾尚未确认，暂不可用；不要重新保存。')
      : t('本次材料和一次续期机会已确认登记；未验证令牌，也未发送换证。')
  else if (unknown) resultText = t('保存未确认；请主动查询原操作，不要再次确认或重新保存。')
  else if (data?.phase === 'awaiting_confirmation' && !visible.unknown) resultText = t('等待本次确认，尚未登记续期机会')
  else if (data?.phase === 'failed') resultText = t('本次登记未完成')
  else if (data?.phase === 'canceled') resultText = t('本次登记等待已取消')
  else if (data?.phase === 'expired') resultText = t('本次登记确认已过期')

  return <section className='plugin-login-enrollment' hidden={!active} aria-label={t('CodeArts 显式首次登记')}>
    <div><h3 className='text-sm font-semibold'>{t('CodeArts 显式首次登记')}</h3>
      <p className='mt-1 text-xs text-muted-foreground'>{t('登记一次续期机会，不发送换证')}</p></div>
    <Alert variant='warning'><AlertDescription>{t('仅登记本次完整材料的一次续期机会；不会验证令牌、启用自动续期、解封任何旧登记或发起换证请求。')}</AlertDescription></Alert>
    {showPrepare ? <>
      <SecretTextarea id={`${prefix}-credential`} label={t('完整 CodeArts JSON 原文')} resetKey={active ? 'active' : 'hidden'}
        ref={input} rows={5} maxLength={CODEARTS_CREDENTIAL_LIMIT} defaultValue=''
        autoComplete='off' autoCapitalize='none' spellCheck={false} disabled={disabled || busy || open}
        placeholder={t('粘贴本次完整 CodeArts JSON 原文；如需备注名，请将 name 写在原 JSON 内。')}
        onChange={event => {
          if (!draftSource.current) draftSource.current = captureCodeArtsEnrollmentSource(sourceEnabled)
          if (!draftSource.current?.current()) { sourceLost(); return }
          const value = event.currentTarget.value
          setHasCredential(value.length > 0); setInvalidCredential(value.length > 0 && !codeArtsCredentialFits(value))
        }} />
      <p className='text-xs text-muted-foreground'>{t('最多 128 KiB；原文不会在界面解析或重新拼接，发送准备请求前会立即清空。此入口不另配代理。')}</p>
      {invalidCredential ? <Alert variant='warning'>{t('请粘贴非空的完整 CodeArts JSON 原文，且 UTF-8 大小不超过 128 KiB。')}</Alert> : null}
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
        <DialogHeader><DialogTitle>{t('确认 CodeArts 本次首次登记')}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogDescription>{t('登记一次续期机会，不发送换证')}</DialogDescription>
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
            <span>{t('我仅同意以本次粘贴的完整材料登记一次续期机会；不验证令牌，不解封任何旧槽，不发送请求。')}</span>
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
