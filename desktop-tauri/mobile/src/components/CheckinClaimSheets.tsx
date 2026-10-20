import { useEffect, useState } from 'react'
import { Sheet, SheetContent, SheetHeader, SheetBody, SheetTitle, SheetFooter } from '@ui'
import { Icon } from './Icon'
import { ProviderLogo } from './ProviderLogo'
import { Badge, Empty, ErrorNote } from './ui'
import { store } from '../state/store'
import {
  claimOnboarding,
  claimZcodePlan,
  fetchOnboarding,
  fetchZcodeCaptchaConfig,
  previewZcodePlans,
  type OnboardingStatus,
  type OnboardingTask,
  type ZcodeCaptchaConfig,
  type ZcodePlan,
} from '../api/gateway'
import { CaptchaCancelledError, cancelCaptcha, solveCaptcha } from '../lib/aliyunCaptcha'
import { isToday } from '../lib/format'
import { t } from '../i18n'

/**
 * 签到中心「新手福利与活动」两块的领取抽屉。
 *
 * ── 为什么这两块原本是「点不动」的 ────────────────────────────
 * 它们最初只是**入口清单**（后端 `extras.onboarding` / `extras.plans` 只给
 * 账号 id 与名字，任务详情要按账号惰性查），手机端第一版没接动作，只画了
 * 徽章。徽章写着「去领取」却点不动 —— 这是明确的坏体验，所以补上。
 *
 * ── 两条链的动作形态不同 ──────────────────────────────────────
 *   · **新手福利**（Loomy / 小浣熊 / CodeArts / OfficeAce / 智谱）：一条
 *     `POST .../onboarding/claim` 领全部，服务端幂等，重复点不重复加分；
 *   · **活动套餐**（ZCode）：先 `preview` 探测，再逐份 `zcode-claim` 领取。
 *     领取可能要过阿里云滑块（上游的风控配置说了算），所以那条链路多一步
 *     验证码 —— 见 lib/aliyunCaptcha。
 *
 * ── 为什么任务详情是「打开才查」────────────────────────────────
 * 快照里的行是零上游请求的（后端刻意如此：进签到中心不该为每个账号打一次
 * 上游）。查询是用户点开这一行才发生的动作，失败也只影响这张抽屉。
 */

/* ─── 新手福利 ─────────────────────────────── */

export function OnboardingSheet({
  account,
  onClose,
  onChanged,
}: {
  account: { id: string; name?: string; provider?: string } | null
  onClose: () => void
  onChanged: () => void
}) {
  const [data, setData] = useState<OnboardingStatus | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const id = account?.id || ''

  useEffect(() => {
    if (!id) return
    let cancelled = false
    setData(null)
    setError('')
    fetchOnboarding(id)
      .then(next => {
        if (!cancelled) setData(next)
      })
      .catch(failure => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : String(failure))
      })
    return () => {
      cancelled = true
    }
  }, [id])

  const claim = async () => {
    if (!id || busy) return
    setBusy(true)
    setError('')
    try {
      const result = await claimOnboarding(id)
      setData(result)
      // 后端对本次结果有交代时以它为准（OfficeAce 会回「本次到账 N 积分」）
      const message = typeof result.message === 'string' ? result.message.trim() : ''
      if (result.failed) {
        store.toast(t('新手福利有 {n} 项领取失败，可重试', { n: result.failed }), 'err')
      } else if (message) {
        store.toast(message)
      } else {
        store.toast(
          t('✅ 新手福利领取完成，累计 {n} 积分', { n: Number(result.earned) || 0 }),
        )
      }
      onChanged()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setBusy(false)
    }
  }

  const tasks: OnboardingTask[] = Array.isArray(data?.tasks) ? data?.tasks || [] : []
  const pending = tasks.filter(task => !task.done && !task.blocked).length
  const settled = data?.settled === true

  return (
    <Sheet open={Boolean(account)} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('新手福利')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {account ? (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <ProviderLogo id={account.provider || ''} />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, fontWeight: 600 }}>{account.name || account.id}</div>
                  <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                    {settled
                      ? t('一次性福利已全部结清')
                      : t('已得 {earned} / {total} 积分', {
                          earned: Number(data?.earned) || 0,
                          total: Number(data?.total) || 0,
                        })}
                  </div>
                </div>
                {settled ? (
                  <Badge tone='ok'>{t('已领取')}</Badge>
                ) : pending > 0 ? (
                  <Badge tone='brand'>{t('{n} 项待领', { n: pending })}</Badge>
                ) : null}
              </div>

              {error ? <ErrorNote text={error} /> : null}
              {!data && !error ? (
                <div className='m-skeleton' style={{ height: 96 }} />
              ) : null}

              {data && !tasks.length ? (
                <Empty text={t('这个账号没有新手任务')} />
              ) : null}

              {tasks.length ? (
                <div className='m-rows' style={{ borderRadius: 'var(--m-r-md)', border: '1px solid var(--m-line)' }}>
                  {tasks.map((task, index) => (
                    <div
                      key={task.key || index}
                      className='m-row'
                      style={{ alignItems: 'flex-start', opacity: task.blocked ? 0.55 : 1 }}
                    >
                      <span
                        className='m-logo'
                        style={{
                          width: 26,
                          height: 26,
                          borderRadius: 8,
                          background: task.done ? 'var(--m-ok-soft)' : 'var(--m-surface-3)',
                          borderColor: 'transparent',
                          color: task.done ? 'var(--m-ok)' : 'var(--m-text-3)',
                        }}
                      >
                        <Icon name={task.done ? 'check' : 'gift'} style={{ width: 14, height: 14 }} />
                      </span>
                      <div className='m-grow' style={{ minWidth: 0 }}>
                        <div style={{ fontSize: 13, lineHeight: 1.45 }}>{task.title || task.key || '—'}</div>
                        {task.blocked ? (
                          <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 3 }}>
                            {t('暂不可领（未满足条件）')}
                          </div>
                        ) : task.points ? (
                          <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 3 }}>
                            {t('{n} 积分', { n: task.points })}
                          </div>
                        ) : null}
                      </div>
                      <span style={{ flex: 'none', fontSize: 11.5, color: task.done ? 'var(--m-ok)' : 'var(--m-text-3)' }}>
                        {task.done ? t('已领取') : task.blocked ? t('不可领') : t('待领取')}
                      </span>
                    </div>
                  ))}
                </div>
              ) : null}

              <p style={{ margin: 0, fontSize: 11.5, lineHeight: 1.7, color: 'var(--m-text-3)' }}>
                {t('新手福利是一次性任务：领完就没了，重复点不会重复加分。')}
              </p>
            </>
          ) : null}
        </SheetBody>
        <SheetFooter style={{ flexDirection: 'column', alignItems: 'stretch', gap: 8 }}>
          <button
            type='button'
            className='m-btn'
            disabled={busy || !data || pending === 0}
            onClick={() => void claim()}
          >
            <Icon name='gift' />
            {busy
              ? t('领取中…')
              : pending > 0
                ? t('一键领取（{n}）', { n: pending })
                : t('已全部领取')}
          </button>
          <button type='button' className='m-btn m-ghost' onClick={onClose}>
            {t('关闭')}
          </button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

/* ─── ZCode 活动套餐 ───────────────────────── */

type PlanRow = ZcodePlan

export function ZcodePlansSheet({
  account,
  onClose,
  onChanged,
}: {
  account: {
    id: string
    name?: string
    provider?: string
    claimPlans?: Record<string, number> | null
  } | null
  onClose: () => void
  onChanged: () => void
}) {
  const [plans, setPlans] = useState<PlanRow[] | null>(null)
  const [deployed, setDeployed] = useState(true)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [claiming, setClaiming] = useState('')
  /**
   * 本次打开抽屉里刚领到的套餐 id。
   *
   * 不能只靠 `account.claimPlans`：那是打开抽屉那一刻的快照，领完之后父组件
   * 虽然会重拉签到中心，但这张抽屉拿到的 prop 不会跟着换 —— 按钮会一直停在
   * 「领取」上，用户会以为没领到。本地记一份，领成功立刻标上。
   */
  const [claimedNow, setClaimedNow] = useState<Set<string>>(new Set())
  const id = account?.id || ''

  /** 本地台账里「今天（北京时间）已领」的套餐 id */
  const claimedToday = (planId: string): boolean => {
    if (claimedNow.has(planId)) return true
    const ledger = account?.claimPlans
    if (!ledger || typeof ledger !== 'object') return false
    const at = Number(ledger[planId])
    return at > 0 && isToday(at)
  }

  const load = async () => {
    if (!id) return
    setError('')
    try {
      const result = await previewZcodePlans(id)
      setPlans(Array.isArray(result.plans) ? result.plans : [])
      setDeployed(result.deployed !== false)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      setPlans([])
    }
  }

  useEffect(() => {
    if (!id) return
    setPlans(null)
    setError('')
    // 换账号要清掉上一个账号的「刚领过」，否则会标错行
    setClaimedNow(new Set())
    void load()
    // 换账号才重查；load 每次渲染都会重建，不能进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  /**
   * 领一份。
   *
   * 先问上游「此刻要不要滑块」：不要就一次请求直接领；要就走验证码 ——
   * 验证串与领取请求都在 `solveCaptcha` 的 `request` 回调里完成（与桌面端
   * `ui/zcode-claim.js` 同一顺序）。
   */
  const claim = async (plan: PlanRow) => {
    if (!id || !plan.planId || claiming) return
    setClaiming(plan.planId)
    setError('')
    try {
      const config: ZcodeCaptchaConfig = await fetchZcodeCaptchaConfig(id).catch(() => ({
        enabled: false,
      }))
      const planId = plan.planId

      const submit = async (verifyParam: string) => {
        const result = await claimZcodePlan(id, {
          planId,
          captchaVerifyParam: verifyParam,
          captchaRegion: config.region || '',
        })
        if (!result.ok) {
          // 失败也是 200：`failureLabel` 是后端归一过的中文原因，直接显示
          throw new Error(result.failureLabel || result.message || t('领取失败'))
        }
        return result
      }

      if (!config.enabled) {
        await submit('')
      } else {
        await solveCaptcha(
          { region: config.region || 'ga', prefix: config.prefix, sceneId: config.sceneId },
          async verifyParam => {
            await submit(verifyParam)
            return { captchaResult: true, bizResult: true }
          },
        )
      }

      store.toast(t('已领取「{name}」', { name: plan.name || plan.planId }))
      setClaimedNow(prev => new Set(prev).add(planId))
      onChanged()
      await load()
    } catch (failure) {
      if (failure instanceof CaptchaCancelledError) {
        store.toast(t('已取消领取'))
      } else {
        setError(failure instanceof Error ? failure.message : String(failure))
      }
    } finally {
      setClaiming('')
      setBusy('')
    }
  }

  return (
    <Sheet
      open={Boolean(account)}
      onOpenChange={next => {
        if (next) return
        // 关抽屉时把还在等的滑块一起收掉，否则它会在后台超时
        cancelCaptcha()
        onClose()
      }}
    >
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('活动套餐')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {account ? (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <ProviderLogo id={account.provider || 'zcode'} />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, fontWeight: 600 }}>{account.name || account.id}</div>
                  <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                    {t('活动按自然日发新套餐，每天可领一次')}
                  </div>
                </div>
                <button
                  type='button'
                  className='m-icon-btn'
                  aria-label={t('刷新')}
                  disabled={busy === 'refresh'}
                  onClick={() => {
                    setBusy('refresh')
                    void load().finally(() => setBusy(''))
                  }}
                >
                  <Icon name='refresh' />
                </button>
              </div>

              {error ? <ErrorNote text={error} /> : null}
              {!plans && !error ? <div className='m-skeleton' style={{ height: 110 }} /> : null}

              {plans && !plans.length && !error ? (
                <Empty
                  text={
                    deployed
                      ? t('当前没有可领取的套餐')
                      : t('活动尚未开始（上游接口未部署）')
                  }
                />
              ) : null}

              {(plans || []).map(plan => {
                // 领取状态在渲染时算，不用 load 时缓存的字段：刚领完那一刻
                // claimedNow 与台账要合并判断，缓存会把新领的那份又标回「可领」
                const claimed = claimedToday(plan.planId || '')
                return (
                  <div
                    key={plan.planId}
                    className='m-card m-card-pad'
                    style={{ display: 'flex', flexDirection: 'column', gap: 8 }}
                  >
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <div style={{ flex: 1, minWidth: 0, fontSize: 13.5, fontWeight: 600 }}>
                        {plan.name || plan.planId}
                      </div>
                      {claimed ? <Badge tone='ok'>{t('今日已领')}</Badge> : null}
                    </div>

                    {plan.description ? (
                      <div style={{ fontSize: 11.8, lineHeight: 1.55, color: 'var(--m-text-2)' }}>
                        {plan.description}
                      </div>
                    ) : null}

                    {plan.entitlements?.length ? (
                      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                        {plan.entitlements.map((item, index) => (
                          <div
                            key={index}
                            style={{ fontSize: 11.5, color: 'var(--m-text-3)', display: 'flex', gap: 6 }}
                          >
                            <span style={{ color: 'var(--m-text-2)' }}>{item.showName || '—'}</span>
                            <span>
                              {[item.grantUnits, item.unitType].filter(Boolean).join(' ')}
                              {item.period ? ` · ${periodLabel(item.period)}` : ''}
                            </span>
                          </div>
                        ))}
                      </div>
                    ) : null}

                    <div style={{ fontSize: 11, color: 'var(--m-text-3)' }}>
                      {windowText(plan)}
                    </div>

                    <button
                      type='button'
                      className='m-btn m-sm'
                      disabled={claimed || Boolean(claiming)}
                      onClick={() => void claim(plan)}
                      style={{ alignSelf: 'flex-start', marginTop: 2 }}
                    >
                      <Icon name='gift' />
                      {claiming === plan.planId
                        ? t('领取中…')
                        : claimed
                          ? t('今日已领')
                          : t('领取')}
                    </button>
                  </div>
                )
              })}

              <p style={{ margin: 0, fontSize: 11.5, lineHeight: 1.7, color: 'var(--m-text-3)' }}>
                {t('领取通常需要过一次滑块验证；活动套餐多为当天稍晚生效，领取后可在桌面端「账号」里看套餐明细。')}
              </p>
            </>
          ) : null}
        </SheetBody>
        <SheetFooter>
          <button type='button' className='m-btn m-ghost' style={{ flex: 1 }} onClick={onClose}>
            {t('关闭')}
          </button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

/** 权益周期 → 文案（与后端 `period_label` 同一套措辞） */
function periodLabel(period?: string): string {
  const value = String(period || '').toLowerCase()
  if (value === 'daily') return t('每日')
  if (value === 'weekly') return t('每周')
  if (value === 'monthly') return t('每月')
  return ''
}

/** 时间窗：开始 → 到期；只有一个时只说那一个 */
function windowText(plan: ZcodePlan): string {
  const start = stamp(plan.startsAt)
  const end = stamp(plan.endsAt)
  if (start && end) return `${start} → ${end}`
  if (end) return t('到期 {time}', { time: end })
  if (start) return t('开始 {time}', { time: start })
  return ''
}

/** unix 秒 → 本地时间串（后端给的是秒） */
function stamp(seconds?: number): string {
  const value = Number(seconds)
  if (!Number.isFinite(value) || value <= 0) return ''
  return new Date(value * 1000).toLocaleString()
}
