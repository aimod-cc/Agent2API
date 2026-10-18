/**
 * Agent2API · 模型管理页的**数据层**（快照 store / 取数 / 写入 / 对外契约）。
 *
 * 从 models-page.tsx 拆出来：那个文件装完「左栏 + 表格 + 两个弹窗」的视图层已超过项目约定的
 * 单文件体量，而这一层的边界很清楚 —— **有没有 JSX**。依赖是单向的（视图层 import 它，它不
 * 认识视图层），所以拆开不会引入环。
 *
 * 它不是岛：文件名是 .ts，不会被 src/index.tsx 的 `islands/*.tsx` glob 加载；页面级的岛仍然
 * 只有 models-page.tsx 一个。页面级的说明（两种数据源、静态表头为什么要就地重排、控件替换的
 * 取舍）都在 models-page.tsx 的文件头。
 *
 * ── 状态放模块级快照 + useSyncExternalStore（照 port-panel / update-panel）──
 * 对外契约方法（render / load / selectProvider / refreshAll）从 React 之外调用，且必须与界面
 * 共用同一份状态；组件内部的 useState 做不到这一点。所以状态是一份模块级快照，改动一律走
 * patch()（换新对象再通知订阅者），组件用 useSyncExternalStore 订阅它。
 * **选中项归一化放在 patch 里**而不是渲染期：渲染期改状态会与 React 的渲染顺序打架。
 *
 * 本页自持内置家那份数据，不经过 app.js 的 state（那是 /api/session 的快照，轮询会整份覆盖）。
 */

import { buildIndex as buildReasoningIndex } from './models-reasoning'
import * as customSource from './models-custom-source'
import type { CustomProviderRecord, ManageMapping, ManageModel, ManageView } from './models-custom-source'
import type { SegmentedControlOption } from '@ui'
import { t } from '../i18n'

/* ─── 类型 ───────────────────────────────────── */

export type Align = 'left' | 'center' | 'right'
type ColumnDecl = { key: string; label?: string; align?: Align }
export type ColumnView = ColumnDecl & { align?: Align }
type ColSettingsHandle = { apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[] }
type ColSettingsSpec = {
  id: string
  label?: string
  columns: ColumnDecl[]
  mount?: () => Element | null
  onChange?: () => void
  buttonPlacement?: 'first' | 'last'
}

type ProviderOption = { id: string; label: string }
type UpstreamOption = { id: string; label: string; off: boolean }
/** 映射弹窗的上下文：行内「＋映射」给 `{target, provider}`，点 chip 上的等级标再带上 `alias` */
export type MappingContext = { alias?: string; target: string; provider: string }
/** 添加模型弹窗的上下文：选中自定义家时这家是锁定的（见 openCustomModel） */
export type CustomModelContext = { provider: string; locked: boolean }
/**
 * 能力弹窗的上下文：只带 `(provider, id)` 两个定位键 —— 弹窗打开时从当前
 * 快照里查那一行（展示名、当前生效值、覆盖标记都从行上读）。数据在弹窗开着
 * 期间被刷新（目录刷新 / 别的写操作）时，行还在就跟着更新、行没了由弹窗
 * 自己提示（它拿不到上下文了）。
 */
export type CapabilityContext = { provider: string; id: string }

type Filters = { provider: string; state: string; search: string; sort: string; visibility: string }

/** 表格现造的绑定（默认绑定 alias == target 由模型行合成，不在 mappings 里另占一格） */
export type Binding = {
  alias: string
  target: string
  provider: string
  enabled: boolean
  isDefault?: boolean
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型读取」而不是 declare global 往 Window 上加属性：workbuddyDesktop /
 * wbApp / wbConfirm / wbProviders 这些是多个岛共用的桥，若每个岛各 declare 一份，接口合并会因
 * 同名属性类型不一致直接报 TS2717 —— 并行迁移时必然互相撞车。本文件只 declare 自己独占的
 * wbModelsPanel（见文件末尾）。
 */
export type SharedWindow = {
  workbuddyDesktop?: {
    getModelManage(): Promise<ManageView | null | undefined>
    addModelMapping(
      alias: string, target: string, provider: string, reasoning?: string, enabled?: boolean,
    ): Promise<ManageView | null | undefined>
    removeModelMapping(alias: string, target: string, provider: string): Promise<ManageView | null | undefined>
    addCustomModel(provider: string, id: string): Promise<ManageView | null | undefined>
    removeCustomModel(provider: string, id: string): Promise<ManageView | null | undefined>
    /** 能力位覆盖（只服务内置家；自定义家走 wbModelsCustom.setCapabilities） */
    setModelCapabilities(
      provider: string, id: string, patch: Record<string, number | boolean | null>,
    ): Promise<ManageView | null | undefined>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** HTML 转义（app.js 里的全局单份实现，只给确认框的正文用） */
    esc?: (value: unknown) => string
    formatTime?: (value: unknown) => string
    /** 当前页标识：首屏只在用户正看着本页时才自拉一次 */
    readonly currentPage?: string
  }
  /** 裸全局：app.js 的顶层函数声明，旧实现直接写 formatTime(...) */
  formatTime?: (value: unknown) => string
  wbConfirm?: {
    ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean>
  }
  wbColSettings?: {
    register(spec: ColSettingsSpec): ColSettingsHandle
    syncStaticHead(id: string, table: Element | null | undefined, viewHidden?: ReadonlySet<string> | null): void
  }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
  wbProviders?: {
    all?: () => Array<{ id?: string; label?: string }>
    labelOf?: (id: string) => string
    refreshCustom?: () => Promise<unknown>
  }
  /** 预置提供商目录（preset-providers.js）：自定义家记录不带图标，按名字回match预置图标用 */
  wbPresetProviders?: {
    list?: Array<{ key?: string; name?: string; icon?: string }>
    presetOf?: (key: string) => { key?: string; name?: string; icon?: string } | null
    iconOf?: (key: string) => string
  }
  wbCustomProvidersUi?: { remove?: (providerId: string) => Promise<boolean> }
  wbAccountAddForms?: { openNewCustomForm?: () => void }
  wbModelsFetchModal?: {
    open(options: {
      providerId: string; custom?: boolean; name?: string
      onDone?: () => void; onRefreshed?: () => void
    }): void
  }
}

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
export function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/**
 * 确认框正文的转义：跨文件引用一律走 wbApp（esc 是 app.js 里的全局单份实现），只有它还没
 * 就绪时才自己转一遍 —— 宁可重复一次，也不能让插值漏出去（wbConfirm.ask 收的是 HTML 片段）。
 */
export function esc(value: unknown): string {
  const fn = shared().wbApp?.esc
  if (fn) return fn(value)
  return String(value ?? '').replace(/[&<>"']/g, char => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char] || char
  ))
}

/** 时间戳 → 本地时间串（0 与非法值返回空串，调用处据此省略那半句提示） */
export function formatTime(value: unknown): string {
  const fn = shared().formatTime ?? shared().wbApp?.formatTime
  return String(fn?.(value) ?? '')
}

/** 归一化判重口径：去空白 + 大小写不敏感（与后端一致） */
const norm = (value: unknown): string => String(value ?? '').trim().toLowerCase()
export const same = (left: unknown, right: unknown): boolean => norm(left) === norm(right)

/* ─── 常量 ───────────────────────────────────── */

/** 「全部」视图里每组默认只展开这么多行，其余折叠成一行「展开其余 N 个」 */
export const GROUP_LIMIT = 8

/** 筛选的落盘键（走 wbFilterMemory：provider / state / search 三个维度各存各的） */
const FILTERS_KEY = 'workbuddy-desktop-models-filters'

const MODEL_STATES: readonly string[] = ['all', 'enabled', 'disabled', 'mapped']
/** 状态分段控件上的标签（键与 MODEL_STATES 一一对应，顺序也照它；只有展示串走 t()，value 是匹配/落盘用的枚举值） */
const MODEL_STATE_LABEL: Record<string, string> = {
  all: t('全部'), enabled: t('已启用'), disabled: t('已禁用'), mapped: t('有映射'),
}
/** 选项提到模块级：SegmentedControl 每拿到新数组都要重新量滑块位置，常量能省掉这轮测量 */
export const MODEL_STATE_OPTIONS: readonly SegmentedControlOption<string>[] = MODEL_STATES.map(value => ({
  value, label: MODEL_STATE_LABEL[value],
}))

/**
 * 排序档位（issue #161：按倍率排序，便于挑便宜的模型）。
 *
 * `default` 是「不做额外排序」—— 组内仍按后端清单原序（只把禁用的沉到组末尾，
 * 见 models-page 的组内排序说明），与加这个档位之前**逐字一致**。
 *
 * 倍率取自清单上的 `credits` 文本（如 `x0.7 credits`），解析不出的一律排在最后：
 * 「查不到倍率」与「倍率是 0」是两件事，按 0 参与排序会把未知的顶到最前面。
 */
const MODEL_SORTS: readonly string[] = ['default', 'rate-asc', 'rate-desc']
const MODEL_SORT_LABEL: Record<string, string> = {
  default: t('默认'), 'rate-asc': t('倍率升序'), 'rate-desc': t('倍率降序'),
}
export const MODEL_SORT_OPTIONS: readonly SegmentedControlOption<string>[] = MODEL_SORTS.map(value => ({
  value, label: MODEL_SORT_LABEL[value],
}))

/* ─── 可见性筛选 ───────────────────────────────── */

/**
 * 可见性筛选项的取值。
 *
 * ── 为什么「全部」排第一 ────────────────────────────────────
 * 与上面状态筛选（`MODEL_STATES` = `all / enabled / disabled / mapped`）同一
 * 习惯：「全部」是**归零档**，排在首位。分段控件的滑块从左边起第一格出发，
 * 用户想「看全」时下意识往左找，把它放在末尾会多扫一眼。
 *
 * 注意「排第一」不等于「默认选中」：默认值仍是 `visible`（见
 * `DEFAULT_FILTERS`），理由见下。
 *
 * ── 为什么默认是 `visible` 而不是 `all` ─────────────────────
 * 上游目录是**超集**：里面混着官方客户端根本不展示的模型（如 Antigravity 的
 * 2.5 / 2.0 系列、Qoder 的 `enable:false` 条目）。这些模型在本仓**默认已被
 * 关闭默认绑定**（见 `model_rules::seed_hidden_disabled`），也就是说它们本来
 * 就不可用；管理页一打开就让它们占满表格，反而把用户真正能用的模型挤下去。
 * 所以默认只看可见的，要看全的自己去切（切到「全部」会落盘记住）。
 *
 * ── 为什么有 `undeclared` 这一档 ────────────────────────────
 * 只有四家（Antigravity / Qoder / Raccoon / CodeArts）有可见性判据，其余家
 * 的条目根本没有 `visibility` 字段。「这家没这个信息」与「这家说它不可见」
 * 必须分开：前者不代表不可用，混进 `hidden` 会让用户误以为那些家的模型都被
 * 关掉了。`undeclared` 在筛选上归入 `visible`（不藏）—— 与各家「没有依据就
 * 不藏」的纪律一致。
 */
const MODEL_VISIBILITIES: readonly string[] = ['all', 'visible', 'hidden', 'undeclared']
const MODEL_VISIBILITY_LABEL: Record<string, string> = {
  all: t('全部'), visible: t('可见'), hidden: t('不展示'), undeclared: t('未声明'),
}
export const MODEL_VISIBILITY_OPTIONS: readonly SegmentedControlOption<string>[] = MODEL_VISIBILITIES.map(value => ({
  value, label: MODEL_VISIBILITY_LABEL[value],
}))

/** 归一化一行的可见性：`'shown'` → `visible`，`'hidden'` → `hidden`，其余 → `undeclared` */
export function visibilityOf(model: ManageModel): 'visible' | 'hidden' | 'undeclared' {
  const raw = typeof model.visibility === 'string' ? model.visibility.trim().toLowerCase() : ''
  if (raw === 'hidden') return 'hidden'
  if (raw === 'shown' || raw === 'visible') return 'visible'
  return 'undeclared'
}

/**
 * 列的 key 用表格里既有的 `data-col`（index.html 的 `<col class="c-xxx" data-col="xxx">`、
 * 表头 th、table-columns.js 的列宽登记三处同名，键名只有一套）。顺序即默认渲染顺序。
 */
const COLUMNS: ColumnDecl[] = [
  // 「选择」列：整行的批量操作入口（照账号页勾选列的口径 —— 列设置里要有名字，
  // 表格里那格是「全选」复选框，没有表头文案）
  { key: 'check', label: t('选择'), align: 'center' },
  { key: 'model', label: t('上游模型') },
  { key: 'rate', label: t('倍率') },
  // 「可见性」紧挨「来源」：两者回答的是同一类问题（这条记录哪来的 / 上游
  // 认不认它），并排放读起来才连贯。没判据的家显示 `—`（见 models-page 的
  // `visibilityCell`）—— 那是「这家没这个信息」，不是「确认可见」。
  { key: 'visibility', label: t('可见性') },
  { key: 'source', label: t('来源') },
  // 能力位两列（顺序、文案与 model-capability 的键序对应）：数值合并在
  // 「上下文 / 输出」一格里，三个布尔合并成一列徽章 —— 分成五列会把
  // 复合控件最宽的「模型映射」列挤到不可用（列数取舍见交付说明）
  { key: 'budget', label: t('上下文 / 输出') },
  { key: 'caps', label: t('能力') },
  { key: 'alias', label: t('模型映射') },
  { key: 'act', label: t('操作'), align: 'right' },
]

/**
 * 自定义家没有「倍率」「来源」这两个概念（它们是内置家清单的字段）：选中自定义家时这两列
 * **按视图隐藏**，列设置里的配置本身不动 —— 切回内置家原样恢复。表头（syncStaticHead 的
 * 第三个参数）与数据行（visibleColumns）读同一份过滤结果，不会各画一个样。
 */
const CUSTOM_HIDDEN_COLUMNS: ReadonlySet<string> = new Set(['rate', 'source', 'visibility'])

/**
 * 左栏「自定义提供商」组那颗「全部」的哨兵 id：选中它时右栏聚合**所有**自定义家的
 * 模型（不含内置家 —— 内置组那颗「全部」用的是 'all'，两颗是并列的两份聚合）。
 * `custom-` 前缀 + 12 位十六进制，与它撞车判据上不可能（isCustom 判前缀但候选名单
 * 只认目录里实有的记录，哨兵不在名单里）。
 */
const ALL_CUSTOM = 'custom-all'

const EMPTY_VIEW: ManageView = { models: [], mappings: [] }

/**
 * 默认筛选。
 *
 * `visibility: 'visible'` 是**有意偏离「全部」**的唯一一处：上游目录里混着
 * 官方客户端根本不展示的模型（本仓已默认关闭它们的默认绑定），把它们摆在表里
 * 只会盖住用户真能用的那些。想看全的切到「未声明 / 全部」即可（落盘，会记住）。
 */
const DEFAULT_FILTERS: Filters = { provider: 'all', state: 'all', search: '', sort: 'default', visibility: 'visible' }

/* ─── 共享快照（外部 store）──────────────────── */

export type PanelState = {
  /** 左栏选中的提供商：'all' / 某个内置家 id / 某个 custom- id */
  provider: string
  stateFilter: string
  /** 可见性筛选（全部 / 可见 / 不展示 / 未声明），见 MODEL_VISIBILITIES */
  visibilityFilter: string
  search: string
  /** 排序档位（默认 / 倍率升序 / 倍率降序），见 MODEL_SORTS */
  sort: string
  /** 内置家的 manage 视图（null = 还没拉到） */
  data: ManageView | null
  /** 已展开全部行的提供商集合 */
  expanded: ReadonlySet<string>
  /** 行内操作在途标记（防同一行连点） */
  pending: ReadonlySet<string>
  /** 映射弹窗的上下文；null = 关着（弹窗内部状态归弹窗自己管） */
  mapping: MappingContext | null
  /** 添加模型弹窗的上下文；null = 关着 */
  customModel: CustomModelContext | null
  /** 能力弹窗的上下文；null = 关着 */
  capability: CapabilityContext | null
}

/**
 * 读上次会话的筛选。不能做成模块顶层的常量：wbFilterMemory 由 filter-memory.js 挂在 window
 * 上，而那个文件排在 islands/ui.js **之后**（index.html 1455 行），首次渲染是否已经晚于它
 * 取决于解析器有没有在两个脚本之间让出 —— 读不到就返回默认值并留个记号，由
 * restoreSavedFilters 在挂载后补读一次，免得把「筛选记忆」整个丢掉。
 */
function readSavedFilters(): Filters {
  const memory = shared().wbFilterMemory
  if (!memory) return { ...DEFAULT_FILTERS }
  const saved = memory.load(FILTERS_KEY, { ...DEFAULT_FILTERS })
  return {
    provider: String(saved.provider || 'all'),
    // 存坏的状态档位回落「全部」（原生 select 赋一个不存在的值也是这个结果）
    state: MODEL_STATES.includes(saved.state) ? saved.state : 'all',
    search: String(saved.search || ''),
    // 存坏的排序档位回落「默认」（与状态档位同一口径）
    sort: MODEL_SORTS.includes(saved.sort) ? saved.sort : 'default',
    // 存坏的可见性档位回落「可见」（与默认值一致，不是「全部」）
    visibility: MODEL_VISIBILITIES.includes(saved.visibility) ? saved.visibility : 'visible',
  }
}

const initialFilters = readSavedFilters()
/** 上次会话的筛选是否已经进过界面（只补读一次） */
let filtersRestored = Boolean(shared().wbFilterMemory)
/** 用户是否已经动过筛选：动过就不再回灌，免得把用户刚敲进去的字冲掉 */
let filtersTouched = false

let snapshot: PanelState = {
  provider: initialFilters.provider,
  stateFilter: initialFilters.state,
  visibilityFilter: initialFilters.visibility,
  search: initialFilters.search,
  sort: initialFilters.sort,
  data: null,
  expanded: new Set<string>(),
  pending: new Set<string>(),
  mapping: null,
  customModel: null,
  capability: null,
}

/** 快照的订阅者（当前只有页面那一个 root） */
const subscribers = new Set<() => void>()

export function getSnapshot(): PanelState {
  return snapshot
}

export function subscribe(listener: () => void): () => void {
  subscribers.add(listener)
  return () => {
    subscribers.delete(listener)
  }
}

/**
 * 打补丁并通知界面：useSyncExternalStore 靠引用比较判变化，必须换新对象。
 *
 * 选中项归一化（认不出的取值落回「全部」）也在这里做，理由见 resolveProvider。
 */
function patch(next: Partial<PanelState>): void {
  const merged = { ...snapshot, ...next }
  const provider = resolveProvider(merged)
  const switched = resolveProvider(snapshot) !== provider
  snapshot = provider === merged.provider ? merged : { ...merged, provider }
  // 「三元组 → 思考等级」索引跟着数据换：chip 上那枚等级标读它，而数据一换索引就得跟着换，
  // 否则改完等级、列表重绘后 chip 上还是旧的那个字（旧实现是每次 render 重建一次，
  // 这里改成跟着快照走 —— 覆盖所有改动路径，且没有「依赖某次 render 刚跑过」的隐式前提）
  rebuildReasoningIndex()
  // 换了视图（内置 ↔ 自定义）就得重排静态表头：自定义家按视图隐藏倍率 / 来源两列。
  // 放在这里而不是只放在 selectProvider —— 选中项也可能被归一化改掉（那家被删了），
  // 那条路同样会换视图，而旧实现只在 selectProvider 里同步表头（漏了归一化那一路）。
  if (switched) syncHead()
  for (const listener of subscribers) listener()
}

/** 落盘筛选（三个维度各写各的，patch 覆盖存量 —— 与 wbFilterMemory.save 同一口径） */
function saveFilters(next: Record<string, string>): void {
  shared().wbFilterMemory?.save(FILTERS_KEY, next)
}

/** filter-memory.js 排在岛之后：首屏没读到就补读一次（用户已经动过筛选就不回灌） */
export function restoreSavedFilters(): void {
  if (filtersRestored || filtersTouched || !shared().wbFilterMemory) return
  filtersRestored = true
  const saved = readSavedFilters()
  if (saved.provider !== snapshot.provider || saved.state !== snapshot.stateFilter
    || saved.search !== snapshot.search || saved.sort !== snapshot.sort
    || saved.visibility !== snapshot.visibilityFilter) {
    patch(saved)
  }
}

/* ─── 数据读取 ───────────────────────────────── */

/**
 * 选中项白名单校验：存盘里记的那家可能已经被删了（或内置清单这次没拉到它），认不出就回落
 * 「全部」—— 否则右栏会是一张永远空的表，而用户找不到原因。
 *
 * **两份数据都到位才判定**：缺任何一份都可能只是「还没加载完」（providers.js 排在岛之后
 * 加载、首屏那次 /api/models/manage 也未必回来），而误判的代价是用户存过的选中项被静默清掉。
 */
export function resolveProvider(state: PanelState): string {
  const id = state.provider
  if (id === 'all' || id === ALL_CUSTOM) return id
  if (customSource.list().some(provider => provider.id === id)) return id
  if (Array.isArray(state.data?.models) && state.data.models.some(model => (model.provider || '') === id)) return id
  if (!state.data || !directoryReady()) return id
  return 'all'
}

/** 归一化后的选中项（渲染与所有取数都读它，保证表头 / 数据行 / 左栏是同一家） */
export function currentProvider(): string {
  return resolveProvider(snapshot)
}

/**
 * 目录缓存（providers.js）是否已经就位。它排在岛**之后**加载，首屏那一瞬可能还没有 ——
 * 那时候不能把「查不到这家」当成「这家已被删除」。
 */
export function directoryReady(): boolean {
  return Boolean(shared().wbProviders)
}

/** 当前选中项是不是「自定义 · 全部」聚合视图 */
export function isAllCustomView(): boolean {
  return currentProvider() === ALL_CUSTOM
}

/** 当前选中项是不是自定义家（单家或「自定义 · 全部」聚合都算 —— 列隐藏的口径一致） */
function isCustomView(): boolean {
  return isAllCustomView() || customSource.isCustom(currentProvider())
}

/**
 * 自定义家视图的缓存：目录缓存是权威数据、重建很便宜，但一次渲染里会被读好几次
 * （表格 + 弹窗候选 + 索引），所以按「该家记录的引用」记一份 —— 目录一刷新（记录换新对象）
 * 自动失效，读到的永远是新值。
 */
let customViewCache: { id: string; record: CustomProviderRecord | null; view: ManageView } | null = null

function customViewOf(id: string): ManageView {
  const record = customSource.record(id)
  if (!customViewCache || customViewCache.id !== id || customViewCache.record !== record) {
    customViewCache = { id, record, view: customSource.buildView(id) || EMPTY_VIEW }
  }
  return customViewCache.view
}

/**
 * 「自定义 · 全部」聚合视图的缓存：各自定义家的行拼成一份同形数据（**不含**内置家
 * —— 左栏那颗「全部」是内置组的，这颗是自定义组自己的），一次渲染里同样被读好几次
 * （表格 + 左栏计数 + 弹窗候选 + 索引）。失效判据是**目录缓存的引用**：providers.js
 * 每次 fetchCustomProviders 都换整份新对象，listSource 拿到的引用一变即失效；引用
 * 相等 = 目录没变 = 拼装结果必然一致，直接复用。空目录给 EMPTY_VIEW（与旧版单个
 * 自定义家没有模型时的空表同一形状，空态文案由视图层按条目区分）。
 */
let aggregateCache: { directory: readonly unknown[]; view: ManageView } | null = null

function aggregateView(): ManageView {
  const directory = customSource.listSource()
  if (aggregateCache && aggregateCache.directory === directory) {
    return aggregateCache.view
  }
  const models: ManageModel[] = []
  const mappings: ManageMapping[] = []
  for (const provider of customSource.list()) {
    const view = customViewOf(String(provider.id || ''))
    models.push(...(view.models || []))
    mappings.push(...(view.mappings || []))
  }
  aggregateCache = { directory, view: models.length || mappings.length ? { models, mappings } : EMPTY_VIEW }
  return aggregateCache.view
}

/** 当前视图的数据（两个数据源在这里合流，下游渲染 / 搜索 / 候选自动跟着选中的家走） */
export function viewData(): ManageView {
  const provider = currentProvider()
  if (customSource.isCustom(provider)) return customViewOf(provider)
  if (provider === ALL_CUSTOM) return aggregateView()
  return snapshot.data || EMPTY_VIEW
}

export function models(): ManageModel[] {
  const view = viewData()
  return Array.isArray(view.models) ? view.models : []
}

export function mappings(): ManageMapping[] {
  const view = viewData()
  return Array.isArray(view.mappings) ? view.mappings : []
}

/**
 * 「三元组 → 思考等级」查询闭包。必须**每次渲染前重建一次**：数据换了索引就得跟着换，否则
 * 用户改完等级、列表重绘，chip 上还是旧的那个字。初值给一个恒返回空串的闭包 —— 在第一次
 * render 之前调用它（理论上不会，但弹窗是独立入口）也不会炸，只是显示成「未绑定」。
 */
let reasoningOf = (_alias: unknown, _target: unknown, _provider: unknown): string => ''

function rebuildReasoningIndex(): void {
  reasoningOf = buildReasoningIndex(mappings())
}

/**
 * 内置各家的 id → {label, n}（左栏内置组条目）。从**内置全量**（data.models）收集，
 * 不能用 models() —— 后者跟着选中项走，选中某一家的那一刻其余各家的计数会全变 0。
 * （resolveProvider 也读它做「认得出这家」的判定，语义与旧版一致：只认有清单的内置家。）
 */
export function builtinRailCounts(): Map<string, { label: string; n: number }> {
  const counts = new Map<string, { label: string; n: number }>()
  const rows = Array.isArray(snapshot.data?.models) ? snapshot.data.models : []
  for (const model of rows) {
    const key = model.provider || ''
    if (!key) continue
    const entry = counts.get(key) || { label: model.providerLabel || key, n: 0 }
    entry.n++
    counts.set(key, entry)
  }
  return counts
}

/**
 * 自定义各家在左栏的计数（家 id → {label, n}）。
 *
 * 「自定义 · 全部」条目的总数与各单家条目的计数都读它。逐家读 buildView（单家视图
 * 缓存照常工作）；目录未就位时是空 Map，行为与旧版一致。
 */
export function customRailItems(): Map<string, { label: string; n: number }> {
  const counts = new Map<string, { label: string; n: number }>()
  for (const provider of customSource.list()) {
    const id = String(provider.id || '')
    if (!id) continue
    const view = customViewOf(id)
    counts.set(id, {
      label: String(provider.name || id),
      n: (view.models || []).length,
    })
  }
  return counts
}

/**
 * 左栏当前展示的内置家 id（有清单的家）——「获取模型」弹窗据此组装刷新范围
 * （见 models-fetch-modal.tsx 的 scopeProviders）：模型管理页看不到的家不该出现在刷新结果里。
 */
function builtinProviders(): string[] {
  return [...builtinRailCounts().keys()]
}
/**
 * 某家清单的最近拉取时刻（毫秒；0 = 未知 / 从未成功过）。
 * 给「获取模型」弹窗的「更新日期」列用（那个弹窗打开时不自动拉取，第一屏只能靠这个值说明
 * 各家清单有多旧）。数据取自 manage 视图的 `refreshedAt`，与本页「来源」列的悬停提示同源。
 */
function providerRefreshedAt(providerId: string): number {
  const rows = Array.isArray(snapshot.data?.models) ? snapshot.data.models : []
  const hit = rows.find(model => model.provider === providerId)
  return Number(hit?.refreshedAt) || 0
}

/** 提供商下拉选项（id + 展示名；按数据里出现的顺序去重） */
export function providerOptions(): ProviderOption[] {
  const seen = new Map<string, string>()
  for (const model of models()) {
    const key = model.provider || ''
    // 展示名走词典：后端给的 providerLabel 是注册名（「WorkBuddy 国内版」这类），
    // 不查表的话非中文界面会整列回落中文
    if (key && !seen.has(key)) seen.set(key, t(model.providerLabel || key))
  }
  return [...seen].map(([id, label]) => ({ id, label }))
}

/**
 * 某一家当前清单里的模型（映射弹窗的「上游模型」下拉数据源）。
 *
 * 含已禁用的行：映射是「名字 → 名字」的静态规则，与启停正交 —— 用户完全可能先建好映射、
 * 之后才把那个模型打开。把它们藏起来会让「为什么我的模型不在下拉里」变成一个查不出的问题。
 * 排序：启用的在前（与表格分组内同一取舍），组内保持后端顺序。
 */
export function upstreamOptions(providerId: string): UpstreamOption[] {
  if (!providerId) return []
  return models()
    .filter(model => (model.provider || '') === providerId)
    .sort((a, b) => Number(a.enabled === false) - Number(b.enabled === false))
    .map(model => ({
      id: model.id,
      // 展示名与 id 不同才补在括号里，避免出现「GLM-5.3（GLM-5.3）」这种重复
      label: model.name && model.name !== model.id ? t('{id}（{name}）', { id: model.id, name: model.name }) : model.id,
      off: model.enabled === false,
    }))
}

/**
 * 添加模型弹窗的提供商候选。
 *
 * 与映射弹窗的 `providerOptions()` **刻意不同**：那个只列「表格里出现过的家」（因为映射必须
 * 挂到一行上），而这里要列**全部可登记的家** —— 用户完全可能先给还没登录的家配好模型清单，
 * 等加上账号就生效。`wbProviders.all()` 读的是 /api/session 的注册表全量（含 count=0 的家），
 * 自定义家再补上（它们不在注册表摘要里）。退化路径：wbProviders 没加载时回落到表格里出现过
 * 的家（少几个选项，但不会让弹窗空着打不开）。
 */
export function customProviderOptions(): ProviderOption[] {
  const options: ProviderOption[] = []
  const builtin = shared().wbProviders?.all?.()
  if (Array.isArray(builtin)) {
    for (const item of builtin) {
      const id = String(item?.id || '')
      if (id) options.push({ id, label: t(String(item?.label || id)) })
    }
  }
  for (const provider of customSource.list()) {
    const id = String(provider.id || '')
    if (id) options.push({ id, label: String(provider.name || id) })
  }
  return options.length ? options : providerOptions()
}

/* ─── 列设置 ─────────────────────────────────── */

let colSettings: ColSettingsHandle | null = null
/** 表格元素（静态表头同步要用它；由 <table ref> 在提交时写入） */
let tableEl: HTMLTableElement | null = null

/** 该表当前可见的列（顺序即配置顺序；列设置未就绪时退回全部列） */
export function visibleColumns(): ColumnView[] {
  const columns: ColumnView[] = colSettings ? colSettings.apply(COLUMNS) : COLUMNS
  if (!isCustomView()) return columns
  return columns.filter(column => !CUSTOM_HIDDEN_COLUMNS.has(column.key))
}

/** 静态表头就地重排（顺序 / 显隐 / 对齐），并让列宽那一层按当前可见列重对一遍 */
export function syncHead(): void {
  shared().wbColSettings?.syncStaticHead('models', tableEl, isCustomView() ? CUSTOM_HIDDEN_COLUMNS : null)
}

/**
 * 登记这张表：读回本地列配置，并把「列设置」按钮插进操作区末尾（排在那排操作按钮**之后**：
 * 这一排有明确主次 —— 添加 / 刷新是主操作，列设置是「怎么看这张表」的辅助开关）。
 *
 * 必须在挂载后调：按钮要插进 React 渲染出来的 `.head-actions` 里，且 `syncStaticHead` 要求
 * 登记表已存在（它按 id 查配置，没登记就直接返回）。
 */
export function registerColumnSettings(): void {
  if (colSettings) return
  const handle = shared().wbColSettings?.register({
    id: 'models',
    label: t('模型管理表'),
    columns: COLUMNS,
    mount: () => document.querySelector('.page[data-page="gateway"] .panel-head .head-actions'),
    buttonPlacement: 'last',
    // 表头重排 + 数据行按新的列集合重画（两处读同一份配置，不会各画一个样）
    onChange: () => { syncHead(); render() },
  })
  colSettings = handle || null
  // 注册之前的那一次渲染读的是「全列」兜底值：用户存过的显隐 / 顺序要在首屏就落到数据行上
  if (colSettings) render()
}

/* ─── 取数与写入 ─────────────────────────────── */

/** 在途加载的序号：force 重入时用它作废更早那次的响应 */
let loadSeq = 0
let loading = false

/**
 * 拉内置家的 manage 清单并重绘。
 *
 * `force` 供「切页刷新」这类**必须落地**的调用使用：默认情况下 loading 守卫会把重复调用
 * 合并掉，但切页时用户刚在账号页改过东西（加 / 删提供商），这次刷新被吞掉就等于整页没刷新
 * —— 所以 force 放行并发，并用序号保证**只有最后一次的结果生效**。
 *
 * 失败也要重绘：左栏与自定义家的表格读的是本地目录缓存，它们不该跟着这次网络失败一起停更
 * （内置家的那份保持上一份成功结果）。
 */
export async function load({ force = false }: { force?: boolean } = {}): Promise<void> {
  if (loading && !force) return
  const seq = ++loadSeq
  loading = true
  // 补读一次筛选记忆（挂载那一刻 filter-memory.js 可能还没执行，见 restoreSavedFilters）
  restoreSavedFilters()
  try {
    const next = await shared().workbuddyDesktop?.getModelManage()
    // 期间若有更新的一次加载发起，本次结果作废
    if (seq !== loadSeq) return
    patch({ data: next && Array.isArray(next.models) ? next : null })
  } catch (error) {
    if (seq !== loadSeq) return
    render()
    toast(t('读取模型清单失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    if (seq === loadSeq) loading = false
  }
}

/**
 * 切到本页时的整页刷新（app.js 的 showPage 转发进来），两件事：
 *   1. **自定义提供商目录**（providers.js 的缓存）：左栏那组条目、每个家的模型清单与
 *      「这家还在不在」全读它。提供商是运行期数据，可能在账号页被添加 / 改名 / 删除，而这一页
 *      自持数据、不随主状态轮询更新；
 *   2. **内置家的 manage 清单**：上游目录可能被别处的「获取模型」刷新过。
 * 顺序不能颠倒：目录缓存是自定义家的数据源，先刷它再重绘，左栏与表格才是同一份数据画出来的。
 * 目录刷新失败不阻断 —— 按现有缓存重绘，总比整页停更好。
 */
export async function refreshAll(): Promise<void> {
  try {
    await shared().wbProviders?.refreshCustom?.()
  } catch { /* 目录偶发打不通：用现有缓存重绘，别把整页刷新拖没 */ }
  await load({ force: true })
}

/** 重绘（app.js 的轮询提醒 / 列设置变更 / 目录可能变了时调用） */
export function render(): void {
  patch({})
}

/**
 * 写操作返回的数据就位。内置家的写接口都返回最新的 manage_view，直接替换；自定义家走整表
 * 提交、没有返回体（next 为 null）—— 它的新数据已经刷进目录缓存，重绘时读得到。
 */
export function accept(next: unknown): void {
  const view = next as ManageView | null
  // 返回体带不带整份 manage 视图，由**写的那家**决定（内置家带、自定义家不带），
  // 与当前选中哪个视图无关 —— 「自定义 · 全部」聚合里勾了一行内置家模型做批量操作时，
  // 返回的仍是内置 manage 视图，照常落快照
  if (view && Array.isArray(view.models)) {
    patch({ data: view })
    return
  }
  render()
}

/** 开关 / 新增 / 改一条绑定（alias == target 时即该模型的默认绑定） */
export async function writeBinding(
  provider: string, alias: string, target: string, change: { reasoning?: string; enabled?: boolean },
): Promise<unknown> {
  if (customSource.isCustom(provider)) return customSource.setBinding(provider, alias, target, change)
  const api = shared().workbuddyDesktop
  if (!api) throw new Error(t('后端桥不可用'))
  return api.addModelMapping(alias, target, provider, change.reasoning, change.enabled)
}

export async function writeRemoveMapping(provider: string, alias: string, target: string): Promise<unknown> {
  if (customSource.isCustom(provider)) return customSource.removeMapping(provider, alias, target)
  const api = shared().workbuddyDesktop
  if (!api) throw new Error(t('后端桥不可用'))
  return api.removeModelMapping(alias, target, provider)
}

export async function writeAddModel(provider: string, id: string): Promise<unknown> {
  if (customSource.isCustom(provider)) return customSource.addModel(provider, id)
  const api = shared().workbuddyDesktop
  if (!api) throw new Error(t('后端桥不可用'))
  return api.addCustomModel(provider, id)
}

export async function writeRemoveModel(provider: string, id: string): Promise<unknown> {
  if (customSource.isCustom(provider)) return customSource.removeModel(provider, id)
  const api = shared().workbuddyDesktop
  if (!api) throw new Error(t('后端桥不可用'))
  return api.removeCustomModel(provider, id)
}

/**
 * 保存一条模型的能力位覆盖（两个数据源各走各的写路径，与 `writeBinding` 同一条分流）：
 *   · 自定义家：记录里整表提交（`customSource.setCapabilities`）；
 *   · 内置家：`POST /api/models/capabilities`（覆盖层落在 `modelRules.capabilities`，
 *     清单出口统一应用，见后端 `model_rules` 的「能力位覆盖」）。
 *
 * `capabilities` 是**全量六键**（弹窗一次提交全部）：`null` = 恢复继承 / 清除、
 * 有值 = 覆盖 —— 与后端那条接口的三态协议同源。
 */
export async function writeCapabilities(
  provider: string,
  id: string,
  capabilities: Record<string, number | boolean | null>,
): Promise<unknown> {
  if (customSource.isCustom(provider)) return customSource.setCapabilities(provider, id, capabilities)
  const api = shared().workbuddyDesktop
  if (!api) throw new Error(t('后端桥不可用'))
  return api.setModelCapabilities(provider, id, capabilities)
}

/* ─── 行内操作 ───────────────────────────────── */

/**
 * 行内操作执行器。`key` 是防重入标记（提供商:模型 id，或 alias:target:provider）—— 同名模型
 * 在多家同时存在，只按 id 记会把两家的行一起标成「执行中」。
 */
export async function runRowAction(key: string, run: () => Promise<unknown>, doneText?: string): Promise<void> {
  if (snapshot.pending.has(key)) return
  patch({ pending: new Set(snapshot.pending).add(key) })
  try {
    accept(await run())
    if (doneText) toast(doneText)
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    const pending = new Set(snapshot.pending)
    pending.delete(key)
    patch({ pending })
  }
}

/**
 * 行的启停判定：还有任一条生效的绑定（默认或别名）就算启用，全部关闭才算禁用。
 * 「已启用 / 已禁用」筛选与组内排序共用这一份口径，避免两处各判各的。
 */
export function rowEnabled(model: ManageModel): boolean {
  return bindingsOf(model).some(binding => binding.enabled !== false)
}

/**
 * 某一行的全部绑定（照抄 OmniProxy 的映射 chips）：默认绑定（alias == target）由模型行现造，
 * 其余按 target + 提供商匹配。同一对外名在多家出现是主备关系，所以每行的 chips 只属于自己
 * 那行（带 provider 精确匹配；旧版全局条目 provider 缺失，对任何家都命中）。
 */
export function bindingsOf(model: ManageModel): Binding[] {
  const provider = model.provider || ''
  const matched = mappings().filter(mapping => same(mapping.target, model.id)
    && (!mapping.provider || same(mapping.provider, provider)))
  const unique = new Map<string, ManageMapping>()
  for (const binding of matched) {
    const key = String(binding.alias).toLowerCase()
    if (!unique.has(key) || binding.provider) unique.set(key, binding)
  }
  const idKey = String(model.id).toLowerCase()
  const defaults = unique.get(idKey)
  unique.delete(idKey)
  const rows: Binding[] = [{
    alias: model.id,
    target: model.id,
    provider,
    enabled: model.enabled !== false && defaults?.enabled !== false,
    isDefault: true,
  }]
  rows.push(...unique.values())
  return rows
}

/** 行内操作的防重入键：同名模型在多家同时存在时，`id` 不足以定位一行 */
export const rowKeyOf = (model: ManageModel): string => `${model.provider || ''}:${model.id}`
/** chip 操作的防重入键：同一对外名 + 上游 + 家才唯一确定一条映射 */
export const bindingKeyOf = (alias: string, target: string, provider: string): string => `${alias}:${target}:${provider || ''}`

export function expandGroup(provider: string): void {
  patch({ expanded: new Set(snapshot.expanded).add(provider) })
}

export function collapseGroup(provider: string): void {
  const expanded = new Set(snapshot.expanded)
  expanded.delete(provider)
  patch({ expanded })
}

export function setStateFilter(next: string): void {
  const value = MODEL_STATES.includes(next) ? next : 'all'
  if (value === snapshot.stateFilter) return
  filtersTouched = true
  patch({ stateFilter: value })
  saveFilters({ state: value })
}

/** 排序档位（与状态筛选同一套落盘口径：换档即写，重开面板原样恢复） */
export function setSort(next: string): void {
  const value = MODEL_SORTS.includes(next) ? next : 'default'
  if (value === snapshot.sort) return
  filtersTouched = true
  patch({ sort: value })
  saveFilters({ sort: value })
}

/** 可见性筛选项（与状态筛选同一套落盘口径） */
export function setVisibilityFilter(next: string): void {
  const value = MODEL_VISIBILITIES.includes(next) ? next : 'visible'
  if (value === snapshot.visibilityFilter) return
  filtersTouched = true
  patch({ visibilityFilter: value })
  saveFilters({ visibility: value })
}

export function setSearch(next: string): void {
  filtersTouched = true
  patch({ search: next })
  // 各敲一个字写一次 localStorage，量小无感（搜索框是受控的：整张表跟着搜索词重画，
  // 与旧实现每敲一个字 innerHTML 重画一次同一量级）
  saveFilters({ search: next })
}

/**
 * 切换选中的提供商。左栏点击与账号页「模型清单 →」跳转都走它。选中项落盘（跨次启动记忆）；
 * 表头按视图重排一次（自定义家隐藏倍率 / 来源两列），其余交给重绘。
 */
export function selectProvider(id: string): void {
  patch({ provider: String(id ?? '').trim() || 'all' })
  saveFilters({ provider: snapshot.provider })
  syncHead()
}

/**
 * 删除一个自定义提供商（左栏条目上那颗 ×）。
 *
 * 删除逻辑（二次确认 / 级联删账号 / 目录与账号列表刷新）全在 custom-provider-ui.js 里，与
 * 账号设置弹窗那颗「删除提供商」同一实现；这里只做两件事：转发，然后把本页重画一遍。
 * 删掉的是**当前选中的那家**时不用特判：patch 里的选中项归一化认不出已删的家，会自动回落
 * 「全部」。
 */
export async function removeCustomProvider(providerId: string): Promise<void> {
  const api = shared().wbCustomProvidersUi
  // 正常加载顺序下它一定在（custom-provider-ui.js 在本岛之后加载）。真缺了就说一声 ——
  // 点了 × 什么都不发生比报错更难查
  if (!api?.remove) { toast(t('删除入口未就绪，请重试或重启应用'), 'err'); return }
  if (!(await api.remove(providerId))) return
  // 重拉而非只重绘：该家从后端目录里消失了（它的模型不再参与路由），内置家那份 manage
  // 视图里的承载关系可能跟着变
  await load({ force: true })
}

/** 「＋ 新建自定义提供商」：就地打开「添加账号」弹窗并直达新建表单（不跳页） */
export function openAddCustomProvider(): void {
  shared().wbAccountAddForms?.openNewCustomForm?.()
}

/** 打开映射弹窗。`context` 是行内入口带的上下文（提供商 + 上游模型锁定，只填对外名）。 */
export function openMapping(context: MappingContext): void {
  // 索引不用在这里重建：patch 每次都重建（见 patch 的说明），弹窗的初值读的就是最新的那份
  patch({ mapping: context })
}

export function closeMapping(): void {
  patch({ mapping: null })
}

/**
 * 打开「添加模型」弹窗。无上下文（只有顶部按钮一个入口），每次都是新增。
 * 选中自定义家时这家是**锁定**的：模型就登记到它名下，不必（也不该）再选一次；内置家则
 * 预选当前正在看的那一家（点了某家的分段再来加模型时，这就是他要的家）。
 */
export function openCustomModel(): void {
  const provider = currentProvider()
  // 「自定义 · 全部」不预选：模型要登记到某一家名下，聚合视图说明用户没定到哪一家，
  // 交给弹窗自己选（与内置「全部」视图同一行为）；单家视图锁定那一家
  const custom = customSource.isCustom(provider)
  const preselect = custom || (provider !== 'all' && provider !== ALL_CUSTOM) ? provider : ''
  patch({ customModel: { provider: preselect, locked: custom } })
}

export function closeCustomModel(): void {
  patch({ customModel: null })
}

/**
 * 打开能力弹窗（表格的「上下文 / 输出」与「能力」两列，点格子即入口）。
 * 上下文只带 `(provider, id)`：弹窗打开期间数据可能被刷新，生效值从当前
 * 快照里现读（见 `modelRowOf`），不把值拷进上下文。
 */
export function openCapability(provider: string, id: string): void {
  patch({ capability: { provider, id } })
}

export function closeCapability(): void {
  patch({ capability: null })
}

/**
 * 按 `(provider, id)` 取一行模型（能力弹窗读生效值 / 覆盖标记 / 展示名）。
 *
 * id 忽略大小写（与全仓的模型名比对口径一致）；行不在（目录刷新后模型
 * 消失、自定义家被删）返回 null，由弹窗提示 + 关闭。
 */
export function modelRowOf(provider: string, id: string): ManageModel | null {
  return models().find(model => (model.provider || '') === provider && same(model.id, id)) || null
}

/**
 * 提供商的展示名：自定义家走目录缓存里的记录名，内置家走注册表，都没有时
 * 回落 id。直接印 provider id 时自定义家会显示成一串 `custom-3f2a91b04c7e`，
 * 用户认不出是哪一家。
 */
export function providerLabelOf(provider: string): string {
  if (customSource.isCustom(provider)) {
    // 自定义家是用户自己起的名字（运行期数据），不进词典
    const name = customSource.record(provider)?.name
    return (typeof name === 'string' && name) || provider
  }
  return t(shared().wbProviders?.labelOf?.(provider) || provider)
}

/**
 * 「获取模型」：打开弹窗，本体在 models-fetch-modal.tsx（全局 wbModelsFetchModal）。
 * 本函数只负责把上下文（选中项、展示名、成功回调）递过去，不掺和弹窗内部的事。
 *
 * 内置家的刷新范围 = 左栏实有清单的家 ∪ 有启用账号的家（弹窗里组装），因此标题只写
 * 「内置提供商」，实际刷了哪几家由结果表逐行列出。
 *
 * 内置家刷新真落地后必须**重拉一次**：本页的数据是自持的，而刷新走的是弹窗里那条
 * /api/models/refresh —— 目录在后端换了一份，本页手里的 manage 视图（左栏计数、行的
 * 「来源」列、新模型的行）却还是旧快照。
 */
export function refreshModels(): void {
  const provider = currentProvider()
  const custom = customSource.isCustom(provider)
  const name = custom
    ? (customSource.record(provider)?.name || provider)
    : t('内置提供商')
  shared().wbModelsFetchModal?.open({
    // 「自定义 · 全部」聚合视图没有单一的上游可拉（各家 Base URL 都不同），
    // 把 providerId 置空，弹窗按内置家的「多家范围」模式组装刷新名单
    providerId: provider === ALL_CUSTOM ? '' : provider,
    custom,
    name,
    // 自定义家：导入成功 → 目录缓存已刷新，重绘即可（数据源是本地缓存）
    onDone: () => render(),
    // 内置家：远程目录落地 → 后端清单换了，必须重拉（force：这次是「刷新已落地」的收尾，
    // 不能被一个更早的在途加载吞掉）
    onRefreshed: () => { void load({ force: true }) },
  })
}

/* ─── 对外契约 ───────────────────────────────── */

/**
 * visibleColumns 导出给 table-columns.js：列宽那一层要按当前可见列算（覆盖值落到哪个 <col>、
 * 末列不给把手），两边读同一份配置才不会各算一个样。
 * selectProvider 导出给账号页的自定义提供商弹窗（「模型清单 →」跳过来并选中该家）。
 * providerRefreshedAt 导出给「获取模型」弹窗的「更新日期」列；builtinProviders 给它组装刷新范围。
 */
window.wbModelsPanel = {
  render, load, refreshAll, refreshModels, visibleColumns, selectProvider, builtinProviders,
  providerRefreshedAt,
}

declare global {
  interface Window {
    /** 模型管理页（替换 ui/models-panel.js，接口与原实现一致） */
    wbModelsPanel?: {
      render(): void
      load(options?: { force?: boolean }): Promise<void>
      refreshAll(): Promise<void>
      refreshModels(): void
      visibleColumns(): ColumnView[]
      selectProvider(id: string): void
      builtinProviders(): string[]
      providerRefreshedAt(providerId: string): number
    }
  }
}

/**
 * 「三元组 → 思考等级」的读取入口。索引本身（reasoningOf）由 patch 每次重建（见那里），
 * 这里包一层是为了别把可变绑定当值导出 —— 视图层读到的永远是当前那一份索引。
 */
export function levelOf(alias: unknown, target: unknown, provider: unknown): string {
  return reasoningOf(alias, target, provider)
}

/**
 * 表格元素的登记口（静态表头同步要用它）。视图层不能直接给模块内的 `tableEl` 赋值
 *（导入的绑定是只读的），所以走这个 setter —— 它由 `<table ref>` 在提交时调用。
 */
export function setTableEl(el: HTMLTableElement | null): void {
  tableEl = el
}

/**
 * 摘掉表格里**不属于 React 树**的节点（修复 #195，由视图层每次提交后调用）。
 *
 * `<table>` 的直接子级只该有 colgroup / thead / tbody#models 三样，行全部由
 * React 渲染。但 React **不会**清理外部插进表格的节点：注入实验确认，手工塞进
 * tbody 的行、或塞进 table 的独立 tbody，之后 render() / load() 都不会把它们
 * 摘掉，会一直留在表里。issue #195 的现场正是这样一行：不属于当前数据、不计入
 * 标题统计，且它所在的那个 tbody 让它下面少了一条分隔线（tbody 最后一行没有
 * 下边框）。
 *
 * 判据是 React 给每个渲染元素挂的 fiber 标记（`__reactFiber$…` / 旧版
 * `__reactInternalInstance$…`）：外部插入的节点没有这两个键。摘的全是「不属于
 * React」的节点，所以清理本身不会与 React 的 diff 打架，正常渲染一个都不会碰。
 */
export function pruneForeignTableNodes(): void {
  const table = tableEl
  if (!table) return
  const owned = (el: Element): boolean => Object.keys(el).some(
    key => key.startsWith('__reactFiber$') || key.startsWith('__reactInternalInstance$'))
  for (const child of [...table.children]) {
    if (child.tagName === 'TBODY') {
      if (child.id !== 'models') child.remove()
    } else if (child.tagName === 'TR' && !owned(child)) {
      // 直接落在 table 下的 tr 会被浏览器按「匿名 tbody」渲染成一行，同样要清
      child.remove()
    }
  }
  const tbody = table.querySelector('tbody#models')
  if (!tbody) return
  for (const row of [...tbody.children]) {
    if (!owned(row)) row.remove()
  }
}
