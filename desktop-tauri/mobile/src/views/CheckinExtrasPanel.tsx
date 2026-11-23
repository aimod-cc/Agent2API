import { useState } from 'react'
import {
  activateOfficeaceSubscription,
  claimCampaignReward,
  claimOnboarding,
  claimedPlansToday,
  getCampaignRewards,
  getGrowth,
  getOfficeaceSubscription,
  getOnboarding,
  growthTaskCompleted,
  growthTaskRunnable,
  redeemCampaignBenefit,
  runGrowthSchool,
  runGrowthTask,
  runGrowthTravel,
  type CampaignEntry,
  type CampaignPreview,
  type GrowthEntry,
  type GrowthResult,
  type OfficeaceSubscriptionState,
  type OnboardingEntry,
  type OnboardingResult,
  type PlanEntry,
} from '../core/api'
import { formatDateTime, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import { CommonCard, EmptyBlock, TonalChip } from '../ui/components'
import { ProviderAvatar } from '../ui/icons'

export function OnboardingSectionPanel({
  items,
  onReload,
}: {
  items: OnboardingEntry[]
  onReload: () => Promise<void>
}) {
  const [results, setResults] = useState<Record<string, OnboardingResult>>({})
  const [busyId, setBusyId] = useState('')
  const [errors, setErrors] = useState<Record<string, string>>({})

  if (!items.length) {
    return <EmptyBlock icon='gift' title={t('暂无新手福利账号')} subtitle={t('支持新人奖励的账号会自动出现在这里')} />
  }

  const runQuery = async (id: string, refresh = false) => {
    if (busyId) return
    setBusyId(`q:${id}`)
    setErrors(prev => ({ ...prev, [id]: '' }))
    try {
      const next = await getOnboarding(id, refresh)
      setResults(prev => ({ ...prev, [id]: next }))
      if (refresh) await onReload()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setErrors(prev => ({ ...prev, [id]: msg }))
      store.toast(msg, 'danger')
    } finally {
      setBusyId('')
    }
  }

  const runClaim = async (id: string) => {
    if (busyId) return
    setBusyId(`c:${id}`)
    setErrors(prev => ({ ...prev, [id]: '' }))
    try {
      const next = await claimOnboarding(id)
      setResults(prev => ({ ...prev, [id]: next }))
      store.toast(t('已领取 {n} 项新手任务', { n: next.claimed ?? 0 }), 'success')
      await onReload()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setErrors(prev => ({ ...prev, [id]: msg }))
      store.toast(msg, 'danger')
    } finally {
      setBusyId('')
    }
  }

  return (
    <>
      {items.map(item => {
        const res = results[item.id]
        const tasks = res?.tasks || item.settled?.tasks || []
        const settled = res?.settled || Boolean(item.settled && tasks.length > 0 && tasks.every(x => x.done))
        const earned = res?.earned ?? item.settled?.earned
        const total = res?.total ?? item.settled?.total
        return (
          <CommonCard key={item.id}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <ProviderAvatar provider={item.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 15, fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {item.name || item.id}
                </div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                  {providerDisplayName(item.provider)}
                  {earned != null && total != null ? ` · ${earned}/${total}` : ''}
                </div>
              </div>
              <TonalChip tone={settled ? 'success' : 'primary'}>
                {settled ? t('已结清') : t('待领取')}
              </TonalChip>
            </div>

            {tasks.length ? (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginTop: 10 }}>
                {tasks.map(task => (
                  <div
                    key={task.key || task.title}
                    className='fc-inner-block'
                    style={{ padding: '8px 12px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
                  >
                    <span style={{ fontSize: 13 }}>{task.title || task.key}</span>
                    <TonalChip tone={task.done ? 'success' : task.blocked ? 'danger' : 'warning'}>
                      {task.done ? t('已完成') : task.blocked ? t('受限') : t('可领 {p}', { p: task.points ?? 0 })}
                    </TonalChip>
                  </div>
                ))}
              </div>
            ) : null}

            {errors[item.id] ? (
              <div style={{ fontSize: 12.5, color: 'var(--fc-danger)', marginTop: 8 }}>{errors[item.id]}</div>
            ) : null}

            <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
              <button
                type='button'
                className='fc-btn is-sm is-tonal'
                disabled={Boolean(busyId)}
                onClick={() => void runQuery(item.id, true)}
              >
                {busyId === `q:${item.id}` ? t('查询中…') : t('查询进度')}
              </button>
              <button
                type='button'
                className='fc-btn is-sm is-primary'
                disabled={Boolean(busyId) || settled}
                onClick={() => void runClaim(item.id)}
              >
                {busyId === `c:${item.id}` ? t('领取中…') : settled ? t('已结清') : t('一键领取')}
              </button>
            </div>
          </CommonCard>
        )
      })}
    </>
  )
}

export function CampaignsAndPlansSectionPanel({
  plans,
  campaigns,
  onOpenPlan,
  onReload,
}: {
  plans: PlanEntry[]
  campaigns: CampaignEntry[]
  onOpenPlan: (entry: PlanEntry) => void
  onReload: () => Promise<void>
}) {
  const [previews, setPreviews] = useState<Record<string, CampaignPreview>>({})
  const [codes, setCodes] = useState<Record<string, string>>({})
  const [subs, setSubs] = useState<Record<string, OfficeaceSubscriptionState>>({})
  const [busyKey, setBusyKey] = useState('')

  const loadCampaign = async (id: string) => {
    if (busyKey) return
    setBusyKey(`load:${id}`)
    try {
      const next = await getCampaignRewards(id)
      setPreviews(prev => ({ ...prev, [id]: next }))
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const claimReward = async (id: string, campaignId: string) => {
    if (busyKey) return
    setBusyKey(`claim:${id}:${campaignId}`)
    try {
      const res = await claimCampaignReward(id, campaignId)
      store.toast(
        res.outcome === 'claimed'
          ? t('领奖成功')
          : res.outcome === 'already_claimed'
            ? t('该奖励已领取')
            : t('已提交，请稍后刷新核对'),
        res.outcome === 'failed' ? 'danger' : 'success',
      )
      const next = await getCampaignRewards(id)
      setPreviews(prev => ({ ...prev, [id]: next }))
      await onReload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const redeemCode = async (id: string) => {
    const code = (codes[id] || '').trim()
    if (!code || busyKey) return
    setBusyKey(`redeem:${id}`)
    try {
      const res = await redeemCampaignBenefit(id, code)
      store.toast(res.message || (res.outcome === 'claimed' ? t('兑换成功') : t('已提交兑换')), res.outcome === 'failed' ? 'danger' : 'success')
      if (res.outcome === 'claimed') setCodes(prev => ({ ...prev, [id]: '' }))
      await onReload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const checkOfficeace = async (id: string) => {
    if (busyKey) return
    setBusyKey(`sub:${id}`)
    try {
      const res = await getOfficeaceSubscription(id)
      setSubs(prev => ({ ...prev, [id]: res }))
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const activateOfficeace = async (id: string) => {
    if (busyKey) return
    setBusyKey(`act:${id}`)
    try {
      const res = await activateOfficeaceSubscription(id, true)
      store.toast(res.message || t('已发起开通请求'), 'success')
      await checkOfficeace(id)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  if (!plans.length && !campaigns.length) {
    return <EmptyBlock icon='gift' title={t('暂无活动福利账号')} subtitle={t('有套餐领取或活动福利的账号会在此展示')} />
  }

  return (
    <>
      {plans.map(entry => {
        const claimedCount = claimedPlansToday(entry.claimPlans).length
        return (
          <CommonCard key={`plan:${entry.id}`}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <ProviderAvatar provider={entry.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 15, fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {entry.name || entry.id}
                </div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                  {providerDisplayName(entry.provider)}
                  {entry.claimAt ? ` · ${formatDateTime(entry.claimAt)}` : ''}
                </div>
              </div>
              {claimedCount > 0 ? <TonalChip tone='success'>{t('今日已领 {n}', { n: claimedCount })}</TonalChip> : null}
              <button type='button' className='fc-btn is-sm is-primary' onClick={() => onOpenPlan(entry)}>
                {t('套餐明细')}
              </button>
            </div>
          </CommonCard>
        )
      })}

      {campaigns.map(entry => {
        const data = previews[entry.id]
        const sub = subs[entry.id]
        return (
          <CommonCard key={`camp:${entry.id}`}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <ProviderAvatar provider={entry.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 15, fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {entry.name || entry.id}
                </div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>{providerDisplayName(entry.provider)}</div>
              </div>
              <button
                type='button'
                className='fc-btn is-sm is-tonal'
                disabled={Boolean(busyKey)}
                onClick={() => void loadCampaign(entry.id)}
              >
                {busyKey === `load:${entry.id}` ? t('查询中…') : t('查询活动')}
              </button>
            </div>

            {entry.provider === 'officeace' ? (
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, marginTop: 10 }}>
                <span style={{ fontSize: 12.5, color: 'var(--fc-text-muted)' }}>
                  {sub ? `${t('试用资格')}：${sub.status}` : t('可查询并激活免费试用订阅')}
                </span>
                <div style={{ display: 'inline-flex', gap: 6 }}>
                  <button
                    type='button'
                    className='fc-btn is-sm'
                    disabled={Boolean(busyKey)}
                    onClick={() => void checkOfficeace(entry.id)}
                  >
                    {t('查状态')}
                  </button>
                  {sub?.canActivate ? (
                    <button
                      type='button'
                      className='fc-btn is-sm is-primary'
                      disabled={Boolean(busyKey)}
                      onClick={() => void activateOfficeace(entry.id)}
                    >
                      {t('激活试用')}
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}

            {data?.campaigns?.length ? (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 10 }}>
                {data.campaigns.map(rw => {
                  const canClaim = rw.status === 'claimable'
                  const chipTone = rw.status === 'claimed' ? 'success' : canClaim ? 'primary' : 'neutral'
                  return (
                    <div
                      key={rw.campaignId}
                      className='fc-inner-block'
                      style={{ padding: '10px 12px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
                    >
                      <div style={{ minWidth: 0, flex: 1 }}>
                        <div style={{ fontSize: 13.5, fontWeight: 600 }}>{rw.title}</div>
                        {rw.description ? <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>{rw.description}</div> : null}
                      </div>
                      {canClaim ? (
                        <button
                          type='button'
                          className='fc-btn is-sm is-primary'
                          disabled={Boolean(busyKey)}
                          onClick={() => void claimReward(entry.id, rw.campaignId)}
                        >
                          {t('领取')}
                        </button>
                      ) : (
                        <TonalChip tone={chipTone}>
                          {rw.status === 'claimed' ? t('已领取') : rw.status === 'inactive' ? t('未开始') : t('不可领')}
                        </TonalChip>
                      )}
                    </div>
                  )
                })}
              </div>
            ) : data ? (
              <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginTop: 8 }}>{t('暂无可领取的活动奖励')}</div>
            ) : null}

            {data?.redemption?.supported ? (
              <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
                <input
                  className='fc-input'
                  value={codes[entry.id] || ''}
                  onChange={e => setCodes(prev => ({ ...prev, [entry.id]: e.target.value }))}
                  placeholder={t('输入兑换码')}
                />
                <button
                  type='button'
                  className='fc-btn is-primary is-sm'
                  style={{ flex: 'none' }}
                  disabled={Boolean(busyKey) || !(codes[entry.id] || '').trim()}
                  onClick={() => void redeemCode(entry.id)}
                >
                  {t('兑换')}
                </button>
              </div>
            ) : null}
          </CommonCard>
        )
      })}
    </>
  )
}

export function GrowthSectionPanel({
  items,
  onReload,
}: {
  items: GrowthEntry[]
  onReload: () => Promise<void>
}) {
  const [dataMap, setDataMap] = useState<Record<string, GrowthResult>>({})
  const [busyKey, setBusyKey] = useState('')
  const [runningAllId, setRunningAllId] = useState('')

  if (!items.length) {
    return <EmptyBlock icon='spark' title={t('暂无成长任务账号')} subtitle={t('支持成长任务、猫猫旅行或开学季活动的账号将显示于此')} />
  }

  const refreshAccount = async (id: string) => {
    if (busyKey || runningAllId) return
    setBusyKey(`load:${id}`)
    try {
      const next = await getGrowth(id)
      setDataMap(prev => ({ ...prev, [id]: next }))
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const runSingleTask = async (id: string, code: string, mode: 'full' | 'claim') => {
    if (busyKey || runningAllId) return
    setBusyKey(`task:${id}:${code}`)
    try {
      const res = await runGrowthTask(id, code, mode)
      store.toast(res.message || res.error || t('任务已执行'), res.error ? 'danger' : 'success')
      const next = await getGrowth(id)
      setDataMap(prev => ({ ...prev, [id]: next }))
      await onReload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const runBatchTasks = async (id: string) => {
    const tasks = dataMap[id]?.tasks?.tasks || []
    const runnable = tasks.filter(task => growthTaskRunnable(task, 'full'))
    if (!runnable.length || busyKey || runningAllId) return
    setRunningAllId(id)
    let okCount = 0
    try {
      for (const task of runnable) {
        try {
          const mode = growthTaskCompleted(task) ? 'claim' : 'full'
          const res = await runGrowthTask(id, task.code, mode)
          if (!res.error) okCount++
        } catch {
          // 单条失败继续跑下一条
        }
      }
      store.toast(t('成长任务完成 {ok}/{total}', { ok: okCount, total: runnable.length }), 'success')
      const next = await getGrowth(id)
      setDataMap(prev => ({ ...prev, [id]: next }))
      await onReload()
    } finally {
      setRunningAllId('')
    }
  }

  const runTravelOnce = async (id: string) => {
    if (busyKey || runningAllId) return
    setBusyKey(`travel:${id}`)
    try {
      const res = await runGrowthTravel(id)
      store.toast(res.message || res.error || t('猫猫旅行已执行'), res.error ? 'danger' : 'success')
      await refreshAccount(id)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const runSchoolOnce = async (id: string, lottery: boolean) => {
    if (busyKey || runningAllId) return
    setBusyKey(`school:${id}`)
    try {
      const res = await runGrowthSchool(id, lottery)
      store.toast(res.message || res.error || t('开学季任务已执行'), res.error ? 'danger' : 'success')
      await refreshAccount(id)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  return (
    <>
      {items.map(item => {
        const res = dataMap[item.id]
        const tasks = res?.tasks?.tasks || []
        const runnableCount = tasks.filter(task => growthTaskRunnable(task, 'full')).length
        const busy = Boolean(busyKey || runningAllId)
        return (
          <CommonCard key={item.id}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <ProviderAvatar provider={item.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 15, fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {item.name || item.id}
                </div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                  {providerDisplayName(item.provider)}
                  {res?.energy != null ? ` · ${t('能量')} ${res.energy}` : ''}
                  {res?.streak != null ? ` · ${t('连登 {n} 天', { n: res.streak })}` : ''}
                </div>
              </div>
              <button
                type='button'
                className='fc-btn is-sm is-tonal'
                disabled={busy}
                onClick={() => void refreshAccount(item.id)}
              >
                {busyKey === `load:${item.id}` ? t('刷新中…') : res ? t('刷新') : t('查询任务')}
              </button>
            </div>

            {res ? (
              <>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 10 }}>
                  {runnableCount > 0 ? (
                    <button
                      type='button'
                      className='fc-btn is-sm is-primary'
                      disabled={busy}
                      onClick={() => void runBatchTasks(item.id)}
                    >
                      {runningAllId === item.id ? t('执行中…') : t('一键完成可做 ({n})', { n: runnableCount })}
                    </button>
                  ) : null}
                  {res.travel ? (
                    <button
                      type='button'
                      className='fc-btn is-sm is-tonal'
                      disabled={busy}
                      onClick={() => void runTravelOnce(item.id)}
                    >
                      {t('猫猫旅行')}
                    </button>
                  ) : null}
                  {res.school ? (
                    <button
                      type='button'
                      className='fc-btn is-sm is-tonal'
                      disabled={busy}
                      onClick={() => void runSchoolOnce(item.id, true)}
                    >
                      {t('开学季+抽奖')}
                    </button>
                  ) : null}
                </div>

                {tasks.length ? (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginTop: 10 }}>
                    {tasks.map(task => {
                      const done = growthTaskCompleted(task)
                      const claimed = task.status === 'claimed'
                      const canRun = growthTaskRunnable(task, 'full')
                      return (
                        <div
                          key={task.code}
                          className='fc-inner-block'
                          style={{ padding: '8px 12px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
                        >
                          <div style={{ minWidth: 0, flex: 1 }}>
                            <div style={{ fontSize: 13.5, fontWeight: 600 }}>{task.title}</div>
                            {task.target ? (
                              <div style={{ fontSize: 11.5, color: 'var(--fc-text-muted)' }}>
                                {task.current ?? 0} / {task.target}
                              </div>
                            ) : null}
                          </div>
                          {claimed ? (
                            <TonalChip tone='success'>{t('已领奖')}</TonalChip>
                          ) : canRun ? (
                            <button
                              type='button'
                              className='fc-btn is-sm is-tonal'
                              disabled={busy}
                              onClick={() => void runSingleTask(item.id, task.code, done ? 'claim' : 'full')}
                            >
                              {done ? t('领奖') : t('完成')}
                            </button>
                          ) : (
                            <TonalChip>{task.locked ? t('未解锁') : t('不支持')}</TonalChip>
                          )}
                        </div>
                      )
                    })}
                  </div>
                ) : null}
              </>
            ) : null}
          </CommonCard>
        )
      })}
    </>
  )
}
