import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  accountCheckinStatus,
  CHECKIN_SECTION_OPTIONS,
  checkinActionLabel,
  checkinPlanProgress,
  checkinStatusLabel,
  checkinStatusTone,
  checkinSummary,
  getFullCheckinCenter,
  modesOf,
  rowCompleted,
  rowMessage,
  runCheckin,
  runCheckinActivity,
  type CheckinAccount,
  type CheckinCenter,
  type CheckinRecordFilters,
  type CheckinResult,
  type CheckinSection,
  type PlanEntry,
} from '../core/api'
import { formatDateTime, syncProviderLabels } from '../core/format'
import { t } from '../core/i18n'
import { store, useStore } from '../core/store'
import {
  CommonCard,
  CommonScaffold,
  ConnectedButtonGroup,
  DashboardCard,
  EmptyBlock,
  SectionHeader,
  SegmentedControl,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { ProviderAvatar } from '../ui/icons'
import {
  CampaignsAndPlansSectionPanel,
  GrowthSectionPanel,
  OnboardingSectionPanel,
} from './CheckinExtrasPanel'
import { CheckinPlansSheet } from './CheckinPlansSheet'
import { CheckinActivitySheet } from './CheckinActivitySheet'
import { CheckinRecordsPanel, CheckinRecordsStickyControls } from './CheckinRecordsPanel'
import { CheckinSchedulePanel } from './CheckinSchedulePanel'

type MainTab = 'daily' | 'schedules' | 'records'

const MODE_LABELS: Record<string, string> = {
  checkin: '签到',
  keepalive: '保活',
  claim: '领取',
  keepalive_claim: '保活+领取',
}

export function CheckinView() {
  const { navTabs } = useStore()
  const [mainTab, setMainTab] = useState<MainTab>('daily')
  const [section, setSection] = useState<CheckinSection>('daily')
  const [center, setCenter] = useState<CheckinCenter | null>(null)
  const [loading, setLoading] = useState(true)
  const [busyKey, setBusyKey] = useState('')
  const [lastResult, setLastResult] = useState<CheckinResult | null>(null)
  const [modes, setModes] = useState<Record<string, string>>({})
  const [activePlan, setActivePlan] = useState<PlanEntry | null>(null)
  const [inspectAccount, setInspectAccount] = useState<CheckinAccount | null>(null)
  const [recordFilters, setRecordFilters] = useState<CheckinRecordFilters>({
    range: 'today',
    outcome: 'all',
    kind: 'all',
    provider: 'all',
    keyword: '',
  })
  const [clearRecordsOpen, setClearRecordsOpen] = useState(false)
  const [recordsNonce, setRecordsNonce] = useState(0)

  const onBack = navTabs.includes('checkin') ? undefined : () => store.returnFromSubPage()

  const loadCenter = useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    try {
      const next = await getFullCheckinCenter(signal)
      syncProviderLabels(next.auto?.providerOptions)
      setCenter(next)
    } catch (err) {
      if (!signal?.aborted) {
        store.toast(err instanceof Error ? err.message : String(err), 'danger')
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  useEffect(() => {
    const ac = new AbortController()
    void loadCenter(ac.signal)
    return () => ac.abort()
  }, [loadCenter])

  const refreshAll = async () => {
    await loadCenter()
    if (mainTab === 'records') setRecordsNonce(n => n + 1)
    void store.refresh(true)
  }

  const sectionOptions = useMemo(() => {
    if (!center) return [{ id: 'daily' as CheckinSection, label: t('每日签到') }]
    return CHECKIN_SECTION_OPTIONS.filter(opt => {
      if (opt.value === 'daily') return true
      if (opt.value === 'onboarding') return center.extras.onboarding.length > 0
      if (opt.value === 'plans') return center.extras.plans.length > 0 || (center.extras.campaigns?.length || 0) > 0
      if (opt.value === 'growth') return center.extras.growth.length > 0
      return false
    }).map(opt => ({ id: opt.value as CheckinSection, label: t(opt.label) }))
  }, [center])

  useEffect(() => {
    if (!sectionOptions.some(o => o.id === section)) {
      setSection('daily')
    }
  }, [section, sectionOptions])

  const triggerCheckinAll = async () => {
    if (busyKey) return
    setBusyKey('all')
    try {
      const res = await runCheckin()
      setLastResult(res)
      const s = checkinSummary(res)
      store.toast(
        t('执行完成：成功 {ok} · 跳过 {skip} · 失败 {fail}', {
          ok: s.succeeded,
          skip: s.skipped,
          fail: s.failed,
        }),
        s.failed > 0 ? 'danger' : 'success',
      )
      await refreshAll()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const triggerSingle = async (accountId: string, mode: string) => {
    if (busyKey) return
    setBusyKey(`acc:${accountId}`)
    try {
      if (mode && mode !== 'checkin') {
        const row = await runCheckinActivity(accountId, mode)
        const ok = rowCompleted(row)
        setLastResult({
          succeeded: ok ? 1 : 0,
          skipped: 0,
          failedCount: ok ? 0 : 1,
          results: [row],
        })
        store.toast(rowMessage(row) || (ok ? t('执行成功') : t('执行完成')), ok ? 'success' : 'danger')
      } else {
        const res = await runCheckin(accountId)
        setLastResult(res)
        const s = checkinSummary(res)
        const firstRow = res.results?.[0]
        const firstMsg = (firstRow as { error?: string } | undefined)?.error || firstRow?.message
        store.toast(
          s.failed > 0
            ? firstMsg || t('签到未成功')
            : s.succeeded > 0
              ? t('签到成功')
              : t('今日已签到'),
          s.failed > 0 ? 'danger' : 'success',
        )
      }
      await refreshAll()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const plansByAccount = useMemo(() => {
    const map = new Map<string, PlanEntry>()
    for (const p of center?.extras?.plans || []) map.set(p.id, p)
    return map
  }, [center?.extras?.plans])

  const badgeText = center ? `${center.daily.todayDone}/${center.daily.todayEligible}` : undefined

  const stickyHeader = (
    <>
      <SegmentedControl<MainTab>
        value={mainTab}
        onChange={setMainTab}
        options={[
          { id: 'daily', label: t('每日签到') },
          { id: 'schedules', label: t('定时计划') },
          { id: 'records', label: t('签到记录') },
        ]}
      />
      {mainTab === 'daily' && sectionOptions.length > 1 ? (
        <ConnectedButtonGroup<CheckinSection>
          value={section}
          onChange={setSection}
          options={sectionOptions}
        />
      ) : null}
      {mainTab === 'records' && center ? (
        <CheckinRecordsStickyControls
          filters={recordFilters}
          providerOptions={center.auto.providerOptions}
          onChange={patch => setRecordFilters(prev => ({ ...prev, ...patch }))}
          onClearRequest={() => setClearRecordsOpen(true)}
        />
      ) : null}
    </>
  )

  return (
    <CommonScaffold
      title={t('签到中心')}
      badge={badgeText ? <TonalChip tone='primary'>{badgeText}</TonalChip> : undefined}
      onBack={onBack}
      stickyHeader={stickyHeader}
      actions={
        <TonalIconButton
          icon='refresh'
          title={t('刷新')}
          disabled={loading}
          onClick={() => void refreshAll()}
        />
      }
    >
      {loading && !center ? (
        <EmptyBlock icon='calendar' title={t('正在加载签到中心…')} />
      ) : !center ? (
        <EmptyBlock
          icon='alert'
          title={t('无法读取签到中心')}
          action={
            <button type='button' className='fc-btn is-primary is-sm' onClick={() => void loadCenter()}>
              {t('重试')}
            </button>
          }
        />
      ) : mainTab === 'daily' ? (
        section === 'onboarding' ? (
          <OnboardingSectionPanel items={center.extras.onboarding} onReload={() => loadCenter()} />
        ) : section === 'plans' ? (
          <CampaignsAndPlansSectionPanel
            plans={center.extras.plans}
            campaigns={center.extras.campaigns || []}
            onOpenPlan={setActivePlan}
            onReload={() => loadCenter()}
          />
        ) : section === 'growth' ? (
          <GrowthSectionPanel items={center.extras.growth} onReload={() => loadCenter()} />
        ) : (
          <>
            <CommonCard>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
                <div>
                  <div style={{ fontSize: 13, color: 'var(--fc-text-muted)', fontWeight: 600 }}>{t('今日进度')}</div>
                  <div style={{ fontSize: 24, fontWeight: 800, marginTop: 2 }}>
                    {center.daily.todayDone}
                    <span style={{ fontSize: 14, color: 'var(--fc-text-muted)', fontWeight: 600 }}>
                      {' '}/ {center.daily.todayEligible}
                    </span>
                  </div>
                </div>
                <button
                  type='button'
                  className='fc-btn is-primary is-pill'
                  disabled={Boolean(busyKey)}
                  onClick={() => void triggerCheckinAll()}
                >
                  {busyKey === 'all' ? t('签到中…') : t('全部签到')}
                </button>
              </div>
            </CommonCard>

            {lastResult?.results?.length ? (
              <CommonCard>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 6 }}>
                  <span style={{ fontSize: 13.5, fontWeight: 700 }}>{t('本次执行结果')}</span>
                  <button
                    type='button'
                    className='fc-btn is-sm'
                    onClick={() => setLastResult(null)}
                  >
                    {t('收起')}
                  </button>
                </div>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                  {lastResult.results.slice(0, 8).map((row, idx) => (
                    <div
                      key={row.id || idx}
                      className='fc-inner-block'
                      style={{ padding: '8px 10px', display: 'flex', justifyContent: 'space-between', gap: 8 }}
                    >
                      <span style={{ fontSize: 12.5, fontWeight: 600 }}>{row.name || row.id}</span>
                      <span style={{ fontSize: 12, color: rowCompleted(row) ? 'var(--fc-success)' : 'var(--fc-danger)' }}>
                        {rowMessage(row) || (rowCompleted(row) ? t('已完成') : t('待处理'))}
                      </span>
                    </div>
                  ))}
                </div>
              </CommonCard>
            ) : null}

            {!center.daily.providers.length ? (
              <EmptyBlock icon='calendar' title={t('当前暂无支持每日签到的账号')} />
            ) : (
              center.daily.providers.map(group => {
                const availModes = modesOf(group)
                const hasActivityModes = availModes.some(m => m === 'keepalive' || m === 'claim' || m === 'full')
                const currentMode = modes[group.id] || availModes[0] || 'checkin'
                return (
                  <DashboardCard
                    key={group.id}
                    icon={<ProviderAvatar provider={group.id} />}
                    title={group.label}
                    subtitle={group.description || undefined}
                    trailing={
                      <TonalChip tone={group.doneCount === group.totalCount && group.totalCount > 0 ? 'success' : 'neutral'}>
                        {`${group.doneCount}/${group.totalCount}`}
                      </TonalChip>
                    }
                  >
                    {availModes.length > 1 ? (
                      <div style={{ marginBottom: 10 }}>
                        <ConnectedButtonGroup
                          inCard
                          value={currentMode}
                          onChange={next => setModes(prev => ({ ...prev, [group.id]: next }))}
                          options={availModes.map(m => ({ id: m, label: t(MODE_LABELS[m] || m) }))}
                        />
                      </div>
                    ) : null}
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      {group.accounts.map(acc => {
                        const status = accountCheckinStatus(acc)
                        const chipTone = checkinStatusTone(status)
                        const mappedTone = chipTone === 'ok' ? 'success' : chipTone === 'warn' ? 'warning' : chipTone === 'bad' ? 'danger' : chipTone === 'brand' ? 'primary' : 'neutral'
                        const planEntry = group.manualPlans ? plansByAccount.get(acc.id) || { id: acc.id, name: acc.name, provider: group.id, claimAt: null, claimPlans: null } : null
                        const actionLabel =
                          currentMode !== 'checkin'
                            ? t(MODE_LABELS[currentMode] || currentMode)
                            : t(checkinActionLabel(acc))
                        return (
                          <div
                            key={acc.id}
                            className='fc-inner-block'
                            style={{ padding: '10px 12px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
                          >
                            <div style={{ minWidth: 0, flex: 1 }}>
                              <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                                <span style={{ fontSize: 14, fontWeight: 600 }}>{acc.name}</span>
                                <TonalChip tone={mappedTone}>{t(checkinStatusLabel(status))}</TonalChip>
                                {acc.streakDays > 0 ? <TonalChip>{t('连签 {n} 天', { n: acc.streakDays })}</TonalChip> : null}
                              </div>
                              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                                {acc.checkinAt ? `${t('上次')}: ${formatDateTime(acc.checkinAt)}` : t('今日未签到')}
                                {checkinPlanProgress(acc.dailyCheckin)
                                  ? ` · ${acc.dailyCheckin!.completedCount}/${acc.dailyCheckin!.totalCount}`
                                  : ''}
                              </div>
                            </div>
                            <div style={{ display: 'inline-flex', gap: 6, flex: 'none' }}>
                              {planEntry ? (
                                <button
                                  type='button'
                                  className='fc-btn is-sm'
                                  onClick={() => setActivePlan(planEntry)}
                                >
                                  {t('套餐')}
                                </button>
                              ) : null}
                              {hasActivityModes ? (
                                <button
                                  type='button'
                                  className='fc-btn is-sm'
                                  onClick={() => setInspectAccount(acc)}
                                >
                                  {t('详情')}
                                </button>
                              ) : null}
                              <button
                                type='button'
                                className={`fc-btn is-sm ${acc.checkedInToday && currentMode === 'checkin' ? 'is-tonal' : 'is-primary'}`}
                                disabled={Boolean(busyKey) || !acc.available}
                                onClick={() => void triggerSingle(acc.id, currentMode)}
                              >
                                {busyKey === `acc:${acc.id}` ? t('执行中') : actionLabel}
                              </button>
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  </DashboardCard>
                )
              })
            )}
            <SectionHeader title={t('提示')} />
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', padding: '0 4px' }}>
              {t('可切换上方「定时计划」管理自动签到时段，或在「签到记录」查看详细执行明细。')}
            </div>
          </>
        )
      ) : mainTab === 'schedules' ? (
        <CheckinSchedulePanel center={center} onReload={() => loadCenter()} />
      ) : (
        <CheckinRecordsPanel
          filters={recordFilters}
          clearPromptOpen={clearRecordsOpen}
          onCloseClearPrompt={() => setClearRecordsOpen(false)}
          refreshNonce={recordsNonce}
        />
      )}

      <CheckinPlansSheet
        entry={activePlan}
        onClose={() => setActivePlan(null)}
        onChange={() => void loadCenter()}
      />
      <CheckinActivitySheet
        account={inspectAccount}
        onClose={() => setInspectAccount(null)}
      />
    </CommonScaffold>
  )
}
