import { createRoot } from 'react-dom/client'
import * as React from 'react'
import { Badge, Button, Progress, SegmentedControl, Spinner } from '@ui'
import { PROVIDER_ICONS } from './add-provider-pick'
import { formatTime, shared, type OnboardingTask } from './accounts-shared'
import { claimedPlanIdsToday } from './accounts-domain'
import {
  claimOnboarding, getCheckinStore, groupTone, loadCheckinCenter, nextScheduleRun,
  onboardingExpanded, queryOnboarding, runAllCheckin, setCheckinTab, showCheckinTab,
  signSingleAccount, subscribeCheckinStore, toggleOnboardingExpand, toggleProviderExpand,
  type CheckinProviderGroup, type CheckinStore, type CheckinTab,
} from './checkin-state'
import { ScheduleTab } from './checkin-schedule-tab'
import { RecordsTab } from './checkin-records-tab'
import { openZcodePlans } from './accounts-data'
import { t } from '../i18n'

/**
 * 签到中心（React 岛）。
 *
 * ── 这页管什么 ──────────────────────────────────────────────
 * 全部签到类动作的统一入口，页内分三个标签页（SegmentedControl 切换，
 * 状态与持久化在 checkin-state）：
 *   · **签到中心**（本文件 CenterTab）：总览卡 + 每日签到 + 新手福利（Loomy /
 *     小浣熊 / CodeArts，惰性查询）+ 活动福利（ZCode）；
 *   · **定时签到**（checkin-schedule-tab）：总开关 + 多条定时条目 + 保活模型链；
 *   · **签到记录**（checkin-records-tab）：每账号一行的明细台账（schema v11）。
 * 数据从哪来：快照走 `GET /api/checkin-center` 一次拉全（load）；后端快照的
 * `auto` 段与 GET /api/auto-checkin 同形（含条目数组 schedules），总览卡直接读。
 * 页面零轮询：签到动作完成后由动作层重拉快照，主状态刷新交给 wbApp.refresh。
 *
 * ── 挂载：面板岛模式，root 直接建在既有的页面区块上 ──────────────
 * 与 accounts-page / docs-page 同一手法（见那两处的文件头）。对外契约
 * `window.wbCheckinPanel = { load }`：app.js 切到本页时调，保证切入这次刷新落地。
 */

/* ─── 常量 ─────────────────────────────────── */

/**
 * 各提供商的一句链路说明（与 core::auto_checkin 模块头的口径一致）。
 * 值是界面文案（中文即键）：在定义处 t()，每日签到行与空账号占位行共用同一份译法。
 */
const PROVIDER_DESC: Record<string, string> = {
  workbuddy: t('腾讯每日签到接口 · 仅国内版'),
  'workbuddy-intl': t('每日活跃任务 · 领取才加积分，保活只维持活跃 · 两者可分开执行'),
  raccoon: t('桌面端每日积分链路'),
  autoclaw: t('官方客户端的每日签到任务'),
  'autoclaw-intl': t('与国内版同一套任务接口 · 站点不同'),
  qoder: t('活动（campaign）领取 · 每天 10:00 刷新'),
  'qoder-intl': t('带设备风控身份领取「每日 100 Credits」 · 需要 UMID 组件'),
  trae: t('SOLO 的 checkin_credits 领取 · 按自然日 0 点刷新'),
  loomy: t('无独立签到接口 · 每天替账号打一次首次登录积分'),
  kuku: t('「免费领积分」的每日任务 · 逐个领取'),
  officeace: t('一次领当天全部奖励（每日签到 + 新人礼） · 幂等由上游保证'),
  // CodeArts 的每日福利 2026-10 起并入本链（此前是「活动福利」里那颗手动「去领取」）：
  // 它执行的是 ops 活动交付（三个接口），一句话说明里点出这一点，用户才不会
  // 拿它跟别家「一次请求」的签到比快慢。
  codearts: t('每日福利 · 领取 → 确认 → 回读核实 · 按北京时间自然日重置'),
}

/** 页内标签页的档位（值与 checkin-state 的 CheckinTab 一致） */
const CHECKIN_TAB_OPTIONS = [
  { value: 'center', label: t('签到中心') },
  { value: 'schedule', label: t('定时签到') },
  { value: 'records', label: t('签到记录') },
] as const

/* ─── 小组件 ───────────────────────────────── */

/** 新手福利分组的提供商展示名（缺省回落 provider id 本身） */
function onboardingProviderLabel(provider: string): string {
  return provider === 'raccoon' ? '小浣熊' : provider === 'loomy' ? 'Loomy'
    : provider === 'codearts' ? 'CodeArts'
    : provider === 'officeace' ? 'OfficeAce 果办' : provider
}

function useCheckinStore(): CheckinStore {
  const [store, setStore] = React.useState(getCheckinStore())
  React.useEffect(() => subscribeCheckinStore(() => setStore(getCheckinStore())), [])
  return store
}

/** 提供商行的状态徽章（全部已签 / 部分 / 待签 / 无账号） */
function GroupBadge({ group }: { group: CheckinProviderGroup }) {
  const tone = groupTone(group)
  if (tone === 'none') return <Badge variant='outline' shape='tag'>{t('无账号')}</Badge>
  if (tone === 'done') return <Badge variant='success' shape='tag'>{t('已签')}</Badge>
  if (tone === 'part') return <Badge variant='warning' shape='tag'>{t('部分')}</Badge>
  return <Badge variant='brand' shape='tag'>{t('待签')}</Badge>
}

/** 提供商图标：收录过的用真实图标，否则首字母徽章（与添加账号弹窗同一回落） */
function ProviderLogo({ id, label }: { id: string; label: string }) {
  // WorkBuddy 国际版没有专属图标文件，回落到同品牌那张（两家本来就是一条产品线）
  const icon = PROVIDER_ICONS[id === 'workbuddy-intl' ? 'workbuddy' : id]
  return (
    <span className='ck-prov-logo'>
      {icon ? <img src={icon} alt='' /> : (label.slice(0, 1) || '·')}
    </span>
  )
}

/** 展开区里的账号明细行（每日签到） */
function AccountRows({ group }: { group: CheckinProviderGroup }) {
  const signing = getCheckinStore().signing
  // 在途判定按 `${id}:${mode}`：三颗按钮各自的转圈互不牵连
  const busy = (id: string, mode: string) => signing.has(`${id}:${mode}`)
  return (
    <table className='ck-table'>
      <thead>
        <tr>
          <th>{t('账号')}</th>
          <th>{t('今日签到')}</th>
          <th>{t('上次签到时间')}</th>
          <th aria-label={t('操作')} />
        </tr>
      </thead>
      <tbody>
        {group.accounts.map(account => {
          // WorkBuddy 国际版执行的是「每日活跃任务」：三颗按钮对应三档手动粒度
          // （保活 / 领取 / 保活+领取）—— 领取才会加积分，保活只维持活跃，
          // 分开是当初一颗按钮混做两件事留下的教训。
          // 判据用**分组 id** 而不是账号的 `edition`：Qoder 国际版拆家后公开
          // 形态同样带 `edition: "intl"`，按 edition 判会把它的账号派去
          // WorkBuddy 的日活接口（上游稳定 400「该账号不是 WorkBuddy 国际版
          // 账号」）。只有 `workbuddy-intl` 这一组执行日活链。
          const dailyActivity = group.id === 'workbuddy-intl'
          return (
            <tr key={account.id}>
              <td>{account.name || account.id}</td>
              <td>
                {account.checkedInToday
                  ? <Badge variant='success' shape='tag'>{t('已签')}</Badge>
                  : <Badge variant='brand' shape='tag'>{t('待签')}</Badge>}
              </td>
              <td className='text-subtle'>{account.checkinAt ? formatTime(account.checkinAt) : '—'}</td>
              <td className='text-right'>
                {dailyActivity ? (
                  <span className='inline-flex items-center gap-1'>
                    <Button
                      size='sm' variant='ghost'
                      disabled={busy(account.id, 'keepalive')}
                      title={t('用免费模型维持账号活跃 · 不领取奖励')}
                      onClick={() => void signSingleAccount(account.id, 'keepalive')}
                    >
                      {busy(account.id, 'keepalive') ? t('保活中…') : t('保活')}
                    </Button>
                    <Button
                      size='sm' variant='ghost'
                      disabled={busy(account.id, 'claim')}
                      title={t('探测活动并领取日活奖励 · 不做保活')}
                      onClick={() => void signSingleAccount(account.id, 'claim')}
                    >
                      {busy(account.id, 'claim') ? t('领取中…') : t('领取')}
                    </Button>
                    <Button
                      size='sm' variant='ghost'
                      disabled={busy(account.id, 'full')}
                      title={t('探测 + 领取 + 保活 · 与定时签到同一套组合')}
                      onClick={() => void signSingleAccount(account.id, 'full')}
                    >
                      {busy(account.id, 'full') ? t('执行中…') : t('保活+领取')}
                    </Button>
                  </span>
                ) : (
                  <Button
                    size='sm'
                    variant='ghost'
                    disabled={busy(account.id, 'checkin')}
                    onClick={() => void signSingleAccount(account.id)}
                  >
                    {busy(account.id, 'checkin')
                      ? t('签到中…')
                      : account.checkedInToday ? t('重签') : t('签到')}
                  </Button>
                )}
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

/** 每日签到卡的一行（可展开看账号明细） */
function ProviderRow({ group, expanded }: { group: CheckinProviderGroup; expanded: boolean }) {
  const hasAccounts = group.totalCount > 0
  return (
    <div className={expanded ? 'ck-prov-row open' : 'ck-prov-row'}>
      <div
        className='ck-prov-main'
        onClick={() => hasAccounts && toggleProviderExpand(group.id)}
        onKeyDown={event => {
          if (hasAccounts && (event.key === 'Enter' || event.key === ' ')) {
            event.preventDefault()
            toggleProviderExpand(group.id)
          }
        }}
        role={hasAccounts ? 'button' : undefined}
        tabIndex={hasAccounts ? 0 : -1}
        aria-expanded={hasAccounts ? expanded : undefined}
      >
        <ProviderLogo id={group.id} label={group.label} />
        <div className='ck-prov-info'>
          <div className='ck-prov-name'>{group.label}</div>
          <div className='ck-prov-desc'>{PROVIDER_DESC[group.id] ?? ''}</div>
        </div>
        <div className='ck-prov-right'>
          <span className='ck-prov-count'>
            {hasAccounts ? t('{total} 账号 · {done} 已签', { total: group.totalCount, done: group.doneCount }) : '—'}
          </span>
          <GroupBadge group={group} />
          {hasAccounts ? <span className='ck-chev' aria-hidden>›</span> : null}
        </div>
      </div>
      {expanded ? (
        <div className='ck-prov-detail'>
          {group.id === 'qoder-intl' ? <UmidHint /> : null}
          <AccountRows group={group} />
        </div>
      ) : null}
    </div>
  )
}

/**
 * Qoder 国际版的 UMID 组件提示（展开后的分组详情顶部）。
 *
 * 国际版签到依赖本机的设备风控身份（`Cosy-MachineToken` 三件套），组件来源
 * 三种：本机 Qoder 客户端、qodercli 解压、网关数据目录安装（Linux 一键装）。
 * 这里只读一次状态：装了就说明来源，没装且平台支持安装就给按钮（Linux/Docker
 * 用户唯一能自助的路径）；不支持安装的平台只说明怎么补 —— Windows/macOS 装
 * 客户端或 qodercli 后无需重启网关（组件发现按次执行）。
 */
function UmidHint(): React.ReactElement | null {
  const [state, setState] = React.useState<{ available?: boolean; source?: string | null; installSupported?: boolean; installing?: boolean } | null>(null)
  const load = React.useCallback(async (): Promise<void> => {
    try {
      const response = await fetch('/api/qoder-umid')
      if (response.ok) setState(await response.json())
    } catch { /* 状态是锦上添花，拉不到不挡签到 */ }
  }, [])
  React.useEffect(() => { void load() }, [load])
  if (!state || state.available) return null
  const install = async (): Promise<void> => {
    try {
      setState(prev => prev ? { ...prev, installing: true } : prev)
      const response = await fetch('/api/qoder-umid/install', { method: 'POST' })
      if (!response.ok) {
        const payload = await response.json().catch(() => null)
        shared().wbApp?.toast?.(t('UMID 组件安装失败：{error}', { error: String(payload?.error ?? response.status) }), 'err')
      } else {
        shared().wbApp?.toast?.(t('✅ UMID 组件已安装，下轮签到即可领取国际版积分'))
      }
    } catch (error) {
      shared().wbApp?.toast?.(t('UMID 组件安装失败：{error}', { error: error instanceof Error ? error.message : String(error) }), 'err')
    } finally {
      void load()
    }
  }
  return (
    <div className='flex flex-wrap items-center gap-2 px-[2px] pb-1 text-[12px] text-muted-foreground'>
      <span>{t('国际版签到需要 UMID 组件生成设备风控身份（本机未检测到）。')}</span>
      {state.installSupported
        ? <Button variant='outline' size='sm' disabled={state.installing === true} onClick={() => { void install() }}>
            {state.installing ? t('安装中…') : t('一键安装组件')}
          </Button>
        : <span>{t('安装 Qoder 客户端或 qodercli 后即可（无需重启网关）。')}</span>}
    </div>
  )
}

/** 新手福利一行（含惰性查询 / 一键领取 / 可展开收起的行级任务清单） */
function OnboardingRow({ row }: { row: { id: string; name: string; provider?: string } }) {
  const provider = row.provider ?? 'loomy'
  const cache = getCheckinStore().onboarding.get(row.id)
  const tasks: OnboardingTask[] = cache?.tasks ?? []
  const unclaimed = cache?.unclaimed ?? 0
  const hasDetail = cache?.status === 'loaded' && tasks.length > 0
  const expanded = hasDetail && onboardingExpanded(row.id)
  return (
    <div className={expanded ? 'ck-prov-row open' : 'ck-prov-row'}>
      <div
        className='ck-prov-main'
        onClick={hasDetail ? () => toggleOnboardingExpand(row.id) : undefined}
        onKeyDown={event => {
          if (hasDetail && (event.key === 'Enter' || event.key === ' ')) {
            event.preventDefault()
            toggleOnboardingExpand(row.id)
          }
        }}
        role={hasDetail ? 'button' : undefined}
        tabIndex={hasDetail ? 0 : -1}
        aria-expanded={hasDetail ? expanded : undefined}
      >
        <ProviderLogo id={provider} label={onboardingProviderLabel(provider)} />
        <div className='ck-prov-info'>
          <div className='ck-prov-name'>{row.name || row.id}</div>
          <div className='ck-prov-desc'>
            {cache?.status === 'loaded'
              ? t('已领 {claimed}/{total} · 累计 {earned} 积分', {
                claimed: tasks.length - unclaimed,
                total: tasks.length,
                earned: cache.total ? `${cache.earned} / ${cache.total}` : cache.earned,
              })
              : t('尚未查询任务状态')}
            {/* 记忆路径没有「查询于」（那是后端结算时刻，不是这一次查询）：
                一次性福利领完后按「结算于」如实交代这份结果从哪来 */}
            {cache?.settled && cache.settledAt
              ? ` · ${t('结算于 {time}', { time: formatTime(cache.settledAt) })}`
              : cache?.checkedAt ? ` · ${t('查询于 {time}', { time: formatTime(cache.checkedAt) })}` : ''}
          </div>
        </div>
        {/* stopPropagation：右侧按钮不触发行的展开 / 收起（点「查询任务」不该顺手折起清单） */}
        <div className='ck-prov-right' onClick={event => event.stopPropagation()}>
          {cache?.status === 'loaded' && unclaimed > 0
            ? <Badge variant='warning' shape='tag'>{t('{n} 项待领', { n: unclaimed })}</Badge>
            : cache?.status === 'loaded'
              ? <Badge variant='success' shape='tag'>{t('全部领取')}</Badge>
              : cache?.status === 'error'
                ? <Badge variant='destructive' shape='tag'>{t('查询失败')}</Badge>
                : null}
          {/* 手点「查询任务」= 强制实查（refresh）：结算过的账号进页面走记忆、
              零上游，这颗按钮是唯一的"现在去问一次上游"入口（见 checkin-state） */}
          <Button
            size='sm'
            variant='outline'
            disabled={cache?.status === 'loading' || cache?.claiming === true}
            onClick={() => void queryOnboarding(row.id, { expand: true, refresh: true })}
          >
            {cache?.status === 'loading' ? t('查询中…') : t('查询任务')}
          </Button>
          <Button
            size='sm'
            variant='outline'
            disabled={cache?.status !== 'loaded' || unclaimed === 0 || cache?.claiming === true}
            onClick={() => void claimOnboarding(row.id)}
          >
            {cache?.claiming ? t('领取中…') : t('一键领取（{n}）', { n: unclaimed })}
          </Button>
          {hasDetail ? <span className='ck-chev' aria-hidden>›</span> : null}
        </div>
      </div>
      {cache?.status === 'error' ? (
        <div className='ck-fold-note'>{t('查询失败：{error}', { error: cache.error ?? '' })}</div>
      ) : null}
      {expanded ? (
        <div className='ck-prov-detail'>
          {tasks.map(task => (
            <div className='ck-task-line' key={task.key}>
              <span className={task.done ? 'ck-task-tick done' : 'ck-task-tick'}>
                {task.done ? '✓' : '·'}
              </span>
              <span>{task.title}</span>
              <span className='ck-task-pts'>+{task.points}</span>
              <span className='ck-task-st'>
                {task.claiming
                  ? <Badge variant='warning' shape='tag'>{t('领取中')}</Badge>
                  : task.done
                    ? <Badge variant='success' shape='tag'>{t('已领取')}</Badge>
                    : task.blocked
                      ? <Badge variant='outline' shape='tag'>{t('暂不可领')}</Badge>
                      : task.error
                        ? <Badge variant='destructive' shape='tag'>{task.error}</Badge>
                        : <Badge variant='brand' shape='tag'>{t('可领取')}</Badge>}
              </span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  )
}

/**
 * 活动福利一行（ZCode 的限时套餐领取；本页只提供入口，领取由弹窗内的逐份按钮发起）。
 *
 * ── 为什么是按份判、不按「今天领过没」───────────────────────
 * 「已领取过」在上游是按**套餐**判的（领了 A 之后 B 照样能领），所以这一行不按
 * 「今天领过没」把入口堵死，而是：行上标出**今天已领几份 / 今天未领**（台账判据
 * 复用账号页域层的 `claimedPlanIdsToday`，北京时间的日界只在那一处），按钮始终
 * 开着 —— 点开套餐明细弹窗（`wbZcodePlans`）逐份看：可领取的、名下的（含未生效
 * 与已过期）都在那里，领取也在那里。旧版点这里直接进原生确认框，既看不出逐份
 * 状态、也显示不了刚领到还没生效的套餐。
 *
 * ── CodeArts 那一行去哪了 ───────────────────────────────────
 * 它的**每日福利** 2026-10 起并入了每日签到（`core::billing::checkin` 的
 * codearts 分支，后端也把 `extras.welfare` 撤了），不再有这颗手动「去领取」：
 * 同一个账号在这两处各挂一个「领」的入口只会让人不确定该点哪个。
 * （一次性新人礼仍在「新手福利」，那是另一条链。）
 */
function PlanRow({ row }: {
  row: { id: string; name: string; claimAt?: number | null; claimPlans?: Record<string, number> | null }
}) {
  // ZCode 的逐份领取台账（只算今天、按北京时间判；`row.claimPlans` 是后端落盘事实）
  const claimedIds = claimedPlanIdsToday({ claimPlans: row.claimPlans } as never)
  const claimed = claimedIds.length > 0
  return (
    <div className='ck-welfare-row'>
      <ProviderLogo id='zcode' label='ZCode' />
      <div className='ck-prov-info'>
        <div className='ck-prov-name'>
          {row.name || row.id}
          {claimed
            ? <Badge variant='success' shape='tag' title={t('本地台账里今天（北京时间）领过的套餐：{plans}', { plans: claimedIds.join(t('、')) })}>
              {t('今天已领 {n} 份', { n: claimedIds.length })}
            </Badge>
            : <Badge variant='outline' shape='tag' title={t('本地台账里今天还没领过；活动期内每天发一份新套餐')}>{t('今天未领')}</Badge>}
        </div>
        <div className='ck-prov-desc'>
          {claimed
            ? t('点「套餐明细」看领到了哪几份、什么时候生效；活动期内每天可再领')
            : row.claimAt
              ? t('上次领取 {time} · 领取需通过滑块验证码', { time: formatTime(row.claimAt) })
              : t('限时体验套餐（start-plan），活动期内每天一份 · 领取需滑块验证码')}
        </div>
      </div>
      <div className='ck-prov-right'>
        <Button size='sm' variant='outline' onClick={() => openZcodePlans(row.id, row.name)}>
          {claimed ? t('套餐明细') : t('去领取')}
        </Button>
      </div>
    </div>
  )
}

/** 空账号家的占位行（没有账号时该行不可展开） */
function EmptyProviderRow({ group }: { group: CheckinProviderGroup }) {
  return (
    <div className='ck-prov-row'>
      <div className='ck-prov-main' style={{ cursor: 'default' }}>
        <ProviderLogo id={group.id} label={group.label} />
        <div className='ck-prov-info'>
          <div className='ck-prov-name'>{group.label}</div>
          <div className='ck-prov-desc'>{PROVIDER_DESC[group.id] ?? ''}</div>
        </div>
        <div className='ck-prov-right'>
          <Badge variant='outline' shape='tag'>{t('无账号')}</Badge>
        </div>
      </div>
    </div>
  )
}

/* ─── 签到中心标签页 ───────────────────────── */

/**
 * 总览卡 + 三块任务面板。原「自动签到设置卡」与「最近签到记录」时间线已迁出：
 * 设置长成了多条目（「定时签到」页），记录长成了明细（「签到记录」页），
 * 两张总览卡改成入口（点击切页）。
 */
function CenterTab({ store }: { store: CheckinStore }) {
  const snapshot = store.snapshot
  // 「累计签到记录」卡的读数：签到动作后随快照一起重取（只取 1 行拿 total）
  const [recordTotal, setRecordTotal] = React.useState<number | null>(null)
  React.useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const api = shared().workbuddyDesktop as {
          getCheckinRecords?: (params: Record<string, unknown>) => Promise<{ total?: number } | null | undefined>
        } | undefined
        const data = await api?.getCheckinRecords?.({ limit: 1 })
        if (alive) setRecordTotal(Number(data?.total) || 0)
      } catch { /* 总览卡是锦上添花：拉不到显示 —，不挡签到 */ }
    })()
    return () => { alive = false }
  }, [store.version])

  const daily = snapshot?.daily
  const auto = snapshot?.auto
  const outCount = (daily?.outOfScope ?? []).reduce((sum, item) => sum + (Number(item.count) || 0), 0)
  const onboardingRows = snapshot?.extras.onboarding ?? []
  const onboardingChecked = onboardingRows.filter(row => store.onboarding.get(row.id)?.status === 'loaded')
  const onboardingUnclaimed = onboardingRows.reduce((sum, row) => sum + (store.onboarding.get(row.id)?.unclaimed ?? 0), 0)
  const welfarePlanRows = snapshot?.extras.plans ?? []
  // 定时签到总览：条目数（启用 / 总数）与聚合「下次执行」（多条目里最近的一次）
  const scheduleEntries = Array.isArray(auto?.schedules) ? auto!.schedules! : []
  const scheduleEnabled = scheduleEntries.filter(entry => entry.enabled === true).length
  const scheduleNext = nextScheduleRun(auto)

  return (
    <div>
      {/* ── 总览卡条 ── */}
      <div className='ck-stats'>
        <div className='ck-stat'>
          <div className='ck-stat-label'>{t('今日签到进度')}</div>
          <div className='ck-stat-value'>
            {daily ? <> {daily.todayDone}<small> / {daily.todayEligible}</small></> : <span className='ck-pending'>—</span>}
          </div>
          <Progress className='ck-stat-bar' value={daily && daily.todayEligible > 0 ? (daily.todayDone / daily.todayEligible) * 100 : 0} />
          <div className='ck-stat-foot'>{outCount > 0 ? t('另有 {n} 个账号不参与每日签到', { n: outCount }) : t('全部账号均可签到')}</div>
        </div>
        <div
          className='ck-stat ck-stat-link'
          onClick={() => void showCheckinTab('schedule')}
          role='button'
          tabIndex={0}
          onKeyDown={event => { if (event.key === 'Enter') void showCheckinTab('schedule') }}
        >
          <div className='ck-stat-label'>{t('定时签到')}</div>
          <div className='ck-stat-value'>
            {auto
              ? <>{scheduleEnabled}<small> / {scheduleEntries.length} {t('条启用')}</small></>
              : <span className='ck-pending'>—</span>}
          </div>
          <div className='ck-stat-foot'>
            {auto?.enabled !== true
              ? t('总开关已关闭 · 在「定时签到」页打开')
              : scheduleNext
                ? t('下次 {date} · 点此管理条目', { date: formatTime(scheduleNext) })
                : t('暂无启用的条目 · 点此添加')}
          </div>
        </div>
        <div className='ck-stat'>
          <div className='ck-stat-label'>{t('待领新手福利')}</div>
          <div className='ck-stat-value'>
            {onboardingRows.length
              ? <>{onboardingUnclaimed}<small> {t('项')}</small></>
              : <span className='ck-pending'>{t('无')}</span>}
          </div>
          <div className='ck-stat-foot'>
            {onboardingRows.length
              ? t('已有结果 {checked} / 共 {total} 个账号', { checked: onboardingChecked.length, total: onboardingRows.length })
              : t('没有支持新手福利的账号')}
          </div>
        </div>
        <div
          className='ck-stat ck-stat-link'
          onClick={() => void showCheckinTab('records')}
          role='button'
          tabIndex={0}
          onKeyDown={event => { if (event.key === 'Enter') void showCheckinTab('records') }}
        >
          <div className='ck-stat-label'>{t('累计签到记录')}</div>
          <div className='ck-stat-value'>
            {recordTotal === null ? <span className='ck-pending'>—</span> : recordTotal}
          </div>
          <div className='ck-stat-foot'>{t('每账号一条明细 · 点此查看「签到记录」')}</div>
        </div>
      </div>

      {/* ── 任务面板（单列；设置与记录各自成页）── */}
      <div className='ck-main'>
        <section className='panel'>
          <div className='panel-head'>
            <h2>{t('每日签到')}</h2>
            <span className='panel-sub'>{t('点击行展开账号明细 · 单账号签到不受范围限制')}</span>
            <Button
              size='sm'
              variant='default'
              className='ml-auto'
              disabled={store.runningAll || (auto?.running === true)}
              onClick={() => void runAllCheckin()}
            >
              {store.runningAll || auto?.running === true ? t('签到中…') : t('立即全部签到')}
            </Button>
          </div>
          <div>
            {(daily?.providers ?? []).map(group =>
              group.totalCount > 0
                ? <ProviderRow key={group.id} group={group} expanded={store.expanded.has(group.id)} />
                : <EmptyProviderRow key={group.id} group={group} />,
            )}
            {(daily?.outOfScope ?? []).length > 0 ? (
              <div className='ck-fold-note'>
                {t('不参与每日签到：{list}。CodeArts 的一次性奖励在下方「新手福利」，它的每日福利就在上方「每日签到」里；ZCode 的套餐领取见「活动福利」。', {
                  // 列表项里的 provider 名与原因都是后端数据（snapshot 的 outOfScope），原样透出
                  list: daily!.outOfScope
                    .map(item => t('{label} ×{count}（{reason}）', { label: item.label, count: item.count, reason: item.reason }))
                    .join(t('；')),
                })}
              </div>
            ) : null}
          </div>
        </section>

        <section className='panel'>
          <div className='panel-head'>
            <h2>{t('新手福利')}</h2>
            <span className='panel-sub'>
              {t('一次性福利 · 签到后自动查询并领取，领完记在本机不再查上游 · 点击行展开任务清单')}
            </span>
            {onboardingRows.length > 0 ? (
              <Button
                size='sm'
                variant='outline'
                className='ml-auto'
                onClick={() => {
                  for (const row of onboardingRows) {
                    // 已结算的账号（快照带回了记忆）不进这一轮：它们的结论不会变，
                    // 「全部查询」的语义是「把还没查的查一遍」。真要重问上游，
                    // 逐行那颗「查询任务」才是强制实查的入口（refresh）。
                    if (row.settled && typeof row.settled === 'object') continue
                    void queryOnboarding(row.id)
                  }
                }}
              >
                {t('全部查询')}
              </Button>
            ) : null}
          </div>
          {onboardingRows.length ? (
            <div>
              {onboardingRows.map(row => <OnboardingRow key={row.id} row={row} />)}
            </div>
          ) : (
            <div className='ck-empty'>{t('没有支持新手福利的账号 —— 目前有 Loomy、小浣熊、CodeArts 三家。')}</div>
          )}
        </section>

        <section className='panel'>
          <div className='panel-head'>
            <h2>{t('活动福利')}</h2>
            <span className='panel-sub'>{t('限时体验套餐 · 领取需滑块验证码 · 逐份查看与领取')}</span>
          </div>
          {welfarePlanRows.length > 0 ? (
            <div>
              {welfarePlanRows.map(row => <PlanRow key={row.id} row={row} />)}
            </div>
          ) : (
            <div className='ck-empty'>{t('没有可领套餐的账号（ZCode 未添加）。')}</div>
          )}
        </section>
      </div>
    </div>
  )
}

/* ─── 页面（Tab 壳）─────────────────────────── */

function CheckinPage() {
  const store = useCheckinStore()
  const snapshot = store.snapshot

  // 挂载即拉一次（app.js 切页钩子是主要入口，这里兜「直接刷新落在本页」的场景）
  React.useEffect(() => { void loadCheckinCenter() }, [])

  // 顶栏那块是本页徽标的镜像（app.js 的 renderTopbarStatus 读 #checkin-badge）：
  // 数据一更新就让它跟上，否则要等下一次主状态轮询（20 秒）才同步 —— 与 tasks-panel 同款
  React.useEffect(() => {
    // renderTopbarStatus 不在 accounts-shared 的 SharedWindow 声明里，窄读
    const app = window as unknown as { wbApp?: { renderTopbarStatus?: () => void } }
    app.wbApp?.renderTopbarStatus?.()
  }, [store.version])

  if (!store.loaded) {
    return (
      <div className='ck-loading'>
        <Spinner className='size-5' />
        <span>{t('正在读取签到数据…')}</span>
      </div>
    )
  }

  const auto = snapshot?.auto
  const enabled = auto?.enabled === true

  return (
    <div>
      {store.loadError ? (
        <div className='ck-page-error'>
          <span>{t('读取签到数据失败：{error}', { error: store.loadError })}</span>
          <Button size='sm' variant='outline' onClick={() => void loadCheckinCenter()}>{t('重试')}</Button>
        </div>
      ) : null}

      {/* ── Tab 条：左档位 + 右总开关徽标 ──
          徽标必须三个标签页都在（app.js 的 renderTopbarStatus 按 id 镜像它，
          放进某个标签页里就会在另外两页丢失镜像），放 Tab 条右端全局可见 */}
      <div className='ck-tabs'>
        <SegmentedControl
          options={CHECKIN_TAB_OPTIONS}
          value={store.tab}
          onValueChange={next => setCheckinTab(next as CheckinTab)}
          aria-label={t('签到中心分区')}
        />
        <Badge
          id='checkin-badge'
          className='ml-auto'
          variant={!auto ? 'destructive' : enabled ? 'success' : 'outline'}
          data-tone={!auto ? 'bad' : enabled ? 'ok' : ''}
        >
          {!auto ? t('不可用') : enabled ? (auto?.lastFiredToday ? t('今日已执行') : t('已开启')) : t('已关闭')}
        </Badge>
      </div>

      {store.tab === 'center' ? <CenterTab store={store} /> : null}
      {store.tab === 'schedule' ? <ScheduleTab /> : null}
      {store.tab === 'records' ? <RecordsTab /> : null}
    </div>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="checkin"]'

let mounted = false

/** 把 React root 直接建在页面区块上（不套宿主 div，理由见文件头） */
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<CheckinPage />)
}

declare global {
  interface Window {
    /** 签到中心页（app.js 切页时调 load；数据自持，不随主状态轮询） */
    wbCheckinPanel?: { load(): Promise<void> }
  }
}

// 对外契约：app.js 切到本页时调 load（保证「切入这次刷新一定落地」）
const load = (): Promise<void> => loadCheckinCenter()
window.wbCheckinPanel = { load }

// 脚本排在页面骨架之后（islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

// 首屏自持加载：app.js 的 showPage 在本脚本加载前就执行过（那时 wbCheckinPanel
// 还不存在，切页那次调用落空），用户上次若停在本页，这里补一次 —— 与 tasks-panel 同款
if (shared().wbApp?.currentPage === 'checkin') void load()
