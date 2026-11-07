import * as React from 'react'
import {
  Badge, Button, Card, CardAction, CardFooter, CardHeader, CardTitle, Empty, Progress,
  SegmentedControl, cn,
} from '@ui'
import {
  growthOf, loadCenter, patch, queryGrowth, runAllCheckin, runGrowth,
  runSchool, runTravelOnce, signSingle, store,
} from './checkin-store'
import {
  CHECKIN_SECTION_OPTIONS, MODE_LABEL, MODE_TITLE, formatShort, formatTime, modesOf,
  type CheckinAccount, type CheckinCenter, type GrowthEntry, type OnboardingEntry,
  type ProviderGroup,
} from './checkin-types'
import { CheckinRow } from './checkin-row'
import { CheckinUmid } from './checkin-umid'
import { PlansSection } from './checkin-plans-section'
import { OfficeaceSubscriptionRow } from './checkin-officeace-row'
import { claimOnboarding, onboardingOf, queryOnboarding } from './checkin-onboarding'
import { t } from '../i18n'
import { growthTaskCompleted, growthTaskRunnable } from '../../../ui-shared/checkin'
import {
  accountCheckinStatus, checkinActionLabel, checkinPlanProgress, checkinStatusLabel, checkinStatusTone,
  type AccountCheckinStatus,
} from '../../../ui-shared/checkin-result'

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

/* ─── 总览卡 ───────────────────────────────── */

function StatCard({ label, value, foot, extra, onClick }: {
  label: string
  value: React.ReactNode
  foot?: React.ReactNode
  extra?: React.ReactNode
  onClick?: () => void
}): React.ReactElement {
  const className = onClick ? 'ck-stat ck-stat-link' : 'ck-stat'
  return (
    <Card
      render={onClick ? <Button variant='outline' nativeButton={false} render={<div />} /> : undefined}
      className={`${className} block h-auto w-full whitespace-normal text-left font-normal`}
      onClick={onClick}
    >
      <div className="ck-stat-label">{label}</div>
      <div className="ck-stat-value">{value}</div>
      {foot ? <div className="ck-stat-foot">{foot}</div> : null}
      {extra}
    </Card>
  )
}

/**
 * 总览卡（四张，与旧实现逐项一致）。
 *
 * 旧版是：今日签到（带进度条）/ 定时签到（可点进「定时签到」页）/
 * 待领新手福利 / 累计签到记录（可点进「签到记录」页）。
 * 点卡切页走自定义事件，理由见 `checkin-page.tsx` 的 `wb:checkin-tab`。
 */
function StatsGrid({ center }: { center: CheckinCenter }): React.ReactElement {
  const { daily, extras } = center
  const auto = center.auto
  const percent = daily.todayEligible > 0
    ? (daily.todayDone / daily.todayEligible) * 100
    : 0
  const outCount = daily.outOfScope.reduce((sum, item) => sum + item.count, 0)

  // 定时签到卡：启用的条目数与下一条的执行时刻
  const enabledEntries = (auto?.schedules ?? []).filter(entry => entry.enabled)
  const nextRun = enabledEntries
    .map(entry => Number(entry.nextRunAt))
    .filter(stamp => Number.isFinite(stamp) && stamp > 0)
    .sort((a, b) => a - b)[0]

  // 新手福利卡：已查到的账号里有几项待领
  let onboardingChecked = 0
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    const cache = store.onboarding.get(row.id)
    if (cache?.status !== 'loaded') continue
    onboardingChecked += 1
    if (!cache.settled) onboardingPending += cache.tasks.filter(task => !task.done && !task.blocked).length
  }

  const goTab = (tab: string) => window.dispatchEvent(new CustomEvent('wb:checkin-tab', { detail: { tab } }))

  return (
    <div className="ck-stats">
      <StatCard
        label="今日签到"
        value={daily.todayEligible > 0
          ? <>{daily.todayDone}<small> / {daily.todayEligible}</small></>
          : <span className="ck-pending">—</span>}
        extra={daily.todayEligible > 0
          ? <Progress className="ck-stat-bar" value={percent} />
          : null}
        foot={outCount > 0 ? `另有 ${outCount} 个账号不参与每日签到` : '全部账号均可签到'}
      />
      <StatCard
        label="定时签到"
        value={enabledEntries.length > 0
          ? <>{enabledEntries.length}<small> / {auto?.schedules?.length ?? 0} 条启用</small></>
          : <span className="ck-pending">—</span>}
        foot={auto?.enabled !== true
          ? '总开关已关闭 · 在「定时签到」页打开'
          : nextRun
            ? `下次 ${formatTime(nextRun)} · 点此管理条目`
            : '暂无启用的条目 · 点此添加'}
        onClick={() => goTab('schedule')}
      />
      <StatCard
        label="待领新手福利"
        value={extras.onboarding.length > 0
          ? onboardingChecked > 0
            ? <>{onboardingPending}<small> 项</small></>
            : <span className="ck-pending">—</span>
          : <span className="ck-pending">无</span>}
        foot={extras.onboarding.length > 0
          ? `已有结果 ${onboardingChecked} / 共 ${extras.onboarding.length} 个账号`
          : '没有支持新手福利的账号'}
        onClick={() => patch({ centerSection: 'onboarding' })}
      />
      <StatCard
        label="累计签到记录"
        value={store.recordsLoaded ? store.recordsTotal : <span className="ck-pending">—</span>}
        foot="每账号一条明细 · 点此查看「签到记录」"
        onClick={() => goTab('records')}
      />
    </div>
  )
}

/**
 * 范围外账号的说明（**卡片的脚注**，不是独立卡片）。
 *
 * 它是「每日签到」那张卡的脚注：解释「为什么这些账号没出现在上面的列表里」，
 * 所以跟着操作提示一起待在 `CardFooter` 里，不占行上的位置。
 */
function OutOfScopeNote({ items }: { items: { label: string; reason: string; count: number }[] }): React.ReactElement {
  const list = items
    .map(item => `${item.label} ×${item.count}（${item.reason}）`)
    .join('；')
  return <span>{t('不参与每日签到')}：{list}。</span>
}

/* ─── 每日签到 ─────────────────────────────── */

function StatusBadge({ status }: { status: AccountCheckinStatus }): React.ReactElement {
  const variants = { ok: 'success', warn: 'warning', bad: 'destructive', mute: 'outline', brand: 'brand' } as const
  return <Badge variant={variants[checkinStatusTone(status)]} shape='tag'>{t(checkinStatusLabel(status))}</Badge>
}

/** 同类未完成状态明确显示；混合状态仍保留“部分 / 待签”汇总。 */
function GroupBadge({ group }: { group: ProviderGroup }): React.ReactElement {
  if (!group.totalCount) return <Badge variant="outline" shape="tag">无账号</Badge>
  if (group.doneCount >= group.totalCount) return <StatusBadge status='done' />
  const states = group.accounts.map(accountCheckinStatus)
  if (states.includes('confirming')) return <StatusBadge status='confirming' />
  if (states.includes('failed')) return <StatusBadge status='failed' />
  if (states.includes('captcha')) return <StatusBadge status='captcha' />
  if (states.every(status => status === 'inactive')) return <StatusBadge status='inactive' />
  if (states.every(status => status === 'ineligible')) return <StatusBadge status='ineligible' />
  if (group.doneCount > 0) return <Badge variant="warning" shape="tag">部分</Badge>
  return <StatusBadge status='pending' />
}

/* ─── 三个档位共用的状态筛选 ─────────────────── */

type StatusFilter = 'all' | 'done' | 'pending'

/**
 * 一条筛选器的三个档位：全部 / 已完成 / 未完成。
 *
 * 三个档位的动作词不同（签到 / 领取 / 完成），筛选的**结构**是同一套 ——
 * 所以只换标签、不换逻辑，计数徽标也在这里一起算。
 */
function filterOptions(labels: readonly [string, string, string], total: number, done: number) {
  return [
    { value: 'all' as const, label: t(labels[0]), count: total },
    { value: 'done' as const, label: t(labels[1]), count: done },
    { value: 'pending' as const, label: t(labels[2]), count: total - done },
  ]
}

/** 按状态过滤一个列表（`doneOf` 由各档位自己给判据） */
function byStatus<T>(rows: T[], filter: StatusFilter, doneOf: (row: T) => boolean): T[] {
  if (filter === 'all') return rows
  return rows.filter(row => filter === 'done' ? doneOf(row) : !doneOf(row))
}

/**
 * 展开区里的一行明细（账号 / 任务 / 成长三处共用同一档字号与节奏）。
 *
 * 原来这里是张表格（账号 / 今日签到 / 上次签到时间 / 操作）—— 表头占一行、
 * 列宽固定，在一段只有几行的展开区里比内容还显眼。现在改成「名 + 读数 + 操作」
 * 的紧凑行，与活动福利那边的明细同一形态。
 */
function DetailLine({ children }: { children: React.ReactNode }): React.ReactElement {
  return <div className='flex items-center gap-3 py-1.5 text-[12.5px]'>{children}</div>
}

/** 明细行的左侧「名 + 连登」；连登 0 = 还没读到过，不显示成「连登 0 天」 */
function DetailName({ name, streakDays }: { name: string; streakDays: number }): React.ReactElement {
  return (
    <span className='min-w-0 flex-1 truncate'>
      {name}
      {streakDays > 0 ? <span className='text-muted-foreground'> · {t('连登 {days} 天', { days: streakDays })}</span> : null}
    </span>
  )
}

/** 任务行的对勾：已完成是绿底勾，未完成是一个空心点 */
function TaskTick({ done }: { done: boolean }): React.ReactElement {
  return (
    <span className={cn('grid size-4 shrink-0 place-items-center rounded-full text-[10px]',
      done ? 'bg-success-soft text-success' : 'bg-control text-muted-foreground')}>
      {done ? '✓' : '·'}
    </span>
  )
}

/** 账号明细（展开区）；`accounts` 是筛选后的那一份，空了的行在调用方就被跳过 */
function AccountList({ accounts, group }: { accounts: CheckinAccount[]; group: ProviderGroup }): React.ReactElement {
  const modes = modesOf(group)
  // 分档执行由插件申报：keepalive / claim / full 各一颗按钮 —— 领取才加积分、
  // 保活只维持活跃，分开是刻意的
  const dailyActivity = modes.some(mode => mode === 'keepalive' || mode === 'claim' || mode === 'full')
  return (
    <div className='flex flex-col'>
      {accounts.map(account => (
        <AccountLine key={account.id} account={account} modes={modes} dailyActivity={dailyActivity} manualPlans={group.manualPlans === true} />
      ))}
    </div>
  )
}

function AccountLine({ account, modes, dailyActivity, manualPlans }: {
  account: CheckinAccount
  modes: string[]
  dailyActivity: boolean
  manualPlans: boolean
}): React.ReactElement {
  const busy = (mode: string) => store.signing.has(`${account.id}:${mode}`)
  const status = accountCheckinStatus(account)
  const daily = account.dailyCheckin
  const progress = checkinPlanProgress(daily) && daily
    ? t('已领 {done} / {total} 份套餐', { done: daily.completedCount, total: daily.totalCount })
    : ''
  return (
    <DetailLine>
      <DetailName name={account.name} streakDays={account.streakDays} />
      {account.available ? null : <Badge variant='outline' shape='tag'>{t('凭据不可用')}</Badge>}
      {/* 异常信息优先于进度读数：它是「为什么还没签上」的答案 */}
      {daily?.message && status !== 'done' ? (
        <span className='max-w-[320px] flex-1 truncate text-[11.5px] text-muted-foreground'
          title={t(daily.message)}>{t(daily.message)}</span>
      ) : progress ? (
        <span className='shrink-0 text-[11.5px] text-muted-foreground'>{progress}</span>
      ) : null}
      {daily?.kind === 'rolling' && daily.until ? <span className='shrink-0 tabular-nums text-muted-foreground'>
        {t('下次检查：{time}', { time: formatShort(daily.until) })}
      </span> : null}
      <span className='shrink-0 tabular-nums text-muted-foreground'>
        {account.checkinAt ? t('上次 {time}', { time: formatShort(account.checkinAt) }) : t('从未签到')}
      </span>
      <StatusBadge status={status} />
      <span className='flex shrink-0 items-center gap-0.5'>
        {dailyActivity
          ? modes.map(mode => (
            <Button
              key={mode}
              size="xs"
              variant="ghost"
              disabled={busy(mode) || !account.available}
              title={MODE_TITLE[mode]}
              onClick={() => void signSingle(account.id, mode)}
            >
              {busy(mode) ? `${MODE_LABEL[mode] ?? mode}中…` : (MODE_LABEL[mode] ?? mode)}
            </Button>
          ))
          : (
            <Button
              size="xs"
              variant="ghost"
              disabled={busy('checkin') || !account.available}
              onClick={() => {
                if (manualPlans && window.wbZcodePlans?.open) {
                  window.wbZcodePlans.open({ id: account.id, name: account.name })
                } else {
                  void signSingle(account.id, 'checkin')
                }
              }}
            >
              {manualPlans ? t('套餐明细') : busy('checkin')
                ? t(daily?.queryOnly ? '查询中…' : '签到中…') : t(checkinActionLabel(account))}
            </Button>
          )}
      </span>
    </DetailLine>
  )
}

function ProviderRow({ group, accounts }: { group: ProviderGroup; accounts: CheckinAccount[] }): React.ReactElement {
  const [expanded, setExpanded] = React.useState(false)
  return (
    <CheckinRow provider={group.id} name={group.label}
      // 读数说的是**这一家整体**（4 个账号 · 2 个已签）：筛选只改下面明细的条数，
      // 摘要不跟着变 —— 否则「未签到」筛选下会读成「2 个账号 · 0 个已签」这种怪句子。
      // 插件申报的那句说明是这一家的规则，挂在行下方的注释条里。
      description={t('{total} 个账号 · {done} 个已签', { total: group.totalCount, done: group.doneCount })}
      note={group.description || group.uiHooks?.includes('umid_component') ? <>
        {group.description ? <p className='m-0'>{t(group.description)}</p> : null}
        {group.uiHooks?.includes('umid_component') ? <CheckinUmid /> : null}
      </> : undefined}
      expandable expanded={expanded} onExpandedChange={setExpanded}
      summary={<GroupBadge group={group} />}>
      <AccountList accounts={accounts} group={group} />
    </CheckinRow>
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
  const unclaimed = cache.settled ? 0 : tasks.filter(task => !task.done && !task.blocked).length
  const status = cache.status

  return (
    <CheckinRow provider={row.provider} name={row.name}
      expandable={tasks.length > 0} expanded={expanded} onExpandedChange={setExpanded}
      description={<>
        {cache.settled
          ? t('一次性福利已结清，无需重复查询')
          : status === 'loaded'
            ? `已领 ${tasks.filter(task => task.done).length}/${tasks.length} · 累计 ${cache.earned} 积分`
            : status === 'loading' ? '正在查询…' : '尚未查询任务状态'}
        {cache.settledAt ? ` · ${t('结算于 {time}', { time: formatTime(cache.settledAt) })}`
          : cache.checkedAt ? ` · 查询于 ${formatTime(cache.checkedAt)}` : ''}
      </>}
      summary={cache.settled
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
          disabled={status === 'loading' || cache.claiming} /> : null}
      </> : undefined}
      actions={<>
        <Button
          size="sm"
          variant="outline"
          disabled={status === 'loading' || cache?.claiming === true}
          onClick={() => void queryOnboarding(row.id, true)}
        >
          {status === 'loading' ? '查询中…' : '查询任务'}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={status !== 'loaded' || cache.claiming || cache.settled || (row.provider !== 'officeace' && unclaimed === 0)}
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
  const running = cache.running || cache.runningSchool

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

export function CenterTab({ center }: { center: CheckinCenter }): React.ReactElement {
  const { daily, extras } = center
  const hasAnything = daily.providers.length > 0 || daily.outOfScope.length > 0
    || extras.onboarding.length > 0 || extras.plans.length > 0 || extras.growth.length > 0
    || (extras.campaigns?.length ?? 0) > 0
  const section = store.centerSection
  const runningAll = store.runningAll || center.auto?.running === true

  // 三个档位各有一枚筛选，互不影响（切走档位会卸载卡片，回来时回到「全部」）
  const [dutyFilter, setDutyFilter] = React.useState<StatusFilter>('all')
  const [onboardingFilter, setOnboardingFilter] = React.useState<StatusFilter>('all')
  const [growthFilter, setGrowthFilter] = React.useState<StatusFilter>('all')

  // 新手福利：查过几个账号、其中还有几项待领（明细要打上游，没查过的算不出来）
  let onboardingChecked = 0
  let onboardingPending = 0
  for (const row of extras.onboarding) {
    const cache = store.onboarding.get(row.id)
    if (cache?.status !== 'loaded') continue
    onboardingChecked += 1
    if (!cache.settled) onboardingPending += cache.tasks.filter(task => !task.done && !task.blocked).length
  }

  /* ── 三个档位的筛选（判据都在这里，行组件只负责画）── */

  const dutyTotal = daily.providers.reduce((sum, group) => sum + group.totalCount, 0)
  const dutyDone = daily.providers.reduce((sum, group) => sum + group.doneCount, 0)
  const dutyRows = daily.providers
    .map(group => ({
      group,
      accounts: byStatus(group.accounts, dutyFilter, account => accountCheckinStatus(account) === 'done'),
    }))
    .filter(entry => entry.accounts.length > 0)

  /** 新手福利：查过、且手上没有还能领的 = 已领取；没查过算未领取 */
  const onboardingDone = (row: OnboardingEntry) => {
    const cache = store.onboarding.get(row.id)
    if (cache?.status !== 'loaded') return false
    if (cache.settled) return true
    return !cache.tasks.some(task => !task.done && !task.blocked)
  }
  const onboardingRows = byStatus(extras.onboarding, onboardingFilter, onboardingDone)
  const onboardingDoneCount = extras.onboarding.filter(onboardingDone).length

  /** 成长任务：任务树查过、且手上没有还能完成的 = 已完成 */
  const growthDone = (row: GrowthEntry) => {
    const cache = growthOf(row.id)
    if (cache?.status !== 'loaded' || cache.tasks.length === 0) return false
    return cache.tasks.filter(task => growthTaskRunnable(task, 'full'))
      .every(task => growthTaskCompleted(task))
  }
  const growthRows = byStatus(extras.growth, growthFilter, growthDone)
  const growthDoneCount = extras.growth.filter(growthDone).length

  return (
    <div className="ck-main">
      {hasAnything ? <StatsGrid center={center} /> : null}
      <div className="ck-tabs">
        <SegmentedControl
          options={CHECKIN_SECTION_OPTIONS}
          value={section}
          onValueChange={centerSection => patch({ centerSection })}
          aria-label={t('签到功能分类')}
          className="max-w-full flex-wrap"
        />
      </div>
      {section === 'daily' && daily.providers.length === 0 ? <EmptyState /> : null}

      {section === 'daily' && daily.providers.length > 0 ? (
        <Card render={<section />} className="panel">
          <CardHeader className="border-b-0 pb-1">
            <CardTitle>{t('每日签到')}</CardTitle>
            <span className="text-[11.5px] text-muted-foreground">
              {daily.todayEligible > 0
                ? t('{done} / {total} 个账号已签', { done: daily.todayDone, total: daily.todayEligible })
                : t('今天没有需要签到的账号')}
            </span>
            <CardAction>
              <Button
                size="sm"
                disabled={runningAll}
                onClick={() => void runAllCheckin()}
              >
                {runningAll ? t('签到中…') : t('立即全部签到')}
              </Button>
              <SegmentedControl
                aria-label={t('按签到状态筛选')}
                options={filterOptions(['全部', '已签到', '未签到'], dutyTotal, dutyDone)}
                value={dutyFilter}
                onValueChange={setDutyFilter}
              />
            </CardAction>
          </CardHeader>
          <div className="pb-1 pt-1">
            {dutyRows.map(entry => (
              <ProviderRow key={entry.group.id} group={entry.group} accounts={entry.accounts} />
            ))}
            {dutyRows.length === 0 ? (
              <Empty className="py-6">
                {dutyFilter === 'done'
                  ? t('今天还没有账号签到，切到「全部」看看哪些还没签。')
                  : t('今天的账号都签完了。')}
              </Empty>
            ) : null}
          </div>
          {/* 操作提示与范围外账号都是这张卡的**脚注** */}
          <CardFooter>
            <span>{t('点击行展开账号明细 · 单账号签到不受范围限制')}</span>
            {daily.outOfScope.length > 0 ? <OutOfScopeNote items={daily.outOfScope} /> : null}
          </CardFooter>
        </Card>
      ) : null}

      {section === 'onboarding' ? (
        <Card render={<section />} className="panel">
          <CardHeader className="border-b-0 pb-1">
            <CardTitle>{t('新手福利')}</CardTitle>
            <span className="text-[11.5px] text-muted-foreground">
              {extras.onboarding.length > 0
                ? t('已有结果 {checked} / 共 {total} 个账号 · {pending} 项待领', {
                  checked: onboardingChecked,
                  total: extras.onboarding.length,
                  pending: onboardingPending,
                })
                : t('没有支持新手福利的账号')}
            </span>
            <CardAction>
              {extras.onboarding.length > 0 ? (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    for (const row of extras.onboarding) {
                      void queryOnboarding(row.id)
                    }
                  }}
                >
                  {t('全部查询')}
                </Button>
              ) : null}
              <SegmentedControl
                aria-label={t('按领取状态筛选')}
                options={filterOptions(['全部', '已领取', '未领取'], extras.onboarding.length, onboardingDoneCount)}
                value={onboardingFilter}
                onValueChange={setOnboardingFilter}
              />
            </CardAction>
          </CardHeader>
          {onboardingRows.length > 0
            ? <div className="pb-1 pt-1">{onboardingRows.map(row => <OnboardingRow key={row.id} row={row} />)}</div>
            : <Empty className="py-6">
              {extras.onboarding.length === 0
                ? t('没有支持新手福利的账号。')
                : onboardingFilter === 'done'
                  ? t('还没有账号领完新手福利，切到「全部」看看待领的项。')
                  : t('新手福利都领完了。')}
            </Empty>}
          <CardFooter>
            {t('一次性福利 · 已领取状态保存在网关，领完无需重复查询 · 点击行展开任务清单')}
          </CardFooter>
        </Card>
      ) : null}

      {/* 活动福利单独一个文件：它多出「全部 / 未领取 / 已领取」筛选 */}
      {section === 'plans'
        ? <PlansSection campaigns={extras.campaigns ?? []} plans={extras.plans} />
        : null}

      {section === 'growth' ? (
        <Card render={<section />} className="panel">
          <CardHeader className="border-b-0 pb-1">
            <CardTitle>{t('成长任务')}</CardTitle>
            <span className="text-[11.5px] text-muted-foreground">
              {extras.growth.length > 0
                ? t('WorkBuddy 国内版 · {n} 个账号', { n: extras.growth.length })
                : t('没有支持成长任务的账号')}
            </span>
            <CardAction>
              <SegmentedControl
                aria-label={t('按完成状态筛选')}
                options={filterOptions(['全部', '已完成', '未完成'], extras.growth.length, growthDoneCount)}
                value={growthFilter}
                onValueChange={setGrowthFilter}
              />
            </CardAction>
          </CardHeader>
          {growthRows.length > 0
            ? <div className="pb-1 pt-1">{growthRows.map(row => <GrowthRow key={row.id} row={row} />)}</div>
            : <Empty className="py-6">
              {extras.growth.length === 0
                ? t('没有支持成长任务的账号（需要 WorkBuddy 国内版）。')
                : growthFilter === 'done'
                  ? t('还没有账号完成全部任务，切到「全部」看看可完成的项。')
                  : t('成长任务都完成了。')}
            </Empty>}
          <CardFooter>
            {t('查询任务树后一键点亮 / 领奖 · 含开学季与猫猫旅行 · 点击行展开明细')}
          </CardFooter>
        </Card>
      ) : null}
    </div>
  )
}
