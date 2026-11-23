import * as React from 'react'
import { Alert, Button, Input, Label, Spinner } from '@ui'
import { SearchableSelect } from './searchable-select'
import { Captcha } from './plugin-login-captcha'
import { Alicom4Captcha, type Alicom4Config } from './plugin-login-alicom4'
import { isAlicom4Config, safeLoginQrUrl } from './plugin-captcha-frames'
import { SmsLoginInteraction, type SmsInteraction } from './plugin-login-sms'
import { ManagedLoginInteraction, managedStepKey, readManagedInteraction, type ManagedInteraction, type ManagedLoginScope } from './plugin-login-managed'
import type { SmsConfig } from '../ui-islands/src/islands/add-account-dynamic-fields'

export type LoginRequest = <T>(method: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown,
  options?: { signal?: AbortSignal }) => Promise<T>

type Interaction = {
  type?: 'form'
  id: string; title: string; message: string
  fields: Array<{ key: string; label: string; type: 'text' | 'select'; secret: boolean; optional: boolean; options: Array<{ value: string; label: string }> }>
  captcha?: { gt: string; challenge: string; success: boolean; new_captcha: boolean } | Alicom4Config | null
  qr?: { url: string; expiresAt: number } | null
}

export function PluginLoginInteraction({ flow, request, t, openExternal, openEmbedded, managed, smsConfig, onStartSms, startingSms, smsStartDisabled, remark, onRemarkChange }: {
  flow: string
  request: LoginRequest
  t: (key: string, vars?: Record<string, string | number>) => string
  openExternal: (url: string) => void | Promise<void>
  openEmbedded?: (flow: string, interaction: string) => void | Promise<void>
  managed?: ManagedLoginScope
  smsConfig?: SmsConfig
  onStartSms?: () => unknown | Promise<unknown>
  startingSms?: boolean
  smsStartDisabled?: boolean
  remark?: string
  onRemarkChange?: (value: string) => void
}) {
  const [interaction, setInteraction] = React.useState<Interaction | SmsInteraction | ManagedInteraction | null>(null)
  const [flowDone, setFlowDone] = React.useState(false)
  const flowDoneRef = React.useRef(false)
  const [values, setValues] = React.useState<Record<string, string>>({})
  const [captcha, setCaptcha] = React.useState<unknown>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [readError, setReadError] = React.useState('')
  const [captchaGeneration, setCaptchaGeneration] = React.useState(0)
  const latest = React.useRef({ request, managed, smsConfig, t })
  const alive = React.useRef(true)
  const submitting = React.useRef(false)
  const signature = React.useRef('')
  const generation = React.useRef(0)
  const read = React.useRef<AbortController | null>(null)
  const deadlines = React.useRef(new Map<string, number>())
  const attempted = React.useRef(new Set<string>())
  const captchaAutoStarted = React.useRef(new Set<string>())
  const attemptKey = (step: ManagedInteraction) => JSON.stringify([step.id, step.captcha?.configId])
  const callbackSubmitted = React.useRef(new Set<string>())
  const browserOpened = React.useRef(new Set<string>())
  const managedKey = React.useRef('')
  const managedInstance = React.useRef(0)
  const managedStatus = React.useRef('')
  const submitOwner = React.useRef<symbol | null>(null)
  latest.current = { request, managed, smsConfig, t }

  const accept = React.useCallback((next: ManagedInteraction, startedAt: number) => {
    const scope = latest.current.managed
    if (!alive.current || !scope?.current()) return
    const key = managedStepKey(next)
    const deadline = Math.min(scope.deadline, startedAt + next.remainingMs, deadlines.current.get(key) ?? Infinity)
    deadlines.current.set(key, deadline)
    if (managedKey.current !== key) {
      generation.current += 1
      submitting.current = false
      submitOwner.current = null
      managedInstance.current += 1
      managedKey.current = key
    }
    managedStatus.current = next.status
    setReadError('')
    setInteraction(next)
  }, [])

  const refresh = React.useCallback(async () => {
    if (!flow || flowDoneRef.current || submitting.current || read.current || latest.current.managed && !latest.current.managed.current()) return
    const token = generation.current
    const controller = new AbortController()
    read.current = controller
    const startedAt = performance.now()
    try {
      const result = await latest.current.request<{ interaction: Interaction | SmsInteraction | ManagedInteraction | null }>(
        'GET', `/api/session/login/interaction?state=${encodeURIComponent(flow)}`, undefined, { signal: controller.signal })
      if (!alive.current || generation.current !== token || submitting.current || controller.signal.aborted) return
      if (latest.current.managed && !latest.current.managed.current()) return
      if (result.interaction?.type === 'managed') {
        const scope = latest.current.managed
        if (!scope) throw new Error('managed_scope_missing')
        accept(readManagedInteraction(result.interaction, flow, scope), startedAt)
        return
      }
      if (latest.current.managed) {
        if (result.interaction !== null) throw new Error('managed_step_expected')
        managedKey.current = ''
        setInteraction(null); setReadError('')
        return
      }
      const next = JSON.stringify(result.interaction)
      if (signature.current !== next) {
        signature.current = next
        setInteraction(result.interaction)
        setValues({}); setCaptcha(null); setError('')
      }
      if (latest.current.smsConfig && result.interaction === null) {
        flowDoneRef.current = true
        setFlowDone(true)
      }
      setReadError('')
    } catch {
      // 终态仍由父级 /wait 处理；托管步骤或短信步骤读取失败时显示可读错误，不静默吞错。
      if (alive.current && generation.current === token && (latest.current.managed || latest.current.smsConfig) && !controller.signal.aborted) {
        managedKey.current = ''
        setInteraction(null)
        const { t } = latest.current
        setReadError(t('登录步骤读取失败，请刷新当前步骤或重新发起登录。'))
      }
    } finally { if (read.current === controller) read.current = null }
  }, [flow, managed?.key, accept])

  React.useEffect(() => {
    alive.current = true
    generation.current += 1
    signature.current = ''; managedKey.current = ''; managedInstance.current += 1
    deadlines.current.clear(); attempted.current.clear(); captchaAutoStarted.current.clear(); callbackSubmitted.current.clear(); browserOpened.current.clear()
    submitting.current = false; submitOwner.current = null
    flowDoneRef.current = false
    setInteraction(null); setFlowDone(false); setValues({}); setCaptcha(null); setReadError('')
    if (!flow) return () => { alive.current = false; generation.current += 1 }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 1500)
    const stop = () => {
      alive.current = false; generation.current += 1
      read.current?.abort(); read.current = null; window.clearInterval(timer); setInteraction(null)
    }
    window.addEventListener('pagehide', stop)
    return () => {
      alive.current = false; generation.current += 1
      read.current?.abort(); read.current = null
      window.clearInterval(timer); window.removeEventListener('pagehide', stop)
    }
  }, [flow, refresh])

  const preparing = interaction?.type === 'managed' && interaction.status === 'preparing'
  React.useEffect(() => {
    if (!preparing) return
    const timer = window.setInterval(() => void refresh(), 250)
    return () => window.clearInterval(timer)
  }, [preparing, refresh])

  async function submit() {
    if (!interaction || interaction.type === 'sms' || interaction.type === 'managed' || submitting.current) return
    const token = ++generation.current
    submitting.current = true; setBusy(true); setError('')
    try {
      await latest.current.request('POST', '/api/session/login/interaction', {
        state: flow, interaction: interaction.id, inputs: values, captcha,
      })
      if (alive.current && generation.current === token) { setValues({}); setCaptcha(null); setCaptchaGeneration(value => value + 1) }
    } catch (error) {
      if (alive.current && generation.current === token) { setError(error instanceof Error ? error.message : String(error)); setCaptcha(null); setCaptchaGeneration(value => value + 1) }
    } finally {
      if (generation.current === token) {
        submitting.current = false
        if (alive.current) { setBusy(false); await refresh() }
      }
    }
  }
  const smsFallback: SmsInteraction | null = interaction?.type === 'sms'
    ? interaction
    : smsConfig && !managed && !interaction && !(flow && flowDone)
      ? {
          id: '',
          type: 'sms',
          title: t('手机验证码登录'),
          sms: {
            config: smsConfig,
            phone: '',
            countryCode: smsConfig.countryCodes[0]?.value || '',
            sent: false,
            expired: false,
            retryAfterSeconds: 0,
            expiresAfterSeconds: 0,
            message: '',
            challenge: smsConfig.challenge ?? null,
          },
        }
      : null
  if (flow && readError) return <Alert variant='destructive' className='flex flex-col gap-2'>
    {readError}<Button variant='outline' onClick={() => void refresh()}>{t('刷新当前步骤')}</Button>
  </Alert>
  if (flow && flowDone && smsConfig && !interaction) return <p role='status' className='flex items-center gap-2 text-xs text-muted-foreground'>
    <Spinner /> {t('等待账号登录完成…')}
  </p>
  if (smsFallback) return <SmsLoginInteraction flow={flow} interaction={smsFallback}
    request={request} refresh={refresh} openExternal={openExternal} onStart={onStartSms} starting={startingSms}
    startDisabled={smsStartDisabled} remark={remark} onRemarkChange={onRemarkChange}
    onBusy={value => {
      generation.current += 1; submitting.current = value
      if (value) { read.current?.abort(); read.current = null }
    }} t={t} />
  if (!flow || !interaction || interaction.type === 'sms') return null
  if (interaction.type === 'managed') {
    if (!managed?.current()) return null
    const key = managedStepKey(interaction)
    const instance = managedInstance.current
    const stepScope: ManagedLoginScope = { ...managed,
      current: () => managed.current() && managedKey.current === key && managedInstance.current === instance
        && (interaction.status !== 'ready' || managedStatus.current === 'ready') }
    return <ManagedLoginInteraction key={`${managed.key}:${instance}:${key}`} step={interaction} scope={stepScope}
      deadline={deadlines.current.get(key) ?? 0} request={request} refresh={refresh}
      accept={(next, startedAt) => { if (stepScope.current()) accept(next, startedAt) }}
      attemptedBefore={attempted.current.has(attemptKey(interaction))} markAttempted={() => { attempted.current.add(attemptKey(interaction)) }}
      captchaAutoStartedBefore={captchaAutoStarted.current.has(attemptKey(interaction))} markCaptchaAutoStarted={() => { captchaAutoStarted.current.add(attemptKey(interaction)) }}
      browserOpenedBefore={browserOpened.current.has(key)} markBrowserOpened={() => { browserOpened.current.add(key) }}
      callbackSubmitted={callbackSubmitted.current.has(interaction.id)} markCallbackSubmitted={value => {
        if (!stepScope.current()) return
        if (value) callbackSubmitted.current.add(interaction.id)
        else callbackSubmitted.current.delete(interaction.id)
      }}
      onSubmitting={(value, owner) => {
        if (managedKey.current !== key || managedInstance.current !== instance || !alive.current || !value && submitOwner.current !== owner) return
        submitOwner.current = value ? owner : null
        generation.current += 1; submitting.current = value
        if (value) { read.current?.abort(); read.current = null }
      }} openExternal={openExternal} openEmbedded={openEmbedded} t={t} />
  }
  return <section className='flex flex-col gap-3 rounded-lg border border-hairline p-3' aria-label={interaction.title}>
    <h3 className='text-sm font-semibold'>{interaction.title}</h3>
    {interaction.message ? <p className='whitespace-pre-wrap text-xs text-muted-foreground'>{interaction.message}</p> : null}
    {interaction.qr ? <img className='mx-auto h-56 w-56 rounded-lg bg-white object-contain' src={safeLoginQrUrl(interaction.qr.url)}
      alt={interaction.title} referrerPolicy='no-referrer' /> : null}
    {interaction.fields.map(field => <Label key={field.key} className='flex flex-col items-start gap-2'>
      {field.label}
      {field.type === 'select' ? <SearchableSelect disabled={busy} value={values[field.key] || ''}
        onValueChange={value => setValues(previous => ({ ...previous, [field.key]: value }))}
        className='w-full' aria-label={field.label} options={field.options} searchable={field.options.length > 8}
        displayValue={field.options.find(item => item.value === values[field.key])?.label || t('请选择')} />
        : <Input disabled={busy} type={field.secret ? 'password' : 'text'} autoComplete='off' spellCheck={false}
        value={values[field.key] || ''} onChange={event => {
          const value = event.currentTarget.value
          setValues(previous => ({ ...previous, [field.key]: value }))
        }} />}
    </Label>)}
    {interaction.captcha ? isAlicom4Config(interaction.captcha)
      ? <Alicom4Captcha key={`${interaction.id}:${captchaGeneration}`} config={interaction.captcha} disabled={busy} onChange={setCaptcha} t={t} />
      : <Captcha key={`${interaction.id}:${interaction.captcha.challenge}:${captchaGeneration}`} config={interaction.captcha} disabled={busy} onChange={setCaptcha} t={t} /> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {!interaction.qr ? <Button type='button' disabled={busy || Boolean(interaction.captcha && !captcha)} onClick={() => void submit()}>{t('继续')}</Button> : null}
  </section>
}
