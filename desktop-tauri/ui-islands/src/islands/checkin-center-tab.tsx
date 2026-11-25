import * as React from 'react'
import {
  Button, Card, CardAction, CardFooter, CardHeader, CardTitle, Empty, SegmentedControl,
} from '@ui'
import {
  growthOf, loadCenter, runAllCheckin, store,
} from './checkin-store'
import {
  formatTime,
  type BenefitKind, type CheckinCenter, type GrowthEntry, type OnboardingEntry,
} from './checkin-types'
import { CheckinUmid } from './checkin-umid'
import { type PlansCounts } from './checkin-plans-section'
import { BenefitsPanel } from './checkin-benefits-panel'
import { ProviderRail, type ProviderOption } from './checkin-provider-rail'
import { DailyAccountList } from './checkin-daily-panel'
import { byStatus, filterOptions, type StatusFilter } from './checkin-shared'
import { onboardingOf } from './checkin-onboarding'
import { onboardingSummary } from './checkin-onboarding-cache'
import { t } from '../i18n'
import { growthTaskCompleted, growthTaskRunnable } from '../../../ui-shared/checkin'
import { accountCheckinStatus } from '../../../ui-shared/checkin-result'
import { CheckinPluginTasks } from './checkin-plugin-tasks'

/**
 * 「签到中心」标签页：总览卡 + 每日签到 + 福利与任务（新手福利、限时活动、体验套餐、成长任务）。
 *
 * ── 数据全部来自快照 ────────────────────────────────────────
 * 分组、已签判定、范围外原因都在网关算好（`api::checkin_center`）。这里只画。
 * **不要**按 provider id 写死任何判断 —— 那是插件申报的（`description` /
 * `modes` / `uiHooks` / `onboarding` / `plans` / `growth`）。
 *
 * ── 惰性块 ──────────────────────────────────────────────────
 * 新手福利与成长任务的**明细**要打上游，快照刻意不带（快照必须「零上游请求」）。
 * 行上有「查询任务」按钮，点了才查。
 */

/* ─── 空态 ─────────────────────────────────── */

/**
 * 没装插件时的空态。
 *
 * 这是「签到中心常驻菜单」的代价，也是它的价值：用户看到一个明确的说明与去处，
 * 而不是一个凭空消失的菜单项。判定依据是快照里 `checkinFeatures` 为空 ——
 * 那是插件申报表，空 = 没有任何插件提供签到能力。
 */
function EmptyState(): React.ReactElement {
  const hasFeature = Object.keys(store.center?.checkinFeatures ?? {}).length > 0
  const hasAccounts = (store.center?.daily?.providers?.length ?? 0) > 0
  const missingPlugin = !hasFeature
  const message = missingPlugin
    ? '还没有可签到的提供商。签到能力由提供商插件提供，请先到「插件」页安装 Agent2API 提供商插件。'
    : !hasAccounts
      ? '插件已安装，但还没有可签到的账号。到「账号」页添加并启用账号后，这里会自动出现。'
      : ''
  if (!message) return <></>
  return (
    <Card render={<section />} className="panel">
      <CardHeader className="panel-head">
        <h2>每日签到</h2>
      </CardHeader>
      <Empty className="ck-empty-state">
        <p>{message}</p>
        <div className="ck-empty-actions">
          {/* 用壳的 `showPage` 而不是改 location：宿主是单页应用，没有 hash 路由 */}
          <Button
            size="sm"
            variant="outline"
            onClick={() => window.wbApp?.showPage?.(missingPlugin ? 'plugins' : 'accounts')}
          >
            去「{missingPlugin ? '插件' : '账号'}」页
          </Button>
          <Button size="sm" variant="ghost" onClick={() => void loadCenter()}>
            重新检查
          </Button>
        </div>
      </Empty>
    </Card>
  )
}

/* ─── 总览指标 ─────────────────────────────── */

/**
 * 总览指标（一条紧凑横条，四格）。
 *
 * 点击行为：今日签到 → 切到「每日签到」；定时签到 → 切到「定时签到」页；
 * 福利与任务 → 切到「福利与任务」；累计记录 → 切到「签到记录」页。
 */
function MetricCell({ label, value, unit, foot, onClick }: {
  label: string
  value: React.ReactNode
  unit?: React.ReactNode
  foot?: React.ReactNode
  onClick?: () => void
}): React.ReactElement {
  const content = <>
    <span className='ck-metric-label'>{label}</span>
    <span className='ck-metric-value'>{value}{unit ? <small>{unit}</small> : null}</span>
    {foot ? <span className='ck-metric-foot'>{foot}</span> : null}
  </>
  return onClick
    ? <button type='button' className='ck-metric ck-metric-link' onClick={onClick}>{content}</button>
    : <div className='ck-metric'>{content}</div>
}

export function CheckinStats({ center }: { center: CheckinCenter }): React.ReactElement | null {
  const { daily, extras } = center
  const hasAnything = daily.providers.length > 0 || daily.outOfScope.length > 0
    || extras.onboarding.length > 0 || extras.plans.length > 0 || extras.growth.length > 0
    || (extras.campaigns?.length ?? 0) > 0
    || (center.pluginTasks?.length ?? 0) > 0
  if (!hasAnything) return null
  const auto = center.auto
  const outCount = daily.outOfScope.reduce((sum, item) => sum + item.count, 0)

  // 定时签到：启用的条目数与下一条的执行时刻
  const enabledEntries = (auto?.schedules ?? []).filter(entry => entry.enabled)
  const nextRun = enabledEntries
    .map(entry => Number(entry.nextRunAt))
    .filter(stamp => Number.isFinite(stamp) && stamp > 0)
    .sort((a, b) => a - b)[0]

  // 新手福利：已查到的账号里有几项待领
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    onboardingPending += onboardingSummary(onboardingOf(row.id)).pending
  }

  // 福利与任务：去重账号总数 + 各分类简报
  const benefitAccountIds = new Set<string>([
    ...extras.onboarding.map(row => row.id),
    ...(extras.campaigns ?? []).map(row => row.id),
    ...extras.plans.map(row => row.id),
    ...extras.growth.map(row => row.id),
  ])
  const benefitAccounts = benefitAccountIds.size
  const activityCount = (extras.campaigns?.length ?? 0) + extras.plans.length
  const benefitTasks = (center.pluginTasks ?? []).filter(
    row => row.category === 'rewards' || row.category === 'growth',
  ).length
  const benefitFootParts: string[] = []
  if (onboardingPending > 0) benefitFootParts.push(t('新手待领 {n} 项', { n: onboardingPending }))
  else if (extras.onboarding.length > 0) benefitFootParts.push(t('新手 {n}', { n: extras.onboarding.length }))
  if (activityCount > 0) benefitFootParts.push(t('活动 {n}', { n: activityCount }))
  if (extras.growth.length > 0) benefitFootParts.push(t('成长 {n}', { n: extras.growth.length }))
  if (benefitTasks > 0 && benefitAccounts === 0) benefitFootParts.push(t('自动任务 {n} 项', { n: benefitTasks }))

  const goTab = (tab: string) => window.dispatchEvent(new CustomEvent('wb:checkin-tab', { detail: { tab } }))

  return (
    <div className='ck-metrics'>
      <MetricCell
        label={t('今日签到')}
        value={daily.todayEligible > 0 ? daily.todayDone : <span className='ck-pending'>—</span>}
        unit={daily.todayEligible > 0 ? `/ ${daily.todayEligible}` : undefined}
        foot={outCount > 0
          ? t('另有 {n} 个账号不参与', { n: outCount })
          : t('全部账号均可签到')}
        onClick={() => goTab('daily')}
      />
      <MetricCell
        label={t('定时签到')}
        value={enabledEntries.length > 0 ? enabledEntries.length : <span className='ck-pending'>—</span>}
        unit={enabledEntries.length > 0 ? `/ ${auto?.schedules?.length ?? 0} 条启用` : undefined}
        foot={auto?.enabled !== true
          ? t('总开关已关闭')
          : nextRun ? t('下次 {time}', { time: formatTime(nextRun) }) : t('暂无启用的条目')}
        onClick={() => goTab('schedule')}
      />
      <MetricCell
        label={t('福利与任务')}
        value={benefitAccounts > 0
          ? benefitAccounts
          : benefitTasks > 0 ? benefitTasks : <span className='ck-pending'>{t('无')}</span>}
        unit={benefitAccounts > 0 ? t('个账号') : benefitTasks > 0 ? t('项任务') : undefined}
        foot={benefitFootParts.length > 0 ? benefitFootParts.join(' · ') : t('暂无可用的福利与任务')}
        onClick={() => goTab('benefits')}
      />
      <MetricCell
        label={t('累计签到记录')}
        value={typeof center.recordsTotal === 'number'
          ? center.recordsTotal
          : store.recordsLoaded ? store.recordsTotal : <span className='ck-pending'>—</span>}
        foot={t('每账号一条明细')}
        onClick={() => goTab('records')}
      />
    </div>
  )
}

/* ─── 主体 ─────────────────────────────────── */

/** provider id → 展示名（快照只给 id 时用共享目录兜底，取不到就原样显示 id） */
function providerLabel(id: string): string {
  return window.wbProviders?.labelOf?.(id) || id
}

export function CenterTab({ center }: { center: CheckinCenter }): React.ReactElement {
  const { daily, extras } = center
  const section = store.centerSection
  const runningAll = store.runningAll || center.auto?.running === true

  // 两个档位各有一枚状态筛选与一个「选中的提供商」，互不影响
  const [filters, setFilters] = React.useState<Record<string, StatusFilter>>({
    daily: 'all', benefits: 'all',
  })
  const [providerIds, setProviderIds] = React.useState<Record<string, string>>({
    daily: '', benefits: '',
  })
  const [benefitKind, setBenefitKind] = React.useState<BenefitKind>('all')
  // 活动福利的「已领 / 总数 / 可见数」由行内查询结果决定，所以由 PlansSection 上报
  const [plansCounts, setPlansCounts] = React.useState<PlansCounts>({
    claimed: 0, total: 0, visible: 0, claimable: 0, busy: false, byProvider: {},
  })
  const reportPlansCounts = React.useCallback((counts: PlansCounts) => {
    setPlansCounts(current => current.claimed === counts.claimed && current.total === counts.total
      && current.visible === counts.visible
      && current.claimable === counts.claimable && current.busy === counts.busy
      && JSON.stringify(current.byProvider) === JSON.stringify(counts.byProvider) ? current : counts)
  }, [])
  const filter = filters[section] ?? 'all'
  const rawProviderId = providerIds[section] ?? ''
  const setFilter = (next: StatusFilter) => setFilters(current => ({ ...current, [section]: next }))
  const setProviderId = (next: string) => setProviderIds(current => ({ ...current, [section]: next }))

  // 新手福利：还有几项待领（明细要打上游，没查过的算不出来）
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    onboardingPending += onboardingSummary(onboardingOf(row.id)).pending
  }

  /* ── 左栏：当前档位里的提供商 ── */

  const campaignRows = extras.campaigns ?? []
  const plansRows = extras.plans

  const benefitAccountMap = new Map<string, Set<string>>()
  const addBenefitAccount = (provider: string, id: string) => {
    const key = provider || ''
    let set = benefitAccountMap.get(key)
    if (!set) {
      set = new Set()
      benefitAccountMap.set(key, set)
    }
    set.add(id)
  }
  for (const row of extras.onboarding) addBenefitAccount(row.provider, row.id)
  for (const row of campaignRows) addBenefitAccount(row.provider, row.id)
  for (const row of plansRows) addBenefitAccount(row.provider, row.id)
  for (const row of extras.growth) addBenefitAccount(row.provider, row.id)

  const uniqueBenefitIds = new Set<string>([
    ...extras.onboarding.map(row => row.id),
    ...campaignRows.map(row => row.id),
    ...plansRows.map(row => row.id),
    ...extras.growth.map(row => row.id),
  ])

  const railOptions: ProviderOption[] = section === 'daily'
    ? daily.providers.map(group => ({ id: group.id, label: group.label, done: group.doneCount, total: group.totalCount }))
    : [...benefitAccountMap].map(([id, ids]) => ({
      id,
      label: providerLabel(id),
      done: null,
      total: ids.size,
    }))

  const railTaskCategories = section === 'daily' ? ['checkin'] : ['rewards', 'growth']
  for (const row of center.pluginTasks ?? []) {
    if (!railTaskCategories.includes(row.category) || railOptions.some(option => option.id === row.provider)) continue
    railOptions.push({ id: row.provider, label: row.providerLabel || providerLabel(row.provider), done: null, total: 0, isTask: true })
  }

  const providerId = rawProviderId && railOptions.some(option => option.id === rawProviderId) ? rawProviderId : ''

  const railTotal = section === 'daily'
    ? railOptions.reduce<{ done: number; all: number }>((sum, option) => ({
      done: sum.done + (option.done ?? 0), all: sum.all + option.total,
    }), { done: 0, all: 0 })
    : { done: null, all: uniqueBenefitIds.size }

  /* ── 每日签到判据 ── */

  const dutyTotal = daily.providers.reduce((sum, group) => sum + group.totalCount, 0)
  const dutyDone = daily.providers.reduce((sum, group) => sum + group.doneCount, 0)
  const dutyEntries = daily.providers
    .filter(group => !providerId || group.id === providerId)
    .map(group => ({
      group,
      accounts: byStatus(group.accounts, filter, account => accountCheckinStatus(account) === 'done'),
    }))
    .filter(entry => entry.accounts.length > 0)

  /* ── 福利与任务判据 ── */

  const onboardingDone = (row: OnboardingEntry) => onboardingSummary(onboardingOf(row.id)).claimed
  const scopedOnboarding = providerId ? extras.onboarding.filter(row => (row.provider || '') === providerId) : extras.onboarding
  const onboardingRows = byStatus(scopedOnboarding, filter, onboardingDone)
  const onboardingBusy = store.onboardingBatch !== null || onboardingRows.some(row => {
    const cache = onboardingOf(row.id)
    return cache.status === 'loading' || cache.claiming
  })

  /** 成长任务：任务树查过、且手上没有还能完成的 = 已完成 */
  const growthDone = (row: GrowthEntry) => {
    const cache = growthOf(row.id)
    if (cache?.status !== 'loaded' || cache.tasks.length === 0) return false
    return cache.tasks.filter(task => growthTaskRunnable(task, 'full'))
      .every(task => growthTaskCompleted(task))
  }
  const scopedGrowth = providerId ? extras.growth.filter(row => (row.provider || '') === providerId) : extras.growth
  const growthRows = byStatus(scopedGrowth, filter, growthDone)
  const growthBusy = store.growthBatch !== null || growthRows.some(row => {
    const cache = growthOf(row.id)
    return cache.status === 'loading' || cache.running || cache.runningSchool
  })

  const scopedCampaigns = providerId ? campaignRows.filter(row => (row.provider || '') === providerId) : campaignRows
  const scopedPlans = providerId ? plansRows.filter(row => (row.provider || '') === providerId) : plansRows
  const scopedRewardsTasks = (center.pluginTasks ?? []).filter(
    row => (!providerId || row.provider === providerId) && row.category === 'rewards',
  )
  const scopedGrowthTasks = (center.pluginTasks ?? []).filter(
    row => (!providerId || row.provider === providerId) && row.category === 'growth',
  )

  const availableBenefitKinds: Array<Exclude<BenefitKind, 'all'>> = []
  if (scopedOnboarding.length > 0) availableBenefitKinds.push('onboarding')
  if (scopedCampaigns.length > 0 || scopedPlans.length > 0 || scopedRewardsTasks.length > 0) availableBenefitKinds.push('activity')
  if (scopedGrowth.length > 0 || scopedGrowthTasks.length > 0) availableBenefitKinds.push('growth')

  const activeBenefitKind: BenefitKind = availableBenefitKinds.includes(benefitKind as Exclude<BenefitKind, 'all'>)
    ? benefitKind
    : 'all'

  const showOnboarding = activeBenefitKind === 'all' || activeBenefitKind === 'onboarding'
  const showActivity = activeBenefitKind === 'all' || activeBenefitKind === 'activity'
  const showGrowth = activeBenefitKind === 'all' || activeBenefitKind === 'growth'

  let benefitsTotal = 0
  let benefitsDone = 0
  if (showOnboarding) {
    benefitsTotal += scopedOnboarding.length
    benefitsDone += scopedOnboarding.filter(onboardingDone).length
  }
  if (showActivity) {
    const activityTotal = providerId
      ? plansCounts.byProvider[providerId]?.total ?? (scopedCampaigns.length + scopedPlans.length)
      : plansCounts.total || (campaignRows.length + plansRows.length)
    const activityDone = providerId
      ? plansCounts.byProvider[providerId]?.claimed ?? 0
      : plansCounts.claimed
    benefitsTotal += activityTotal
    benefitsDone += activityDone
  }
  if (showGrowth) {
    benefitsTotal += scopedGrowth.length
    benefitsDone += scopedGrowth.filter(growthDone).length
  }

  /* ── 标题与状态 ── */

  const dailyScopedTotal = providerId ? (railOptions.find(o => o.id === providerId)?.total ?? 0) : dutyTotal
  const dailyScopedDone = providerId
    ? daily.providers.find(group => group.id === providerId)?.doneCount ?? 0
    : dutyDone

  const headerTitle = section === 'daily' ? t('每日签到') : t('福利与任务')
  const headerSummary = section === 'daily'
    ? (daily.todayEligible > 0
      ? t('{done} / {total} 个账号已签', { done: daily.todayDone, total: daily.todayEligible })
      : t('今天没有需要签到的账号'))
    : (onboardingPending > 0
      ? t('新手福利、活动套餐与成长任务 · {n} 项待领', { n: onboardingPending })
      : t('新手福利、活动套餐与成长任务'))

  const dailyEmptyText = dailyScopedTotal === 0
    ? t('没有可签到的账号')
    : filter === 'done'
      ? t('今天还没有账号签到，切到「全部」看看哪些还没签。')
      : t('今天的账号都签完了。')

  const railVisible = railOptions.length > 0

  return (
    <div className="ck-center">
      {section === 'daily' && daily.providers.length === 0 ? <EmptyState /> : null}

      {/* 这个档位没有任何账号与任务：不给左栏（空的分类列表比一句空态更让人困惑） */}
      {railVisible || (section === 'daily' && daily.providers.length === 0) ? null : (
        <Card render={<section />} className="panel ck-task-panel">
          <CardHeader className="ck-task-head">
            <CardTitle>{headerTitle}</CardTitle>
            <span className="text-[11.5px] text-muted-foreground">{headerSummary}</span>
          </CardHeader>
          <Empty className="ck-empty">
            {section === 'daily' ? dailyEmptyText : t('没有支持福利与任务的账号，请添加账号并更新提供商插件。')}
          </Empty>
        </Card>
      )}

      {railVisible ? (
        <div className="ck-split">
          <ProviderRail
            options={railOptions}
            selected={providerId}
            total={railTotal}
            onSelect={setProviderId}
          />

          <Card render={<section />} className="panel ck-task-panel">
            <CardHeader className="ck-task-head">
              <CardTitle>{headerTitle}</CardTitle>
              <span className="text-[11.5px] text-muted-foreground">{headerSummary}</span>
              <CardAction>
                {section === 'daily' ? (
                  <>
                    <Button size="sm" disabled={runningAll} onClick={() => void runAllCheckin()}>
                      {runningAll ? t('签到中…') : t('全部签到')}
                    </Button>
                    <SegmentedControl
                      aria-label={t('按签到状态筛选')}
                      options={filterOptions(['全部', '已签到', '未签到'], dailyScopedTotal, dailyScopedDone)}
                      value={filter}
                      onValueChange={setFilter}
                    />
                  </>
                ) : (
                  <>
                    {availableBenefitKinds.length > 1 ? (
                      <SegmentedControl
                        aria-label={t('按福利类型筛选')}
                        options={[
                          { value: 'all' as const, label: t('全部') },
                          ...availableBenefitKinds.map(kind => ({
                            value: kind,
                            label: kind === 'onboarding' ? t('新手') : kind === 'activity' ? t('活动') : t('成长'),
                          })),
                        ]}
                        value={activeBenefitKind}
                        onValueChange={next => setBenefitKind(next as BenefitKind)}
                      />
                    ) : null}
                    {benefitsTotal > 0 ? (
                      <SegmentedControl
                        aria-label={t('按状态筛选')}
                        options={filterOptions(['全部', '已完成', '未完成'], benefitsTotal, benefitsDone)}
                        value={filter}
                        onValueChange={setFilter}
                      />
                    ) : null}
                  </>
                )}
              </CardAction>
            </CardHeader>

            {/* 选中某一家时，把这家插件申报的说明与 UMID 面板挂出来 */}
            {section === 'daily' && providerId ? (() => {
              const group = daily.providers.find(item => item.id === providerId)
              if (!group) return null
              const hasUmid = group.uiHooks?.includes('umid_component') === true
              if (!group.description && !hasUmid) return null
              return <div className='ck-provider-note-row'>
                {group.description ? <p>{t(group.description)}</p> : null}
                {hasUmid ? <CheckinUmid /> : null}
              </div>
            })() : null}

            {/* 只有这一块滚动：页头、指标、分类与左栏都固定 */}
            <div className="ck-task-scroll">
              {section === 'daily' ? (
                <>
                  <DailyAccountList entries={dutyEntries} showProvider={!providerId} />
                  <div className='px-4 pt-3'>
                    <CheckinPluginTasks center={center} provider={providerId} categories={['checkin']} />
                  </div>
                  {dutyEntries.length === 0 ? <Empty className="ck-empty">{dailyEmptyText}</Empty> : null}
                </>
              ) : (
                <BenefitsPanel
                  center={center}
                  providerId={providerId}
                  filter={filter}
                  onFilterChange={setFilter}
                  benefitKind={activeBenefitKind}
                  onboardingRows={onboardingRows}
                  onboardingBusy={onboardingBusy}
                  growthRows={growthRows}
                  growthBusy={growthBusy}
                  plansCounts={plansCounts}
                  onPlansCountsChange={reportPlansCounts}
                />
              )}
            </div>

            <CardFooter className="ck-task-foot">
              {section === 'daily'
                ? t('单账号签到不受范围限制')
                : t('点击账号行展开明细 · 资格、额度与有效期以官方返回为准，不会购买或订阅套餐')}
            </CardFooter>
          </Card>
        </div>
      ) : null}
    </div>
  )
}
