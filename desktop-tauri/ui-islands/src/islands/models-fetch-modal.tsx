import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from '@ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { t } from '../i18n'
import { ModelsFetchError } from './models-fetch-error'
import { ModelsFetchTable, type RefreshItem, type Row } from './models-fetch-table'

/**
 * Agent2API · 「获取模型」弹窗（模型管理页那颗按钮的本体）。
 *
 * 替换 ui/models-fetch-modal.js（innerHTML 拼 .modal-mask / .models-table 那套老类名）；
 * 对外接口与原实现**完全一致**：
 * `window.wbModelsFetchModal.open({ providerId, custom, name, onDone, onRefreshed })`，
 * 调用点（models-panel.js 的 refreshModels）一行都不用改。
 *
 * ── 两种提供商，两种形态 ────────────────────────────────────
 * · **自定义家**：清单由用户逐条登记，「拉取 → 勾选 → 导入」完整成立 —— 拉回来的 id 与本地清单
 *   比对，已在清单里的标「已添加」且复选框**禁用**（不重复导入），默认勾选未添加的那些；导入走
 *   整表提交（models-custom-source.js 的 addModels）。
 * · **内置家**：清单来自平台统一目录，上游有什么就自动有什么，没有「导入」这一步 —— 弹窗退化成
 *   「刷新各家远程目录 + 把逐家结果列清楚」（旧实现原先只有一句 3.5 秒就消失的 toast）。
 *
 * ── 打开**不自动拉取** ─────────────────────────────────────
 * 一次拉取是**逐家打上游**（十家各一次网络请求），而用户点开弹窗可能只是想看一眼各家的清单时效、
 * 或改一下「模型来源」用哪个账号；打开即拉等于把「看一眼」也变成一次全量刷新，还让「本次结果」与
 * 「上次结果」在界面上分不清。所以第一屏只渲染待获取的名单，拉取由用户点最右那颗「获取模型」触发。
 *
 * ── 逐行拉取：只刷一家，不动别家 ─────────────────────────────
 * 底部那颗是**全刷**；每一行最右还有一颗「获取 / 重新获取」——它把请求里的 `providers` 白名单收窄
 * 成这一家（后端逐家打上游，名单外的家不打网络、也不进结果，见 api/models.rs 的 `parse_providers`），
 * 回来后把这一条**并进**结果表，其余行原样留着。理由：目录刷新的成本是逐家一律打一次上游，而用户
 * 点进来常常只是刚给某一家换了账号、或想补一家的清单 —— 为这一家把十家全打一遍既慢又吵。
 *
 * 因此表体是**一条行模型**（见 buildRows）：待获取的家与已有结果的家合成同一种行，行序 = 注册表序
 * （= 左栏序；后端逐家结果与它同序，见 providers::PROVIDERS 与 implemented_kinds）。
 *
 * ── 两个回调的语义都是「后端数据变了，你那边该重取一次」──────────
 * 本弹窗只把变化告诉调用方，不替它取数：`onDone` 在自定义家导入成功后、`onRefreshed` 在内置家远程
 * 目录**真落地**后（`result.refreshed > 0`；失败 / 跳过时清单一个字没变，不通知）。
 *
 * ── 与旧实现的**刻意**差别（结构性的，业务口径不动）────────────
 * 1. 表元素 id 保留（`fm-table-intl` / `fm-table-custom`）：列宽拖动层（table-columns.js）正是按
 *    这两个选择器找表的（见它 TABLES 的 root），`col` / `th` 的 `data-col` 也是它定位列的依据 ——
 *    去掉 id 会让这两张表的列宽拖动与持久化静默失效；其余 DOM id 一律不留。
 * 2. 列宽百分数从 CSS 搬到 `<col>` 的类名上（旧 CSS 按 `#fm-table-intl .f-provider` 写，随 id 一起
 *    失效）；表仍需 `table-layout: fixed`（旧 .models-table 带的），否则百分数不生效。
 * 3. 「模型来源」的落盘记忆改成直接读同一个 localStorage 键，理由见 loadSavedSource。
 */

/* ─── 类型 ───────────────────────────────────── */

/** 账号公开形态里本弹窗用到的字段（其余不关心） */
type Account = {
  id?: unknown; provider?: string; name?: string; nickname?: string; email?: string
  nameCustom?: boolean
  available?: boolean; enabled?: boolean
}

/** 内置家注册表里的一项（`wbProviders.all()`）；自定义家目录里的一项（`customList()`） */
type ProviderMeta = { id?: string; label?: string }
type CustomProvider = { id?: string; models?: Array<{ id?: unknown }> }

type RefreshResponse = { results?: RefreshItem[]; refreshed?: unknown }

/** open() 的入参（与旧实现逐字一致） */
type OpenOptions = {
  providerId: string
  /** 由调用方判好：true = 自定义家 */
  custom?: boolean
  /** 展示名（标题用）；缺省回落 providerId */
  name?: string
  /** 自定义家：导入成功后回调；内置家：远程目录真落地后回调（各自重取自己的数） */
  onDone?: () => void
  onRefreshed?: () => void
}

/** 表体的四种屏态：还没拉过 / 拉取中 / 拉取失败 / 有结果 —— 用联合而不是几个布尔，是为了让
 *  「还没拉」与「拉失败」在类型上就分得开（两者界面文案不同）。 */
type Status = 'idle' | 'loading' | 'error' | 'ready'

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。刻意用「局部窄类型 + 转型读取」而不是 declare global
 * 往 Window 上加属性：这些桥是多个岛共用的，若每个岛各 declare 一份，接口合并会因同名属性类型
 * 不一致直接报 TS2717 —— 并行迁移时必然互相撞车。本文件只 declare 自己独占的
 * wbModelsFetchModal（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: {
    /** POST /api/models/refresh：`{accounts, providers}` → `{results, refreshed, …}` */
    refreshModels(payload: {
      accounts: Record<string, string>; providers: string[]
    }): Promise<RefreshResponse | null | undefined>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    getState?: () => { accounts?: { accounts?: Account[] } } | null | undefined
    formatTime?: (value: unknown) => string
  }
  /** 裸全局：app.js 的顶层函数声明，旧实现直接写 formatTime(...) */
  formatTime?: (value: unknown) => string
  wbProviders?: {
    all?: () => ProviderMeta[] // 内置家清单（同步读主状态，只有内置家）
    customList?: () => CustomProvider[] // 自定义家目录缓存（同步读）
    /** 通用管理 API 调用（自定义家那条 POST /api/custom-providers/fetch-models 走它） */
    customRequest?: (method: string, path: string, body?: unknown) => Promise<{ models?: unknown[] } | null | undefined>
  }
  wbModelsPanel?: {
    /** 模型管理页左栏**实有清单**的家（本弹窗的刷新范围读它） */
    builtinProviders?: () => string[]
    /** 某家清单当前的拉取时刻（毫秒，0 = 未知 / 从未成功过） */
    providerRefreshedAt?: (providerId: string) => number
  }
  wbModelsCustom?: { addModels?: (id: string, modelIds: string[]) => Promise<unknown> } // 批量登记
  wbFilterMemory?: { save?: (key: string, patch: Record<string, string>) => void } // 只用它的 save
  wbAccountsModel?: {
    /** 家的特性表（emailAsName 的家用邮箱当展示名，见 accountLabel） */
    providerFeatures?: (provider?: string) => { emailAsName?: boolean } | undefined
    byPriorityOrder?: (a: Account, b: Account) => number // 账号页同一条优先级排序
  }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 常量 ───────────────────────────────────── */

/**
 * 目录刷新**不走账号维度**的家（与后端 `ProviderAdapter::refresh_uses_account` 的 false 集合同
 * 源）：Cline 的清单接口无鉴权、内容是全局的，「用哪个账号去拉」对它没有意义 —— 那一列显示为空
 * 占位，而不是给一个选了也一样的下拉。
 */
const ACCOUNTLESS_REFRESH = new Set(['cline-free', 'cline-pass'])

/** 「模型来源」的落盘键：`{providerId: accountId}`（见 loadSavedSource 的说明） */
const SOURCE_KEY = 'workbuddy-desktop-fetch-model-source'

/* ─── 纯函数工具 ──────────────────────────────── */

/** 判重口径：去空白 + 大小写不敏感（与后端及其它入口一致） */
function norm(value: unknown): string {
  return String(value ?? '').trim().toLowerCase()
}

/** 脚注 / toast 的统一出口：toast 几秒后就没了，脚注留一份 */
function toast(message: string, kind?: 'err' | 'ok') {
  shared().wbApp?.toast?.(message, kind)
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 时间戳 → 本地时间串（app.js 的 formatTime：0 与非法值返回空串，调用处换占位符）。裸全局与
 *  wbApp.formatTime 是同源的两份，取得到哪个用哪个 —— 岛的模块体可能先于 app.js 执行，但真正
 *  取数的时刻一定在用户点开弹窗之后，两者都已就绪。 */
function formatTime(value: number): string {
  const fn = shared().formatTime ?? shared().wbApp?.formatTime
  return String(fn?.(value) ?? '')
}

/** 主状态里的账号清单（与旧实现同一来源，不另拉接口） */
function accounts(): Account[] {
  return shared().wbApp?.getState?.()?.accounts?.accounts || []
}

/** 该家当前清单里的模型 id（读目录缓存；自定义家才有意义） */
function managedIds(providerId: string): Set<string> {
  const list = shared().wbProviders?.customList?.() || []
  const models = list.find(item => item.id === providerId)?.models
  return new Set((Array.isArray(models) ? models : []).map(model => norm(model?.id)).filter(Boolean))
}

/** 账号的展示名：与账号页同一分流（displayNameOf）—— 设过备注（nameCustom）
 *  恒用备注名；未打标维持旧口径：以邮箱报名字的家（Qoder / AutoClaw 国际版 /
 *  Accio）直接用邮箱，其余取昵称 / 名称 / 邮箱 / id。 */
function accountLabel(account: Account): string {
  if (account?.nameCustom === true && account?.name) return account.name
  const email = String(account?.email || '').trim()
  if (shared().wbAccountsModel?.providerFeatures?.(account?.provider)?.emailAsName && email) return email
  const name = String(account?.nickname || account?.name || email || account?.id || '').trim()
  return name || t('未命名账号')
}

/** 该家可用于拉取目录的账号（「模型来源」下拉的选项）。过滤 = `available`（后端口径：启用 + 有
 *  凭证）+ `enabled`；排序用账号页同一条 `byPriorityOrder`（前端渲染层与后端 `order_key` 是同构
 *  实现，两边算出的「第一个账号」必然是同一条）。账号清单直接读主状态，不另拉接口。 */
function sourceAccounts(providerId: string): Account[] {
  if (ACCOUNTLESS_REFRESH.has(providerId)) return []
  const order = shared().wbAccountsModel?.byPriorityOrder
  const list = accounts().filter(account => (account?.provider || 'workbuddy') === providerId
    && account?.available !== false && account?.enabled !== false)
  return order ? list.sort(order) : list
}

/**
 * 本次刷新的范围：**模型管理页左栏实有清单的家 ∪ 有启用账号的家**。
 *
 * 与「模型来源」下拉同一哲学（见 sourceAccounts）：用户在界面上都看不到的家（没有启用账号、清单
 * 也为空）不该参加刷新 —— 刷它只会得到一行「缺少登录态」，而用户既没有账号可选、列表里也根本
 * 没有这家，无从理解。名单随请求带给后端（见 api/models.rs 的 providers）：名单外的家不打网络、
 * 也不进结果。
 *
 * 「有启用账号的家」并进来，是为「刚加完账号、清单还是空的」这一档：它暂时不在左栏，但用户马上
 * 就该看到它 —— 刷一次清单就有了。左栏数据缺席（页面数据未加载）时不降级成全量：宁缺毋滥，自动
 * 刷新任务会兜底。
 */
function scopeProviders(): string[] {
  const ids = new Set(shared().wbModelsPanel?.builtinProviders?.() || [])
  for (const account of accounts()) {
    if (account?.available === false || account?.enabled === false) continue
    ids.add(account?.provider || 'workbuddy')
  }
  return [...ids]
}

/** 待获取那一屏要列的家：按注册表顺序（与模型管理页左栏同一序）列 scoped 名单 —— `all()` 只有
 *  内置家，自定义家的 id 即使进了 scopeProviders 也不会出现在这里。 */
function pendingProviders(): ProviderMeta[] {
  const scoped = new Set(scopeProviders())
  return (shared().wbProviders?.all?.() || []).filter(item => scoped.has(item?.id ?? ''))
}

/**
 * 读「模型来源」的落盘记忆：`{providerId: accountId}` 的扁平对象。
 *
 * 为什么走 localStorage 而不是主状态：这是一份纯界面偏好（「用哪个账号去拉这家的目录」），不参与
 * 转发，后端没有它的位置；而「上次选的那条」要跨重启生效，只能落盘。
 *
 * 读法与旧实现有一处**刻意**差别：旧实现写的是 `wbFilterMemory.load(SOURCE_KEY, {})`，而那个
 * load 会把结果**按 defaults 的键收窄**（`for (const name of Object.keys(defaults))`）—— defaults
 * 是空对象时它永远返回 {}，落盘的记忆实际上从来没被读回来过（只有本次会话里改过的值在内存里有
 * 效）。这里直接读同一个键（形状就是 wbFilterMemory.save 写进去的那份），让「上次改的那条还在
 * 列表里就用它」真的成立。若要逐字保留旧行为，把函数体换成
 * `shared().wbFilterMemory?.load?.(SOURCE_KEY, {}) ?? {}`。
 */
function loadSavedSource(): Record<string, string> {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(SOURCE_KEY) || '{}')
    if (!raw || typeof raw !== 'object') return {}
    const out: Record<string, string> = {}
    // 非字符串 / 空串一律丢掉：存坏的值不该让下拉选中一个不存在的账号
    for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
      if (typeof value === 'string' && value) out[key] = value
    }
    return out
  } catch {
    return {} // 存坏 / 存储不可用：记忆是锦上添花，不能挡弹窗打开
  }
}

/** 落盘（合并写回，patch 覆盖存量 —— 与 wbFilterMemory.save 同一口径） */
function saveSavedSource(patch: Record<string, string>) {
  shared().wbFilterMemory?.save?.(SOURCE_KEY, patch)
}

/* ─── 弹窗本体 ───────────────────────────────── */

/** 弹窗组件：`options` 是 open() 的入参，`onClose` 由命令式外壳提供 */
function ModelsFetchModal({ options, onClose }: { options: OpenOptions; onClose: () => void }) {
  const providerId = options.providerId
  const custom = Boolean(options.custom)
  const name = options.name || providerId
  const { onDone, onRefreshed } = options

  const [status, setStatus] = React.useState<Status>('idle')
  const [errorText, setErrorText] = React.useState('')
  const [hint, setHint] = React.useState('')
  /**
   * 在途状态（全刷 / 逐行 / 导入，三者互斥）：置真期间不许关窗，关掉会让「到底成没成」变成未知；
   * 三种的按钮文案不同（前两种都是「获取中…」，导入是「导入中…」），所以记的是种类而不是一个布尔。
   */
  const [busyKind, setBusyKind] = React.useState<'load' | 'import' | 'row' | null>(null)
  /** 逐行取数时**哪一家**在途（`busyKind === 'row'` 时有值）：那一行的按钮转「获取中…」 */
  const [busyRow, setBusyRow] = React.useState('')
  const busy = busyKind !== null
  const failed = status === 'error'
  /**
   * 本次会话是否**已经拉过**一次（打开时是 false）。弹窗打开**不自动拉取**，所以「拉过没有」
   * 是一份真实状态：按钮文案（获取模型 / 重新获取）与自定义家的汇总行都读它。
   */
  const [fetched, setFetched] = React.useState(false)
  /** 自定义家：上游拉回来的模型 id（去重、保序、原样大小写） */
  const [upstream, setUpstream] = React.useState<string[]>([])
  /** 自定义家：本地清单里已有的 id（小写集合，判「已添加」） */
  const [managed, setManaged] = React.useState<ReadonlySet<string>>(() => new Set<string>())
  /** 自定义家：勾选中的 id（原样大小写，导入时按它提交） */
  const [picked, setPicked] = React.useState<ReadonlySet<string>>(() => new Set<string>())
  /** 内置家：逐家刷新结果（status === 'ready' 时的表体） */
  const [results, setResults] = React.useState<RefreshItem[]>([])
  /** 自定义家：搜索词（受控；旧实现是 input 事件即时过滤） */
  const [keyword, setKeyword] = React.useState('')
  /** 「模型来源」的落盘记忆（打开时读一次；选中即写回并更新本状态） */
  const [savedSource, setSavedSource] = React.useState<Record<string, string>>(loadSavedSource)

  /** 内置家：本次范围里的家（与 scopeProviders 同源，也就是真正会进请求的名单） */
  const pending = custom ? [] : pendingProviders()
  /** 内置家的表体行（待获取的家 + 已有结果的家，见 buildRows）；自定义家没有这条模型 */
  const rows = custom ? [] : buildRows()

  /**
   * 汇总行（表格左端，旧实现 paintSummary 的文案与状态语义）：自定义家是「已选 N / 上游共
   * M / 清单已有 K」，内置家是「N 家 · 成功 X · 失败 Y」。
   */
  const summary = custom
    // 还没拉过时三个计数都是 0，列出来只会让人以为「上游没有模型」——那与「还没拉」是两件事
    ? fetched
      ? t('已选 {picked} 个 · 上游共 {upstream} 个 · 清单已有 {managed} 个',
        { picked: picked.size, upstream: upstream.length, managed: managed.size })
      : t('尚未获取')
    : intlSummary(rows)

  /**
   * 内置家的表体行：按注册表顺序（= 左栏顺序）列出本次范围里的家，已有结果的那几行挂上自己那一条
   * 结果。行序不随后端结果的到达顺序变 —— 逐行「获取」回来后只有那一行的内容变化，位置不动。
   */
  function buildRows(): Row[] {
    const byProvider = new Map(results.map(item => [String(item.provider || ''), item]))
    const built: Row[] = pending.map(meta => {
      const id = String(meta.id || '')
      return { provider: id, label: String(meta.label || id), item: byProvider.get(id) }
    })
    const known = new Set(built.map(row => row.provider))
    // 后端给了结果、却不在本次待获取名单里的家（理论上不该有）：附在末尾，别把它藏起来
    for (const item of results) {
      const id = String(item.provider || '')
      if (!known.has(id)) built.push({ provider: id, label: String(item.providerLabel || id), item })
    }
    return built
  }

  /**
   * 内置家的汇总文案（待获取 / 刷新中 / 失败 / 逐行结果四档）。计数按**行模型**算而不是按
   * `results.length`：逐行刷过一家之后表里仍是全部家，成功数只该算真拿到结果的那几家。
   */
  function intlSummary(list: Row[]): string {
    if (status === 'loading') return t('正在刷新…')
    // 失败时不留上一轮的读数：那是旧结果，与整屏的「获取失败」自相矛盾
    if (status === 'error') return ''
    const items = list.map(row => row.item).filter((item): item is RefreshItem => Boolean(item))
    if (!items.length) {
      // 点了刷、但（这些家）一条结果都没回来：如实说「没得到结果」，别提「点获取开始」
      if (fetched) return t('刷新完成，但没有得到任何结果')
      return list.length ? t('共 {n} 家 · 点「获取模型」开始', { n: list.length }) : t('0 家')
    }
    const done = items.filter(item => item.status === 'refreshed').length
    const failed = items.filter(item => item.status === 'failed').length
    return failed
      ? t('{total} 家 · 成功 {done} · 失败 {failed}', { total: list.length, done, failed })
      : t('{total} 家 · 成功 {done}', { total: list.length, done })
  }

  /**
   * 「模型来源」格的有效选中值 —— 三档优先级（逐字照抄旧实现 sourcePicker 的口径）：**落盘的
   * 选择**（上次改的那条还在列表里就用它）→ **后端回读的实际账号**（`item.accountId`；用户没选过
   * 时它就是「这次真正用的是谁」，队首）→ **列表第一条**（默认 = 队首可用账号，与后端默认选取同
   * 一条）。无可选账号与不走账号维度的家都返回空串。
   */
  function selectSource(pid: string, reported?: unknown): { list: Account[]; selected: string } {
    const list = sourceAccounts(pid)
    if (!list.length) return { list, selected: '' }
    const saved = String(savedSource[pid] || '')
    const reportedId = String(reported || '')
    const has = (id: string) => list.some(account => String(account.id) === id)
    return { list, selected: has(saved) ? saved : has(reportedId) ? reportedId : String(list[0].id) }
  }

  /**
   * 本次刷新「每家点名用哪个账号」的映射（请求体里的 `accounts`）。优先级：**表体下拉的当前值**
   * （用户刚改的）→ **落盘的记忆**（上次改的）→ 不带该家（后端按默认选取 = 队首可用账号）。行由
   * 渲染方整体重绘，所以「当前值」就是此刻屏幕上那些行的有效选中值。
   *
   * 与旧实现的一处差别（刻意的）：旧实现这一层读的是 DOM（`#fm-tbody select[data-provider]`），
   * 但它读之前就把表体换成了「正在获取…」，那层实际永远读不到东西 —— 生效的只有落盘值。这里按
   * 注释里写的口径来：把当前渲染出的行的有效选中值一并带上，请求体与用户屏幕上的下拉一致。值本身
   * 与后端默认选取是同一条账号（队首可用），行为不变。
   */
  function buildSourceMap(rows: Array<{ provider: string; reported?: unknown }>) {
    const map: Record<string, string> = {}
    for (const [id, accountId] of Object.entries(savedSource)) if (accountId) map[id] = accountId
    for (const row of rows) {
      const { selected } = selectSource(row.provider, row.reported)
      if (selected) map[row.provider] = selected
    }
    return map
  }

  /**
   * 表里每一行的「模型来源」当前值（buildSourceMap 按它取「下拉的当前值」）：行模型把**待获取的家
   * 也包含在内**，所以逐行刷过一家之后，其余行下拉里的选择照样进请求体（旧实现只按一屏结果取，
   * 逐行刷过之后就只剩那一行了）。拉取中 / 失败时表体是一行提示、没有下拉，此时这份映射只由
   * selectSource 的回落链给出，与会话里的落盘值同源。
   */
  const sourceRows: Array<{ provider: string; reported?: unknown }> =
    rows.map(row => ({ provider: row.provider, reported: row.item?.accountId }))

  /** 选中即落盘 —— 下次打开弹窗、「重新获取」都以它为准（见 loadSavedSource） */
  function rememberSource(pid: string, accountId: string) {
    setSavedSource(prev => ({ ...prev, [pid]: accountId }))
    saveSavedSource({ [pid]: accountId })
  }

  /* ─── 取数与导入 ─────────────────────────── */

  async function load() {
    if (busy) return
    setBusyKind('load')
    setHint('')
    setErrorText('')
    setStatus('loading')
    try {
      if (custom) {
        const data = await shared().wbProviders?.customRequest?.(
          'POST', '/api/custom-providers/fetch-models', { providerId })
        // 去重（去空白 + 大小写不敏感）、保序、原样大小写 —— 与旧实现同一口径
        const seen = new Set<string>()
        const ids = (Array.isArray(data?.models) ? data.models : [])
          .map(value => String(value ?? '').trim())
          .filter(id => {
            const key = norm(id)
            if (!id || seen.has(key)) return false
            seen.add(key)
            return true
          })
        const managedNow = managedIds(providerId)
        setUpstream(ids)
        setManaged(managedNow)
        // 默认勾选「未添加」的那些 —— 与 OmniProxy 同一默认值：用户点进来就是想补新的
        setPicked(new Set(ids.filter(id => !managedNow.has(norm(id)))))
        setFetched(true)
        setStatus('ready')
        return
      }
      // 「模型来源」下拉的选择与刷新范围随请求带上（范围见 scopeProviders）
      const refresh = shared().workbuddyDesktop?.refreshModels
      if (!refresh) throw new Error(t('模型目录刷新不可用，请重新打开面板'))
      const providers = pending.map(item => item.id).filter((id): id is string => Boolean(id))
      const result = await refresh({ accounts: buildSourceMap(sourceRows), providers })
      if (!Array.isArray(result?.results) || result.results.some(item => !item || typeof item.provider !== 'string'
        || !['refreshed', 'failed', 'skipped'].includes(item.status || ''))
        || providers.some(provider => result.results!.filter(item => item.provider === provider).length !== 1)) {
        throw new Error(t('网关未返回逐家模型目录刷新结果'))
      }
      setResults(result.results)
      setFetched(true)
      setStatus('ready')
      // 真刷到新目录时通知调用方 —— 刷新落地只改了后端那份目录，模型管理页自持的 manage 视图
      // （左栏计数 / 行的「来源」列）读的是另一条接口（/api/models/manage），不重拉就停在旧快照。
      // 失败 / 跳过时不通知：清单一个字没变，重拉只会白跑一趟。回调不依赖本弹窗还在。
      if (Number(result?.refreshed) > 0) onRefreshed?.()
    } catch (error) {
      setErrorText(errorMessage(error))
      setStatus('error')
    } finally {
      setBusyKind(null)
    }
  }

  /**
   * **逐行取数**（每行最右那颗）：只刷这一家。请求里的 `providers` 收窄成 `[row.provider]` —— 后端
   * 逐家打上游，名单外的家不打网络、也不进结果（见 api/models.rs 的 `parse_providers`），所以这是一
   * 次「只针对一家」的真刷新，不是拿缓存糊弄。
   *
   * 回来后**并进**结果表（mergeResults）而不是整表替换：其余行（含还没拉过的「待获取」行）原样
   * 留着，界面上只有这一行发生变化。
   *
   * 失败**不**整屏换成「获取失败」：表里其他行的结果是有效的，一行失败不该把它们一起抹掉 ——
   * 只留脚注 + toast 说清原因，并把当前行标为失败；其他行保留本轮已有结果。
   * 与全刷同一口径：`refreshed > 0` 才通知调用方（失败 / 跳过的清单一个字都没变）。
   */
  async function loadRow(row: Row) {
    if (busy || !row.provider) return
    setBusyKind('row')
    setBusyRow(row.provider)
    setHint('')
    try {
      const refresh = shared().workbuddyDesktop?.refreshModels
      if (!refresh) throw new Error(t('模型目录刷新不可用，请重新打开面板'))
      const result = await refresh({
        accounts: buildSourceMap([{ provider: row.provider, reported: row.item?.accountId }]),
        providers: [row.provider],
      })
      const items = Array.isArray(result?.results) ? result.results.filter(item => item?.provider === row.provider) : []
      if (items.length !== 1 || !['refreshed', 'failed', 'skipped'].includes(items[0].status || '')) {
        throw new Error(t('网关未返回逐家模型目录刷新结果'))
      }
      const item = items[0]
      setResults(prev => mergeResults(prev, [item]))
      setFetched(true)
      setStatus('ready')
      if (Number(result?.refreshed) > 0) onRefreshed?.()
    } catch (error) {
      const message = errorMessage(error)
      setResults(prev => mergeResults(prev, [{
        provider: row.provider, providerLabel: row.label, status: 'failed', message,
      }]))
      setFetched(true)
      setStatus('ready')
      setHint(message)
      toast(t('获取失败：{message}', { message }), 'err')
    } finally {
      setBusyKind(null)
      setBusyRow('')
    }
  }

  /** 把逐家结果并进现有结果表：同名行整行替换、新家追加 —— 逐行取数只动自己那一行（同名判定按
   *  provider id，与后端逐行结果的键一致）。 */
  function mergeResults(previous: RefreshItem[], items: RefreshItem[]): RefreshItem[] {
    const byProvider = new Map(items.map(item => [String(item.provider || ''), item]))
    const merged = previous.map(item => byProvider.get(String(item.provider || '')) ?? item)
    const known = new Set(merged.map(item => String(item.provider || '')))
    for (const item of items) {
      if (!known.has(String(item.provider || ''))) merged.push(item)
    }
    return merged
  }

  /**
   * 导入勾选的模型：整表提交（已存在的后端也会跳过，前端先按本地清单挡一道）。
   * 成功先关窗再回调 —— 与旧实现同序（`close()` 打头、`done?.()` 收尾），回调是
   * 「后端数据变了」的通知，不依赖本弹窗还在。
   */
  async function importPicked() {
    if (!custom || busy || !picked.size) return
    const values = [...picked]
    setBusyKind('import')
    setHint('')
    try {
      const added = Number(await shared().wbModelsCustom?.addModels?.(providerId, values)) || 0
      toast(t('✅ 已导入 {added} 个模型（{skipped} 个已存在，跳过）',
        { added, skipped: values.length - added }))
      setBusyKind(null)
      onClose()
      onDone?.()
    } catch (error) {
      const message = errorMessage(error)
      toast(t('导入失败：{message}', { message }), 'err')
      setHint(message)
      setBusyKind(null)
    }
  }

  /** 底部那颗（自定义家「取消」/ 内置家「完成」）：与 Esc 同一条收口，在途时不许关 */
  function handleCloseClick() {
    if (!busy) onClose()
  }

  /** 全选未添加：把上游里所有「不在本地清单」的 id 勾上（在已选之上追加，不清空） */
  function selectAllUnadded() {
    const next = new Set(picked)
    for (const id of upstream) if (!managed.has(norm(id))) next.add(id)
    setPicked(next)
  }

  /** 「模型来源」格：该家账号的下拉（用哪个账号去打这家的目录接口）。无可选账号与不走账号维度的
   *  家都显示空占位，占位的 title 说明原因（「这家清单是全局的」/「这家还没有可用账号」）。 */
  function sourceCell(pid: string, reported?: unknown) {
    const { list, selected } = selectSource(pid, reported)
    if (!list.length) {
      const why = ACCOUNTLESS_REFRESH.has(pid)
        ? t('这家的模型清单是全局的，不跟账号走')
        : t('这家还没有可用账号，刷新会使用默认登录态')
      return <span className='text-[11.5px] text-muted-foreground' title={why}>—</span>
    }
    const current = list.find(account => String(account.id) === selected)
    return (
      <SearchableSelect value={selected} onValueChange={value => rememberSource(pid, value)}
        className='w-full' title={t('用哪个账号去拉这家的模型清单')} aria-label={t('模型来源')}
        searchPlaceholder={t('搜索账号…')} displayValue={current ? accountLabel(current) : '—'}
        options={list.map(account => ({ value: String(account.id), label: accountLabel(account) }))} />
    )
  }

  /** 勾 / 取消勾一个模型：复选框与整行点击共用这一条出口，两处口径不会走偏 */
  function togglePicked(id: string, next: boolean) {
    setPicked(prev => {
      const out = new Set(prev)
      if (next) out.add(id)
      else out.delete(id)
      return out
    })
  }

  /* ─── 结构 ───────────────────────────────── */

  return (
    <Dialog
      open
      onOpenChange={(next, eventDetails) => {
        // 关闭请求（Esc / 点遮罩 / 右上角 ✕）全部汇到这里。在途时拒绝关闭：请求已经发出，关掉会让
        // 「到底成没成」变成未知状态（旧实现的 close() 同样在 busy 时早退）。必须走
        // eventDetails.cancel() —— 光「不更新 open prop」拦不住 Base UI 的 store。
        if (next) return
        if (busy) {
          eventDetails.cancel()
          return
        }
        onClose()
      }}
    >
      {/* 宽度：内置家是宽表（旧 .modal-wide ≈ 920px），自定义家只有三列（旧
          .modal-narrow ≈ 552px，与宽表区分开）；两侧各留 24px 与遮罩内边距对齐。
          两档都在旧值上加宽 10%（920 → 1012 / 552 → 607）：七列那张表挤在 920px 里时
          「说明」那格几乎每行都折行，宽一点能少折几行。 */}
      <DialogContent className={failed ? 'w-[min(640px,calc(100vw-48px))]'
        : custom ? 'w-[min(607px,calc(100vw-48px))]' : 'w-[min(1012px,calc(100vw-48px))]'}>
        <DialogHeader>
          <DialogTitle>{t('获取模型 — {name}', { name })}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {failed ? <ModelsFetchError message={errorText} /> : (
            <>
              <div className='flex flex-wrap items-center gap-2.5'>
                <span className='text-[11.5px] text-muted-foreground'>{summary}</span>
                {custom && (
                  <InputGroup className='w-[240px] flex-none'>
                    <InputGroupInput type='search' placeholder={t('搜索模型 ID…')} autoComplete='off'
                      value={keyword} onChange={event => setKeyword(event.currentTarget.value)} />
                    <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
                  </InputGroup>
                )}
                <div className='mr-auto' />
                {custom && (
                  <>
                    <Button variant='ghost' size='sm' onClick={selectAllUnadded}>{t('全选未添加')}</Button>
                    <Button variant='ghost' size='sm' onClick={() => setPicked(new Set())}>{t('清空')}</Button>
                  </>
                )}
                <Button variant='outline' size='sm' title={t('刷新全部家（逐家打上游）')} disabled={busy}
                  onClick={() => { void load() }}>
                  {busyKind === 'load' ? t('获取中…') : fetched ? t('重新获取') : t('获取模型')}
                </Button>
              </div>
              <ModelsFetchTable custom={custom} status={status} rows={rows}
                upstream={upstream} managed={managed} picked={picked} keyword={keyword}
                busy={busy} busyRow={busyRow} onPick={togglePicked}
                onFetchRow={row => { void loadRow(row) }} sourceCell={sourceCell} formatTime={formatTime}
                refreshedAt={pid => Number(shared().wbModelsPanel?.providerRefreshedAt?.(pid)) || 0} />
            </>
          )}
        </DialogBody>
        <DialogFooter>
          {!failed && hint && (
            <span className='min-w-0 text-[11.5px] text-muted-foreground [overflow-wrap:anywhere]'>{hint}</span>
          )}
          <div className='mr-auto' />
          <Button variant='outline' onClick={handleCloseClick} disabled={busy}>
            {failed ? t('关闭') : custom ? t('取消') : t('完成')}
          </Button>
          {failed ? (
            <Button variant='default' disabled={busy} onClick={() => { void load() }}>{t('重试')}</Button>
          ) : custom && (
            <Button variant='default' disabled={!picked.size || busy} onClick={() => { void importPicked() }}>
              {busyKind === 'import' ? t('导入中…')
                : picked.size ? t('导入选中的 {n} 个模型', { n: picked.size }) : t('导入')}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 命令式外壳：与旧实现的 window.wbModelsFetchModal 接口一致 ─── */

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null

function unmountModal() {
  if (root) { root.unmount(); root = null }
  if (host) { host.remove(); host = null }
}

/**
 * 打开弹窗：按需建壳（与 conc-dialog / request-clear-modal 同一手法）。`providerId` 是当前左栏
 * 选中的那家；`custom` 由调用方判好。**打开只建壳，不拉取** —— 拉取要用户点「获取模型」才发生。
 */
function openModal(options: OpenOptions) {
  if (!options || (options.custom && !options.providerId)) return
  unmountModal() // 重复打开先拆掉上一份（旧实现同样是 close() 打头）
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(<ModelsFetchModal options={options} onClose={unmountModal} />)
}

declare global {
  interface Window {
    /** 「获取模型」弹窗（替换 ui/models-fetch-modal.js，接口与原实现一致） */
    wbModelsFetchModal?: { open(options: OpenOptions): void }
  }
}

window.wbModelsFetchModal = { open: openModal }
