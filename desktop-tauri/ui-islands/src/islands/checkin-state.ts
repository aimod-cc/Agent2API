/**
 * 签到中心的**状态 store**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * ── 数据分两层 ──────────────────────────────────────────────
 *   · 快照层 —— `GET /api/checkin-center` 的一次聚合：每日签到分组（判定在后端，
 *     与批量签到同一对判据）、自动签到设置、签到历史台账、一次性项的账号清单。
 *     页面打开 / 切入时拉一次，签到动作完成后整体重拉。
 *   · 惰性层 —— 新手任务（Loomy / 小浣熊）的任务状态。它是上游查询，快照刻意不带
 *     （见 api::checkin_center 的模块头）；这里按账号缓存查询结果，
 *     界面用「上次查询时间」如实呈现缓存的新旧。
 *
 * ── 为什么是模块级 store + useSyncExternalStore ──────────────
 * 对外契约（`wbCheckinPanel.load()`）从 React 之外调用（app.js 切页钩子），
 * 必须与界面共用同一份状态 —— 组件内部的 useState 做不到。与 accounts-store
 * 同一模式：状态是一份模块级快照，改动一律走 patch()（换新对象再通知）。
 */

import {
  errorMessage, formatTime, shared, toast,
  type OnboardingTask, type OnboardingTaskRaw,
} from './accounts-shared'
import { t } from '../i18n'

/* ─── 快照类型（/api/checkin-center 的响应，字段可能缺，逐个归一）─── */

export type CheckinAccountRow = {
  id: string
  name: string
  available: boolean
  checkinAt: number | null
  checkedInToday: boolean
  /** cn / intl（缺失 null）：WorkBuddy 国际版在这张表里执行的是「领日活」 */
  edition?: string | null
}

export type CheckinProviderGroup = {
  id: string
  label: string
  accounts: CheckinAccountRow[]
  doneCount: number
  totalCount: number
}

export type OutOfScopeGroup = { label: string; reason: string; count: number }

export type CheckinHistoryEntry = {
  at?: number | null
  date?: string | null
  reason?: string | null
  succeeded?: number | null
  /** 活跃保活数（WorkBuddy 国际版）：与 succeeded 分开统计，不落 checkinAt */
  active?: number | null
  total?: number | null
  skipped?: number | null
  failed?: string[] | null
  failedCount?: number | null
}

/** WorkBuddy 国际版日活保活的模型链（config.json 的 checkinKeepalive） */
export type KeepaliveState = {
  models: string[]
  defaultModels: string[]
}

/** 自动签到的状态（与 /api/auto-checkin 同形；tasks-panel 迁来同款读法） */
export type AutoCheckinState = {
  enabled?: boolean
  time?: string
  providers?: string[]
  providerOptions?: Array<{ id: string; label: string }>
  lastFiredToday?: boolean
  nextRunAt?: number | null
  lastResult?: CheckinHistoryEntry | null
  running?: boolean
}

/**
 * 一条定时签到**条目**的视图（/api/auto-checkin 与 /api/checkin-schedules 的
 * `schedules[]` 元素；字段可能缺，读的时候逐个兜底）。
 * `displayLabel` 是后端给好的展示名（label 为空退到 time），界面不必再兜。
 */
export type ScheduleEntry = {
  id: string
  label?: string
  displayLabel?: string
  enabled?: boolean
  time?: string
  providers?: string[]
  lastFiredDate?: string | null
  lastFiredToday?: boolean
  nextRunAt?: number | null
  lastResult?: CheckinHistoryEntry | null
}

/** GET /api/checkin-schedules 的响应：聚合状态 + 条目数组（同一份 state() 形状） */
export type SchedulesSnapshot = AutoCheckinState & {
  schedules?: ScheduleEntry[]
}

/** 一条签到记录（/api/checkin-records 的行；字段可能缺） */
export type CheckinRecordRow = {
  rowId?: number
  ts?: number
  /** 北京时间的自然日（上游的「哪天的签到」） */
  date?: string
  /** schedule / startup / manual / single / activity */
  source?: string
  reason?: string
  accountId?: string
  accountName?: string
  provider?: string
  /** success / already / pending / failed */
  outcome?: string
  /** 「今日已完成」投影（success|already） */
  ok?: boolean
  /** 收益 / 上游文案（「今日积分 +100」在这里） */
  message?: string | null
  error?: string | null
  /** 国际版保活成功 */
  keptActive?: boolean
}

export type CheckinCenterSnapshot = {
  daily: {
    providers: CheckinProviderGroup[]
    outOfScope: OutOfScopeGroup[]
    todayDone: number
    todayEligible: number
  }
  extras: {
    /** 新手任务行；`settled` 是后端的一次性福利结算记忆（已结清才非空） */
    onboarding: Array<{ id: string; name: string; provider?: string; settled?: OnboardingSettledRaw }>
    /** ZCode 的限时套餐行（后端落盘的本地领取台账原样透传）。
     *  曾还有一条 CodeArts 福利行（带 `welfare` 台账），2026-10 它的每日福利并入
     *  每日签到后后端把那条撤了 —— 见 `api::checkin_center` 的说明。 */
    plans: Array<{ id: string; name: string; provider?: string; claimAt?: number | null; claimPlans?: Record<string, number> | null }>
    /** 成长任务行的账号清单（WorkBuddy 国内版；任务详情前端展开时惰性查询） */
    growth?: Array<{ id: string; name: string; provider?: string }>
  }
  auto: SchedulesSnapshot
  /** WorkBuddy 国际版日活保活的模型链（后端快照直接给当前生效值 + 缺省值） */
  keepalive?: KeepaliveState
  history: CheckinHistoryEntry[]
}

/**
 * 后端的一次性福利**结算记忆**（`settled` / `settledAt` / 快照行上的 `settled` 对象）：
 * 一次性福利全部领完那一刻落盘，之后查询零上游、恒是「已领完」这个结论
 * （见 server 的 `core::providers::onboarding_memory`）。
 */
export type OnboardingSettledRaw = {
  at?: unknown
  tasks?: unknown
  earned?: unknown
  total?: unknown
}

/** 一个新手任务账号的任务缓存（Loomy / 小浣熊 / CodeArts；status: idle = 还没查过） */
export type OnboardingCache = {
  status: 'idle' | 'loading' | 'loaded' | 'error'
  error?: string
  checkedAt?: number
  tasks: OnboardingTask[]
  unclaimed: number
  earned: number
  total: number
  claiming: boolean
  /**
   * 这份结果来自**结算记忆**（后端已确认这条一次性福利领完）：进页面直接用
   * 快照渲染，不必再查上游；「查询任务」按钮仍可强制实查（`refresh`）。
   * 记忆路径不写 `checkedAt`（那是「查询于」，记忆是「结算于」，见 `settledAt`）。
   */
  settled?: boolean
  /** 结算时刻（后端记忆里的 `at`；仅 `settled` 时有） */
  settledAt?: number
}

/* ─── 成长任务（WorkBuddy 国内版；GET/POST /api/accounts/{id}/growth*）─── */

/** 一条成长任务的展示行（`GET /api/accounts/{id}/growth` 的 tasks 元素） */
export type GrowthTaskRow = {
  code: string
  title: string
  description?: string
  /** not_accepted / accepted / completed / claimed（上游口径） */
  status: string
  current: number
  target: number
  locked?: boolean
  unlockAt?: unknown
  /** 在映射表里（未支持的任务执行时会保守跳过，界面如实标注） */
  supported: boolean
  unforgeable?: boolean
  unforgeableReason?: string
  kind?: string
  source?: string
}

/** 执行结果的逐任务行（`POST .../growth/run` 的 results 元素） */
export type GrowthRunRow = {
  code: string
  /** done / already / skipped / failed（school 段同形） */
  outcome: string
  message: string
  credit?: number
  energy?: number
}

/** 成长任务账号的惰性缓存（status: idle = 还没查过） */
export type GrowthCache = {
  status: 'idle' | 'loading' | 'loaded' | 'error'
  error?: string
  checkedAt?: number
  /** 能量余额 / 连登天数（只读读数；拉不到 null） */
  energy: number | null
  streak: number | null
  tasks: GrowthTaskRow[]
  travel: {
    hasBuddy?: boolean | null
    buddyName?: string
    state?: string
    dailyLimitReached?: boolean
    rewardCredit?: number
    error?: string | null
  } | null
  school: {
    inPeriod?: boolean
    tasks?: GrowthTaskRow[]
    lottery?: { balance?: number; totalEarned?: number } | null
    error?: string | null
  } | null
  /** 全量执行在途 */
  running: boolean
  /** 开学季 + 抽奖在途 */
  runningSchool: boolean
  /** 猫猫旅行巡检在途 */
  runningTravel: boolean
  /** 最近一次执行的结果行（任务 + 开学季归并在这里展示） */
  results?: GrowthRunRow[]
  /** 最近一次抽奖结果文本（开学季「执行并抽奖」后回填） */
  lotteryText?: string
  /** 最近一次开学季任务执行的逐条概要（「执行并抽奖」后回填） */
  schoolText?: string
}

/* ─── 本岛用到的桥（在 accounts-shared 的 AccountsBridge 之上补签到两组）─── */

type CheckinBridge = {
  getCheckinCenter?(): Promise<CheckinCenterSnapshot | null | undefined>
  getAutoCheckin?(): Promise<AutoCheckinState>
  saveAutoCheckin?(patch: Record<string, unknown>): Promise<AutoCheckinState>
  runAutoCheckinNow?(): Promise<(CheckinHistoryEntry & AutoCheckinState) | null | undefined>
  /** 定时签到的**条目**（列表 / 保存 / 删除 / 立即执行；与 bridge.rs 成对） */
  getCheckinSchedules?(): Promise<SchedulesSnapshot | null | undefined>
  saveCheckinSchedule?(patch: Record<string, unknown>): Promise<SchedulesSnapshot | null | undefined>
  deleteCheckinSchedule?(id: string): Promise<{ success?: boolean } | null | undefined>
  runCheckinSchedule?(id: string): Promise<(CheckinHistoryEntry & SchedulesSnapshot) | null | undefined>
  /** 签到记录明细台账（查询 / 清空；参数对象里空值桥层自动不带） */
  getCheckinRecords?(params: Record<string, unknown>): Promise<{ records?: CheckinRecordRow[]; total?: number } | null | undefined>
  clearCheckinRecords?(params: Record<string, unknown>): Promise<{ success?: boolean; removed?: number } | null | undefined>
  /** WorkBuddy 国际版日活保活的模型链（读 / 存；与 bridge.rs、web_shim.rs 三处成对） */
  getCheckinKeepalive?(): Promise<KeepaliveState>
  saveCheckinKeepalive?(models: string[]): Promise<KeepaliveState>
  /** WorkBuddy 国际版日活任务的手动粒度入口（mode: full | claim | keepalive） */
  runCheckinActivity?(id: string, mode: string): Promise<Record<string, unknown> | null | undefined>
  checkinAllAccounts?(id?: string | null): Promise<{
    results?: Array<Record<string, unknown>>
    succeeded?: number
    active?: number
    total?: number
  } | null | undefined>
  getOnboardingTasks?(id: string, refresh?: boolean): Promise<{
    tasks?: OnboardingTaskRaw[]
    earned?: unknown
    total?: unknown
    unclaimed?: unknown
    settled?: unknown
    settledAt?: unknown
  } | null | undefined>
  claimOnboardingTasks?(id: string): Promise<{
    results?: Array<Record<string, unknown>>
    tasks?: OnboardingTaskRaw[]
    earned?: unknown
    total?: unknown
    unclaimed?: unknown
    settled?: unknown
    settledAt?: unknown
    /** 后端对本次领取结果的一句话交代（OfficeAce 会回，见 checkin-state 的 claimOnboarding） */
    message?: unknown
  } | null | undefined>
  /** 成长任务盘点（只读；与 bridge.rs、web_shim.rs 三处成对） */
  getGrowthStatus?(id: string): Promise<{
    energy?: unknown
    streak?: unknown
    tasks?: { tasks?: unknown; error?: unknown }
    travel?: unknown
    school?: unknown
  } | null | undefined>
  /** 成长任务执行（**单任务**；界面的「一键完成」在本地循环逐条调用） */
  runGrowthTask?(id: string, code: string, mode?: string): Promise<{
    code?: unknown
    outcome?: unknown
    message?: unknown
    credit?: unknown
    energy?: unknown
  } | null | undefined>
  /** 开学季任务 + 可选抽奖 */
  runGrowthSchool?(id: string, lottery?: boolean): Promise<{
    results?: unknown
    lottery?: unknown
  } | null | undefined>
  /** 猫猫旅行巡检一次 */
  runGrowthTravel?(id: string): Promise<{
    outcome?: unknown
    message?: unknown
    credit?: unknown
  } | null | undefined>
  /**
   * Qoder UMID 组件（设备风控身份）的状态与安装（与 bridge.rs、web_shim.rs 三处成对）。
   *
   * `available` 决定签到页那条提示出不出现，`installSupported` 决定给「一键安装」
   * 按钮还是文字说明（只有 Linux / headless 支持下载安装），`installing` 让界面在
   * 安装期间持续回读状态。
   */
  getQoderUmid?(): Promise<{
    available?: boolean
    source?: string | null
    installSupported?: boolean
    installing?: boolean
  } | null | undefined>
  installQoderUmid?(): Promise<{ version?: string; registry?: string; installedAt?: number } | null | undefined>
}

export function bridge(): CheckinBridge {
  return (shared().workbuddyDesktop ?? {}) as CheckinBridge
}

/**
 * 账号页视图桥的窄读（wbAccountsView 由 accounts-page 自挂，不在 accounts-shared
 * 的 SharedWindow 声明里）。这里只取「签到后刷新余额读数」这一个动作。
 */
function accountsView(): { wbAccountsView?: { refreshUsageAfterCheckin?: (id?: string | null) => unknown } } {
  return window as unknown as { wbAccountsView?: { refreshUsageAfterCheckin?: (id?: string | null) => unknown } }
}

/* ─── 页内标签页（签到中心 / 定时签到 / 签到记录）────────── */

export type CheckinTab = 'center' | 'schedule' | 'records'

/** 持久化键（命名仿 settings 页的 workbuddy-desktop-settings-cat） */
const TAB_STORAGE_KEY = 'workbuddy-desktop-checkin-tab'

const CHECKIN_TABS: readonly CheckinTab[] = ['center', 'schedule', 'records']

/** 上次停留的标签页（没存过 / 值不认 → 签到中心）。localStorage 挂了给缺省。 */
function readStoredTab(): CheckinTab {
  try {
    const value = localStorage.getItem(TAB_STORAGE_KEY)
    return CHECKIN_TABS.includes(value as CheckinTab) ? (value as CheckinTab) : 'center'
  } catch {
    return 'center'
  }
}

/** 切换标签页（持久化；数据加载交给各标签页自己挂载时的 effect） */
export function setCheckinTab(tab: CheckinTab): void {
  if (tab === store.tab) return
  try {
    localStorage.setItem(TAB_STORAGE_KEY, tab)
  } catch { /* 隐私模式等存不进就算了：会话内仍然生效 */ }
  patch({ tab })
}

/**
 * 切换并确保签到数据就位（对外深链契约：顶栏徽标、总览卡的跳转都走它 ——
 * 与 settings 页 showCategory 同一款）。定时签到 / 签到记录两页的数据由各自的
 * 挂载 effect 拉，这里只兜签到中心的快照。
 */
export async function showCheckinTab(tab: CheckinTab): Promise<void> {
  setCheckinTab(tab)
  if (!store.loaded) await loadCheckinCenter()
}

/* ─── store ─────────────────────────────────── */

export type CheckinStore = {
  /** 每次变更 +1；React 靠它判断「该重画了」（快照对象同时被换掉） */
  version: number
  /** 当前标签页（页内导航；localStorage 持久化） */
  tab: CheckinTab
  /** 聚合快照（null = 还没读到 / 读失败） */
  snapshot: CheckinCenterSnapshot | null
  /** 快照至少成功拉过一次（失败后的重试交给用户手点刷新，不自动轮询） */
  loaded: boolean
  loadError: string
  /** 展开的每日签到分组（按提供商 id） */
  expanded: ReadonlySet<string>
  /** 「立即全部签到」在途（防重 + 按钮文案） */
  runningAll: boolean
  /** 自动签到设置保存中（开关 / 时刻 / 范围控件临时禁用） */
  autoSaving: boolean
  /** 签到时刻的编辑草稿：null = 跟随后端值 */
  autoTimeDraft: string | null
  /** 保活模型链保存中（清单的增删 / 拖动 / 保存按钮临时禁用） */
  keepaliveSaving: boolean
  /** 保活模型链的编辑草稿：null = 跟随后端值；数组 = 用户正在编辑的清单 */
  keepaliveDraft: string[] | null
  /** 单账号签到在途（行内按钮的去重闸） */
  signing: ReadonlySet<string>
  /** 新手任务的按账号缓存（Loomy / 小浣熊） */
  onboarding: ReadonlyMap<string, OnboardingCache>
  /** 成长任务的按账号缓存（WorkBuddy 国内版） */
  growth: ReadonlyMap<string, GrowthCache>
}

let store: CheckinStore = {
  version: 0,
  tab: readStoredTab(),
  snapshot: null,
  loaded: false,
  loadError: '',
  expanded: new Set<string>(),
  runningAll: false,
  autoSaving: false,
  autoTimeDraft: null,
  keepaliveSaving: false,
  keepaliveDraft: null,
  signing: new Set<string>(),
  onboarding: new Map<string, OnboardingCache>(),
  growth: new Map<string, GrowthCache>(),
}

const subscribers = new Set<() => void>()

function emitChange(): void {
  for (const notify of subscribers) notify()
}

/** 换新对象再通知（与 accounts-store 的 patch 同一口径） */
function patch(next: Partial<CheckinStore>): void {
  store = { ...store, ...next, version: store.version + 1 }
  emitChange()
}

export function getCheckinStore(): CheckinStore {
  return store
}

export function subscribeCheckinStore(notify: () => void): () => void {
  subscribers.add(notify)
  return () => subscribers.delete(notify)
}

/* ─── 归一（后端字段可能缺，读的时候逐个兜底）─── */

function normalizeTask(raw: OnboardingTaskRaw): OnboardingTask | null {
  const key = typeof raw?.key === 'string' ? raw.key : ''
  if (!key) return null
  return {
    key,
    title: typeof raw.title === 'string' && raw.title ? raw.title : key,
    group: typeof raw.group === 'string' && raw.group ? raw.group : '',
    points: Number(raw.points) || 0,
    done: raw.done === true,
    blocked: raw.blocked === true,
  }
}

function normalizeTasks(raw: unknown): OnboardingTask[] {
  const list = Array.isArray(raw) ? raw : []
  return list.map(normalizeTask).filter((task): task is OnboardingTask => task !== null)
}

function numberOf(value: unknown): number {
  return Number(value) || 0
}

/* ─── 动作 ─────────────────────────────────── */

/**
 * 拉聚合快照。签到动作完成后也走这里重拉（签到会改 checkinAt / 台账 /
 * 自动签到状态，一份接口全兜住）。失败不清空已有快照 —— 老数据还在展示，
 * 错误提示挂在页面头。
 */
export async function loadCheckinCenter(): Promise<void> {
  try {
    const data = await bridge().getCheckinCenter?.()
    if (!data) throw new Error(t('后端未返回签到中心数据'))
    patch({
      snapshot: data,
      loaded: true,
      loadError: '',
      // 时刻草稿跟着后端值走（首次加载 / 异地修改后都以服务端为准）
      autoTimeDraft: null,
      keepaliveDraft: null,
    })
    // 快照带回了结算记忆 ⇒ 直接用它铺缓存，页面不必为已领完的账号再查一次上游
    seedSettledCaches(data.extras?.onboarding ?? [])
  } catch (error) {
    patch({ loaded: true, loadError: errorMessage(error) })
  }
}

/**
 * 用快照里的结算记忆铺新手任务缓存（`settled` 非空的那些账号）。
 *
 * ── 为什么直接铺而不是「查一次再确认」───────────────────────
 * 记忆本身就是后端的结论（一次性福利已全部领完，见 onboardingSettled 的说明），
 * 再问一次上游只会得到同一个答案。这也是本功能的初衷：进页面不该为一条早就
 * 领完的福利反复查询。用户想实查仍可点「查询任务」（`refresh`），那时记忆若与
 * 上游不符会被后端覆盖 —— 铺在这里的只是当前这份快照。
 *
 * 界面这边**刚实查过、且查出了待领项**的账号不动：那说明记忆已被后端清掉
 * （上游出了新一期活动），只是手上这份快照还是旧的。
 */
function seedSettledCaches(rows: Array<{ id: string; settled?: OnboardingSettledRaw }>): void {
  for (const row of rows) {
    const memory = row.settled
    if (!memory || typeof memory !== 'object') continue
    const current = store.onboarding.get(row.id)
    if (current?.status === 'loaded' && current.unclaimed > 0) continue
    const tasks = normalizeTasks(memory.tasks)
    const at = numberOf(memory.at)
    patchOnboarding(row.id, {
      status: 'loaded',
      error: undefined,
      // 记忆不是「查询」来的：checkedAt 留空，界面按 settledAt 显示「结算于」
      checkedAt: undefined,
      settled: true,
      settledAt: at > 0 ? at : undefined,
      tasks,
      unclaimed: 0,
      earned: numberOf(memory.earned),
      total: numberOf(memory.total),
    })
  }
}

/** 展开 / 收起一个提供商分组 */
export function toggleProviderExpand(id: string): void {
  const next = new Set(store.expanded)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  patch({ expanded: next })
}

/**
 * 展开 / 收起一个账号的新手任务清单。
 *
 * 键加 `onb:` 前缀与每日签到的提供商分组共用同一个 `expanded` 集合（两边的
 * 交互语义一致，没必要开第二份状态）；账号 id 与提供商 id 不会撞（前者是
 * 落盘的主键），前缀是给读代码的人的——一眼看出这颗键属于哪张卡。
 */
export function toggleOnboardingExpand(id: string): void {
  const key = `onb:${id}`
  const next = new Set(store.expanded)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  patch({ expanded: next })
}

/** 某账号的新手任务清单当前是否展开 */
export function onboardingExpanded(id: string): boolean {
  return store.expanded.has(`onb:${id}`)
}

/** 「立即全部签到」：走 POST /api/auto-checkin/run（与定时同一条执行体，自带防重） */
export async function runAllCheckin(): Promise<void> {
  if (store.runningAll) return
  patch({ runningAll: true })
  try {
    const result = await bridge().runAutoCheckinNow?.()
    if (result) {
      const succeeded = numberOf(result.succeeded)
      const active = numberOf(result.active)
      const total = numberOf(result.total)
      const failed = numberOf(result.failedCount)
      const activeText = active > 0 ? t('，日活保活 {n} 个', { n: active }) : ''
      if (failed > 0) {
        toast(t('签到完成：{succeeded}/{total} 成功{active}，{failed} 个失败', {
          succeeded, total, active: activeText, failed,
        }), 'err')
      } else {
        toast(t('✅ 签到完成：{succeeded}/{total} 个账号成功领取{active}', {
          succeeded, total, active: activeText,
        }))
      }
      // 余额读数是账号页自己缓存里的，不查它还是签到前的旧值（不 await：
      // 那是账号页的动作层，静默刷新，结果落在余额列上）
      accountsView().wbAccountsView?.refreshUsageAfterCheckin?.(null)
    }
  } catch (error) {
    toast(t('签到失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
      patch({ runningAll: false })
      await loadCheckinCenter()
      // 快照到位后自动处理新手任务（不 await：签到结果已经播报过，
      // 查询与领取是后台的一次跟进，进度直接落在「待领新手任务」卡上）
      void autoProcessOnboarding(getCheckinStore().snapshot?.extras.onboarding ?? [])
      shared().wbApp?.refresh?.()
  }
}

/**
 * 单账号签到 / 日活任务（签到中心明细行上的按钮）。
 *
 * ── 四种模式 ────────────────────────────────────────────────
 *   - `checkin`（缺省）：普通单账号签到，走既有端点（除 WorkBuddy 国际版外
 *     的所有账号都是它）；
 *   - `full` / `claim` / `keepalive`：WorkBuddy 国际版日活任务的三档手动粒度
 *     （保活+领取 / 只领取 / 只保活），走独立端点 —— 与批量签到那条「恒走
 *     完整组合」的入口分开，粒度细分只属于手动按钮。
 *
 * 在途键是 `${id}:${mode}`：三颗按钮各自的转圈互不牵连，点「保活」不禁用
 * 「领取」。结果行的 toast 按模式分流 —— 国际版的结果必须分清「保活」与
 * 「领取」两件事，混在一句话里正是当初造成困惑的根源。
 */
export async function signSingleAccount(id: string, mode: 'checkin' | 'full' | 'claim' | 'keepalive' = 'checkin'): Promise<void> {
  const busyKey = `${id}:${mode}`
  if (store.signing.has(busyKey)) return
  const next = new Set(store.signing)
  next.add(busyKey)
  patch({ signing: next })
  try {
    let row: Record<string, unknown> | undefined
    if (mode === 'checkin') {
      const result = await bridge().checkinAllAccounts?.(id)
      row = result?.results?.[0]
    } else {
      row = (await bridge().runCheckinActivity?.(id, mode)) ?? undefined
    }
    const claim = row?.claim as
      | { success?: boolean; msg?: string; alreadyCompleted?: boolean }
      | undefined
    const activity = row?.activity as
      | {
          pokeSucceeded?: boolean | null
          report?: { succeeded?: boolean } | null
          webConversation?: { succeeded?: boolean } | null
        }
      | null
      | undefined
    const reportOk = activity?.report?.succeeded === true
    const webOk = activity?.webConversation?.succeeded === true
    if (row?.error) {
      toast(t('签到失败：{error}', { error: String(row.error) }), 'err')
    } else if (mode === 'keepalive') {
      // 只保活：结果就一句话（保活成功 / 失败），不存在领取语义。
      // 活跃上报（点亮连登的那条通道）与网页会话（issue #277：「当日活跃」
      // 计数的判定通道）成功同样算保活成功；三者都失败才是失败。
      if (activity?.pokeSucceeded === true || webOk || reportOk)
        toast(t('✅ {message}', { message: claim?.msg || t('活跃保活完成') }))
      else toast(claim?.msg || t('活跃保活失败'), 'err')
    } else if (claim?.success === true) {
      toast(t('✅ {message}', { message: claim.msg || t('签到成功') }))
    } else if (claim?.alreadyCompleted === true) {
      toast(claim?.msg || t('今日已领取'), 'err')
    } else if (reportOk) {
      // 活跃上报成功（连登点亮）但没领到 —— 后端 msg 把「活动未开启」一并交代
      toast(t('✅ {message}', { message: claim?.msg || t('活跃上报完成，但活动未开启') }))
    } else if (activity?.pokeSucceeded === true || webOk) {
      // 完整模式：保活成功（保活对话或网页会话）但没领到 —— 后端 msg 把
      // 「活动未开启」一并交代
      toast(t('✅ {message}', { message: claim?.msg || t('活跃保活完成，但活动未开启') }))
    } else {
      toast(claim?.msg || t('未领取'), 'err')
    }
    accountsView().wbAccountsView?.refreshUsageAfterCheckin?.(id)
  } catch (error) {
    toast(t('签到失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    const rest = new Set(getCheckinStore().signing)
    rest.delete(busyKey)
    patch({ signing: rest })
    await loadCheckinCenter()
    // 只处理刚签的这个账号：从快照的新手任务清单里过滤，账号不在清单里则数组为空
    // （清单由后端按 provider 组装：Loomy / 小浣熊 / CodeArts，别的家不发无意义的查询）
    const onboardingRows = (getCheckinStore().snapshot?.extras.onboarding ?? [])
      .filter(row => row.id === id)
    void autoProcessOnboarding(onboardingRows)
    shared().wbApp?.refresh?.()
  }
}

/* ─── 自动签到设置（自 tasks-panel 迁入，提交语义保持一致）─── */

/**
 * 保存签到设置（开关 / 时刻 / 范围共用）。保存期间控件全部禁用；开关一起
 * 提交当前时刻（与旧实现同款：带上未提交的编辑草稿）。
 */
export async function saveAutoCheckin(patchBody: Record<string, unknown>, label: string): Promise<void> {
  if (store.autoSaving) return
  patch({ autoSaving: true })
  try {
    const next = await bridge().saveAutoCheckin?.(patchBody)
    if (next) {
      patch({ snapshot: mergeAuto(next), autoTimeDraft: null })
      toast(t('已保存：{label}', { label }))
    }
  } catch (error) {
    toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ autoSaving: false })
  }
}

/** 签到时刻的编辑草稿（输入过程；提交在 blur / 回车时走 submitAutoTime） */
export function setAutoTimeDraft(value: string): void {
  patch({ autoTimeDraft: value })
}

/** 签到时刻提交（失焦 / 回车）：格式由后端校验，错误原样吐回 */
export async function submitAutoTime(value: string): Promise<void> {
  const time = value.trim()
  if (!time) return
  await saveAutoCheckin(
    { enabled: getCheckinStore().snapshot?.auto?.enabled === true, time },
    t('签到触发时刻'),
  )
}

/** 勾选 / 取消一家签到提供商（提交完整清单，后端校验至少一家） */
export async function toggleAutoProvider(optionId: string, checked: boolean): Promise<void> {
  const snapshot = getCheckinStore().snapshot
  const options = snapshot?.auto?.providerOptions ?? []
  const picked = new Set(snapshot?.auto?.providers ?? [])
  if (checked) picked.add(optionId)
  else picked.delete(optionId)
  // 顺序取 providerOptions 的注册顺序（与后端落盘口径一致）
  const providers = options.map(option => option.id).filter(id => picked.has(id))
  await saveAutoCheckin({ providers }, t('签到提供商'))
}

/** 快照里只换 auto 段（保存响应是新状态，其余快照原样保留） */
function mergeAuto(next: AutoCheckinState): CheckinCenterSnapshot | null {
  const current = getCheckinStore().snapshot
  if (!current) return current
  return { ...current, auto: next }
}

/* ─── WorkBuddy 国际版日活保活的模型链 ─── */

/**
 * 保活模型链的编辑草稿：把整份清单换掉（增 / 删 / 拖动都是「换一份新数组」）。
 *
 * 草稿放在 store 而不是组件里，是为了**切标签页不丢**：签到中心一次只渲染一个
 * 标签页，组件内的 useState 在切走时会连同编辑到一半的清单一起消失。
 */
export function setKeepaliveDraft(models: string[]): void {
  patch({ keepaliveDraft: models.map(item => item.trim()).filter(Boolean) })
}

/**
 * 保存保活模型链（「保存」按钮）。**空清单也会提交** —— 空 = 恢复缺省链，这是
 * 后端 `billing::keepalive::set_models` 的既定语义，界面据此提供「恢复缺省」。
 * （旧实现是输入框失焦提交，那时空串直接 return，「清空恢复缺省」实际做不到。）
 * 成功后快照换成后端返回的完整状态、草稿清空（回到「跟随后端值」）。
 */
export async function submitKeepaliveModels(models: string[]): Promise<void> {
  const clean = models.map(item => item.trim()).filter(Boolean)
  if (getCheckinStore().keepaliveSaving) return
  patch({ keepaliveSaving: true })
  try {
    const next = await bridge().saveCheckinKeepalive?.(clean)
    if (next) {
      const current = getCheckinStore().snapshot
      patch({ snapshot: current ? { ...current, keepalive: next } : current, keepaliveDraft: null })
      toast(t('已保存：保活模型链'))
    }
  } catch (error) {
    toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ keepaliveSaving: false })
  }
}

/* ─── 新手任务（惰性查询 + 一键领取；Loomy / 小浣熊）─── */

/**
 * 签到完成后的自动处理（原账号页「签到后自动弹窗领取」口径的延续）：
 * 对 [`rows`] 里的账号逐个查询任务状态，有未领取的**立即自动领取**。
 *
 * ── 为什么自动领取是安全的 ──────────────────────────────────
 * 新手任务是一次性福利，两家的领取接口都幂等（Loomy 重复上报 alreadyCompleted、
 * 小浣熊已发放过返回 granted=false，都不会重复加分），自动领取不会多拿；
 * 全部领完后查询结果 unclaimed=0，之后签到就只是签到。逐账号串行
 * （与签到同一条防风控口径），单账号查询失败不拖累其它账号。
 *
 * ── CodeArts 为什么也在自动补领里 ──────────────────────────
 * 它的新手任务只有「新人注册礼」这一条，且是**一次性**的：领过一次之后上游就
 * 不再回 `claimable`，于是下一轮查询的 `unclaimed=0`，之后每次签到都只是签到
 * —— 自动补领在这里只会发一次写请求，与 Loomy 同构。
 * 边界仍然留着：本家**不进** `auto_checkin` 的提供商清单（那是后端的定时任务，
 * 无人看守），这里的"自动"只发生在用户刚点过签到之后。另外两类一次性活动
 * （学生认证 / 邀请礼）连入口都没有，判据见 `Campaign::is_newbie_gift`。
 *
 * `rows` 传快照的 `extras.onboarding`（全部支持新手任务的账号）；单账号签到时
 * 传只含该账号的数组（用户点的是谁就处理谁）。领取完成后 claimOnboarding
 * 内部会重拉快照，「待领新手任务」总览卡随之归零。
 *
 * ── 已结算的账号跳过 ────────────────────────────────────────
 * 快照带结算记忆、或缓存里 `settled` 已置位的账号**不发查询**（这正是本功能
 * 要消灭的那次多余上游请求）；真出了新一期活动，用户手点「查询任务」时会被
 * 实查发现（`refresh`），后端届时也会清掉记忆。
 */
async function autoProcessOnboarding(
  rows: Array<{ id: string; provider?: string; settled?: OnboardingSettledRaw }>,
): Promise<void> {
  for (const row of rows) {
    if (settledOf(row.id)) continue
    await queryOnboarding(row.id)
  }
  for (const row of rows) {
    const cache = getCheckinStore().onboarding.get(row.id)
    if (cache?.status !== 'loaded' || cache.claiming || cache.settled) continue
    // OfficeAce：奖励由上游按天自动发放、账本条目一律已到账（待领数恒 0），
    // 「有没有待领」判不出「今天结算过没」—— 与官方客户端每小时自动 claim 同一
    // 动作，这里进来就触发一次（幂等；结算后 settled 置位，当天不再重打）
    if (row.provider === 'officeace' || cache.unclaimed > 0) {
      await claimOnboarding(row.id)
    }
  }
}

/** 这个账号的一次性福利是否已结算（缓存里的 `settled`；铺缓存见 `seedSettledCaches`） */
function settledOf(id: string): boolean {
  return store.onboarding.get(id)?.settled === true
}

function onboardingOf(id: string): OnboardingCache {
  return store.onboarding.get(id) ?? {
    status: 'idle', tasks: [], unclaimed: 0, earned: 0, total: 0, claiming: false,
  }
}

function patchOnboarding(id: string, next: Partial<OnboardingCache>): void {
  const map = new Map(store.onboarding)
  map.set(id, { ...onboardingOf(id), ...next })
  patch({ onboarding: map })
}

/**
 * 查询一个账号的任务状态（不领，只读）。
 *
 * `expand: true`（界面上点「查询任务」）在查到任务后**默认展开**清单 ——
 * 查询这个动作本身就是「我要看」，让它多付一次点击才能看到结果是反的；
 * 签到后的自动处理不展开（autoProcessOnboarding），不让后台动作替用户动界面。
 *
 * `refresh: true`（只有界面上点「查询任务」才传）要求后端**实查上游**：一次性
 * 福利结算过之后后端默认吃记忆（零上游），手点那颗按钮的语义就是「我不信缓存，
 * 现在去问一次」—— 上游真出了新一期活动，这一次就会看到，后端也会同步清掉记忆。
 */
export async function queryOnboarding(
  id: string,
  options?: { expand?: boolean; refresh?: boolean },
): Promise<void> {
  const current = onboardingOf(id)
  if (current.status === 'loading' || current.claiming) return
  patchOnboarding(id, { status: 'loading', error: undefined, settled: false })
  try {
    const data = await bridge().getOnboardingTasks?.(id, options?.refresh === true)
    const tasks = normalizeTasks(data?.tasks)
    const settled = data?.settled === true
    patchOnboarding(id, {
      status: 'loaded',
      checkedAt: Date.now(),
      tasks,
      // blocked（前置没满足，如 CodeArts 新人礼未到门槛）不算待领：把它们计入
      // 待领数会让「一键领取（N）」承诺一件点成失败的事，而一次性的东西失败不起
      unclaimed: tasks.filter(task => !task.done && !task.blocked).length,
      earned: numberOf(data?.earned),
      total: numberOf(data?.total),
      settled,
      settledAt: settled ? numberOf(data?.settledAt) || undefined : undefined,
    })
    if (options?.expand === true && tasks.length > 0 && !onboardingExpanded(id)) {
      toggleOnboardingExpand(id)
    }
  } catch (error) {
    patchOnboarding(id, { status: 'error', error: errorMessage(error), settled: false })
  }
}

/**
 * 一键领取：串行上报全部未完成的 key（服务端幂等，重复点不会重复加分）。
 * 行级状态实时推进；完成后重拉快照 —— 任务状态本身不在快照里，
 * 重拉是为了让时间线 / 总览跟上一轮刚发生的动作。
 */
export async function claimOnboarding(id: string): Promise<void> {
  const current = onboardingOf(id)
  if (current.claiming || current.status !== 'loaded') return
  patchOnboarding(id, { claiming: true })
  try {
    // 行级「领取中」：失败过的行也重试（服务端幂等，重试是唯一出路）
    patchOnboarding(id, {
      tasks: onboardingOf(id).tasks.map(task =>
        task.done ? task : { ...task, claiming: true, error: undefined },
      ),
    })
    const data = await bridge().claimOnboardingTasks?.(id)
    const failedKeys = new Map<string, string>()
    const rows = Array.isArray(data?.results) ? data.results : []
    for (const row of rows) {
      const key = typeof row?.key === 'string' ? row.key : ''
      if (key && row?.ok !== true) failedKeys.set(key, String(row?.error || t('领取失败')))
    }
    const server = normalizeTasks(data?.tasks)
    const merged = (server.length ? server : onboardingOf(id).tasks).map(task => ({
      ...task,
      claiming: false,
      error: failedKeys.get(task.key),
    }))
    const claimedCount = merged.filter(task => task.done && !failedKeys.has(task.key)).length
    // 后端在这一轮把一次性福利结清了（响应带 settled）⇒ 本地也置位：此后进页面
    // 直接用记忆渲染，不再为这个账号发查询（要实查仍可手点「查询任务」）。
    const settled = data?.settled === true
    patchOnboarding(id, {
      claiming: false,
      checkedAt: Date.now(),
      tasks: merged,
      unclaimed: merged.filter(task => !task.done && !task.blocked).length,
      earned: numberOf(data?.earned),
      total: numberOf(data?.total),
      settled,
      settledAt: settled ? numberOf(data?.settledAt) || undefined : undefined,
    })
    const failed = merged.filter(task => task.error && !task.done).length
    // 后端交代本次结果时以它为准（OfficeAce 会回「本次到账 N 积分」/「今天的
    // 奖励已发放」——那种上游的账本条目一律已到账，"有几项 done" 的口径不成立）
    const message = typeof data?.message === 'string' ? data.message.trim() : ''
    if (failed) {
      toast(t('新手福利有 {n} 项领取失败，可重试', { n: failed }), 'err')
    } else if (message) {
      toast(message, 'ok')
    } else if (claimedCount) {
      toast(t('✅ 新手福利领取完成，累计 {n} 积分', { n: numberOf(data?.earned) }))
    }
  } catch (error) {
    const message = errorMessage(error)
    patchOnboarding(id, {
      claiming: false,
      tasks: onboardingOf(id).tasks.map(task => ({
        ...task,
        claiming: false,
        error: task.done ? undefined : message,
      })),
    })
    toast(t('领取失败：{error}', { error: message }), 'err')
  } finally {
    await loadCheckinCenter()
  }
}

/* ─── 成长任务（惰性查询 + 执行；WorkBuddy 国内版）─── */

function normalizeGrowthTasks(raw: unknown): GrowthTaskRow[] {
  const list = Array.isArray(raw) ? raw : []
  return list
    .map(item => {
      const row = (item ?? {}) as Record<string, unknown>
      const code = typeof row.code === 'string' ? row.code : ''
      if (!code) return null
      return {
        code,
        title: typeof row.title === 'string' && row.title ? row.title : code,
        description: typeof row.description === 'string' ? row.description : '',
        status: typeof row.status === 'string' ? row.status : '',
        current: numberOf(row.current),
        target: numberOf(row.target),
        locked: row.locked === true,
        unlockAt: row.unlockAt,
        supported: row.supported === true,
        unforgeable: row.unforgeable === true,
        unforgeableReason: typeof row.unforgeableReason === 'string' ? row.unforgeableReason : '',
        kind: typeof row.kind === 'string' ? row.kind : '',
        source: typeof row.source === 'string' ? row.source : '',
      } as GrowthTaskRow
    })
    .filter((row): row is GrowthTaskRow => row !== null)
}

function normalizeGrowthResults(raw: unknown): GrowthRunRow[] {
  const list = Array.isArray(raw) ? raw : []
  return list
    .map(item => {
      const row = (item ?? {}) as Record<string, unknown>
      const code = typeof row.code === 'string' ? row.code : ''
      if (!code) return null
      return {
        code,
        outcome: typeof row.outcome === 'string' ? row.outcome : 'failed',
        message: typeof row.message === 'string' ? row.message : '',
        credit: numberOf(row.credit),
        energy: numberOf(row.energy),
      } as GrowthRunRow
    })
    .filter((row): row is GrowthRunRow => row !== null)
}

function growthOf(id: string): GrowthCache {
  return store.growth.get(id) ?? {
    status: 'idle', energy: null, streak: null, tasks: [],
    travel: null, school: null,
    running: false, runningSchool: false, runningTravel: false,
  }
}

function patchGrowth(id: string, next: Partial<GrowthCache>): void {
  const map = new Map(store.growth)
  map.set(id, { ...growthOf(id), ...next })
  patch({ growth: map })
}

/** 某账号的成长任务清单当前是否展开（与新手任务共用 `expanded` 集合） */
export function growthExpanded(id: string): boolean {
  return store.expanded.has(`growth:${id}`)
}

/** 展开 / 收起一个账号的成长任务清单 */
export function toggleGrowthExpand(id: string): void {
  const key = `growth:${id}`
  const next = new Set(store.expanded)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  patch({ expanded: next })
}

/**
 * 查询一个账号的成长任务盘点（只读）。
 *
 * 后端一次拉四块（成长任务 / 能量 / 猫猫旅行 / 开学季概况），某一块失败只在该块
 * 给 error（不至于整行变红）；这里如实把每块的失败都收进 cache。
 */
export async function queryGrowth(id: string): Promise<void> {
  const current = growthOf(id)
  if (current.status === 'loading' || current.running) return
  patchGrowth(id, { status: 'loading', error: undefined })
  try {
    const data = await bridge().getGrowthStatus?.(id)
    const tasksBlock = (data?.tasks ?? {}) as { tasks?: unknown; error?: unknown }
    const travel = (data?.travel ?? null) as GrowthCache['travel']
    const schoolRaw = (data?.school ?? null) as (GrowthCache['school'] & { tasks?: unknown }) | null
    patchGrowth(id, {
      status: 'loaded',
      checkedAt: Date.now(),
      energy: data?.energy === null || data?.energy === undefined ? null : numberOf(data.energy),
      streak: data?.streak === null || data?.streak === undefined ? null : numberOf(data.streak),
      tasks: normalizeGrowthTasks(tasksBlock.tasks),
      travel,
      school: schoolRaw
        ? { ...schoolRaw, tasks: normalizeGrowthTasks(schoolRaw.tasks) }
        : null,
      // 任务列表本身的失败挂在行级 error 上（账号级 status 仍是 loaded ——
      // 其余三块有数据可看，不该整行显示成「查询失败」）
      error: typeof tasksBlock.error === 'string' ? tasksBlock.error : undefined,
    })
  } catch (error) {
    patchGrowth(id, { status: 'error', error: errorMessage(error) })
  }
}

/**
 * 执行成长任务（`mode=full` 点亮+领奖；`mode=claim` 只领已完成奖励）。
 *
 * ── 为什么在本地循环而不是一次全量请求 ──────────────────────
 * 后端把执行拆成**单任务**端点：全量逐条留 1s+ 写间隔（上游风控口径）会超过
 * 管理 API 的 60 秒转发超时，一次全量请求会被中途掐断。这里逐条 await、
 * 逐条把结果落进 `results` —— 界面实时看到进度，失败的任务不影响后续。
 *
 * ── 挑选口径 ────────────────────────────────────────────────
 * 只跑映射表支持、非不可伪造、未锁定的任务；`claim` 模式再收窄到「已完成」的
 * （点亮是写操作，不属于「领取」的语义）。
 */
export async function runGrowth(id: string, mode: 'full' | 'claim'): Promise<void> {
  const current = growthOf(id)
  if (current.running) return
  const targets = (current.tasks ?? []).filter(task => {
    if (!task.supported || task.unforgeable === true) return false
    if (task.locked === true) return false
    if (mode === 'claim') {
      return task.status === 'completed' || (task.current >= task.target && task.status !== 'claimed')
    }
    return task.status !== 'claimed'
  })
  if (targets.length === 0) {
    toast(t('没有需要执行的任务'))
    return
  }
  patchGrowth(id, { running: true, results: [], error: undefined })
  let credit = 0
  let energy = 0
  let done = 0
  let failed = 0
  for (const task of targets) {
    try {
      const data = await bridge().runGrowthTask?.(id, task.code, mode)
      const row = normalizeGrowthResults([data])[0]
      if (row) {
        // 逐条落进 results（用户在展开面板里实时看到「跑到哪一条了」）
        patchGrowth(id, { results: [...(growthOf(id).results ?? []), row] })
        credit += row.credit ?? 0
        energy += row.energy ?? 0
        if (row.outcome === 'done' || row.outcome === 'already') done += 1
        if (row.outcome === 'failed') failed += 1
      }
    } catch (error) {
      failed += 1
      patchGrowth(id, {
        results: [...(growthOf(id).results ?? []), {
          code: task.code,
          outcome: 'failed',
          message: errorMessage(error),
          credit: 0,
          energy: 0,
        }],
      })
    }
  }
  patchGrowth(id, { running: false })
  if (failed > 0) {
    toast(t('成长任务完成（{done} 项成功、{failed} 项失败），积分 +{credit} 能量 +{energy}', {
      done, failed, credit, energy,
    }), 'err')
  } else {
    toast(t('✅ 成长任务执行完成：积分 +{credit} 能量 +{energy}', { credit, energy }))
  }
  // 执行会改任务状态：重查一次让清单回到最新（读路径，不再写）
  await queryGrowth(id)
}

/** 执行开学季任务；`lottery=true` 时任务后把抽奖余额抽空。 */
export async function runGrowthSchool(id: string, lottery: boolean): Promise<void> {
  const current = growthOf(id)
  if (current.runningSchool) return
  patchGrowth(id, { runningSchool: true, error: undefined })
  try {
    const data = await bridge().runGrowthSchool?.(id, lottery)
    const results = normalizeGrowthResults(data?.results)
    const lotteryRaw = (data?.lottery ?? null) as { message?: unknown; drawn?: unknown; credit?: unknown } | null
    patchGrowth(id, {
      runningSchool: false,
      results,
      // 逐条概要（开学季的任务码与成长任务不同集合，不塞进任务行的 resultOf 匹配；
      // 直接把执行结论拼成一行文本给界面展示）
      schoolText: results.length
        ? results.map(row => `${row.code}：${row.message}`).join('；')
        : undefined,
      lotteryText: lotteryRaw
        ? String(lotteryRaw.message ?? t('已抽 {n} 次', { n: numberOf(lotteryRaw.drawn) }))
        : undefined,
      checkedAt: Date.now(),
    })
    if (lotteryRaw) {
      toast(t('开学季执行完成：{message}', { message: String(lotteryRaw.message ?? '') }))
    } else {
      toast(t('✅ 开学季任务执行完成'))
    }
    await queryGrowth(id)
  } catch (error) {
    patchGrowth(id, { runningSchool: false, error: errorMessage(error) })
    toast(t('开学季执行失败：{error}', { error: errorMessage(error) }), 'err')
  }
}

/** 猫猫旅行巡检一次（无猫领养 / 空闲派出 / 到站领奖，只推进一步）。 */
export async function runGrowthTravel(id: string): Promise<void> {
  const current = growthOf(id)
  if (current.runningTravel) return
  patchGrowth(id, { runningTravel: true, error: undefined })
  try {
    const data = await bridge().runGrowthTravel?.(id)
    const message = String(data?.message ?? '')
    const outcome = String(data?.outcome ?? '')
    patchGrowth(id, { runningTravel: false, checkedAt: Date.now() })
    if (outcome === 'failed') {
      toast(t('猫猫旅行失败：{message}', { message }), 'err')
    } else {
      toast(t('猫猫旅行：{message}', { message }))
    }
    await queryGrowth(id)
  } catch (error) {
    patchGrowth(id, { runningTravel: false, error: errorMessage(error) })
    toast(t('猫猫旅行失败：{error}', { error: errorMessage(error) }), 'err')
  }
}

/* ─── 展示工具 ─────────────────────────────── */

/** 分组状态：全部已签 / 部分 / 全部待签 / 没有账号 */
export function groupTone(group: CheckinProviderGroup): 'done' | 'part' | 'todo' | 'none' {
  if (!group.totalCount) return 'none'
  if (group.doneCount >= group.totalCount) return 'done'
  if (group.doneCount > 0) return 'part'
  return 'todo'
}

/** 历史条目的一行摘要（时间线副标题） */
export function historyLine(entry: CheckinHistoryEntry): string {
  const succeeded = numberOf(entry.succeeded)
  const active = numberOf(entry.active)
  const total = numberOf(entry.total)
  const skipped = numberOf(entry.skipped)
  const failed = numberOf(entry.failedCount)
  const parts = [t('成功 {n}', { n: succeeded })]
  if (active > 0) parts.push(t('保活 {n}', { n: active }))
  if (skipped > 0) parts.push(t('跳过 {n}', { n: skipped }))
  if (failed > 0) parts.push(t('失败 {n}', { n: failed }))
  return `${parts.join(' · ')}${total ? t('（共 {n} 个账号）', { n: total }) : ''}`
}

/** 「下次执行」的人话（today / tomorrow 相对面板时刻） */
export function nextRunText(auto: AutoCheckinState | null | undefined): string {
  if (auto?.enabled !== true) return t('未开启')
  const at = Number(auto.nextRunAt)
  const time = auto.time ?? ''
  if (!Number.isFinite(at) || at <= 0) return formatTime(at) || t('每天 {time}', { time })
  return t('{date}（每天 {time}）', { date: formatTime(at), time })
}

/**
 * 条目视图里的聚合「下次执行」（多条目里最近的一次；总开关关着 / 都没排上 → null）。
 * 签到中心总览卡与顶栏徽标用聚合值，不必关心是哪一条给的。
 */
export function nextScheduleRun(snapshot: SchedulesSnapshot | null | undefined): number | null {
  const entries = Array.isArray(snapshot?.schedules) ? snapshot!.schedules! : []
  const times = entries
    .filter(entry => entry.enabled === true)
    .map(entry => Number(entry.nextRunAt))
    .filter(value => Number.isFinite(value) && value > 0)
  return times.length ? Math.min(...times) : null
}
