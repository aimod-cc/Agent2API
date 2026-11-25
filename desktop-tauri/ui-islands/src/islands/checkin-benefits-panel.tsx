import * as React from 'react'
import { Badge, Button, Empty, ItemGroup, cn } from '@ui'
import { CheckinRow } from './checkin-row'
import { OfficeaceSubscriptionRow } from './checkin-officeace-row'
import { PlansSection, type PlansCounts } from './checkin-plans-section'
import { CheckinPluginTasks } from './checkin-plugin-tasks'
import { DetailLine, TaskTick, type StatusFilter } from './checkin-shared'
import {
  claimOnboarding, claimOnboardingBatch, onboardingClaimable, onboardingOf, queryOnboarding, queryOnboardingBatch,
} from './checkin-onboarding'
import { onboardingSummary } from './checkin-onboarding-cache'
import {
  growthOf, runGrowth, runGrowthBatch, runSchool, runTravelOnce, queryGrowth, store,
} from './checkin-store'
import {
  formatTime, type BenefitKind, type CheckinCenter, type GrowthEntry, type OnboardingEntry,
} from './checkin-types'
import { growthTaskCompleted, growthTaskRunnable } from '../../../ui-shared/checkin'
import { t } from '../i18n'

/** 分组标题：名称、账号数、可选说明与右侧批量操作 */
function BenefitGroupHeader({ title, count, hint, actions }: {
  title: string
  count: number
  hint?: string
  actions?: React.ReactNode
}): React.ReactElement {
  return (
    <div className='flex flex-wrap items-center gap-2 px-3.5 pb-1.5 pt-3'>
      <span className='text-[11px] font-semibold tracking-[0.08em] text-muted-foreground'>{title}</span>
      <span className='text-[11.5px] text-muted-foreground'>{t('{n} 个账号', { n: count })}</span>
      {hint ? <span className='text-[11.5px] text-muted-foreground'>{hint}</span> : null}
      {actions ? <div className='ml-auto flex shrink-0 items-center gap-1.5'>{actions}</div> : null}
    </div>
  )
}

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

/* ─── 福利与任务主体 ─────────────────────────── */

export function BenefitsPanel({
  center,
  providerId,
  filter,
  onFilterChange,
  benefitKind,
  onboardingRows,
  onboardingBusy,
  growthRows,
  growthBusy,
  plansCounts,
  onPlansCountsChange,
}: {
  center: CheckinCenter
  providerId: string
  filter: StatusFilter
  onFilterChange: (next: StatusFilter) => void
  benefitKind: BenefitKind
  onboardingRows: OnboardingEntry[]
  onboardingBusy: boolean
  growthRows: GrowthEntry[]
  growthBusy: boolean
  plansCounts: PlansCounts
  onPlansCountsChange: (counts: PlansCounts) => void
}): React.ReactElement {
  const showOnboarding = benefitKind === 'all' || benefitKind === 'onboarding'
  const showActivity = benefitKind === 'all' || benefitKind === 'activity'
  const showGrowth = benefitKind === 'all' || benefitKind === 'growth'

  const campaigns = center.extras.campaigns ?? []
  const plans = center.extras.plans
  const scopedActivityCount = campaigns.filter(row => !providerId || row.provider === providerId).length
    + plans.filter(row => !providerId || row.provider === providerId).length
  const hasOnboardingVisible = showOnboarding && onboardingRows.length > 0
  const hasActivityMount = campaigns.length > 0 || plans.length > 0
  const hasActivityVisible = showActivity && (filter === 'all' ? scopedActivityCount > 0 : plansCounts.visible > 0)
  const hasGrowthVisible = showGrowth && growthRows.length > 0

  const taskCategories = benefitKind === 'activity'
    ? ['rewards']
    : benefitKind === 'growth'
      ? ['growth']
      : benefitKind === 'onboarding'
        ? []
        : ['rewards', 'growth']
  const visiblePluginTasks = (center.pluginTasks ?? []).filter(
    row => (!providerId || row.provider === providerId) && taskCategories.includes(row.category),
  )

  const emptyMessage = filter === 'done'
    ? t('暂无已完成或已领取的账号，切到「全部」查看。')
    : filter === 'pending'
      ? t('当前没有待处理的账号。')
      : t('没有支持福利与任务的账号，请添加账号并更新提供商插件。')

  const isAllEmpty = !hasOnboardingVisible && !hasActivityVisible && !hasGrowthVisible && visiblePluginTasks.length === 0

  return (
    <div className='flex flex-col'>
      {hasOnboardingVisible ? (
        <div className='pb-1'>
          <BenefitGroupHeader
            title={t('新手福利')}
            count={onboardingRows.length}
            actions={<>
              <Button
                size='xs'
                variant='outline'
                disabled={onboardingBusy || onboardingRows.length === 0}
                onClick={() => void queryOnboardingBatch(onboardingRows.map(row => row.id))}
              >
                {t('全部查询')}
              </Button>
              <Button
                size='xs'
                variant='outline'
                disabled={onboardingBusy || !onboardingRows.some(row => onboardingClaimable(row.id))}
                onClick={() => void claimOnboardingBatch(onboardingRows.map(row => row.id))}
              >
                {store.onboardingBatch ? t('领取中…') : t('全部领取')}
              </Button>
            </>}
          />
          <ItemGroup>
            {onboardingRows.map(row => <OnboardingRow key={row.id} row={row} />)}
          </ItemGroup>
        </div>
      ) : null}

      {hasActivityMount ? (
        <div hidden={!showActivity}>
          <PlansSection
            campaigns={campaigns}
            plans={plans}
            providerId={providerId}
            filter={filter}
            onFilterChange={onFilterChange}
            onCountsChange={onPlansCountsChange}
            topDivider={hasOnboardingVisible}
            embedded
            hideEmpty
          />
        </div>
      ) : null}

      {hasGrowthVisible ? (
        <div className={cn('pb-1', (hasOnboardingVisible || hasActivityVisible) && 'border-t border-hairline')}>
          <BenefitGroupHeader
            title={t('成长任务')}
            count={growthRows.length}
            actions={<>
              <Button
                size='xs'
                variant='outline'
                disabled={growthBusy || growthRows.length === 0}
                title={t('接单 + 点亮 + 领奖（只处理已实测沉淀的任务）')}
                onClick={() => void runGrowthBatch(growthRows.map(row => row.id), 'full')}
              >
                {store.growthBatch?.mode === 'full' ? t('执行中…') : t('一键完成')}
              </Button>
              <Button
                size='xs'
                variant='outline'
                disabled={growthBusy || growthRows.length === 0}
                title={t('只领取已完成任务的奖励（幂等，重复点不会重复入账）')}
                onClick={() => void runGrowthBatch(growthRows.map(row => row.id), 'claim')}
              >
                {store.growthBatch?.mode === 'claim' ? t('领取中…') : t('领取')}
              </Button>
            </>}
          />
          <ItemGroup>
            {growthRows.map(row => <GrowthRow key={row.id} row={row} />)}
          </ItemGroup>
        </div>
      ) : null}

      {visiblePluginTasks.length > 0 ? (
        <div className={cn('px-4 py-3', (hasOnboardingVisible || hasActivityVisible || hasGrowthVisible) && 'border-t border-hairline')}>
          <CheckinPluginTasks center={center} provider={providerId} categories={taskCategories} />
        </div>
      ) : null}

      {isAllEmpty ? <Empty className='ck-empty'>{emptyMessage}</Empty> : null}
    </div>
  )
}
