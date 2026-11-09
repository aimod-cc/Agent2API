import * as React from 'react'
import {
  Badge, Button, Card, CardAction, CardFooter, CardHeader, CardTitle, Empty, SegmentedControl,
} from '@ui'
import {
  growthOf, loadCenter, queryGrowth, runAllCheckin, runGrowth, runGrowthBatch,
  runSchool, runTravelOnce, store,
} from './checkin-store'
import {
  formatShort, formatTime,
  type CheckinCenter, type GrowthEntry, type OnboardingEntry,
} from './checkin-types'
import { CheckinRow } from './checkin-row'
import { CheckinUmid } from './checkin-umid'
import { PlansSection, type PlansCounts } from './checkin-plans-section'
import { OfficeaceSubscriptionRow } from './checkin-officeace-row'
import { ProviderRail, type ProviderOption } from './checkin-provider-rail'
import { DailyAccountList } from './checkin-daily-panel'
import { DetailLine, TaskTick, byStatus, filterOptions, type StatusFilter } from './checkin-shared'
import { claimOnboarding, claimOnboardingBatch, onboardingClaimable, onboardingOf, queryOnboarding } from './checkin-onboarding'
import { onboardingSummary } from './checkin-onboarding-cache'
import { t } from '../i18n'
import { growthTaskCompleted, growthTaskRunnable } from '../../../ui-shared/checkin'
import { accountCheckinStatus } from '../../../ui-shared/checkin-result'

/**
 * 「签到中心」标签页：总览卡 + 每日签到 + 新手福利 + 活动套餐 + 成长任务。
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
 * 早先是四张独立卡片（每张 130px 上下）：标题一行、大数字一行、脚注一行，
 * 一屏里近三分之一的高度被总览吃掉，下面的账号列表反而要滚动。现在压成一条
 * 横条 —— 标题与数字同一行、脚注缩成一行小灰字，高度约 60px。
 *
 * 点击行为与旧卡片一致：定时签到 → 切到「定时签到」页；待领新手福利 → 切到
 * 新手福利档；累计记录 → 切到「签到记录」页。
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
  let onboardingChecked = 0
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    const summary = onboardingSummary(onboardingOf(row.id))
    if (summary.checked) onboardingChecked += 1
    onboardingPending += summary.pending
  }

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
        label={t('待领新手福利')}
        value={extras.onboarding.length > 0
          ? onboardingChecked > 0 ? onboardingPending : <span className='ck-pending'>—</span>
          : <span className='ck-pending'>无</span>}
        unit={extras.onboarding.length > 0 && onboardingChecked > 0 ? t('项') : undefined}
        foot={extras.onboarding.length > 0
          ? t('已查 {checked} / {total} 个账号', { checked: onboardingChecked, total: extras.onboarding.length })
          : t('没有支持新手福利的账号')}
        onClick={() => goTab('onboarding')}
      />
      <MetricCell
        label={t('累计签到记录')}
        value={store.recordsLoaded ? store.recordsTotal : <span className='ck-pending'>—</span>}
        foot={t('每账号一条明细')}
        onClick={() => goTab('records')}
      />
    </div>
  )
}

/* ─── 每日签到 ─────────────────────────────── */

// 账号行、状态徽标、明细行都在 `checkin-daily-panel.tsx`：左栏改成提供商分类后，
// 右栏直接列账号（不再「每家一行、点开展开」），那一套组件与主文件是两件事。

/* ─── 新手福利 ─────────────────────────────── */

/**
 * 新手福利行。
 *
 * 明细要打上游，快照不带 —— 所以这里的按钮是「查询任务」，点了才查。
 * 状态缓存在 `store.onboarding`（跨标签页保留，切走再回来不用重查）。
 */
function OnboardingRow({ row }: { row: OnboardingEntry }): React.ReactElement {
  const cache = onboardingOf(row.id)
  const [expanded, setExpanded] = React.useState(false)
  const tasks = cache.tasks
  const { claimed, pending: unclaimed } = onboardingSummary(cache)
  const status = cache.status
  const batchClaiming = store.onboardingBatch?.includes(row.id) === true

  return (
    <CheckinRow provider={row.provider} name={row.name}
      expandable={tasks.length > 0} expanded={expanded} onExpandedChange={setExpanded}
      description={<>
        {cache.settled
          ? row.provider === 'officeace' ? t('今天已领') : t('一次性福利已结清，无需重复查询')
          : status === 'loaded'
            ? `已领 ${tasks.filter(task => task.done).length}/${tasks.length} · 累计 ${cache.earned} 积分`
            : status === 'loading' ? '正在查询…' : '尚未查询任务状态'}
        {cache.settledAt ? ` · ${t('结算于 {time}', { time: formatTime(cache.settledAt) })}`
          : cache.checkedAt ? ` · 查询于 ${formatTime(cache.checkedAt)}` : ''}
      </>}
      summary={claimed
        ? <Badge variant="success" shape="tag">已领取</Badge>
        : status === 'loaded' && unclaimed > 0
          ? <Badge variant="warning" shape="tag">{unclaimed} 项待领</Badge>
          : status === 'loaded'
            ? <Badge variant="outline" shape="tag">{t('暂无可领取福利')}</Badge>
            : status === 'error'
              ? <Badge variant="destructive" shape="tag">查询失败</Badge>
              : null}
      note={row.provider === 'officeace' || status === 'error' ? <>
        {status === 'error' && cache.error ? <p>{t('查询失败：{error}', { error: cache.error })}</p> : null}
        {row.provider === 'officeace' ? <OfficeaceSubscriptionRow accountId={row.id}
          disabled={batchClaiming || status === 'loading' || cache.claiming} /> : null}
      </> : undefined}
      actions={<>
        <Button
          size="sm"
          variant="outline"
          disabled={batchClaiming || status === 'loading' || cache.claiming}
          onClick={() => void queryOnboarding(row.id, true)}
        >
          {status === 'loading' ? '查询中…' : '查询任务'}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={batchClaiming || !onboardingClaimable(row.id)}
          onClick={() => void claimOnboarding(row.id)}
        >
          {cache?.claiming ? '领取中…' : unclaimed > 0 ? `一键领取（${unclaimed}）` : '领取'}
        </Button>
      </>}>
      {tasks.map(task => (
        <DetailLine key={task.key}>
          <TaskTick done={task.done} />
          <span className='min-w-0 flex-1 truncate'>{task.title}</span>
          <span className='shrink-0 tabular-nums text-muted-foreground'>+{task.points}</span>
          {task.claiming
            ? <Badge variant="warning" shape="tag">{t('领取中')}</Badge>
            : task.done
              ? <Badge variant="success" shape="tag">{t('已领取')}</Badge>
              : task.blocked
                ? <Badge variant="outline" shape="tag">{t('暂不可领')}</Badge>
                : task.error
                  ? <Badge variant="destructive" shape="tag">{task.error}</Badge>
                  : <Badge variant="brand" shape="tag">{t('可领取')}</Badge>}
        </DetailLine>
      ))}
    </CheckinRow>
  )
}

/* ─── 活动福利 ─────────────────────────────── */

// 这一档整体搬到了 `checkin-plans-section.tsx` —— 它比另外三档多出「全部 / 未领取 /
// 已领取」筛选，行也换成了「行上是摘要、明细收进展开区」，单独一个文件更清楚。

/* ─── 成长任务（WorkBuddy 国内版）────────────── */

/**
 * 成长任务的一行。
 *
 * 三块内容：任务树（逐条点亮 / 领奖）、开学季（任务 + 抽奖）、猫猫旅行。
 * 明细要打一次上游，所以行上是「查询任务」按钮，点了才查。
 *
 * 「一键完成」逐条调用单任务端点（见 `runGrowth` 的说明）：全量一次请求会
 * 超过管理 API 的 60 秒转发超时。
 */
function GrowthRow({ row }: { row: GrowthEntry }): React.ReactElement {
  const cache = growthOf(row.id)
  const tasks = cache.tasks ?? []
  const runnable = tasks.filter(task => growthTaskRunnable(task, 'full'))
  const pending = runnable.filter(task => !growthTaskCompleted(task))
  const loaded = cache.status === 'loaded' && tasks.length > 0
  const [expanded, setExpanded] = React.useState(false)
  const running = cache.running || cache.runningSchool || store.growthBatch?.accountIds.includes(row.id) === true

  return (
    <CheckinRow provider={row.provider || 'workbuddy'} name={row.name || row.id}
      expandable={loaded} expanded={expanded} onExpandedChange={setExpanded}
      description={<>
        {cache.status === 'loaded'
          ? `${runnable.length} 项任务 · ${pending.length} 项可完成`
          : '查询任务树并一键点亮 / 领奖（含开学季与猫猫旅行）'}
        {cache.checkedAt ? ` · 查询于 ${formatTime(cache.checkedAt)}` : ''}
      </>}
      summary={cache.status === 'loaded'
        ? pending.length > 0
          ? <Badge variant="warning" shape="tag">{pending.length} 项可完成</Badge>
          : <Badge variant="success" shape="tag">全部完成</Badge>
        : cache.status === 'error'
          ? <Badge variant="destructive" shape="tag">查询失败</Badge>
          : null}
      note={cache.status === 'error' ? `查询失败：${cache.error}` : undefined}
      actions={<>
        <Button
          size="sm"
          variant="outline"
          disabled={cache.status === 'loading' || running}
          onClick={() => void queryGrowth(row.id)}
        >
          {cache.status === 'loading' ? '查询中…' : '查询任务'}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={cache.status !== 'loaded' || running}
          title="接单 + 点亮 + 领奖（只处理已实测沉淀的任务）"
          onClick={() => void runGrowth(row.id, 'full')}
        >
          {cache.running ? '执行中…' : '一键完成'}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={cache.status !== 'loaded' || running}
          title="只领取已完成任务的奖励（幂等，重复点不会重复入账）"
          onClick={() => void runGrowth(row.id, 'claim')}
        >
          领取
        </Button>
      </>}>
      <div className='border-b border-hairline pb-1.5 text-[11.5px] text-muted-foreground'>
        {cache.energy !== null && cache.energy !== undefined ? t('能量 {value}', { value: cache.energy }) : ''}
        {cache.streak !== null && cache.streak !== undefined ? ` · ${t('连登 {days} 天', { days: cache.streak })}` : ''}
        {cache.energy === null && cache.streak === null ? t('（能量 / 连登读数本次未取到）') : ''}
      </div>
      {tasks.map(task => {
        const result = cache.results.find(item => item.code === task.code)
        return (
          <DetailLine key={task.code}>
            <TaskTick done={task.status === 'claimed'} />
            <span className='min-w-0 flex-1 truncate' title={task.description || task.source || task.code}>
              {task.title || task.code}
              {task.target > 1 ? ` (${task.current}/${task.target})` : ''}
            </span>
            {result ? <span className='shrink-0 text-[11.5px] text-muted-foreground'>{result.message}</span> : null}
            {task.status === 'claimed'
              ? <Badge variant="success" shape="tag">{t('已领取')}</Badge>
              : growthTaskRunnable(task, 'claim')
                ? <Badge variant="brand" shape="tag">{t('可领取')}</Badge>
                : task.unforgeable
                  ? <Badge variant="outline" shape="tag">{t('需真实操作')}</Badge>
                  : !task.supported
                    ? <Badge variant="outline" shape="tag">{t('不支持')}</Badge>
                    : task.locked
                      ? <Badge variant="outline" shape="tag">{t('未解锁')}</Badge>
                      : <Badge variant="warning" shape="tag">{t('待完成')}</Badge>}
          </DetailLine>
        )
      })}
      {/* 开学季与猫猫旅行：各自一个动作按钮（不走任务树的逐条执行） */}
      <div className='mt-1.5 flex flex-wrap items-center gap-2 border-t border-hairline pt-2'>
        <Button
          size="xs"
          variant="outline"
          disabled={running}
          onClick={() => void runSchool(row.id, false)}
        >
          {cache.runningSchool ? t('执行中…') : t('开学季任务')}
        </Button>
        <Button
          size="xs"
          variant="outline"
          disabled={running}
          onClick={() => void runSchool(row.id, true)}
        >
          {t('开学季 + 抽奖')}
        </Button>
        <Button
          size="xs"
          variant="outline"
          disabled={running}
          onClick={() => void runTravelOnce(row.id)}
        >
          {t('猫猫旅行一趟')}
        </Button>
      </div>
    </CheckinRow>
  )
}

/* ─── 主体 ─────────────────────────────────── */

/** provider id → 展示名（快照只给 id 时用共享目录兜底，取不到就原样显示 id） */
function providerLabel(id: string): string {
  return window.wbProviders?.labelOf?.(id) || id
}

/** 一组账号按提供商归类（新手福利 / 成长任务共用；每日签到直接有 providerGroups） */
function groupByProvider(rows: Array<{ provider?: string }>): Array<{ id: string; rows: typeof rows }> {
  const map = new Map<string, typeof rows>()
  for (const row of rows) {
    const id = row.provider || ''
    const list = map.get(id)
    if (list) list.push(row)
    else map.set(id, [row])
  }
  return [...map].map(([id, list]) => ({ id, rows: list }))
}

export function CenterTab({ center }: { center: CheckinCenter }): React.ReactElement {
  const { daily, extras } = center
  const section = store.centerSection
  const runningAll = store.runningAll || center.auto?.running === true

  // 每个任务各有一枚状态筛选与一个「选中的提供商」，互不影响
  const [filters, setFilters] = React.useState<Record<string, StatusFilter>>({
    daily: 'all', onboarding: 'all', plans: 'all', growth: 'all',
  })
  const [providerIds, setProviderIds] = React.useState<Record<string, string>>({
    daily: '', onboarding: '', plans: '', growth: '',
  })
  // 「全部查询」按钮的令牌：活动福利的行组件监听它补查一次
  const [plansQueryToken, setPlansQueryToken] = React.useState(0)
  const [plansClaimToken, setPlansClaimToken] = React.useState(0)
  // 活动福利的「已领 / 总数」由行内查询结果决定，所以由 PlansSection 上报
  const [plansCounts, setPlansCounts] = React.useState<PlansCounts>({ claimed: 0, total: 0, claimable: 0, busy: false, byProvider: {} })
  const reportPlansCounts = React.useCallback((counts: PlansCounts) => {
    setPlansCounts(current => current.claimed === counts.claimed && current.total === counts.total
      && current.claimable === counts.claimable && current.busy === counts.busy
      && JSON.stringify(current.byProvider) === JSON.stringify(counts.byProvider) ? current : counts)
  }, [])
  const filter = filters[section] ?? 'all'
  const providerId = providerIds[section] ?? ''
  const setFilter = (next: StatusFilter) => setFilters(current => ({ ...current, [section]: next }))
  const setProviderId = (next: string) => setProviderIds(current => ({ ...current, [section]: next }))

  // 新手福利：查过几个账号、其中还有几项待领（明细要打上游，没查过的算不出来）
  let onboardingChecked = 0
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    const summary = onboardingSummary(onboardingOf(row.id))
    if (summary.checked) onboardingChecked += 1
    onboardingPending += summary.pending
  }

  /* ── 三个档位的判据（判据都在这里，行组件只负责画）── */

  const dutyTotal = daily.providers.reduce((sum, group) => sum + group.totalCount, 0)
  const dutyDone = daily.providers.reduce((sum, group) => sum + group.doneCount, 0)
  const dutyEntries = daily.providers
    .filter(group => !providerId || group.id === providerId)
    .map(group => ({
      group,
      accounts: byStatus(group.accounts, filter, account => accountCheckinStatus(account) === 'done'),
    }))
    .filter(entry => entry.accounts.length > 0)

  const onboardingDone = (row: OnboardingEntry) => onboardingSummary(onboardingOf(row.id)).claimed
  const onboardingGroups = groupByProvider(extras.onboarding)
  const onboardingDoneCount = extras.onboarding.filter(onboardingDone).length
  const onboardingRows = byStatus(
    providerId ? extras.onboarding.filter(row => (row.provider || '') === providerId) : extras.onboarding,
    filter, onboardingDone,
  )
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
  const growthGroups = groupByProvider(extras.growth)
  const growthDoneCount = extras.growth.filter(growthDone).length
  const growthRows = byStatus(
    providerId ? extras.growth.filter(row => (row.provider || '') === providerId) : extras.growth,
    filter, growthDone,
  )
  const growthBusy = store.growthBatch !== null || growthRows.some(row => {
    const cache = growthOf(row.id)
    return cache.status === 'loading' || cache.running || cache.runningSchool
  })

  /* ── 左栏：当前任务里的提供商（切任务时跟着变）── */

  const campaignRows = extras.campaigns ?? []
  const plansRows = extras.plans
  const planGroups = groupByProvider([...campaignRows, ...plansRows])

  const railOptions: ProviderOption[] = section === 'daily'
    ? daily.providers.map(group => ({ id: group.id, label: group.label, done: group.doneCount, total: group.totalCount }))
    : section === 'onboarding'
      ? onboardingGroups.map(group => ({
        id: group.id, label: providerLabel(group.id),
        done: group.rows.filter(row => onboardingDone(row as OnboardingEntry)).length, total: group.rows.length,
      }))
      : section === 'growth'
        ? growthGroups.map(group => ({
          id: group.id, label: providerLabel(group.id),
          done: group.rows.filter(row => growthDone(row as GrowthEntry)).length, total: group.rows.length,
        }))
        : planGroups.map(group => ({ id: group.id, label: providerLabel(group.id), done: null, total: group.rows.length }))

  const railTotal = railOptions.reduce<{ done: number; all: number }>((sum, option) => ({
    done: sum.done + (option.done ?? 0), all: sum.all + option.total,
  }), { done: 0, all: 0 })

  const sectionMeta = {
    daily: {
      title: t('每日签到'),
      summary: daily.todayEligible > 0
        ? t('{done} / {total} 个账号已签', { done: daily.todayDone, total: daily.todayEligible })
        : t('今天没有需要签到的账号'),
      labels: ['全部', '已签到', '未签到'] as const,
      aria: t('按签到状态筛选'),
      total: providerId ? (railOptions.find(o => o.id === providerId)?.total ?? 0) : dutyTotal,
      done: providerId
        ? daily.providers.find(group => group.id === providerId)?.doneCount ?? 0
        : dutyDone,
    },
    onboarding: {
      title: t('新手福利'),
      summary: extras.onboarding.length > 0
        ? t('已有结果 {checked} / 共 {total} 个账号 · {pending} 项待领', {
          checked: onboardingChecked, total: extras.onboarding.length, pending: onboardingPending,
        })
        : t('没有支持新手福利的账号'),
      labels: ['全部', '已领取', '未领取'] as const,
      aria: t('按领取状态筛选'),
      total: providerId
        ? onboardingGroups.find(group => group.id === providerId)?.rows.length ?? 0
        : extras.onboarding.length,
      done: providerId
        ? onboardingGroups.find(group => group.id === providerId)?.rows.filter(row => onboardingDone(row as OnboardingEntry)).length ?? 0
        : onboardingDoneCount,
    },
    plans: {
      title: t('活动福利'),
      summary: t('限时活动与体验套餐'),
      labels: ['全部', '已领取', '未领取'] as const,
      aria: t('按领取状态筛选'),
      total: providerId
        ? plansCounts.byProvider[providerId]?.total ?? 0
        : plansCounts.total,
      // 活动福利的「已领取」只有查过才知道，计数来自 PlansSection 的上报
      done: providerId
        ? plansCounts.byProvider[providerId]?.claimed ?? 0
        : plansCounts.claimed,
    },
    growth: {
      title: t('成长任务'),
      summary: extras.growth.length > 0
        ? t('{n} 个账号', { n: extras.growth.length })
        : t('没有支持成长任务的账号'),
      labels: ['全部', '已完成', '未完成'] as const,
      aria: t('按完成状态筛选'),
      total: providerId
        ? growthGroups.find(group => group.id === providerId)?.rows.length ?? 0
        : extras.growth.length,
      done: providerId
        ? growthGroups.find(group => group.id === providerId)?.rows.filter(row => growthDone(row as GrowthEntry)).length ?? 0
        : growthDoneCount,
    },
  }[section]

  const emptyText = section === 'daily'
    ? filter === 'done' ? t('今天还没有账号签到，切到「全部」看看哪些还没签。') : t('今天的账号都签完了。')
    : section === 'onboarding'
      ? extras.onboarding.length === 0
        ? t('没有支持新手福利的账号。')
        : filter === 'done'
          ? t('还没有账号领完新手福利，切到「全部」看看待领的项。')
          : t('新手福利都领完了。')
      : section === 'growth'
        ? extras.growth.length === 0
          ? t('没有支持成长任务的账号（需要 WorkBuddy 国内版）。')
          : filter === 'done'
            ? t('还没有账号完成全部任务，切到「全部」看看可完成的项。')
            : t('成长任务都完成了。')
        : t('没有支持活动福利的账号，请添加账号并更新提供商插件。')

  const empty = section === 'daily' ? dutyEntries.length === 0
    : section === 'onboarding' ? onboardingRows.length === 0
      : section === 'growth' ? growthRows.length === 0
        : false

  const railVisible = railOptions.length > 0

  return (
    <div className="ck-center">
      {section === 'daily' && daily.providers.length === 0 ? <EmptyState /> : null}

      {/* 这个任务没有任何账号：不给左栏（空的分类列表比一句空态更让人困惑） */}
      {railVisible || (section === 'daily' && daily.providers.length === 0) ? null : (
        <Card render={<section />} className="panel ck-task-panel">
          <CardHeader className="ck-task-head">
            <CardTitle>{sectionMeta.title}</CardTitle>
            <span className="text-[11.5px] text-muted-foreground">{sectionMeta.summary}</span>
          </CardHeader>
          <Empty className="ck-empty">{emptyText}</Empty>
        </Card>
      )}

      {railVisible ? (
        <div className="ck-split">
          {/* 第三层：提供商分类（跟随当前任务） */}
          <ProviderRail
            options={railOptions}
            selected={providerId}
            total={railTotal}
            onSelect={setProviderId}
          />

          <Card render={<section />} className="panel ck-task-panel">
            <CardHeader className="ck-task-head">
              <CardTitle>{sectionMeta.title}</CardTitle>
              <span className="text-[11.5px] text-muted-foreground">{sectionMeta.summary}</span>
              <CardAction>
                {section === 'daily' ? (
                  <Button size="sm" disabled={runningAll} onClick={() => void runAllCheckin()}>
                    {runningAll ? t('签到中…') : t('全部签到')}
                  </Button>
                ) : null}
                {section === 'onboarding' && extras.onboarding.length > 0 ? <>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={onboardingBusy || onboardingRows.length === 0}
                    onClick={() => {
                      for (const row of onboardingRows) void queryOnboarding(row.id)
                    }}
                  >
                    {t('全部查询')}
                  </Button>
                  <Button size="sm" variant="outline"
                    disabled={onboardingBusy || !onboardingRows.some(row => onboardingClaimable(row.id))}
                    onClick={() => void claimOnboardingBatch(onboardingRows.map(row => row.id))}>
                    {store.onboardingBatch ? t('领取中…') : t('全部领取')}
                  </Button>
                </> : null}
                {section === 'plans' ? <>
                  <Button size="sm" variant="outline" disabled={plansCounts.busy || store.plansBatch !== null || sectionMeta.total === 0}
                    onClick={() => setPlansQueryToken(token => token + 1)}>
                    {store.plansBatch === 'query' ? t('查询中…') : t('全部查询')}
                  </Button>
                  <Button size="sm" variant="outline" disabled={plansCounts.busy || store.plansBatch !== null || plansCounts.claimable === 0}
                    onClick={() => setPlansClaimToken(token => token + 1)}>
                    {store.plansBatch === 'claim' ? t('领取中…') : t('全部领取')}
                  </Button>
                </> : null}
                {section === 'growth' ? <>
                  <Button size="sm" variant="outline" disabled={growthBusy || growthRows.length === 0}
                    title={t('接单 + 点亮 + 领奖（只处理已实测沉淀的任务）')}
                    onClick={() => void runGrowthBatch(growthRows.map(row => row.id), 'full')}>
                    {store.growthBatch?.mode === 'full' ? t('执行中…') : t('一键完成')}
                  </Button>
                  <Button size="sm" variant="outline" disabled={growthBusy || growthRows.length === 0}
                    title={t('只领取已完成任务的奖励（幂等，重复点不会重复入账）')}
                    onClick={() => void runGrowthBatch(growthRows.map(row => row.id), 'claim')}>
                    {store.growthBatch?.mode === 'claim' ? t('领取中…') : t('领取')}
                  </Button>
                </> : null}
                {sectionMeta.done !== null ? (
                  <SegmentedControl
                    aria-label={sectionMeta.aria}
                    options={filterOptions(sectionMeta.labels, sectionMeta.total, sectionMeta.done)}
                    value={filter}
                    onValueChange={setFilter}
                  />
                ) : null}
              </CardAction>
            </CardHeader>

            {/* 选中某一家时，把这家插件申报的说明与 UMID 面板挂出来
                （以前它们挂在「每家一行」的下方注释条里） */}
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

            {/* 只有这一块滚动：页头、指标、任务分类与左栏都固定 */}
            <div className="ck-task-scroll">
              {section === 'daily' ? (
                <DailyAccountList entries={dutyEntries} showProvider={!providerId} />
              ) : null}
              {section === 'onboarding' ? onboardingRows.map(row => <OnboardingRow key={row.id} row={row} />) : null}
              {section === 'plans' ? (
                <PlansSection
                  campaigns={campaignRows}
                  plans={plansRows}
                  providerId={providerId}
                  filter={filter}
                  onFilterChange={setFilter}
                  queryAllToken={plansQueryToken}
                  claimAllToken={plansClaimToken}
                  onCountsChange={reportPlansCounts}
                  embedded
                />
              ) : null}
              {section === 'growth' ? growthRows.map(row => <GrowthRow key={row.id} row={row} />) : null}
              {empty ? <Empty className="ck-empty">{emptyText}</Empty> : null}
            </div>

            <CardFooter className="ck-task-foot">
              {section === 'daily' ? t('单账号签到不受范围限制') : null}
              {section === 'onboarding' ? t('一次性福利 · 已领取状态保存在网关，领完无需重复查询 · 点击行展开任务清单') : null}
              {section === 'plans' ? t('资格、额度与有效期以官方返回为准 · 领取不会购买或订阅套餐') : null}
              {section === 'growth' ? t('查询任务树后一键点亮 / 领奖 · 含开学季与猫猫旅行 · 点击行展开明细') : null}
            </CardFooter>
          </Card>
        </div>
      ) : null}
    </div>
  )
}
