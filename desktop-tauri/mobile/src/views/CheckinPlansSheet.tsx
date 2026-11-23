import { useCallback, useEffect, useRef, useState } from 'react'
import {
  cancelPlanCaptcha,
  captchaPoolStats,
  claimPlan,
  claimedPlansToday,
  fetchAccountUsage,
  fetchUsageSnapshot,
  previewPlans,
  verifyPlanChannel,
  type CaptchaPhase,
  type CaptchaPoolStats,
  type ClaimResult,
  type Entitlement,
  type OwnedPlan,
  type PlanEntry,
  type PreviewResult,
  type UsageRow,
} from '../core/api'
import { formatCompact, formatDateTime } from '../core/format'
import { t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import { BottomSheet, CommonCard, EmptyBlock, TonalChip } from '../ui/components'
import {
  planActivityEnded,
  planClaimResultApplies,
  planClaimState,
  planQueryErrors,
  unconfirmedPlanIds,
} from '../../../ui-shared/zcode-plans'

function stamp(seconds?: number | null): string {
  return seconds ? formatDateTime(seconds * 1000) : ''
}

function entitlementText(item: Entitlement): string {
  const period =
    item.period === 'daily'
      ? t('每日')
      : item.period === 'weekly'
        ? t('每周')
        : item.period === 'monthly'
          ? t('每月')
          : ''
  return [item.showName, item.grantUnits ? `${formatCompact(item.grantUnits)} ${item.unitType || ''}` : '', period]
    .filter(Boolean)
    .join(' · ')
}

function stateView(state?: string): { label: string; tone: 'success' | 'warning' | 'neutral' } {
  if (state === 'active') return { label: t('生效中'), tone: 'success' }
  if (state === 'pending') return { label: t('待生效'), tone: 'warning' }
  return { label: state === 'expired' ? t('已过期') : t('状态未知'), tone: 'neutral' }
}

export function CheckinPlansSheet({
  entry,
  onClose,
  onChange,
}: {
  entry: PlanEntry | null
  onClose: () => void
  onChange: () => void
}) {
  const { session } = useStore()
  const [preview, setPreview] = useState<PreviewResult | null>(null)
  const [usage, setUsage] = useState<UsageRow | null>(null)
  const [captchaStats, setCaptchaStats] = useState<CaptchaPoolStats | null>(null)
  const [previewError, setPreviewError] = useState('')
  const [usageError, setUsageError] = useState('')
  const [receiptError, setReceiptError] = useState('')
  const [captchaError, setCaptchaError] = useState('')
  const [loading, setLoading] = useState(true)
  const [querying, setQuerying] = useState(false)
  const [claiming, setClaiming] = useState('')
  const [claimPhase, setClaimPhase] = useState<CaptchaPhase | ''>('')
  const [verifying, setVerifying] = useState(false)
  const [claimedNow, setClaimedNow] = useState<string[]>([])
  const [claimResults, setClaimResults] = useState<Record<string, ClaimResult>>({})
  const mounted = useRef(true)
  const claimAbort = useRef<AbortController | null>(null)
  const identity = serverIdentity()

  const loadCaptchaStats = useCallback(async () => {
    if (!entry) return
    try {
      const next = await captchaPoolStats(entry.id)
      if (mounted.current && serverIdentity() === identity) {
        setCaptchaStats(next)
        setCaptchaError('')
      }
    } catch {
      if (mounted.current && serverIdentity() === identity) {
        setCaptchaError(t('验证码状态读取失败，请刷新后重试'))
      }
    }
  }, [entry, identity])

  const loadPreview = useCallback(async () => {
    if (!entry) return
    setLoading(true)
    try {
      const next = await previewPlans(entry.id)
      if (mounted.current && serverIdentity() === identity) {
        setPreview(next)
        setPreviewError('')
        onChange()
      }
    } catch (err) {
      if (mounted.current && serverIdentity() === identity) {
        setPreviewError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (mounted.current && serverIdentity() === identity) setLoading(false)
    }
  }, [entry, identity, onChange])

  const queryUsage = useCallback(async () => {
    if (!entry) return
    setQuerying(true)
    try {
      const next = await fetchAccountUsage(entry.id)
      if (!mounted.current || serverIdentity() !== identity) return
      const row = next.results?.find(item => item.id === entry.id) || null
      setUsage(row)
      setUsageError(row?.error || (row ? '' : t('未返回余额数据')))
    } catch (err) {
      if (mounted.current && serverIdentity() === identity) {
        setUsageError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (mounted.current && serverIdentity() === identity) setQuerying(false)
    }
  }, [entry, identity])

  useEffect(() => {
    if (!entry) return
    mounted.current = true
    setPreview(null)
    setUsage(null)
    setPreviewError('')
    setReceiptError('')
    setUsageError('')
    setClaimedNow([])
    setClaimResults({})
    const ac = new AbortController()
    void loadPreview()
    void loadCaptchaStats()
    void fetchUsageSnapshot(ac.signal, [entry.id])
      .then(snap => {
        if (!mounted.current || ac.signal.aborted || serverIdentity() !== identity) return
        const row = snap.results?.find(item => item.id === entry.id)
        if (row) {
          setUsage(row)
          setUsageError(row.error || '')
        } else {
          void queryUsage()
        }
      })
      .catch(() => {
        if (mounted.current && !ac.signal.aborted && serverIdentity() === identity) void queryUsage()
      })
    return () => {
      mounted.current = false
      ac.abort()
      claimAbort.current?.abort()
    }
  }, [entry?.id, identity, loadCaptchaStats, loadPreview, queryUsage])

  if (!entry) return null

  const account = session?.accounts?.accounts?.find(a => a.id === entry.id)
  const stored = account?.claimPlans as Record<string, unknown> | undefined
  const claimed = [...new Set([...claimedPlansToday(stored || entry.claimPlans), ...claimedNow])]
  const plans = preview?.plans || []
  const activityPlans = plans.filter(plan => !planActivityEnded(plan))
  const queryErrors = preview ? planQueryErrors(preview) : []

  const rawOwned = (usage?.usage as { plans?: unknown } | undefined)?.plans
  const owned: OwnedPlan[] = Array.isArray(rawOwned) ? [...(rawOwned as OwnedPlan[])] : []
  for (const plan of plans) {
    const item = plan.ownedPlan
    if (item && !owned.some(p => (item.userPlanId ? p.userPlanId === item.userPlanId : p.planId === item.planId))) {
      owned.push(item)
    }
  }
  const unconfirmed = unconfirmedPlanIds(claimed, plans, owned)

  const runVerifyChannel = async () => {
    if (verifying || claiming) return
    setVerifying(true)
    const ac = new AbortController()
    try {
      await verifyPlanChannel(entry.id, ac.signal)
      await loadCaptchaStats()
      store.toast(t('验证通道已补充令牌'), 'success')
    } catch (err) {
      if ((err as Error)?.name !== 'CaptchaCancelledError') {
        store.toast(err instanceof Error ? err.message : String(err), 'danger')
      }
    } finally {
      if (mounted.current) setVerifying(false)
    }
  }

  const runClaim = async (planId: string) => {
    if (claimAbort.current || verifying) return
    const ac = new AbortController()
    claimAbort.current = ac
    setClaiming(planId)
    setClaimPhase('preparing')
    try {
      const result = await claimPlan(entry.id, planId, ac.signal, phase => {
        if (mounted.current) setClaimPhase(phase)
      })
      if (!mounted.current || ac.signal.aborted || serverIdentity() !== identity) return
      const settledId = result.planId || planId
      setClaimResults(prev => ({ ...prev, [settledId]: { ...result, claimedAt: result.claimedAt || Date.now() } }))
      if (result.ok || result.failure === 'already_claimed') {
        setReceiptError(result.marketingError || '')
        setClaimedNow(prev => [...new Set([...prev, settledId])])
        store.toast(result.ok ? t('领取成功：{planId}', { planId: settledId }) : result.failureLabel || t('今日已领'), 'success')
        void store.refresh(true)
        onChange()
        void queryUsage()
        void loadPreview()
      } else {
        store.toast([result.failureLabel || t('未领取成功'), result.message].filter(Boolean).join('：'), 'danger')
      }
    } catch (err) {
      if (!ac.signal.aborted && (err as Error)?.name !== 'CaptchaCancelledError') {
        store.toast(err instanceof Error ? err.message : String(err), 'danger')
      }
    } finally {
      if (claimAbort.current === ac) claimAbort.current = null
      if (mounted.current) {
        setClaiming('')
        setClaimPhase('')
      }
    }
  }

  const closeSheet = () => {
    if (claimPhase === 'submitting') return
    claimAbort.current?.abort()
    cancelPlanCaptcha()
    onClose()
  }

  return (
    <BottomSheet
      open={Boolean(entry)}
      title={t('套餐明细 — {name}', { name: entry.name || entry.id })}
      onClose={closeSheet}
      actions={
        <button
          type='button'
          className='fc-tonal-icon-btn'
          disabled={loading || Boolean(claiming) || verifying}
          onClick={() => void loadPreview()}
          style={{ background: 'var(--fc-card)', height: 34 }}
        >
          {t('刷新')}
        </button>
      }
      fixedHeader={
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
            <div style={{ minWidth: 0 }}>
              <div style={{ fontSize: 13.5, fontWeight: 600, color: 'var(--fc-text)' }}>{t('人机验证通道')}</div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                {captchaError
                  ? captchaError
                  : captchaStats?.required === false
                    ? t('当前免人机验证')
                    : Number(captchaStats?.ready) > 0
                      ? t('可用令牌：{ready} / {target}', {
                          ready: Number(captchaStats?.ready),
                          target: Number(captchaStats?.target || 3),
                        })
                      : t('暂无预备令牌')}
              </div>
            </div>
            {captchaStats?.required !== false ? (
              <button
                type='button'
                className='fc-btn is-tonal is-sm'
                disabled={verifying || Boolean(claiming)}
                onClick={() => void runVerifyChannel()}
              >
                {verifying ? t('验证中…') : t('补充验证')}
              </button>
            ) : null}
          </div>
        </CommonCard>
      }
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          {claiming ? (
            <button
              type='button'
              className='fc-btn'
              style={{ flex: 1 }}
              disabled={claimPhase === 'submitting'}
              onClick={() => {
                claimAbort.current?.abort()
                cancelPlanCaptcha()
              }}
            >
              {t('取消验证')}
            </button>
          ) : null}
          <button
            type='button'
            className='fc-btn is-primary'
            style={{ flex: 1 }}
            disabled={claimPhase === 'submitting'}
            onClick={closeSheet}
          >
            {t('关闭')}
          </button>
        </div>
      }
    >
      {previewError ? <CommonCard>{previewError}</CommonCard> : null}
      {receiptError ? (
        <CommonCard>{t('套餐已领取，但营销回执未完成：{message}', { message: receiptError })}</CommonCard>
      ) : null}
      {loading ? (
        <EmptyBlock icon='clock' title={t('正在查询本期活动…')} />
      ) : !activityPlans.length ? (
        <EmptyBlock
          icon='gift'
          title={t('暂无本期可领套餐')}
          subtitle={queryErrors.length ? queryErrors.join('；') : t('可点击右上角刷新重试')}
        />
      ) : (
        activityPlans.map(plan => {
          const id = plan.planId || ''
          const savedResult = claimResults[id]
          const result = planClaimResultApplies(plan, savedResult) ? savedResult : undefined
          const locallySettled = result?.ok || result?.failure === 'already_claimed'
          const state = planClaimState(plan, locallySettled ? [id] : plan.claimState ? [] : claimed)
          const taken = state === 'claimed'
          const rejected = !taken && (result?.failure === 'ineligible' || state === 'ineligible')
          const statusLabel = taken
            ? t('已领取')
            : rejected
              ? t('不符合资格')
              : state === 'unknown'
                ? t('待确认')
                : t('可领取')
          const tone = taken ? 'success' : rejected ? 'warning' : 'primary'
          const period = [stamp(plan.startsAt), stamp(plan.endsAt)].filter(Boolean).join(' → ')
          const expiresAt = plan.ownedPlan?.endsAt || plan.claimEndsAt || result?.endsAt
          const msg = result && !result.ok && result.failure !== 'already_claimed' ? result.message || result.failureLabel : plan.claimMessage
          return (
            <CommonCard key={id || plan.name}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
                <div style={{ fontSize: 15, fontWeight: 700, color: 'var(--fc-text)' }}>{plan.name || id}</div>
                <TonalChip tone={tone}>{statusLabel}</TonalChip>
              </div>
              {period ? <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 4 }}>{t('活动时间：{time}', { time: period })}</div> : null}
              {expiresAt ? <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>{t('套餐到期：{time}', { time: stamp(expiresAt) })}</div> : null}
              {msg ? <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginTop: 4 }}>{msg}</div> : null}
              {plan.entitlements?.map((ent, idx) => (
                <div key={idx} style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                  {entitlementText(ent)}
                </div>
              ))}
              <div style={{ marginTop: 10 }}>
                <button
                  type='button'
                  className={`fc-btn is-sm ${taken ? '' : 'is-primary'}`}
                  disabled={Boolean(claiming) || verifying || taken || !id}
                  onClick={() => void runClaim(id)}
                >
                  {claiming === id ? t('领取中…') : taken ? t('已领取') : t('立即领取')}
                </button>
              </div>
            </CommonCard>
          )
        })
      )}

      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, marginBottom: 8 }}>
          <div style={{ fontSize: 14, fontWeight: 700 }}>{t('账号名下套餐')}</div>
          <button
            type='button'
            className='fc-btn is-sm is-tonal'
            disabled={querying || Boolean(claiming)}
            onClick={() => void queryUsage()}
          >
            {querying ? t('查询中…') : t('刷新余额')}
          </button>
        </div>
        {usageError ? <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{usageError}</div> : null}
        {owned.length ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {owned.map(item => {
              const sv = stateView(item.state)
              return (
                <div
                  key={item.userPlanId || item.planId || item.name}
                  className='fc-inner-block'
                  style={{ padding: '10px 12px', display: 'flex', flexDirection: 'column', gap: 4 }}
                >
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
                    <span style={{ fontWeight: 600, fontSize: 13.5 }}>{item.name || item.planId}</span>
                    <TonalChip tone={sv.tone}>{sv.label}</TonalChip>
                  </div>
                  {item.endsAt ? (
                    <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                      {t('到期时间：{time}', { time: stamp(item.endsAt) })}
                    </div>
                  ) : null}
                </div>
              )
            })}
          </div>
        ) : (
          <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)' }}>
            {querying ? t('正在查询余额…') : t('当前暂无套餐记录')}
          </div>
        )}
        {unconfirmed.length ? (
          <div style={{ fontSize: 12, color: 'var(--fc-warning)', marginTop: 8 }}>
            {t('今日已领取但余额尚未同步：{plans}', { plans: unconfirmed.join('、') })}
          </div>
        ) : null}
      </CommonCard>
    </BottomSheet>
  )
}
