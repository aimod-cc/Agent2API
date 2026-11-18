import * as React from 'react'
import { Button, Field, FieldLabel, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { shared, type AccountsBridge } from './accounts-shared'
import { zcodeCaptchaStatsMatch } from '../../../ui-shared/zcode-captcha-binding'
import { zcodePlanLabel, ZCODE_PLAN_CODING, ZCODE_PLAN_START } from './accounts-domain'

type PoolStats = NonNullable<Awaited<ReturnType<NonNullable<AccountsBridge['zcodeCaptchaStats']>>>>
type MintStatus = { state: string; error: string; accountId?: string; provider?: string }
type CaptchaView = { accountId: string; pool: PoolStats | null; mint: MintStatus | null; phase: string }

function mintForAccount(accountId: string): MintStatus | null {
  const value = shared().wbZcodeCaptchaPool?.status?.(accountId)
  return value?.accountId === accountId ? value : null
}

export function poolStatus(pool: PoolStats | null, mint: MintStatus | null): string {
  if (!pool) return t('读取中…')
  const error = pool.error || pool.configError
  if (error) return t('验证码状态读取失败，请刷新后重试')
  if (pool.required === false) return t('当前模型请求免人机验证')
  if (mint?.state === 'verifying') return t('正在验证…')
  if (mint?.state === 'error') {
    const safe = [t('验证码已提交，但状态刷新失败，请手动刷新'), t('验证码提交结果未确认，请刷新状态后重新验证；不会自动重发')]
    return safe.includes(mint.error) ? mint.error : t('验证码验证未完成，请刷新配置后重试')
  }
  if (mint?.state === 'paused') return t('已暂停验证，需要时请手动重试')
  const counts = `${Number(pool.ready) || 0} / ${Number(pool.target) || 3}`
  if (!Number(pool.startPlanAccounts)) return t('未启用套餐池账号，不生成验证码令牌')
  if (Number(pool.ready) > 0) return t('验证码令牌：{counts}', { counts })
  return pool.needsTokens === true
    ? t('套餐池请求需要验证，请主动开始验证后重试请求')
    : t('没有近期套餐池请求，不生成验证码令牌')
}

export function ZcodeCaptchaStatus({ accountId, disabled = false, onBusy }: {
  accountId: string; disabled?: boolean; onBusy?: (busy: boolean) => void
}) {
  const [view, setView] = React.useState<CaptchaView>(() => ({ accountId, pool: null, mint: null, phase: '' }))
  const controller = React.useRef<AbortController | null>(null)
  const mounted = React.useRef(true)
  const generation = React.useRef(0)
  const readGeneration = React.useRef(0)
  const latest = React.useRef({ accountId, onBusy })
  latest.current = { accountId, onBusy }
  const { pool, mint, phase } = view.accountId === accountId ? view : { pool: null, mint: null, phase: '' }
  const refresh = React.useCallback(async () => {
    if (controller.current) return
    const api = shared().workbuddyDesktop
    const epoch = generation.current
    const current = ++readGeneration.current
    const isCurrent = () => mounted.current && epoch === generation.current && current === readGeneration.current
      && latest.current.accountId === accountId && shared().workbuddyDesktop === api
    try {
      if (!api?.zcodeCaptchaStats || !accountId) throw new Error('captcha_api_unavailable')
      const value = await api.zcodeCaptchaStats(accountId)
      if (!isCurrent()) return
      if (!zcodeCaptchaStatsMatch(value, accountId)) throw new Error('invalid_captcha_scope')
      setView(previous => ({ ...previous, accountId, pool: value, mint: mintForAccount(accountId) || previous.mint }))
    } catch {
      if (isCurrent()) setView(previous => ({ ...previous, accountId, pool: { error: t('验证码状态读取失败，请刷新后重试') } }))
    }
  }, [accountId])
  React.useEffect(() => {
    mounted.current = true
    generation.current++
    setView({ accountId, pool: null, mint: null, phase: '' })
    void refresh()
    const timer = window.setInterval(() => void refresh(), 15000)
    const onStatus = (event: Event) => {
      if (!mounted.current || latest.current.accountId !== accountId
        || (event as CustomEvent<MintStatus>).detail?.accountId !== accountId) return
      setView(previous => ({ ...previous, mint: mintForAccount(accountId) }))
      void refresh()
    }
    window.addEventListener('zcode-captcha-status', onStatus)
    return () => {
      mounted.current = false
      generation.current++
      readGeneration.current++
      controller.current?.abort()
      controller.current = null
      latest.current.onBusy?.(false)
      window.clearInterval(timer)
      window.removeEventListener('zcode-captcha-status', onStatus)
    }
  }, [accountId, refresh])

  async function verify() {
    const api = shared().wbZcodeCaptchaPool
    const bridge = shared().workbuddyDesktop
    if (controller.current || disabled || !api?.verify || latest.current.accountId !== accountId) return
    const current = new AbortController()
    const epoch = generation.current
    const notifyBusy = latest.current.onBusy
    const isCurrent = () => mounted.current && epoch === generation.current && latest.current.accountId === accountId
      && shared().workbuddyDesktop === bridge
    readGeneration.current++
    controller.current = current
    setView(previous => ({ ...previous, accountId, phase: 'preparing', mint: { state: 'verifying', error: '', accountId } }))
    notifyBusy?.(true)
    try {
      await api.verify(accountId, current.signal, value => {
        if (isCurrent() && !current.signal.aborted) setView(previous => ({ ...previous, phase: value }))
      })
    } catch (error) {
      if (isCurrent() && !current.signal.aborted && !['AbortError', 'CaptchaCancelledError'].includes((error as Error)?.name)) {
        const message = error instanceof Error ? error.message : ''
        const safe = [t('验证码已提交，但状态刷新失败，请手动刷新'), t('验证码提交结果未确认，请刷新状态后重新验证；不会自动重发')]
        setView(previous => ({ ...previous, mint: { state: 'error', error: safe.includes(message) ? message : t('验证码验证未完成，请刷新配置后重试'), accountId } }))
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

  const busy = Boolean(phase)
  const failed = Boolean(pool?.error || pool?.configError || mint?.state === 'error')
  /**
   * 这一块压成**一行**：一枚状态点 + 一句话 + 必要时的按钮。
   *
   * 「后台只读取状态，不会自动弹出验证码；验证结果一次性使用，短时间内过期」
   * 那条规则挂在 title 上，需要时悬停可查 —— 它是通道的说明，不是当前状态，
   * 不该跟状态抢同一行。
   */
  return (
    <div className='flex flex-wrap items-center gap-2'
      title={t('后台只读取状态，不会自动弹出验证码；验证结果一次性使用，短时间内过期。')}>
      <StatusDot variant={failed ? 'destructive' : pool?.required === false ? 'success' : 'warning'} />
      <span className={cn('text-[12.5px]', failed && 'text-destructive')}>{poolStatus(pool, mint)}</span>
      <div className='ml-auto flex flex-wrap items-center gap-2'>
        {pool?.required !== false && Number(pool?.startPlanAccounts) > 0 ? (
          <Button variant='outline' size='xs' disabled={disabled || busy || Boolean(pool?.error || pool?.configError) || !shared().wbZcodeCaptchaPool?.verify}
            onClick={() => void verify()}>{busy ? t('正在验证…') : t('开始验证')}</Button>
        ) : null}
        {busy ? <Button variant='outline' size='xs' disabled={phase === 'submitting'}
          onClick={() => controller.current?.abort()}>{t('取消验证')}</Button> : null}
        <Button variant='ghost' size='xs' disabled={busy || disabled} onClick={() => void refresh()}>{t('刷新')}</Button>
      </div>
    </div>
  )
}

export function PlanChannelField({ plan, hasJwt, onChange }: {
  plan: string; hasJwt: boolean; onChange: (next: string) => void
}) {
  return (
    <Field orientation='horizontal' className='field-row mt-2.5'>
      <FieldLabel htmlFor='account-plan-channel'>{t('使用套餐')}</FieldLabel>
      <Select value={plan} onValueChange={value => onChange(String(value))}>
        <SelectTrigger id='account-plan-channel' className='min-w-[220px]'><SelectValue>{zcodePlanLabel(plan)}</SelectValue></SelectTrigger>
        <SelectContent>
          <SelectItem value={ZCODE_PLAN_CODING}>{t('编码套餐（Coding Plan）— 走开放平台端点，消耗开放平台的额度（订阅套餐 / 资源包 / 充值余额）')}</SelectItem>
          <SelectItem value={ZCODE_PLAN_START}>{t('套餐池（Start Plan）— 走 zcode.z.ai 端点，消耗 ZCode 套餐池的额度（新用户体验额度 / 活动领取）')}</SelectItem>
        </SelectContent>
      </Select>
      {plan === ZCODE_PLAN_START && !hasJwt ? <span className='detail text-destructive'>{t('该账号没有套餐登录态（jwt），套餐池通道会失败：请重新登录该账号')}</span> : null}
      {plan === ZCODE_PLAN_START ? <p className='detail'>{t('需要人机验证时，在该账号的「套餐明细」中主动开始验证。')}</p> : null}
    </Field>
  )
}
