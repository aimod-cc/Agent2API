import { useEffect, useRef, useState } from 'react'
import { Badge, Button, Checkbox, Spinner } from '@ui'
import type { OfficeaceSubscriptionApi, OfficeaceSubscriptionState, OfficeaceSubscriptionStatus } from './checkin-officeace-types'

type Translate = (key: string, params?: Record<string, string | number>) => string
const STATES = new Set<OfficeaceSubscriptionStatus>([
  'active', 'inactive', 'expired', 'frozen', 'unknown', 'pending',
  'credentials_missing', 'credentials_expired', 'permission_denied', 'unsupported',
])

function statusLabel(status: OfficeaceSubscriptionStatus | undefined, t: Translate): string {
  switch (status) {
    case 'active': return t('已开通')
    case 'inactive': return t('未开通')
    case 'expired': return t('套餐已到期')
    case 'frozen': return t('套餐已冻结')
    case 'pending': return t('开通结果待确认')
    case 'credentials_missing': return t('缺少开通凭证')
    case 'credentials_expired': return t('凭证需刷新')
    case 'permission_denied': return t('授权被拒绝')
    case 'unsupported': return t('暂不支持')
    case 'unknown': return t('状态未确认')
    default: return t('尚未查询')
  }
}

function description(status: OfficeaceSubscriptionStatus | undefined, t: Translate): string {
  switch (status) {
    case 'active': return t('套餐已开通，无需重复操作')
    case 'inactive': return t('尚未开通套餐，可开通免费体验套餐')
    case 'expired':
    case 'frozen': return t('原套餐已到期或冻结，请在官方客户端处理；这里不会续费或变更套餐')
    case 'credentials_missing': return t('缺少套餐开通凭证，请通过果办网页登录补齐授权')
    case 'credentials_expired': return t('套餐查询凭证已过期，请先到账号页刷新凭证；此查询不会自动续期')
    case 'permission_denied': return t('官方拒绝套餐查询，请检查账号授权；这不一定表示 Token 已过期')
    case 'unsupported': return t('账号未使用官方果办网关地址，无法在此开通免费套餐')
    case 'unknown': return t('官方未返回明确的套餐状态，暂不能开通')
    case 'pending': return t('开通请求仍在处理或结果待确认，请稍后查询，不要重复提交')
    default: return t('查询只读取套餐状态，点击开通后才会执行')
  }
}

export function OfficeaceSubscription({ accountId, api, t, active = true, disabled = false, onChange }: {
  accountId: string
  api: OfficeaceSubscriptionApi
  t: Translate
  active?: boolean
  disabled?: boolean
  onChange?: () => void
}) {
  const [data, setData] = useState<OfficeaceSubscriptionState | null>(null)
  const [busy, setBusy] = useState<'query' | 'activate' | ''>('')
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')
  const [consent, setConsent] = useState(false)
  const lock = useRef(false)
  const pollCount = useRef(0)
  const generation = useRef(0)

  useEffect(() => {
    setData(null)
    setError('')
    setMessage('')
    setBusy('')
    setConsent(false)
    pollCount.current = 0
    lock.current = false
    const current = ++generation.current
    return () => { if (generation.current === current) generation.current += 1 }
  }, [accountId])

  const query = async () => {
    if (lock.current || disabled) return
    lock.current = true
    const current = generation.current
    setBusy('query')
    setError('')
    setMessage('')
    try {
      const result = await api.query(accountId)
      if (generation.current !== current) return
      if (!result || !STATES.has(result.status)) throw new Error(t('后端未返回有效的套餐状态'))
      setData(result)
      if (result.status === 'active') onChange?.()
    } catch (cause) {
      if (generation.current !== current) return
      setData(null)
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (generation.current === current) { lock.current = false; setBusy('') }
    }
  }

  useEffect(() => {
    if (active && !data && !error) void query()
  }, [active, accountId])

  useEffect(() => {
    if (!active || disabled || busy || error || data?.status !== 'pending' || pollCount.current >= 6) return
    const timer = window.setTimeout(() => {
      pollCount.current += 1
      void query()
    }, 5000)
    return () => window.clearTimeout(timer)
  }, [active, disabled, busy, error, data, accountId])

  const activate = async () => {
    if (lock.current || disabled || !consent || data?.status !== 'inactive' || data.canActivate !== true) return
    lock.current = true
    const current = generation.current
    setBusy('activate')
    setError('')
    setMessage('')
    pollCount.current = 0
    try {
      const result = await api.activate(accountId, consent)
      if (generation.current !== current) return
      if (!result || !['activated', 'already_active', 'skipped', 'pending'].includes(result.status)) {
        throw new Error(t('后端未返回有效的套餐状态'))
      }
      if (result.status === 'activated' || result.status === 'already_active') {
        setData({ ...result, status: 'active', canActivate: false })
        setMessage(result.message || (result.status === 'activated' ? t('免费套餐已开通并确认生效') : t('套餐已开通，无需重复操作')))
      } else {
        setData({ ...result, status: result.status === 'pending' ? 'pending' : 'unknown', canActivate: false })
        setMessage(result.message || (result.status === 'pending'
          ? t('开通请求仍在处理或结果待确认，请稍后查询，不要重复提交')
          : t('未执行开通，请查询套餐状态查看原因')))
      }
      onChange?.()
    } catch (cause) {
      if (generation.current !== current) return
      setData({ status: 'unknown', canActivate: false })
      setError(cause instanceof Error ? cause.message : String(cause))
      try {
        const latest = await api.query(accountId)
        if (generation.current === current && latest && STATES.has(latest.status)) setData(latest)
      } catch {
        if (generation.current === current) setMessage(t('开通未得到确认，请重新查询套餐状态；不会自动重试开通'))
      }
    } finally {
      if (generation.current === current) { lock.current = false; setBusy('') }
    }
  }

  const status = data?.status
  const variant = status === 'active' ? 'success' : status === 'inactive' ? 'brand'
    : status === 'pending' ? 'warning' : 'outline'
  return <div className='min-w-0 space-y-2' aria-label={t('果办免费体验套餐')}>
    <div className='flex flex-wrap items-center gap-2'>
      <span className='text-sm font-medium text-foreground'>{t('免费体验套餐')}</span>
      <Badge variant={variant} shape='tag'>{statusLabel(status, t)}</Badge>
      <div className='ml-auto flex flex-wrap gap-2'>
        <Button size='sm' variant='outline' disabled={Boolean(busy) || disabled} onClick={() => void query()}>
          {busy === 'query' ? <Spinner /> : null}{busy === 'query' ? t('查询中…') : t('查询套餐')}
        </Button>
        <Button size='sm' disabled={Boolean(busy) || disabled || !consent || status !== 'inactive' || data?.canActivate !== true}
          onClick={() => void activate()}>
          {busy === 'activate' ? <Spinner /> : null}
          {busy === 'activate' ? t('开通中…') : status === 'active' ? t('已开通') : t('开通免费套餐')}
        </Button>
      </div>
    </div>
    <p className='text-xs text-muted-foreground break-words'>{data?.message ? t(data.message) : description(status, t)}</p>
    <p className='text-xs text-muted-foreground'>{t('仅开通官方报价为 0 元的个人体验套餐，固定关闭自动续费；奖励另行领取')}</p>
    {status === 'inactive' && data?.canActivate ? <div className='space-y-1.5'>
      <label className='flex items-start gap-2 text-xs text-muted-foreground'>
        <Checkbox className='mt-0.5' checked={consent} onCheckedChange={checked => setConsent(checked === true)} disabled={Boolean(busy) || disabled} />
        <span>{t('我已阅读并同意以下服务声明，仅授权开通零元体验套餐，不自动续费')}</span>
      </label>
      <div className='flex flex-wrap gap-3 pl-5 text-xs'>
        <a className='text-primary underline underline-offset-2' href='https://www.huaweicloud.com/declaration/officeace.html' target='_blank' rel='noreferrer'>{t('果办服务声明')}</a>
        <a className='text-primary underline underline-offset-2' href='https://www.huaweicloud.com/declaration/agentarts.html' target='_blank' rel='noreferrer'>{t('AgentArts 服务声明')}</a>
      </div>
    </div> : null}
    {data?.autoRenew === false ? <Badge variant='success' shape='tag'>{t('已确认不自动续费')}</Badge> : null}
    {data?.autoRenew === true ? <p role='alert' className='text-sm text-destructive'>{t('官方显示自动续费已开启，请到官方客户端检查')}</p> : null}
    {error ? <p role='alert' className='text-sm text-destructive break-words'>{error}</p> : null}
    {message && message !== data?.message ? <p role='status' className='text-sm text-foreground break-words'>{t(message)}</p> : null}
  </div>
}
