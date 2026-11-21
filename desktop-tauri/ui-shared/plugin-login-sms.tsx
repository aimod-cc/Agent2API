import * as React from 'react'
import { Alert, AlertDescription, Button, Input, Label } from '@ui'
import { SearchableSelect } from './searchable-select'
import { Captcha, type GeetestConfig } from './plugin-login-captcha'
import type { LoginRequest } from './plugin-login-interaction'
import type { SmsConfig } from '../ui-islands/src/islands/add-account-dynamic-fields'

export type SmsInteraction = {
  id: string
  type: 'sms'
  title: string
  sms: {
    config: SmsConfig
    phone: string
    countryCode: string
    sent: boolean
    expired: boolean
    retryAfterSeconds: number
    expiresAfterSeconds: number
    challenge: ({ type: 'geetest' } & GeetestConfig) | { type: 'external'; url: string } | null
  }
}

type Props = {
  flow: string
  interaction: SmsInteraction
  request: LoginRequest
  refresh: () => Promise<void>
  onBusy: (busy: boolean) => void
  openExternal: (url: string) => void | Promise<void>
  onStart?: () => unknown | Promise<unknown>
  starting?: boolean
  startDisabled?: boolean
  t: (key: string, vars?: Record<string, string | number>) => string
}

function describeSmsError(cause: unknown, t: Props['t'], depth = 0): string {
  if (typeof cause === 'string' && cause.trim()) return t(cause.trim())
  if (cause instanceof Error && cause.message.trim()) return t(cause.message.trim())
  if (cause && typeof cause === 'object' && depth < 3) {
    const record = cause as Record<string, unknown>
    for (const key of ['message', 'error', 'msg', 'reason']) {
      const text = describeSmsError(record[key], t, depth + 1)
      if (text) return text
    }
  }
  return depth === 0 ? t('短信登录失败，请重试') : ''
}

export function SmsLoginInteraction({ flow, interaction, request, refresh, onBusy, openExternal, onStart, starting, startDisabled, t }: Props) {
  const { sms } = interaction
  const { config } = sms
  const prefix = React.useId()
  const [phone, setPhone] = React.useState(sms.phone)
  const [country, setCountry] = React.useState(sms.countryCode)
  const [code, setCode] = React.useState('')
  const [name, setName] = React.useState('')
  const [captcha, setCaptcha] = React.useState<unknown>(null)
  const [captchaGeneration, setCaptchaGeneration] = React.useState(0)
  const [busy, setBusy] = React.useState('')
  const [error, setError] = React.useState('')
  const [countdown, setCountdown] = React.useState(sms.retryAfterSeconds)
  const inFlight = React.useRef(false)
  const pendingSend = React.useRef(false)
  const hadServerPhone = React.useRef(Boolean(sms.phone))
  const generation = React.useRef(0)
  const current = React.useRef({ request, refresh, onBusy, onStart })
  current.current = { request, refresh, onBusy, onStart }

  React.useEffect(() => {
    if (!flow && !pendingSend.current) hadServerPhone.current = false
    return () => { generation.current += 1; current.current.onBusy(false) }
  }, [flow])
  React.useEffect(() => {
    if (sms.phone) {
      hadServerPhone.current = true
      setPhone(sms.phone)
      setCountry(sms.countryCode)
    } else if (hadServerPhone.current) {
      hadServerPhone.current = false
      setPhone('')
      setCountry(sms.countryCode)
    } else if (!country && sms.countryCode) {
      setCountry(sms.countryCode)
    }
    setCode('')
    setCaptcha(null)
  }, [interaction.id, sms.phone, sms.countryCode])
  React.useEffect(() => {
    const deadline = Date.now() + sms.retryAfterSeconds * 1000
    const update = () => setCountdown(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)))
    update()
    const timer = window.setInterval(update, 250)
    return () => window.clearInterval(timer)
  }, [sms.retryAfterSeconds, interaction.id])
  React.useEffect(() => {
    if (!pendingSend.current) return
    if (flow && interaction.id && !inFlight.current) {
      pendingSend.current = false
      void submit('send')
      return
    }
    if (!flow && !starting) {
      pendingSend.current = false
      setBusy('')
    }
  }, [flow, interaction.id, starting])

  async function submit(action: 'send' | 'verify' | 'change_phone') {
    if (inFlight.current) return
    if (action === 'send' && (!/^\d+$/.test(phone) || phone.length < config.phoneMinLength || phone.length > config.phoneMaxLength)) {
      setError(t('请填写正确的手机号')); return
    }
    if (action === 'verify' && (!/^\d+$/.test(code) || code.length < config.codeMinLength || code.length > config.codeMaxLength)) {
      setError(t('请填写正确的验证码')); return
    }
    if (action === 'send' && (!flow || !interaction.id)) {
      if (!current.current.onStart || pendingSend.current) return
      pendingSend.current = true
      setBusy('send'); setError('')
      try {
        const started = await current.current.onStart()
        if (started !== true) {
          pendingSend.current = false
          setBusy('')
        }
      } catch (cause) {
        pendingSend.current = false
        setBusy('')
        setError(describeSmsError(cause, t))
      }
      return
    }
    const token = generation.current
    const live = () => generation.current === token
    inFlight.current = true
    setBusy(action); setError(''); current.current.onBusy(true)
    try {
      await current.current.request('POST', '/api/session/login/interaction', {
        state: flow, interaction: interaction.id, action,
        inputs: action === 'send' ? { phone, countryCode: country } : action === 'verify' ? { code, name } : {},
        captcha: action === 'send' ? captcha : null,
      })
      if (live()) { setCode(''); setCaptcha(null); setCaptchaGeneration(value => value + 1) }
    } catch (cause) {
      if (live()) {
        setError(describeSmsError(cause, t))
        if (action === 'send') { setCaptcha(null); setCaptchaGeneration(value => value + 1) }
      }
    } finally {
      inFlight.current = false
      if (live()) { setBusy(''); current.current.onBusy(false); await current.current.refresh() }
    }
  }

  async function openChallenge(url: string) {
    const token = generation.current
    try { await openExternal(url) }
    catch { if (generation.current === token) setError(t('人机验证地址无效，请重新发起登录')) }
  }

  const phoneLocked = Boolean(sms.phone)
  const external = sms.challenge?.type === 'external' ? safeUrl(sms.challenge.url) : ''
  return <section className='flex flex-col gap-3 rounded-lg border border-hairline p-3' aria-label={t('手机验证码登录')}>
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor={`${prefix}-phone`}>{t(config.phoneLabel)}</Label>
      {config.countryCodes.length > 1 ? <SearchableSelect value={country} disabled={Boolean(busy) || phoneLocked}
        onValueChange={setCountry} options={config.countryCodes} className='w-full'
        displayValue={config.countryCodes.find(item => item.value === country)?.label || t('手机区号')}
        aria-label={t('手机区号')} searchable /> : null}
      <div className='flex flex-wrap gap-2'>
        <Input id={`${prefix}-phone`} type='tel' inputMode='tel' autoComplete='tel-national' maxLength={config.phoneMaxLength}
          className='min-w-0 flex-1' value={phone} disabled={Boolean(busy) || phoneLocked}
          placeholder={t(config.phonePlaceholder)} onChange={event => { setPhone(event.currentTarget.value.replace(/\D/g, '')); setError('') }} />
        {phoneLocked ? <Button type='button' variant='outline' disabled={Boolean(busy)} onClick={() => void submit('change_phone')}>
          {t('修改手机号')}</Button> : null}
      </div>
    </div>
    {sms.challenge ? <Alert><AlertDescription className='flex flex-col gap-2'>
      <p>{t('需要先完成人机验证，短信尚未发送。完成后请再次获取验证码。')}</p>
      {sms.challenge.type === 'geetest' ? <Captcha key={`${interaction.id}:${captchaGeneration}`} config={sms.challenge} onChange={setCaptcha} t={t} />
        : external ? <Button type='button' variant='outline' className='self-start' disabled={Boolean(busy)}
          onClick={() => void openChallenge(external)}>
          {t('打开人机验证')}</Button> : <p>{t('人机验证地址无效，请重新发起登录')}</p>}
    </AlertDescription></Alert> : null}
    <Button type='button' variant='outline' className='self-start'
      disabled={Boolean(busy) || (!flow && Boolean(startDisabled)) || countdown > 0 || !phone || (sms.challenge?.type === 'geetest' && !captcha)}
      onClick={() => void submit('send')}>
      {busy === 'send' ? t('发送中…') : countdown > 0 ? t('{n} 秒后重发', { n: countdown }) : t('获取验证码')}
    </Button>
    {sms.sent ? <p className='text-xs text-muted-foreground' role='status'>
      {sms.expired ? t('验证码已过期，请重新获取') : t('验证码已发送，请查收短信')}
    </p> : null}
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor={`${prefix}-code`}>{t('验证码')}</Label>
      <Input id={`${prefix}-code`} inputMode='numeric' autoComplete='one-time-code' maxLength={config.codeMaxLength}
        value={code} disabled={Boolean(busy) || !sms.sent || sms.expired}
        onChange={event => { setCode(event.currentTarget.value.replace(/\D/g, '')); setError('') }} />
    </div>
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor={`${prefix}-name`}>{t('备注名')}</Label>
      <Input id={`${prefix}-name`} value={name} maxLength={100} disabled={Boolean(busy)}
        placeholder={t('可选，留空使用账号昵称')} onChange={event => setName(event.currentTarget.value)} />
    </div>
    {error ? <Alert variant='destructive'><AlertDescription>{error}</AlertDescription></Alert> : null}
    <Button type='button' disabled={Boolean(busy) || !sms.sent || sms.expired || !code} onClick={() => void submit('verify')}>
      {busy === 'verify' ? t('登录中…') : t('登录并添加')}
    </Button>
  </section>
}

function safeUrl(value: string): string {
  try {
    const url = new URL(value)
    return url.protocol === 'https:' && !url.username && !url.password ? url.href : ''
  } catch { return '' }
}
