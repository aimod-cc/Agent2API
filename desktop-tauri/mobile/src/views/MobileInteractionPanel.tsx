import { useCallback, useEffect, useRef, useState } from 'react'
import { isAlicom4Config, safeLoginQrUrl } from '../../../ui-shared/plugin-captcha-frames'
import {
  fetchLoginInteraction,
  submitDynamicCallback,
  submitLoginInteraction,
  type DynamicLoginStart,
  type GenericInteractionStep,
} from '../core/api'
import { copyText } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import { CommonCard } from '../ui/components'
import { PickerSheet, PickerTrigger } from '../ui/pickers'
import { MobileAlicom4Captcha, MobileGeetestCaptcha } from './MobileCaptcha'

type Props = {
  flow: DynamicLoginStart
  statusMsg: string
  onBusyChange: (busy: boolean) => void
  onCancel: () => void
}

export function MobileInteractionPanel({ flow, statusMsg, onBusyChange, onCancel }: Props) {
  const stateToken = flow.state || flow.flow || ''
  const [interaction, setInteraction] = useState<GenericInteractionStep | null>(null)
  const [values, setValues] = useState<Record<string, string>>({})
  const [selectOpen, setSelectOpen] = useState('')
  const [captcha, setCaptcha] = useState<unknown>(null)
  const [captchaGeneration, setCaptchaGeneration] = useState(0)
  const [callbackText, setCallbackText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const alive = useRef(true)
  const submitting = useRef(false)
  const signature = useRef('')
  const generation = useRef(0)
  const readAbort = useRef<AbortController | null>(null)

  const refresh = useCallback(async () => {
    if (!stateToken || submitting.current || readAbort.current) return
    const token = generation.current
    const controller = new AbortController()
    readAbort.current = controller
    try {
      const result = await fetchLoginInteraction(stateToken, controller.signal)
      if (!alive.current || generation.current !== token || submitting.current || controller.signal.aborted) return
      if (!result.interaction || result.interaction.type === 'sms') {
        if (signature.current !== 'null') {
          signature.current = 'null'
          setInteraction(null)
        }
        return
      }
      const next = JSON.stringify(result.interaction)
      if (signature.current !== next) {
        signature.current = next
        setInteraction(result.interaction)
        setValues({})
        setCaptcha(null)
        setError('')
      }
    } catch {
      /* waitDynamicLogin handles terminal status */
    } finally {
      if (readAbort.current === controller) readAbort.current = null
    }
  }, [stateToken])

  useEffect(() => {
    alive.current = true
    generation.current += 1
    signature.current = ''
    setInteraction(null)
    setValues({})
    setCaptcha(null)
    setError('')
    setCallbackText('')
    if (!stateToken) return () => { alive.current = false }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 1500)
    return () => {
      alive.current = false
      generation.current += 1
      readAbort.current?.abort()
      readAbort.current = null
      window.clearInterval(timer)
    }
  }, [stateToken, refresh])

  const handleSubmitStep = async () => {
    if (!interaction || !stateToken || busy) return
    submitting.current = true
    readAbort.current?.abort()
    readAbort.current = null
    setBusy(true)
    onBusyChange(true)
    setError('')
    try {
      const body: Record<string, unknown> = {
        state: stateToken,
        interaction: interaction.id,
        inputs: values,
      }
      if (captcha !== null && captcha !== undefined) body.captcha = captcha
      await submitLoginInteraction(body)
      setCaptcha(null)
      setCaptchaGeneration(v => v + 1)
    } catch (err) {
      setError(err instanceof Error ? t(err.message) : t('提交失败，请重试'))
      setCaptcha(null)
      setCaptchaGeneration(v => v + 1)
    } finally {
      submitting.current = false
      setBusy(false)
      onBusyChange(false)
      void refresh()
    }
  }

  const handleSubmitCallback = async () => {
    if (!stateToken || !callbackText.trim() || busy) return
    setBusy(true)
    onBusyChange(true)
    try {
      const res = await submitDynamicCallback(stateToken, callbackText.trim())
      if (res.accountId || res.accepted) {
        store.toast(t('回调已提交，正在完成验证…'), 'success')
        setCallbackText('')
      }
    } catch (err) {
      store.toast(err instanceof Error ? t(err.message) : t('回调提交失败'), 'danger')
    } finally {
      setBusy(false)
      onBusyChange(false)
    }
  }

  const authLink = flow.authUrl || flow.url || ''
  const showManualCallback = Boolean(authLink || flow.manualCallback)
  const qrSrc = interaction?.qr ? safeLoginQrUrl(interaction.qr.url) : ''

  return (
    <CommonCard>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
        <div style={{ fontSize: 14, fontWeight: 700 }}>
          {interaction?.title || (showManualCallback ? t('完成浏览器授权') : t('继续完成登录'))}
        </div>
        <button type='button' className='fc-btn is-sm is-tonal' disabled={busy} onClick={onCancel}>
          {t('取消登录')}
        </button>
      </div>

      {interaction ? (
        <>
          {interaction.message ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', lineHeight: 1.5, whiteSpace: 'pre-wrap' }}>
              {interaction.message}
            </div>
          ) : null}

          {qrSrc ? (
            <div style={{ display: 'flex', justifyContent: 'center', padding: '8px 0' }}>
              <img
                src={qrSrc}
                alt={interaction.title}
                referrerPolicy='no-referrer'
                style={{ width: 216, height: 216, borderRadius: 12, background: '#fff', objectFit: 'contain', padding: 6 }}
              />
            </div>
          ) : null}

          {interaction.fields.map(field => {
            const val = values[field.key] || ''
            const pickedLabel = field.options.find(opt => opt.value === val)?.label || t('请选择')
            return (
              <div key={field.key} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--fc-text-secondary)' }}>
                  {field.label}
                </span>
                {field.type === 'select' ? (
                  <>
                    <PickerTrigger
                      valueLabel={val ? pickedLabel : undefined}
                      placeholder={t('请选择')}
                      disabled={busy}
                      onClick={() => setSelectOpen(field.key)}
                      ariaLabel={field.label}
                    />
                    <PickerSheet
                      open={selectOpen === field.key}
                      title={field.label}
                      searchable={field.options.length > 8}
                      options={field.options.map(opt => ({ value: opt.value, label: opt.label }))}
                      value={val}
                      onSelect={next => setValues(prev => ({ ...prev, [field.key]: next }))}
                      onClose={() => setSelectOpen('')}
                    />
                  </>
                ) : (
                  <input
                    type={field.secret ? 'password' : 'text'}
                    autoComplete='off'
                    className='fc-input'
                    disabled={busy}
                    value={val}
                    onChange={e => {
                      const next = e.currentTarget.value
                      setValues(prev => ({ ...prev, [field.key]: next }))
                    }}
                  />
                )}
              </div>
            )
          })}

          {interaction.captcha ? (
            isAlicom4Config(interaction.captcha) ? (
              <MobileAlicom4Captcha
                key={`${interaction.id}:${captchaGeneration}`}
                config={interaction.captcha}
                disabled={busy}
                onChange={setCaptcha}
              />
            ) : (
              <MobileGeetestCaptcha
                key={`${interaction.id}:${interaction.captcha.challenge}:${captchaGeneration}`}
                config={interaction.captcha}
                disabled={busy}
                onChange={setCaptcha}
              />
            )
          ) : null}

          {error ? <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{error}</div> : null}

          {!interaction.qr ? (
            <button
              type='button'
              className='fc-btn is-primary'
              style={{ width: '100%' }}
              disabled={busy || Boolean(interaction.captcha && !captcha)}
              onClick={() => void handleSubmitStep()}
            >
              {t('继续')}
            </button>
          ) : null}
        </>
      ) : (
        <>
          {flow.instructions ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', lineHeight: 1.5 }}>
              {flow.instructions}
            </div>
          ) : null}

          {authLink ? (
            <div
              className='fc-inner-block'
              style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
            >
              <div
                className='fc-mono'
                style={{ fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
              >
                {authLink}
              </div>
              <button
                type='button'
                className='fc-btn is-tonal is-sm is-pill'
                onClick={async () => {
                  await copyText(authLink)
                  store.toast(t('已复制授权链接'), 'success')
                }}
              >
                {t('复制链接')}
              </button>
            </div>
          ) : null}

          {showManualCallback ? (
            <>
              <input
                type='text'
                className='fc-input'
                value={callbackText}
                onChange={e => setCallbackText(e.currentTarget.value)}
                placeholder={t('粘贴授权完成后的回调链接或验证码…')}
              />
              <button
                type='button'
                className='fc-btn is-primary'
                style={{ width: '100%' }}
                disabled={!callbackText.trim() || busy}
                onClick={() => void handleSubmitCallback()}
              >
                {t('提交回调完成登录')}
              </button>
            </>
          ) : (
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              {t('正在等待登录授权结果…')}
            </div>
          )}
        </>
      )}

      {statusMsg ? (
        <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{statusMsg}</div>
      ) : null}
    </CommonCard>
  )
}
