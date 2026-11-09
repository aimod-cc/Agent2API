import * as React from 'react'
import { Button, Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from '@ui'
import { planActivityEnded, planClaimResultApplies, planClaimState, planQueryErrors, type ClaimResult, type PreviewPlan, type PreviewResult } from '../../../ui-shared/zcode-plans'
import { useDisplayClock } from '../../../ui-shared/use-display-clock'
import { shared } from './accounts-shared'
import { errorMessage, loadCenter } from './checkin-store'
import { ProviderLogo } from './checkin-row'
import { formatShort, type PlanEntry } from './checkin-types'
import type { PlansRowReporter } from './checkin-plans-section'
import { t } from '../i18n'

export function claimedPlansToday(claimPlans: Record<string, number> | null | undefined): string[] {
  if (!claimPlans || typeof claimPlans !== 'object') return []
  const today = Math.floor((Date.now() + 8 * 3_600_000) / 86_400_000)
  return Object.entries(claimPlans).filter(([, at]) => {
    const stamp = Number(at)
    return Number.isFinite(stamp) && stamp > 0
      && Math.floor((stamp + 8 * 3_600_000) / 86_400_000) === today
  }).map(([id]) => id)
}

export function PlanRow({ row, disabled, onReport }: {
  row: PlanEntry
  disabled: boolean
  onReport: PlansRowReporter
}): React.ReactElement {
  const now = useDisplayClock()
  const [preview, setPreview] = React.useState<PreviewResult | null>(null)
  const [results, setResults] = React.useState<Record<string, ClaimResult>>({})
  const [busy, setBusy] = React.useState<'query' | 'claim' | ''>('')
  const [error, setError] = React.useState('')
  const lock = React.useRef(false)
  const mounted = React.useRef(true)
  const controller = React.useRef<AbortController | null>(null)
  const key = `plan:${row.id}`

  React.useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      controller.current?.abort()
      onReport(key, null)
    }
  }, [key, onReport])

  const plans = (preview?.plans ?? []).filter(plan => !planActivityEnded(plan, now))
  const stored = claimedPlansToday(row.claimPlans)
  const stateOf = (plan: PreviewPlan) => {
    const result = results[String(plan.planId || '')]
    if (planClaimResultApplies(plan, result)) {
      if (result.ok || result.failure === 'already_claimed') return 'claimed'
      return result.failure === 'ineligible' ? 'ineligible' : 'unknown'
    }
    const at = Number(row.claimPlans?.[String(plan.planId || '')])
    if (Number.isFinite(at) && at > 0 && planClaimResultApplies(plan, { ok: true, claimedAt: at })) return 'claimed'
    return planClaimState(plan, plan.claimState ? [] : stored)
  }
  const claimable = plans.filter(plan => plan.planId && stateOf(plan) === 'claimable')
  const claimed = preview ? plans.filter(plan => stateOf(plan) === 'claimed').length : stored.length
  const uncertain = plans.some(plan => stateOf(plan) === 'unknown')

  const query = async () => {
    if (lock.current) return
    lock.current = true
    setBusy('query')
    setError('')
    try {
      const bridge = shared().wbZcodeClaim
      if (!bridge?.preview) throw new Error(t('当前环境不支持领取（桥接方法缺失）'))
      const result = await bridge.preview(row.id) as PreviewResult
      if (!mounted.current) return
      if (!Array.isArray(result?.plans)) throw new Error(t('后端未返回活动福利数据'))
      setPreview(result)
      setResults(current => Object.fromEntries(Object.entries(current)
        .filter(([, receipt]) => receipt.ok || receipt.failure === 'already_claimed')))
      setError(planQueryErrors(result).join('；'))
      void loadCenter()
    } catch (cause) {
      if (mounted.current) { setPreview(null); setError(errorMessage(cause)) }
    } finally {
      lock.current = false
      if (mounted.current) setBusy('')
    }
  }

  const claimAll = async (): Promise<boolean> => {
    if (lock.current || claimable.length === 0) return true
    const bridge = shared().wbZcodeClaim
    if (!bridge?.claim) {
      setError(t('当前环境不支持领取（桥接方法缺失）'))
      return false
    }
    lock.current = true
    const abort = new AbortController()
    controller.current = abort
    setBusy('claim')
    setError('')
    const targets = [...new Map(claimable.map(plan => [String(plan.planId), plan])).values()]
    try {
      for (const plan of targets) {
        if (!mounted.current || abort.signal.aborted) return false
        if (planActivityEnded(plan)) continue
        const planId = String(plan.planId)
        const result = await bridge.claim(row.id, planId, abort.signal) as ClaimResult | null
        if (!mounted.current || abort.signal.aborted) return false
        const receipt = result ?? { ok: false, failure: 'confirming' }
        const taken = receipt.ok || receipt.failure === 'already_claimed'
        setResults(current => ({ ...current, [planId]: {
          ...receipt, claimedAt: taken ? receipt.claimedAt || Date.now() : receipt.claimedAt,
        } }))
        if (!result) {
          setError(t('领取结果待确认，请稍后查询；不会重复提交领取'))
          return false
        }
        if (taken) {
          void loadCenter()
          void shared().wbApp?.refresh?.()
        } else {
          setError(result.message || result.failureLabel || t('活动领取未成功，请重新查询后再试'))
        }
      }
      return true
    } catch (cause) {
      if (mounted.current) {
        setPreview(null)
        setError(errorMessage(cause))
      }
      return false
    } finally {
      if (controller.current === abort) controller.current = null
      lock.current = false
      if (mounted.current) setBusy('')
    }
  }

  React.useLayoutEffect(() => {
    onReport(key, { claimed: claimed > 0 && claimable.length === 0 && !uncertain, claimable: claimable.length,
      busy: Boolean(busy), query, claimAll })
  })

  const description = busy === 'query' ? t('查询中…') : busy === 'claim' ? t('领取中…')
    : claimable.length > 0 ? t('可领取 {n} 项', { n: claimable.length })
      : uncertain ? t('状态待确认')
        : claimed > 0 ? t('今天已领 {n} 份', { n: claimed })
        : preview ? t('当前没有可领取的套餐')
          : row.claimAt ? t('今天未领 · 上次领取 {time}', { time: formatShort(row.claimAt) }) : t('今天未领')
  return <>
    <Item className='gap-3'>
      <ItemMedia><ProviderLogo id={row.provider || 'zcode'} label='ZCode' /></ItemMedia>
      <ItemContent>
        <ItemTitle>{row.name || row.id}</ItemTitle>
        <ItemDescription>{description}</ItemDescription>
      </ItemContent>
      <ItemActions>
        <Button size='sm' variant='outline' disabled={disabled || Boolean(busy)} onClick={() => void query()}>
          {busy === 'query' ? t('查询中…') : t('查询套餐')}
        </Button>
        <Button size='sm' variant={claimed > 0 ? 'outline' : 'default'} disabled={disabled || Boolean(busy)}
          onClick={() => window.wbZcodePlans?.open?.({ id: row.id, name: row.name })}>
          {claimed > 0 ? t('套餐明细') : t('去领取')}
        </Button>
      </ItemActions>
    </Item>
    {error ? <p role='alert' className='m-0 px-3.5 pb-2 text-[12px] text-destructive break-words'>{error}</p> : null}
  </>
}
