import * as React from 'react'
import { Alert, Button, Checkbox, Field, FieldLabel, Label, Spinner, Textarea } from '@ui'
import { clineFreeCredentialFits, CLINE_FREE_CREDENTIAL_LIMIT } from './plugin-cline-free-enrollment-wire'

type Translate = (key: string, vars?: Record<string, string | number>) => string
type Account = { id: string; provider?: string; updatedAt?: number }
export type ClineFreeCredentialRequest = <T>(method: 'POST', path: string, body: unknown,
  options: { retry: 'never' }) => Promise<T>

export function ClineFreeCredentialUpdate({ account, scopeIdentity, disabled, onBusyChange, onSaved, request, t }: {
  account: Account
  scopeIdentity: string
  disabled?: boolean
  onBusyChange?: (busy: boolean) => void
  onSaved: (account: Account & Record<string, unknown>) => void | Promise<void>
  request: ClineFreeCredentialRequest
  t: Translate
}) {
  const scope = JSON.stringify([scopeIdentity, account.id, account.updatedAt])
  const prefix = React.useId()
  const input = React.useRef<HTMLTextAreaElement>(null)
  const pending = React.useRef(false)
  const alive = React.useRef(true)
  const latest = React.useRef(scope)
  latest.current = scope
  const [filled, setFilled] = React.useState(false)
  const [confirmed, setConfirmed] = React.useState(false)
  const [authorizeRefresh, setAuthorizeRefresh] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [notice, setNotice] = React.useState('')
  React.useEffect(() => {
    if (input.current) input.current.value = ''
    setFilled(false); setConfirmed(false); setAuthorizeRefresh(false); setNotice('')
  }, [scope])
  React.useEffect(() => {
    alive.current = true
    return () => { alive.current = false; if (input.current) input.current.value = '' }
  }, [])
  React.useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])
  React.useEffect(() => () => { onBusyChange?.(false) }, [onBusyChange])

  async function save() {
    if (disabled || pending.current || !confirmed || account.provider !== 'cline-free') return
    let credentialJson = input.current?.value || ''
    if (!clineFreeCredentialFits(credentialJson)) {
      setNotice(t('请粘贴当前账号的完整凭证 JSON，大小不要超过 128 KiB。'))
      return
    }
    const original = scope
    const current = () => alive.current && latest.current === original
    pending.current = true
    setBusy(true); setNotice(''); setConfirmed(false)
    try {
      const result = request<{ updated: boolean; account: Account & Record<string, unknown> }>('POST',
        `/api/accounts/${encodeURIComponent(account.id)}/credentials/cline-free`,
        { credentialJson, confirmed: true, authorizeRefresh, expectedUpdatedAt: Number(account.updatedAt) || 0 }, { retry: 'never' })
      credentialJson = ''
      if (input.current) input.current.value = ''
      setFilled(false)
      const saved = await result
      if (!current()) return
      if (saved.updated !== true || saved.account?.id !== account.id || saved.account.provider !== 'cline-free') {
        throw new Error(t('凭证保存结果未确认，请刷新后核对；不会自动重试'))
      }
      await onSaved(saved.account)
    } catch (error) {
      if (current()) setNotice(error instanceof Error ? error.message : t('凭证更新失败，请刷新后核对账号状态。'))
    } finally {
      credentialJson = ''
      pending.current = false
      if (alive.current) setBusy(false)
    }
  }

  const locked = disabled || busy
  return <div className='flex flex-col gap-3'>
    <Alert>{t('在官方重新登录后，用新凭证更新这个账号；需要自动续期时，请同时勾选授权。')}</Alert>
    <Field>
      <FieldLabel htmlFor={`${prefix}-credential`}>{t('完整凭证 JSON')}</FieldLabel>
      <Textarea id={`${prefix}-credential`} ref={input} rows={5} maxLength={CLINE_FREE_CREDENTIAL_LIMIT}
        autoComplete='off' spellCheck={false} disabled={locked} aria-describedby={`${prefix}-privacy`} onChange={event => {
          setFilled(Boolean(event.currentTarget.value.trim())); setConfirmed(false); setNotice('')
        }} />
      <p id={`${prefix}-privacy`} className='m-0 text-xs text-muted-foreground'>{t('凭证仅发送到你当前连接的网关，保存后不会回显。')}</p>
    </Field>
    <Label htmlFor={`${prefix}-confirm`} className='flex items-start gap-2 text-xs leading-relaxed'>
      <Checkbox id={`${prefix}-confirm`} checked={confirmed} disabled={locked || !filled}
        onCheckedChange={value => setConfirmed(value === true)} />
      <span>{t('我确认这份凭证来自当前账号的最新登录，并更新原账号')}</span>
    </Label>
    <Label htmlFor={`${prefix}-refresh`} className='flex items-start gap-2 text-xs leading-relaxed'>
      <Checkbox id={`${prefix}-refresh`} checked={authorizeRefresh} disabled={locked || !filled} onCheckedChange={value => setAuthorizeRefresh(value === true)} />
      <span>{t('同时授权当前账号自动续期；上次结果未确认时必须使用官方重新登录后的新凭据')}</span>
    </Label>
    <div><Button type='button' size='sm' disabled={locked || !filled || !confirmed} onClick={() => void save()}>
      {busy ? <Spinner className='size-3' /> : null}{t('更新当前账号凭证')}
    </Button></div>
    {notice ? <Alert variant='destructive'>{t(notice)}</Alert> : null}
  </div>
}
