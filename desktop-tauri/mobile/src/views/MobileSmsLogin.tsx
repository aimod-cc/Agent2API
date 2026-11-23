import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { formatSmsNotice, safeHttpsUrl } from '../../../ui-shared/plugin-captcha-frames'
import {
  fetchLoginInteraction,
  startDynamicLogin,
  submitLoginInteraction,
  waitDynamicLogin,
  type SmsConfig,
  type SmsInteractionStep,
} from '../core/api'
import { t } from '../core/i18n'
import { openExternalUrl } from '../core/servers'
import { PickerSheet, PickerTrigger } from '../ui/pickers'
import { MobileAlicom4Captcha, MobileGeetestCaptcha } from './MobileCaptcha'

type Props = {
  provider: string
  methodIndex: number
  config: SmsConfig
  remark: string
  disabled?: boolean
  onFlowChange: (stateToken: string) => void
  onSuccess: () => void
}

function errorText(cause: unknown): string {
  if (cause instanceof Error && cause.message.trim()) return t(cause.message.trim())
  if (typeof cause === 'string' && cause.trim()) return t(cause.trim())
  return t('短信登录失败，请重试')
}

export function MobileSmsLogin({
  provider,
  methodIndex,
  config,
  remark,
  disabled = false,
  onFlowChange,
  onSuccess,
}: Props) {
  const prefix = useId()
  const [flow, setFlow] = useState('')
  const [starting, setStarting] = useState(false)
  const [serverStep, setServerStep] = useState<SmsInteractionStep | null>(null)
  const [readError, setReadError] = useState('')
  const [flowDone, setFlowDone] = useState(false)
  const [phone, setPhone] = useState('')
  const [country, setCountry] = useState(config.countryCodes[0]?.value || '')
  const [countryPickerOpen, setCountryPickerOpen] = useState(false)
  const [code, setCode] = useState('')
  const [captcha, setCaptcha] = useState<unknown>(null)
  const [captchaGeneration, setCaptchaGeneration] = useState(0)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const [countdown, setCountdown] = useState(0)

  const alive = useRef(true)
  const submitting = useRef(false)
  const inFlight = useRef(false)
  const pendingSend = useRef(false)
  const pendingCaptcha = useRef<unknown>(null)
  const hadServerPhone = useRef(false)
  const readController = useRef<AbortController | null>(null)
  const signature = useRef('')
  const flowDoneRef = useRef(false)
  const generation = useRef(0)
  const completed = useRef(false)
  const callbacks = useRef({ onFlowChange, onSuccess })
  callbacks.current = { onFlowChange, onSuccess }

  const active: SmsInteractionStep = serverStep ?? {
    id: '',
    type: 'sms',
    title: t('手机验证码登录'),
    sms: {
      config,
      phone: '',
      countryCode: config.countryCodes[0]?.value || '',
      sent: false,
      expired: false,
      retryAfterSeconds: 0,
      expiresAfterSeconds: 0,
      message: '',
      challenge: config.challenge ?? null,
    },
  }
  const { sms } = active

  const finishSuccess = useCallback(() => {
    if (completed.current) return
    completed.current = true
    flowDoneRef.current = true
    setFlowDone(true)
    callbacks.current.onFlowChange('')
    callbacks.current.onSuccess()
  }, [])

  const refresh = useCallback(async () => {
    if (!flow || flowDoneRef.current || submitting.current || readController.current) return
    const token = generation.current
    const controller = new AbortController()
    readController.current = controller
    try {
      const result = await fetchLoginInteraction(flow, controller.signal)
      if (!alive.current || generation.current !== token || submitting.current || controller.signal.aborted) return
      if (result.interaction === null) {
        flowDoneRef.current = true
        setFlowDone(true)
        const wait = await waitDynamicLogin(flow, controller.signal)
        if (!alive.current || generation.current !== token) return
        if (wait.done && wait.saved) {
          finishSuccess()
          return
        }
        if (wait.error) {
          setReadError(t(wait.error))
          callbacks.current.onFlowChange('')
        }
        return
      }
      if (result.interaction.type === 'sms') {
        const next = JSON.stringify(result.interaction)
        if (signature.current !== next) {
          signature.current = next
          setServerStep(result.interaction)
        }
        setReadError('')
      }
    } catch {
      if (alive.current && generation.current === token && !controller.signal.aborted) {
        setReadError(t('登录步骤读取失败，请刷新当前步骤或重新发起登录。'))
      }
    } finally {
      if (readController.current === controller) readController.current = null
    }
  }, [flow, finishSuccess])

  useEffect(() => {
    alive.current = true
    generation.current += 1
    signature.current = ''
    flowDoneRef.current = false
    setServerStep(null)
    setFlowDone(false)
    setReadError('')
    if (!flow) {
      hadServerPhone.current = false
      return () => { alive.current = false }
    }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 1500)
    return () => {
      alive.current = false
      generation.current += 1
      readController.current?.abort()
      readController.current = null
      window.clearInterval(timer)
    }
  }, [flow, refresh])

  useEffect(() => {
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
    if (!pendingSend.current) setCaptcha(null)
  }, [active.id, sms.phone, sms.countryCode, country])

  useEffect(() => {
    const deadline = Date.now() + sms.retryAfterSeconds * 1000
    const update = () => setCountdown(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)))
    update()
    const timer = window.setInterval(update, 250)
    return () => window.clearInterval(timer)
  }, [sms.retryAfterSeconds, active.id])

  const startFlow = useCallback(async (): Promise<boolean> => {
    if (starting || flow || disabled) return false
    setStarting(true)
    setError('')
    try {
      const started = await startDynamicLogin({ provider, methodIndex, inputs: {} })
      const state = started.state || started.flow || ''
      if (!state) throw new Error(t('启动登录失败'))
      callbacks.current.onFlowChange(state)
      setFlow(state)
      return true
    } catch (cause) {
      setError(errorText(cause))
      return false
    } finally {
      setStarting(false)
    }
  }, [starting, flow, disabled, provider, methodIndex])

  const submit = useCallback(async (action: 'send' | 'verify' | 'change_phone') => {
    if (inFlight.current || disabled) return
    if (action === 'send' && (!/^\d+$/.test(phone) || phone.length < config.phoneMinLength || phone.length > config.phoneMaxLength)) {
      setError(t('请填写正确的手机号'))
      return
    }
    if (action === 'verify' && (!/^\d+$/.test(code) || code.length < config.codeMinLength || code.length > config.codeMaxLength)) {
      setError(t('请填写正确的验证码'))
      return
    }
    if (action === 'send' && (!flow || !active.id)) {
      if (pendingSend.current) return
      pendingSend.current = true
      pendingCaptcha.current = captcha
      setBusy('send')
      setError('')
      const ok = await startFlow()
      if (!ok) {
        pendingSend.current = false
        pendingCaptcha.current = null
        setBusy('')
      }
      return
    }
    const token = generation.current
    const live = () => alive.current && generation.current === token
    const submittedCaptcha = action === 'send' ? (pendingCaptcha.current ?? captcha) : null
    pendingCaptcha.current = null
    inFlight.current = true
    submitting.current = true
    generation.current += 1
    const submitGen = generation.current
    readController.current?.abort()
    readController.current = null
    setBusy(action)
    setError('')
    try {
      await submitLoginInteraction({
        state: flow,
        interaction: active.id,
        action,
        inputs: action === 'send' ? { phone, countryCode: country } : action === 'verify' ? { code, name: remark } : {},
        captcha: submittedCaptcha,
      })
      if (alive.current && generation.current === submitGen) {
        setCode('')
        setCaptcha(null)
        setCaptchaGeneration(value => value + 1)
      }
    } catch (cause) {
      if (alive.current && generation.current === submitGen) {
        setError(errorText(cause))
        if (action === 'send') {
          setCaptcha(null)
          setCaptchaGeneration(value => value + 1)
        }
      }
    } finally {
      inFlight.current = false
      submitting.current = false
      if (alive.current && generation.current === submitGen) {
        setBusy('')
        await refresh()
      } else if (live()) {
        setBusy('')
      }
    }
  }, [disabled, phone, code, config, flow, active.id, captcha, startFlow, country, remark, refresh])

  useEffect(() => {
    if (!pendingSend.current) return
    if (flow && serverStep?.id && !inFlight.current) {
      pendingSend.current = false
      void submit('send')
      return
    }
    if (!flow && !starting) {
      pendingSend.current = false
      pendingCaptcha.current = null
      setBusy('')
    }
  }, [flow, serverStep?.id, starting, submit])

  if (flow && readError) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{readError}</div>
        <button type='button' className='fc-btn' onClick={() => void refresh()}>
          {t('刷新当前步骤')}
        </button>
      </div>
    )
  }

  if (flow && flowDone && !serverStep) {
    return (
      <div style={{ fontSize: 13, color: 'var(--fc-text-secondary)' }}>
        {t('等待账号登录完成…')}
      </div>
    )
  }

  const phoneLocked = Boolean(sms.phone)
  const external = sms.challenge?.type === 'external' ? safeHttpsUrl(sms.challenge.url) : ''
  const requiresSolvedCaptcha = sms.challenge?.type === 'geetest' || sms.challenge?.type === 'alicom4'
  const showChallenge = Boolean(sms.challenge && (!sms.sent || countdown <= 0))
  const sendDisabled = Boolean(busy) || Boolean(disabled) || (!flow && starting) || countdown > 0 || !phone || (requiresSolvedCaptcha && !captcha)
  const countryLabel = country
    ? (config.countryCodes.find(item => item.value === country)?.label || `+${country}`)
    : t('手机区号')

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <label htmlFor={`${prefix}-phone`} style={{ fontSize: 12, fontWeight: 600, color: 'var(--fc-text-secondary)' }}>
          {t(config.phoneLabel)}
        </label>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          {config.countryCodes.length > 1 ? (
            <div style={{ flex: '0 0 auto', minWidth: 104 }}>
              <PickerTrigger
                valueLabel={countryLabel}
                disabled={Boolean(busy) || phoneLocked || disabled}
                onClick={() => setCountryPickerOpen(true)}
                ariaLabel={t('手机区号')}
              />
              <PickerSheet
                open={countryPickerOpen}
                title={t('手机区号')}
                searchable={config.countryCodes.length > 8}
                options={config.countryCodes.map(item => ({ value: item.value, label: t(item.label) }))}
                value={country}
                onSelect={setCountry}
                onClose={() => setCountryPickerOpen(false)}
              />
            </div>
          ) : config.countryCodes.length === 1 ? (
            <span
              className='fc-chip tone-primary'
              style={{ flex: '0 0 auto', height: 42, padding: '0 12px', fontSize: 13 }}
            >
              +{config.countryCodes[0].value}
            </span>
          ) : null}
          <input
            id={`${prefix}-phone`}
            type='tel'
            inputMode='tel'
            autoComplete='tel-national'
            maxLength={config.phoneMaxLength}
            className='fc-input'
            style={{ flex: 1, minWidth: 0 }}
            value={phone}
            disabled={Boolean(busy) || phoneLocked || disabled}
            placeholder={t(config.phonePlaceholder)}
            onChange={e => {
              const value = e.currentTarget.value.replace(/\D/g, '')
              setPhone(value)
              setError('')
            }}
          />
          {phoneLocked ? (
            <button
              type='button'
              className='fc-btn is-tonal'
              style={{ flex: '0 0 auto' }}
              disabled={Boolean(busy) || disabled}
              onClick={() => void submit('change_phone')}
            >
              {t('更换')}
            </button>
          ) : null}
        </div>
      </div>

      {sms.message ? (
        <div className='fc-inner-block' style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--fc-text-secondary)' }}>
          {formatSmsNotice(sms.message, t)}
        </div>
      ) : null}

      {showChallenge && sms.challenge ? (
        sms.challenge.type === 'alicom4' ? (
          <MobileAlicom4Captcha
            key={`${active.id}:${captchaGeneration}`}
            config={sms.challenge}
            disabled={Boolean(busy) || disabled}
            onChange={setCaptcha}
          />
        ) : (
          <div className='fc-inner-block' style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <div style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--fc-text-secondary)' }}>
              {t('需要先完成人机验证，短信尚未发送。完成后请再次获取验证码。')}
            </div>
            {sms.challenge.type === 'geetest' ? (
              <MobileGeetestCaptcha
                key={`${active.id}:${captchaGeneration}`}
                config={sms.challenge}
                disabled={Boolean(busy) || disabled}
                onChange={setCaptcha}
              />
            ) : external ? (
              <button
                type='button'
                className='fc-btn'
                disabled={Boolean(busy) || disabled}
                onClick={() => void openExternalUrl(external)}
              >
                {t('打开人机验证')}
              </button>
            ) : (
              <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>
                {t('人机验证地址无效，请重新发起登录')}
              </div>
            )}
          </div>
        )
      ) : null}

      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <label htmlFor={`${prefix}-code`} style={{ fontSize: 12, fontWeight: 600, color: 'var(--fc-text-secondary)' }}>
          {t('验证码')}
        </label>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            id={`${prefix}-code`}
            type='tel'
            inputMode='numeric'
            autoComplete='one-time-code'
            maxLength={config.codeMaxLength}
            className='fc-input'
            style={{ flex: 1, minWidth: 0 }}
            value={code}
            disabled={Boolean(busy) || !sms.sent || sms.expired || disabled}
            placeholder={t('请输入 {n} 位验证码', { n: config.codeMaxLength })}
            onChange={e => {
              const value = e.currentTarget.value.replace(/\D/g, '')
              setCode(value)
              setError('')
            }}
          />
          <button
            type='button'
            className='fc-btn is-tonal'
            style={{ flex: '0 0 auto' }}
            disabled={sendDisabled}
            onClick={() => void submit('send')}
          >
            {busy === 'send' ? t('发送中…') : countdown > 0 ? t('{n} 秒后重发', { n: countdown }) : t('获取验证码')}
          </button>
        </div>
        {sms.sent ? (
          <div style={{ fontSize: 12, color: sms.expired ? 'var(--fc-danger)' : 'var(--fc-success)' }}>
            {sms.expired ? t('验证码已过期，请重新获取') : t('验证码已发送，请查收短信')}
          </div>
        ) : null}
      </div>

      {error ? (
        <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{error}</div>
      ) : null}

      <button
        type='button'
        className='fc-btn is-primary'
        style={{ width: '100%' }}
        disabled={Boolean(busy) || !sms.sent || sms.expired || !code || disabled}
        onClick={() => void submit('verify')}
      >
        {busy === 'verify' ? t('登录中…') : t('登录并添加')}
      </button>
    </div>
  )
}
