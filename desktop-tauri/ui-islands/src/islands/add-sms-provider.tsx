/**
 * 手机号验证码登录（插件申报的 `auth.methods[].type === 'sms'`）。
 *
 * ── 为什么不复用上面那套登录状态机 ──────────────────────────────
 * `oauth` 是「开窗口 → 轮询/回调」，`api` 是「贴凭证 → account.normalize」，
 * 两者都经过 `login.*` 那条异步链路。短信是**两次同步请求**：发码（拿到
 * `deviceIdentifier` 这类中间态）→ 用码换账号，后者直接落库并回 `{account, list}`。
 * 硬套异步框架会让网关为一次短信建后台任务，还要把中间态编进 flow 再拆出来。
 *
 * ── 中间态为什么要回传 ──────────────────────────────────────────
 * 上游把「这次发的码」绑在发码时的那份标识上（QuWork 的 deviceIdentifier、
 * Tabbit 的 uuid、Loomy 的 msgid），登录必须带同一个。网关不替用户保存它
 * （一次登录可以跨多次 HTTP 请求，也可能被放弃），所以界面拿着它随验证请求带回。
 * 插件发码返回的键名**原样透传**：这里只把整个响应对象存下来，不猜字段名。
 */
import * as React from 'react'
import { Alert, AlertDescription, AlertTitle, Button, Input, Label } from '@ui'
import { t } from '../i18n'

export type SmsLoginState = {
  /** 发码成功后插件返回的对象（`{deviceIdentifier}` / `{uuid}` / `{msgid}` …）。 */
  ticket: Record<string, unknown>
  /** 发码成功后展示给用户的提示（有的家会在响应里带文案）。 */
  notice: string
}

type Props = {
  provider: string
  methodLabel: string
  instructions: string
  disabled: boolean
  prefix: string
  /** 手机号与备注名沿用外层统一状态，避免两处各存一份。 */
  phone: string
  name: string
  onPhone: (value: string) => void
  onName: (value: string) => void
  /** 拿到新账号后由外层刷新列表并关闭弹窗。 */
  onAdded: (account: unknown) => Promise<void>
}

/** 常见上游的中间态键：发码响应里出现哪个就原样带回去。 */
function ticketOf(payload: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const key of ['deviceIdentifier', 'deviceId', 'uuid', 'msgid', 'msgId', 'sessionId', 'token']) {
    const value = payload[key]
    if (typeof value === 'string' && value.trim()) out[key] = value
  }
  return out
}

function noticeOf(payload: Record<string, unknown>): string {
  for (const key of ['message', 'notice', 'hint']) {
    const value = payload[key]
    if (typeof value === 'string' && value.trim()) return value
  }
  return ''
}

async function post(path: string, body: Record<string, unknown>): Promise<Record<string, unknown>> {
  const response = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const text = await response.text()
  let payload: unknown = null
  try { payload = text ? JSON.parse(text) : null } catch { payload = null }
  const data = (payload && typeof payload === 'object' && !Array.isArray(payload)
    ? payload as Record<string, unknown> : {})
  if (!response.ok || data.success === false) {
    const message = typeof data.error === 'string' ? data.error
      : typeof (data.error as Record<string, unknown> | undefined)?.message === 'string'
        ? String((data.error as Record<string, unknown>).message)
        : typeof data.message === 'string' ? data.message
          : t('请求失败（HTTP {status}）', { status: String(response.status) })
    throw new Error(message)
  }
  // 管理面把结果包在 {data, success} 里，兼容未包装的形态。
  const inner = data.data
  return (inner && typeof inner === 'object' && !Array.isArray(inner)
    ? inner as Record<string, unknown> : data)
}

export function SmsLoginBlock({
  provider, methodLabel, instructions, disabled, prefix,
  phone, name, onPhone, onName, onAdded,
}: Props) {
  const [state, setState] = React.useState<SmsLoginState | null>(null)
  const [code, setCode] = React.useState('')
  const [sending, setSending] = React.useState(false)
  const [verifying, setVerifying] = React.useState(false)
  const [notice, setNotice] = React.useState('')
  const [error, setError] = React.useState('')
  const [countdown, setCountdown] = React.useState(0)
  const alive = React.useRef(true)

  React.useEffect(() => () => { alive.current = false }, [])
  React.useEffect(() => {
    if (countdown <= 0) return
    const timer = window.setTimeout(() => setCountdown(value => value - 1), 1000)
    return () => window.clearTimeout(timer)
  }, [countdown])

  React.useEffect(() => {
    // 换手机号即作废上一轮发出去的码：上游把码绑在发码时的标识上，
    // 继续用旧 ticket 只会换来一句「验证码错误」。
    setState(null)
    setCode('')
    setNotice('')
    setError('')
  }, [phone])

  const busy = disabled || sending || verifying

  async function send(): Promise<void> {
    if (busy) return
    const value = phone.trim()
    if (!/^\d{6,15}$/.test(value)) {
      setError(t('请填写正确的手机号'))
      return
    }
    setSending(true)
    setError('')
    setNotice('')
    try {
      const payload = await post('/api/session/login/phone/send', { provider, phone: value })
      if (!alive.current) return
      setState({ ticket: ticketOf(payload), notice: noticeOf(payload) })
      setCountdown(60)
      setNotice(noticeOf(payload) || t('验证码已发送，请查收短信'))
    } catch (cause) {
      if (alive.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (alive.current) setSending(false)
    }
  }

  async function verify(): Promise<void> {
    if (busy || !state) return
    const value = code.trim()
    if (!/^\d{4,8}$/.test(value)) {
      setError(t('请填写正确的验证码'))
      return
    }
    setVerifying(true)
    setError('')
    try {
      const payload = await post('/api/session/login/phone/verify', {
        provider,
        phone: phone.trim(),
        code: value,
        ...(name.trim() ? { name: name.trim() } : {}),
        ...state.ticket,
      })
      if (!alive.current) return
      await onAdded(payload.account)
      if (!alive.current) return
      setState(null)
      setCode('')
      setNotice('')
    } catch (cause) {
      if (alive.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (alive.current) setVerifying(false)
    }
  }

  return (
    <div className='flex flex-col gap-3'>
      {instructions ? <p className='whitespace-pre-wrap break-words'>{instructions}</p> : null}
      <Alert>
        <AlertTitle>{methodLabel}</AlertTitle>
        <AlertDescription>{t('用绑定的手机号登录：点「获取验证码」后填入即可。这是该客户端唯一的登录方式。')}</AlertDescription>
      </Alert>
      <div className='flex flex-col gap-1.5'>
        <Label htmlFor={`${prefix}-sms-phone`}>{t('手机号')}<span aria-hidden='true'> *</span></Label>
        <div className='flex gap-2'>
          <Input id={`${prefix}-sms-phone`} inputMode='numeric' autoComplete='tel' maxLength={15}
            value={phone} disabled={busy} placeholder={t('11 位大陆手机号')}
            onChange={event => onPhone(event.currentTarget.value.replace(/[^\d]/g, ''))} />
          <Button type='button' variant='outline' disabled={busy || countdown > 0} onClick={() => void send()}>
            {sending ? t('发送中…') : countdown > 0 ? t('{n} 秒后重发', { n: String(countdown) }) : t('获取验证码')}
          </Button>
        </div>
      </div>
      <div className='flex flex-col gap-1.5'>
        <Label htmlFor={`${prefix}-sms-code`}>{t('验证码')}<span aria-hidden='true'> *</span></Label>
        <Input id={`${prefix}-sms-code`} inputMode='numeric' autoComplete='one-time-code' maxLength={8}
          value={code} disabled={busy || !state} placeholder={t('6 位数字')}
          onChange={event => setCode(event.currentTarget.value.replace(/[^\d]/g, ''))} />
      </div>
      <div className='flex flex-col gap-1.5'>
        <Label htmlFor={`${prefix}-sms-name`}>{t('备注名')}</Label>
        <Input id={`${prefix}-sms-name`} value={name} disabled={busy} maxLength={100}
          placeholder={t('可选，留空则用脱敏手机号')}
          onChange={event => onName(event.currentTarget.value)} />
      </div>
      {notice ? <p className='text-muted-foreground'>{notice}</p> : null}
      {error ? <Alert variant='destructive'><AlertDescription>{error}</AlertDescription></Alert> : null}
      <Button type='button' disabled={busy || !state || !code.trim()} onClick={() => void verify()}>
        {verifying ? t('登录中…') : t('登录并添加')}
      </Button>
    </div>
  )
}
