import { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@ui'
import { captchaPoolStats, verifyPlanChannel, type CaptchaPhase, type CaptchaPoolStats } from '../../api/zcode'
import { serverIdentity } from '../../lib/servers'
import { t } from '../../i18n'

type View = { scope: string; stats: CaptchaPoolStats | null; phase: CaptchaPhase | ''; error: string }

export function ZcodeCaptchaPanel({ accountId, disabled, onBusy }: {
  accountId: string; disabled: boolean; onBusy?: (busy: boolean) => void
}) {
  const identity = serverIdentity()
  const scope = JSON.stringify([identity, accountId])
  const [view, setView] = useState<View>(() => ({ scope, stats: null, phase: '', error: '' }))
  const controller = useRef<AbortController | null>(null)
  const reading = useRef<AbortController | null>(null)
  const mounted = useRef(true)
  const generation = useRef(0)
  const readGeneration = useRef(0)
  const latest = useRef({ scope, onBusy })
  latest.current = { scope, onBusy }
  const currentView = view.scope === scope ? view : { scope, stats: null, phase: '', error: '' }
  const { stats, phase, error } = currentView

  const refresh = useCallback(async (clearError = false) => {
    if (controller.current) return
    reading.current?.abort()
    const current = new AbortController()
    reading.current = current
    const run = ++readGeneration.current
    const epoch = generation.current
    const isCurrent = () => mounted.current && !current.signal.aborted && epoch === generation.current
      && run === readGeneration.current && latest.current.scope === scope && serverIdentity() === identity
    try {
      const value = await captchaPoolStats(accountId, current.signal)
      if (isCurrent()) setView(previous => ({ scope, stats: value, phase: '', error: clearError ? '' : previous.error }))
    } catch {
      if (isCurrent()) setView(previous => ({ ...previous, scope, stats: null, error: t('验证码状态读取失败，请刷新后重试') }))
    } finally {
      if (reading.current === current) reading.current = null
    }
  }, [accountId, identity, scope])

  useEffect(() => {
    mounted.current = true
    generation.current++
    setView({ scope, stats: null, phase: '', error: '' })
    void refresh()
    return () => {
      mounted.current = false
      generation.current++
      readGeneration.current++
      reading.current?.abort()
      reading.current = null
      controller.current?.abort()
      controller.current = null
      latest.current.onBusy?.(false)
    }
  }, [scope, refresh])

  async function verify() {
    if (controller.current || disabled || latest.current.scope !== scope || serverIdentity() !== identity) return
    reading.current?.abort()
    reading.current = null
    readGeneration.current++
    const current = new AbortController()
    const epoch = generation.current
    const notifyBusy = latest.current.onBusy
    const isCurrent = () => mounted.current && generation.current === epoch && latest.current.scope === scope && serverIdentity() === identity
    controller.current = current
    setView(previous => ({ ...previous, scope, error: '', phase: 'preparing' }))
    notifyBusy?.(true)
    try {
      await verifyPlanChannel(accountId, current.signal, next => {
        if (isCurrent() && !current.signal.aborted) setView(previous => ({ ...previous, phase: next }))
      })
    } catch (cause) {
      if (isCurrent() && !current.signal.aborted && !['AbortError', 'CaptchaCancelledError'].includes((cause as Error)?.name)) {
        const message = cause instanceof Error ? cause.message : ''
        const safe = [t('验证码已提交，但状态刷新失败，请手动刷新'), t('验证码提交结果未确认，请刷新状态后重新验证；不会自动重发')]
        setView(previous => ({ ...previous, error: safe.includes(message) ? message : t('验证码验证未完成，请刷新配置后重试') }))
      }
    } finally {
      if (controller.current === current) {
        controller.current = null
        notifyBusy?.(false)
        if (isCurrent()) {
          setView(previous => ({ ...previous, phase: '' }))
          void refresh()
        }
      }
    }
  }

  const failure = error || (stats?.error || stats?.configError ? t('验证码状态读取失败，请刷新后重试') : '')
  const text = stats?.required === false ? t('当前模型请求免人机验证')
    : Number(stats?.ready) > 0 ? t('验证码令牌：{counts}', { counts: `${stats?.ready} / ${stats?.target || 3}` })
      : stats?.needsTokens ? t('套餐池请求需要验证，请主动开始验证后重试请求')
        : t('没有近期套餐池请求，不生成验证码令牌')
  return (
    <section className='m-ck-account'>
      <div className='m-ck-title'>{t('套餐池人机验证')}</div>
      <p className={failure ? 'm-ck-error' : 'm-ck-note'}>{failure || (stats ? text : t('读取中…'))}</p>
      <p className='m-ck-note'>{t('后台只读取状态，不会自动弹出验证码；验证结果一次性使用，短时间内过期。')}</p>
      <div className='m-ck-actions'>
        {stats?.required !== false && Number(stats?.startPlanAccounts) > 0 ? (
          <Button size='sm' variant='outline' disabled={disabled || Boolean(phase) || Boolean(stats?.error || stats?.configError)} onClick={() => void verify()}>
            {phase ? t('正在验证…') : t('开始验证')}
          </Button>
        ) : null}
        {phase ? <Button size='sm' variant='outline' disabled={phase === 'submitting'} onClick={() => controller.current?.abort()}>{t('取消验证')}</Button> : null}
        <Button size='sm' variant='ghost' disabled={disabled || Boolean(phase)} onClick={() => void refresh(true)}>{t('刷新')}</Button>
      </div>
    </section>
  )
}
