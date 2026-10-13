import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  type BadgeProps,
} from '@ui'
import { t } from '../i18n'

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

/** 逐家刷新结果的一条（`/api/models/refresh` 的 results，契约见 adapter.rs）：`status` 三档
 *  refreshed / skipped / failed，`count` 只在 refreshed 时有，`refreshedAt` 是这家清单**当前**
 *  的拉取时刻（失败 / 跳过时是上次成功那次，0 = 从未成功）。响应 = 逐家结果 + 顶层汇总。 */
type RefreshItem = {
  provider?: string; providerLabel?: string; status?: string; count?: unknown
  refreshedAt?: unknown; fixed?: boolean; message?: string; accountId?: string
}
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
  wbTableColumns?: { repaint?: (id: string) => void } // 按登记 id 重画某张表
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 常量 ───────────────────────────────────── */

/**
 * 表格骨架（两种形态各一张表、各存一份列宽，见 table-columns.js 的登记）：`TABLE_ID` 是 DOM
 * 元素 id —— 列宽层按它找表，**必须保留**；`TABLE_COL_ID` 是列宽登记 id（与元素 id 是两个命名
 * 空间，登记按「表」记账，同一登记的元素会在弹窗重建中换好几茬）。`COLS_*` 是列 key 的**顺序
 * 表**，`<colgroup>` 与表头都按它建。
 */
const TABLE_ID = { intl: 'fm-table-intl', custom: 'fm-table-custom' }
const TABLE_COL_ID = { intl: 'fetch-models', custom: 'fetch-models-custom' }
const COLS_INTL: readonly string[] = ['provider', 'source', 'state', 'count', 'note', 'updated']
const COLS_CUSTOM: readonly string[] = ['pick', 'model', 'state']

/**
 * 列宽百分数（逐字照抄旧 CSS 的 `#fm-table-intl .f-*` / `#fm-table-custom .f-*`）：没拖过的列走
 * 这里的百分数，拖过的列由 table-columns.js 写 `<col>` 的 inline width 覆盖（th 上不能写宽度，
 * 它会盖过 col，拖动就失效）。
 *
 * 六列怎么分（每格还要减去 td 的左右内边距）：更新日期 19% 要装下 formatTime 的完整输出（只给
 * 「日期」会在同一天的两次刷新之间分不出先后，而这弹窗的用处之一正是看清单有多旧）；模型来源
 * 25% 由邮箱长度定（emailAsName 的家以邮箱为主名）；说明 24% 比最长那句「远程目录已更新…」窄
 * 一点、会折成两行 —— 折行比截断好，且只有「已刷新」的行吃这句长文案。
 */
const WIDTH_INTL: Record<string, string> = {
  provider: 'w-[15%]', source: 'w-[25%]', state: 'w-[10%]',
  count: 'w-[7%]', note: 'w-[24%]', updated: 'w-[19%]',
}
/** 自定义家的窄形态（三列）：勾选 40px ≈ 8%、状态 92px ≈ 18%、上游模型吃剩余 */
const WIDTH_CUSTOM: Record<string, string> = { pick: 'w-[8%]', model: 'w-[74%]', state: 'w-[18%]' }

/** 表头文案（按列 key 取；空串 = 勾选列没有标题，与旧实现一致） */
const HEAD_LABEL: Record<string, string> = {
  pick: '', provider: t('提供商'), source: t('模型来源'), state: t('状态'), count: t('模型数'),
  note: t('说明'), updated: t('更新日期'), model: t('上游模型'),
}

/**
 * 逐家结果的三种状态（旧实现的 KIND 映射逐字搬过来，kind 从 CSS 类名换成组件库 Badge 的
 * variant）：refreshed → `badge brand`，failed → `badge bad`，skipped → 基础 `badge`（组件库的
 * outline 档）。认不出的 status 走兜底：基础徽章 + 原样回显 status（后端加了新档位时不会显示成
 * 「未知」而无从排查）。
 */
const KIND: Record<string, { variant: NonNullable<BadgeProps['variant']>; label: string }> = {
  refreshed: { variant: 'brand', label: t('已刷新') },
  failed: { variant: 'destructive', label: t('失败') },
  skipped: { variant: 'outline', label: t('跳过') },
}

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

/** 「更新日期」格：这家清单**当前**的拉取时刻。后端逐行给 `refreshedAt`（失败 / 跳过的行是上次
 *  成功那次的时刻，0 = 从未成功过 → 占位）—— 用「本次请求的时刻」会在失败行上撒谎。 */
function refreshedText(item: RefreshItem): string {
  return formatTime(Number(item.refreshedAt) || 0) || '—'
}

/** 逐家结果的徽章与文案（见 KIND 的说明） */
function kindOf(item: RefreshItem): { variant: NonNullable<BadgeProps['variant']>; label: string } {
  return KIND[String(item.status)] ?? { variant: 'outline', label: String(item.status || t('未知')) }
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
  /** 拉取失败的原因：整屏换成「获取失败：…」，同时留一份脚注 */
  const [errorText, setErrorText] = React.useState('')
  /** 脚注（toast 会消失，脚注留一份） */
  const [hint, setHint] = React.useState('')
  /**
   * 在途状态（拉取 / 导入互斥）：置真期间不许关窗，关掉会让「到底成没成」变成未知；两种在途
   * 的按钮文案不同（获取中… / 导入中…），所以记的是种类而不是一个布尔。
   */
  const [busyKind, setBusyKind] = React.useState<'load' | 'import' | null>(null)
  const busy = busyKind !== null
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

  /**
   * 表头与 colgroup 就位后请列宽层重画一次：恢复拖过的列宽、补拖动把手、绑拖动事件（表和表头都
   * 随弹窗重建，把手会随表头一起消失 —— 每轮重建后都要补，见 table-columns.js 的 repaint 说明）。
   * 旧实现是在 renderHead() 末尾同步调的，这里等 DOM 提交之后再调，时机等价。
   */
  React.useEffect(() => {
    shared().wbTableColumns?.repaint?.(custom ? TABLE_COL_ID.custom : TABLE_COL_ID.intl)
  }, [custom])

  /** 内置家：待获取那一屏的家（与 scopeProviders 同源，也就是真正会进请求的名单） */
  const pending = custom ? [] : pendingProviders()
  /** 自定义家：按搜索词过滤后的上游模型 id（大小写不敏感，与旧实现同口径） */
  const needle = keyword.trim().toLowerCase()
  const shown = upstream.filter(id => !needle || id.toLowerCase().includes(needle))
  const done = results.filter(item => item.status === 'refreshed').length
  const failed = results.filter(item => item.status === 'failed').length

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
    : intlSummary()

  /** 内置家的汇总文案（待获取 / 刷新中 / 失败 / 逐家结果四档） */
  function intlSummary(): string {
    if (status === 'loading') return t('正在刷新…')
    // 失败时不留上一轮的读数：那是旧结果，与整屏的「获取失败」自相矛盾
    if (status === 'error') return ''
    if (status === 'ready') {
      if (!results.length) return t('0 家')
      return failed
        ? t('{total} 家 · 成功 {done} · 失败 {failed}', { total: results.length, done, failed })
        : t('{total} 家 · 成功 {done}', { total: results.length, done })
    }
    return pending.length ? t('共 {n} 家 · 点「获取模型」开始', { n: pending.length }) : t('0 家')
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

  /** 当前屏幕上那张表里的行（buildSourceMap 要按它取「下拉的当前值」）；拉取中 / 失败时表体只有
   *  一行提示，没有下拉。 */
  const sourceRows: Array<{ provider: string; reported?: unknown }> = !custom && status === 'ready'
    ? results.map(item => ({ provider: String(item.provider || ''), reported: item.accountId }))
    : !custom && status === 'idle'
      ? pending.map(item => ({ provider: String(item.id || '') }))
      : []

  /** 选中即落盘 —— 下次打开弹窗、「重新获取」都以它为准（见 loadSavedSource） */
  function rememberSource(pid: string, accountId: string) {
    setSavedSource(prev => ({ ...prev, [pid]: accountId }))
    saveSavedSource({ [pid]: accountId })
  }

  /* ─── 取数与导入 ─────────────────────────── */

  /**
   * 按当前形态取数：自定义家拉该家上游，内置家刷新各家目录。
   * 失败一律把整屏换成「获取失败：…」并留一份脚注（toast 会消失，脚注不会）。
   */
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
      const result = await shared().workbuddyDesktop?.refreshModels({
        accounts: buildSourceMap(sourceRows), providers: scopeProviders(),
      })
      setResults(Array.isArray(result?.results) ? result.results : [])
      setFetched(true)
      setStatus('ready')
      // 真刷到新目录时通知调用方 —— 刷新落地只改了后端那份目录，模型管理页自持的 manage 视图
      // （左栏计数 / 行的「来源」列）读的是另一条接口（/api/models/manage），不重拉就停在旧快照。
      // 失败 / 跳过时不通知：清单一个字没变，重拉只会白跑一趟。回调不依赖本弹窗还在。
      if (Number(result?.refreshed) > 0) onRefreshed?.()
    } catch (error) {
      const message = errorMessage(error)
      setErrorText(message)
      setStatus('error')
      setHint(message)
    } finally {
      setBusyKind(null)
    }
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

  /** 整屏一行提示（空态 / 加载 / 失败）：`<td colspan>` 占满整行，colspan 按当前形态的列数算 ——
   *  与旧实现 emptyRow 同一个算法。 */
  function stateRow(text: string) {
    return (
      <TableRow>
        <TableCell colSpan={custom ? COLS_CUSTOM.length : COLS_INTL.length}
          className='py-[26px] text-center text-muted-foreground'>{text}</TableCell>
      </TableRow>
    )
  }

  /** 表格里的数值 / 时间格（旧 CSS 的 .rate）：等宽小字、数字等宽对齐 */
  function rateCell(text: string) {
    return <span className='font-mono text-[11.5px] tabular-nums text-muted-foreground'>{text}</span>
  }

  /** 模型 / 提供商名的格子（旧 CSS 的 .mid .t）：等宽小字、超长省略 */
  function nameCell(text: string) {
    return <div className='truncate font-mono text-[12px] font-semibold'>{text}</div>
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
      <Select value={selected} onValueChange={next => rememberSource(pid, String(next))}>
        <SelectTrigger className='w-full' title={t('用哪个账号去拉这家的模型清单')}
          aria-label={t('模型来源')}>
          {/* 展示文案显式给出：不依赖 value 自动显示（值是 id，而这一格要的是账号名） */}
          <SelectValue>{current ? accountLabel(current) : '—'}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          {list.map(account => (
            <SelectItem key={String(account.id)} value={String(account.id)}>{accountLabel(account)}</SelectItem>
          ))}
        </SelectContent>
      </Select>
    )
  }

  /** 自定义家的表体：一行一个上游模型 + 勾选框 + 是否已在清单里。已在清单里的行整体压淡、复选框
   *  禁用（不重复导入），徽章文案说明后果。 */
  function customBody() {
    if (status === 'idle') return stateRow(t('点右侧「获取模型」从上游拉取这家的模型清单'))
    if (status === 'loading') return stateRow(t('正在获取…'))
    if (status === 'error') return stateRow(t('获取失败：{message}', { message: errorText }))
    // 「没匹配上」与「上游压根没返回」是两件事，文案分开
    if (!shown.length) return stateRow(upstream.length
      ? t('没有匹配「{keyword}」的模型', { keyword: needle })
      : t('上游没有返回任何模型'))
    return shown.map(id => {
      const has = managed.has(norm(id))
      return (
        <TableRow key={id} className={has ? 'opacity-50' : undefined}>
          <TableCell>
            <Checkbox checked={picked.has(id)} disabled={has} aria-label={id}
              onCheckedChange={next => setPicked(prev => {
                const out = new Set(prev); if (next) out.add(id); else out.delete(id); return out
              })} />
          </TableCell>
          <TableCell>{nameCell(id)}</TableCell>
          <TableCell>
            {has ? (
              <Badge variant='brand' title={t('这个模型已经在这家的清单里，不重复导入')}>{t('已添加')}</Badge>
            ) : (
              <Badge variant='outline' title={t('勾上它，导入后即进入这家的清单')}>{t('未添加')}</Badge>
            )}
          </TableCell>
        </TableRow>
      )
    })
  }

  /**
   * 内置家的表体：待获取那一屏（idle）/ 逐家结果（ready）。
   *
   * 为什么第一屏不是一张空表：①「模型来源」下拉是随结果行渲染的，结果还没有时它就不存在 —— 用户
   * 没法先选「用哪个账号去拉」，只能先拉一次、看清用了谁、改完再拉一次；②用户一眼能看到这次会刷
   * 哪些家（与 scopeProviders 同源，也就是真正会进请求的那份名单）。「更新日期」列在这一屏照样有
   * 值：它显示的是各家清单**当前**的时刻（从 manage 视图读，见 providerRefreshedAt）—— 那正是
   * 用户决定「要不要刷」的依据，等刷完才有值就太晚了。
   */
  function intlBody() {
    if (status === 'loading') return stateRow(t('正在获取…'))
    if (status === 'error') return stateRow(t('获取失败：{message}', { message: errorText }))
    if (status === 'idle') {
      if (!pending.length) return stateRow(t('还没有可刷新的提供商：先在账号页添加一个账号'))
      return pending.map(item => {
        const at = formatTime(Number(shared().wbModelsPanel?.providerRefreshedAt?.(String(item.id || ''))) || 0)
        return (
          <TableRow key={String(item.id || '')}>
            <TableCell>{nameCell(String(item.label || item.id || ''))}</TableCell>
            <TableCell>{sourceCell(String(item.id || ''))}</TableCell>
            <TableCell><Badge variant='outline'>{t('待获取')}</Badge></TableCell>
            <TableCell>{rateCell('—')}</TableCell>
            <TableCell>{rateCell('—')}</TableCell>
            <TableCell>{rateCell(at || '—')}</TableCell>
          </TableRow>
        )
      })
    }
    if (!results.length) return stateRow(t('刷新完成，但没有得到任何结果'))
    return results.map((item, index) => {
      const kind = kindOf(item)
      const count = Number(item.count)
      // 已刷新的行固定说清「来源」列会怎么变；其余行优先用后端给的原因（skipped / failed 的
      // message），没给才按 fixed 标记分辨「能力边界」与「本次没取到新内容」—— 按 message 文案
      // 匹配会在措辞调整后静默失效
      const note = item.status === 'refreshed'
        ? t('远程目录已更新，「来源」列会显示为「远程」')
        : item.message || (item.fixed ? t('这家用固定模型清单，无可刷新') : t('本次没有取到新内容'))
      return (
        <TableRow key={String(item.provider ?? index)}>
          <TableCell>{nameCell(String(item.providerLabel || item.provider || ''))}</TableCell>
          <TableCell>{sourceCell(String(item.provider || ''), item.accountId)}</TableCell>
          <TableCell><Badge variant={kind.variant}>{kind.label}</Badge></TableCell>
          <TableCell>{rateCell(count ? t('{n} 个', { n: count }) : '—')}</TableCell>
          <TableCell><span className='text-[11.5px] leading-[1.5] text-muted-foreground'>{note}</span></TableCell>
          <TableCell>{rateCell(refreshedText(item))}</TableCell>
        </TableRow>
      )
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
          .modal-narrow ≈ 552px，与宽表区分开）；两侧各留 24px 与遮罩内边距对齐 */}
      <DialogContent className={custom ? 'w-[min(552px,calc(100vw-48px))]' : 'w-[min(920px,calc(100vw-48px))]'}>
        <DialogHeader>
          <DialogTitle>{t('获取模型 — {name}', { name })}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {/* 工具行：**汇总在左、动作在右**（获取模型是最右那颗）—— 用户扫一眼左边就知道
              这次的结果，右边的按钮才是下一个动作（旧 .fm-tools 的分工） */}
          <div className='flex flex-wrap items-center gap-2.5'>
            <span className='text-[11.5px] text-muted-foreground'>{summary}</span>
            {custom && (
              // 搜索框：图标走 InputGroupAddon，宽度沿用旧 .fm-search 的 240px
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
            {/* 文案按「这次有没有真的拿到一份结果」定：拉失败时仍是「获取模型」（用户要的是再试
                一次），成功之后才是「重新获取」 */}
            <Button variant='outline' size='sm' title={t('从上游拉一次')} disabled={busy}
              onClick={() => { void load() }}>
              {busyKind === 'load' ? t('获取中…') : fetched ? t('重新获取') : t('获取模型')}
            </Button>
          </div>
          <Table id={custom ? TABLE_ID.custom : TABLE_ID.intl}
            // table-fixed：colgroup 的百分数（含列宽层写回的 px）要靠它生效
            className='table-fixed'
            // 滚动盒的高度上限（旧 .fm-wrap 的 max-height）
            containerClassName='max-h-[min(48vh,460px)]'>
            {/* col / th 上的 data-col 是列宽层（table-columns.js）定位列的依据 */}
            <colgroup>
              {(custom ? COLS_CUSTOM : COLS_INTL).map(key => (
                <col key={key} data-col={key} className={custom ? WIDTH_CUSTOM[key] : WIDTH_INTL[key]} />
              ))}
            </colgroup>
            <TableHeader>
              <TableRow>
                {(custom ? COLS_CUSTOM : COLS_INTL).map(key => (
                  <TableHead key={key} data-col={key}>{HEAD_LABEL[key]}</TableHead>
                ))}
              </TableRow>
            </TableHeader>
            {/* 末行不画底线（旧 .models-table tbody tr:last-child td 的规则） */}
            <TableBody className='[&_tr:last-child>td]:border-b-0'>{custom ? customBody() : intlBody()}</TableBody>
          </Table>
        </DialogBody>
        <DialogFooter>
          <span className='min-w-0 text-[11.5px] text-muted-foreground'>{hint}</span>
          <div className='mr-auto' />
          <Button variant='outline' onClick={handleCloseClick}>{custom ? t('取消') : t('完成')}</Button>
          {custom && (
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
  if (!options?.providerId) return
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
