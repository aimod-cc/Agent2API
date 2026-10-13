import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Button, Popover, PopoverContent, SegmentedControl, Switch, cn } from '@ui'
import { t } from '../i18n'

/**
 * Agent2API · 表格列设置（列的显示 / 隐藏 · 顺序 · 对齐）—— React 岛。
 *
 * 替换 ui/table-col-settings.js。那个模块有两层，本次只换第二层：
 *
 * ── 第一层：列配置的能力层（照旧保留）──────────────────────────
 * `register` / `apply` / `configOf` / `syncStaticHead` / `close` 五个方法是对外契约，
 * 四张表（账号表、模型管理、网关 Key、请求日志）在运行期调它们。它们读写的是一份
 * 「列 key → { 显示, 对齐 } 的有序配置」+ localStorage 持久化（键名前缀照旧），
 * 逻辑与口径逐字照抄 —— 这层不是「界面」，是四张表共用的**数据契约**：口径一动
 * （`apply` 拿到的数组顺序、`configOf` 的返回形状、`syncStaticHead` 对静态表头做的事），
 * 四张表的列集合 / 顺序 / 对齐就会与面板各画一个样。所以这里只把它搬进 TS，不改行为。
 *
 * ── 第二层：浮层面板（本次换掉）────────────────────────────────
 * 外壳交给组件库的 `Popover`（Base UI，`modal={false}` 的非模态浮层）：定位（floating-ui）、
 * 点面板外收起、Esc、焦点归位都是它内建的，这里不再自算坐标、不再自己挂 scroll / resize
 * 跟随、不再自己抓 pointerdown 与 keydown。挂载方式照旧：按需建宿主 div + createRoot、
 * 关闭时 `unmount()` + 摘宿主（与 conc-dialog / request-clear-modal 同一手法）—— 浮层是
 * 单例，为一个「平时不存在」的东西常驻 React root 不划算。
 *
 * **锚点直接用那颗齿轮按钮**：PopoverContent 的 `anchor` 属性把 Base UI 的
 * `Positioner.anchor` 透了出来，所以浮层贴的就是 `button.getBoundingClientRect()`，
 * 不必给按钮套包裹层、更不必盖一层触发器当替身（盖上去会挡住按钮自己的 hover 与 title）。
 * 代价见「点外关闭与按钮自己那一下」一段：没有 Trigger，按钮的 click 会被 Base UI
 * 当成「点面板外」，得自己把这一次认出来。
 * 控件仍全部走组件库：显隐 `Switch`、对齐三档 `SegmentedControl`、按钮 `Button`。
 *
 * ── 触发按钮（齿轮）为什么留在命令式一侧 ───────────────────────
 * 它插在 legacy 渲染出来的表头工具条里（`#batch-bar .batch-actions`、`.panel-head .head-actions`），
 * 插入位置与「排在最前 / 最后」由那些脚本的加载期决定；它的 `.colset-btn` / `.open` 样式也还在
 * ui/css 里。那是「打开浮层的触发管道」而不是浮层本体：换成 React 只会多一层宿主对齐成本，
 * 换不来任何行为收益，所以照旧 —— 按钮本身不归 React 管。
 *
 * ── 点外关闭与按钮自己那一下（这段是本次最容易做错的地方）──────
 * Base UI 判定「点面板外」是在 document 的**捕获阶段**（useDismiss 的 click 监听），
 * 而按钮自己的 click 处理器在冒泡阶段 —— 于是「面板开着时点按钮」的顺序是：
 *   ① 捕获：目标不在浮层内、也不是已登记的触发器 → 判为点外 → 关窗（卸载 React root）；
 *   ② 冒泡：按钮的处理器调 togglePanel → 发现没有面板了 → **又开一次**。
 * 结果就是按钮按下去关不掉。解法是把 ① 的那一次 click 认出来：onOpenChange 的
 * eventDetails 带着原始事件，把它记下，按钮的处理器见到同一个事件对象就只清标记、
 * 不再开窗（见 outsidePressEvent / makeButton）。用事件对象本身做判据而不是时间戳：
 * 同一次 click 只会走这两处，跨次不会撞。
 *
 * ── 拖动排序为什么不上拖拽库 ──────────────────────────────────
 * 与旧实现同一取舍：拖动过程中配置一变就要重画（开关与对齐档的状态跟着配置走），
 * 而 HTML5 拖放的会话绑在**那个节点**上，一重画就断；指针事件的状态只在 JS 变量里，
 * 节点换掉不影响会话。这里保留旧实现的 pointerdown / pointermove / pointerup，
 * 视觉照旧：拖动行压淡 50%、跨过哪一行就在哪一行落位、松手才写盘与通知。
 *
 * ── 定位档位与旧 placePanel 的取舍 ────────────────────────────
 * 旧算法是「贴锚点右下、下方放不下就翻向上、贴边收 8px」，逐像素都是磨过的；现在用
 * `side='bottom'` + `align='end'` + `sideOffset=6` + `collisionPadding=8`（后者写在组件库里）
 * 表达同一取向，翻向与贴边交给 floating-ui 的 flip / shift。差别与理由见交付说明；
 * 唯一保留的自算逻辑是「锚点滚出视口 / 被摘掉就收起」—— 浮层库对这两种情况的处理是
 * **跟着贴边**（`data-anchor-hidden`）而不是关闭，与旧行为的差别用户看得见。
 */

/* ─── 常量与类型 ─────────────────────────────── */

/** 本地配置的键名前缀：与旧实现逐字一致，用户已存过的列配置不因这次迁移丢失 */
const STORE_PREFIX = 'agent2api-col-config:'

/** 对齐三档；值直接写进 class 后缀（ta-left / ta-center / ta-right） */
type Align = 'left' | 'center' | 'right'

/**
 * 三档的取值与文案。数组本身也直接喂给 SegmentedControl 的 options
 *（`{ value, label }` 就是它要的形状），所以档位只有这一处定义。
 */
const ALIGNS: readonly { value: Align; label: string }[] = [
  { value: 'left', label: t('左') },
  { value: 'center', label: t('中') },
  { value: 'right', label: t('右') },
]

/** 存盘里的对齐只认这三档（脏值退回该列默认，见 normalize） */
const isAlign = (value: unknown): value is Align => ALIGNS.some(align => align.value === value)

/** 对齐三档的 class 后缀：静态表头重排时先摘后加，三档互斥 */
const ALIGN_CLASSES = ALIGNS.map(align => `ta-${align.value}`)

/** 拖动期间挂在 body 上的类（cursor: grabbing + 禁选中，样式在 ui/css/table-col-settings.css，照旧复用） */
const DRAGGING_CLASS = 'colset-dragging'

/** 锚点按钮「面板开着」的类（`.colset-btn.open` 的样式在 ui/css 里，照旧复用） */
const BUTTON_OPEN_CLASS = 'open'

/**
 * 最近一次「因为点面板外而关闭」的那次 click（原始事件对象）。
 * 面板开着时点按钮，Base UI 会在捕获阶段把它判成点外并关窗，按钮自己的处理器
 * 紧接着在冒泡阶段跑 —— 靠这个标记认出「刚才那一下就是我自己」，只清标记不再开窗。
 * 判据用事件对象而不是时间戳：同一次 click 只会走这两处，跨次不会撞。
 */
let outsidePressEvent: Event | null = null

/** 表头与数据行共用同一套网格：手柄 / 列名 / 显隐开关 / 对齐段。
 *  列名 minmax(78px,1fr)：长列名省略而不是把对齐段挤出去；
 *  对齐段 minmax(106px,auto)：装得下分段控件的三档（与旧面板的 116px 同量级） */
const ROW_GRID = 'grid grid-cols-[18px_minmax(78px,1fr)_44px_minmax(106px,auto)] items-center gap-2'

/** 一行配置：列 key → { 显示, 对齐 }。**数组顺序就是用户拖出来的列顺序** */
type ColConfigItem = { key: string; visible: boolean; align: Align }

/** 表在 register 时声明的列（顺序即渲染顺序，见 register 的说明） */
type ColumnDecl = {
  key: string
  label?: string
  /** 默认是否显示（不声明即显示）；四张表当前都没用到，作为契约的一部分保留 */
  visible?: boolean
  align?: Align
  /** 上一版的默认对齐：只给 normalize 分辨旧存盘里那一档是「用户挑的」还是「旧默认值」 */
  legacyAlign?: Align
}

/** register 的入参（四张表各给一份；mount 是元素、选择器或返回元素的函数） */
type TableSpec = {
  id: string
  label?: string
  columns: ColumnDecl[]
  mount?: string | (() => Element | null)
  /** 改动落地后的回调：各表拿它重画自己（表头重排 + 数据行） */
  onChange?: (config: ColConfigItem[]) => void
  /** 按钮插入位置：默认最前；'last' 插到末尾（模型管理页要求排在主操作之后） */
  buttonPlacement?: 'first' | 'last'
  buttonClass?: string
}

/** 登记项 = 声明 + 当前配置（配置是运行期被面板改写的那一份，四张表也读它） */
type TableEntry = TableSpec & { config: ColConfigItem[] }

/** 存盘形态 v2：`{ v, defaults, items }`；v1 是裸数组（见 normalize） */
type SavedV2 = { v?: number; defaults?: Record<string, string>; items?: unknown[] }

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：wbIcons /
 * wbTableColumns 是多个岛共用的桥，各岛各 declare 一份会因同名属性类型不一致直接报
 * TS2717 —— 并行迁移时必然互相撞车。本文件只声明自己独占的 wbColSettings。
 */
type SharedWindow = {
  /** 齿轮图标（icons.js 挂的；返回一段 SVG 串） */
  wbIcons?: { icon?: (name: string, size?: number) => string }
  /** 列宽层：静态表头重排之后要让它重新对一遍把手与轨道 */
  wbTableColumns?: { repaint?: (id: string) => void }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 配置的读取与归一（口径逐字照抄旧实现）────── */

/** 注册过的表：id → 登记项 */
const registry = new Map<string, TableEntry>()

function defaultsOf(spec: TableSpec): ColConfigItem[] {
  return spec.columns.map(column => ({
    key: column.key,
    visible: column.visible !== false,
    align: column.align || 'left',
  }))
}

/**
 * 存盘的配置 → 当前可用的配置。
 *
 * 归一的三条（都是为了让「改过列定义之后旧的本地配置还能用」）：
 *   1. 存盘里已经不存在的 key 丢掉（那一列被删了）；
 *   2. spec 里新增的 key 按默认值插到它在 spec 里的**相对位置**
 *      （紧跟在 spec 中排在它前面、且用户配置里也存在的那一列之后）；
 *   3. align 只认三档，脏值退回该列默认。
 * 顺序以存盘为准 —— 顺序正是用户拖出来的东西。
 *
 * 新增列插到声明位置而不是追加到末尾：列的位置有时是语义的（账号表的代理列声明在
 * 「账号」与「连接数」之间），追加到末尾会让已存过配置的老用户看到的默认位置与预期不符。
 * 插入只发生在「存盘里根本没有这个 key」时，用户拖过的顺序完全不受影响。
 *
 * 对齐要能分辨「存的是用户挑的」与「存的是当时的默认值」：**默认值本身会随版本调整**，
 * 一律以存盘为准的话老用户永远看不到新默认值。判据是「存盘值 ≠ 当时那份默认值」即视为
 * 用户挑过，于是需要知道**当时**的默认值，两种存盘形态各有一个来源：
 *   · v2 `{ v, defaults, items }`：defaults 是上次写盘时各列的默认对齐快照（精确）；
 *   · v1 裸数组（本次改造前的形态，没有快照）：用列上声明的 `legacyAlign`（= 上一版的
 *     默认对齐），它只对那些默认值改过的列有必要。
 * 两种形态都只在「值恰好等于旧默认」时改判成新默认，除这一种歧义（用户主动选了与旧默认
 * 相同的值）外不会覆盖用户的选择；顺序与显隐完全不受这套判定影响。
 */
function normalize(spec: TableSpec, saved: unknown): ColConfigItem[] {
  // v1 是裸数组，v2 是 { v, defaults, items }。两种都当「列表 + 可选快照」读
  const legacy = Array.isArray(saved)
  const savedObject = !legacy && saved && typeof saved === 'object' ? (saved as SavedV2) : null
  const items: unknown[] = legacy ? (saved as unknown[]) : Array.isArray(savedObject?.items) ? savedObject.items : []
  const snapshot = savedObject?.defaults && typeof savedObject.defaults === 'object' ? savedObject.defaults : null

  const byKey = new Map(spec.columns.map(column => [column.key, column]))
  const defaults = new Map(defaultsOf(spec).map(item => [item.key, item]))
  const out: ColConfigItem[] = [];
  const seen = new Set<string>()
  for (const raw of items) {
    const item = (raw && typeof raw === 'object' ? raw : {}) as { key?: unknown; visible?: unknown; align?: unknown }
    const key = String(item.key || '')
    if (!byKey.has(key) || seen.has(key)) continue
    seen.add(key)
    const column = byKey.get(key) as ColumnDecl
    const fallback = column.align || 'left'
    // 「当时那份默认值」：v2 读快照，v1 读列上声明的旧默认（未声明即当前默认）
    const before = snapshot ? (snapshot[key] ?? fallback) : (column.legacyAlign ?? fallback)
    const stored = isAlign(item.align) ? item.align : null
    out.push({
      key,
      visible: item.visible !== false,
      // 存盘值等于旧默认 → 用户没动过这一列 → 让新默认生效
      align: stored && stored !== before ? stored : fallback,
    })
  }
  for (const column of spec.columns) {
    if (seen.has(column.key)) continue
    insertBySpecOrder(out, spec, { ...(defaults.get(column.key) as ColConfigItem) })
  }
  return out
}

/**
 * 把一个新增列插到 `out` 里、紧跟着它在 `spec.columns` 中的前驱
 * （前驱不在 `out` 里就继续往前找，都找不到则插到最前）。
 *
 * 多个新增列按 `spec.columns` 的顺序依次调用本函数，最终相对次序与 spec 一致 ——
 * 因为每个新列都插在「它前面那个已存在的列」之后，而它前面的新列已经先插好了。
 */
function insertBySpecOrder(out: ColConfigItem[], spec: TableSpec, item: ColConfigItem): void {
  const at = spec.columns.findIndex(column => column.key === item.key)
  for (let index = at - 1; index >= 0; index -= 1) {
    const previous = out.findIndex(entry => entry.key === spec.columns[index].key)
    if (previous >= 0) {
      out.splice(previous + 1, 0, item)
      return
    }
  }
  out.unshift(item)
}

function load(spec: TableSpec): ColConfigItem[] {
  try {
    const raw = localStorage.getItem(STORE_PREFIX + spec.id)
    return normalize(spec, raw ? JSON.parse(raw) : [])
  } catch {
    // 存坏了就退回默认：列设置读不出来不该让整张表渲染不了
    return defaultsOf(spec)
  }
}

/**
 * 写盘。存 `{ v, defaults, items }`：
 *   · `items` 是配置本体（key / visible / align），与 v1 的裸数组同形；
 *   · `defaults` 是**本次写盘时各列的默认对齐**，只为下次启动时能分辨「存的这一档是
 *     用户挑的」还是「当时的默认值」—— 见 normalize。不写这份快照，将来再改默认值时
 *     就没法只对「没动过的列」生效。
 * 多出来的 v / defaults 两个键不影响老版本读它（老代码只读数组本身会失败，于是退回默认
 * —— 那正是升级前的行为，不会更糟）。
 */
function persist(spec: TableEntry): void {
  try {
    const defaults: Record<string, string> = {}
    for (const column of spec.columns) defaults[column.key] = column.align || 'left'
    localStorage.setItem(
      STORE_PREFIX + spec.id,
      JSON.stringify({ v: 2, defaults, items: spec.config }),
    )
  } catch {
    /* 隐私模式等存不了就算了：本次会话内仍然生效 */
  }
}

/* ─── 对外接口 ──────────────────────────────── */

/** register 的返回：表侧拿它按配置渲染（`apply` 是唯一入口，见 register 的说明） */
type ColSettingsHandle = {
  apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[]
  config(): ColConfigItem[]
}

const configOf = (id: string): ColConfigItem[] | null => registry.get(id)?.config || null

const labelOf = (spec: TableSpec, key: string): string =>
  spec.columns.find(column => column.key === key)?.label || key

/**
 * 把一份「按渲染顺序排好的列数组」投影成用户配置的顺序与显隐，并带上对齐。
 * 数组里的每一项至少要带 key（其余字段原样保留，渲染方可以顺手把单元格渲染函数
 * 一起塞进来）。未登记的 id 原样返回 —— 与旧实现一致（列设置还没就绪时表照常渲染）。
 */
function apply<C extends { key: string }>(id: string, columns: C[]): (C & { align: Align })[] {
  const entry = registry.get(id)
  if (!entry) return columns as (C & { align: Align })[]
  const byKey = new Map(columns.map(column => [column.key, column]))
  return entry.config
    .filter(item => item.visible && byKey.has(item.key))
    .map(item => ({ ...(byKey.get(item.key) as C), align: item.align }))
}

/**
 * 改动落地：写盘 → 通知该表重绘 → 面板还开着就重画一遍。
 * `silent` 保留自旧实现（跳过 onChange，只落盘），当前无调用方，留着是为了不改口径。
 */
function commit(spec: TableEntry, { silent = false }: { silent?: boolean } = {}): void {
  persist(spec)
  if (!silent) spec.onChange?.(spec.config)
  if (panel && panel.id === spec.id) panel.repaint()
}

/* ─── 浮层外壳（命令式：按需建宿主、关闭即卸）────── */

/** 浮层的锚点就是那颗齿轮按钮本身（PopoverContent 的 anchor 属性透传给了 Base UI） */
type Anchor = HTMLButtonElement

/** 当前浮层。整页只有这一个（两处各弹一个是不可能的） */
type PanelHandle = {
  id: string
  spec: TableEntry
  /** React 侧注册的重画入口；面板还没挂好时是空操作（旧实现的 renderPanel） */
  repaint: () => void
  /** 卸 React root + 摘宿主；组件自己的清理（拖动会话、锚点 .open）在 unmount 里跑 */
  destroy: () => void
}

let panel: PanelHandle | null = null

function closePanel(): void {
  if (!panel) return
  const current = panel
  // 先清引用再卸：unmount 会同步跑组件的清理，那些清理里若再走到 close 也不会递归
  panel = null
  current.destroy()
}

function openPanelFor(spec: TableEntry, anchor: Anchor): void {
  closePanel()
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  const repaintRef: { current: (() => void) | null } = { current: null }
  const handle: PanelHandle = {
    id: spec.id,
    spec,
    repaint: () => repaintRef.current?.(),
    destroy: () => {
      root.unmount()
      host.remove()
    },
  }
  panel = handle
  root.render(<ColSettingsPanel spec={spec} anchor={anchor} repaintRef={repaintRef} onClose={closePanel} />)
}

/** 同一个表的按钮再点一次 = 收起；点另一张表的按钮 = 换浮层（旧实现同） */
function togglePanel(spec: TableEntry, anchor: Anchor): void {
  if (panel && panel.id === spec.id) closePanel()
  else openPanelFor(spec, anchor)
}

/* ─── 面板本体 ───────────────────────────────── */

/** 一次拖动会话：teardown 摘监听、去 body 类、清拖动态（浮层被关掉时也要能收干净） */
type DragSession = { teardown: () => void }

/** 拖动排序的最小单元：把第 from 个配置项移到 to 的位置 */
function moveItem(config: ColConfigItem[], from: number, to: number): ColConfigItem[] {
  const next = [...config]
  const [item] = next.splice(from, 1)
  next.splice(to, 0, item)
  return next
}

/**
 * 指针落点下面那一行的列 key（拖动时用它判断「跨过了哪一列」）。
 * 只在落点属于本面板的行区时才算数 —— 页面别处也可能有 data-col-key。
 */
function rowKeyAt(x: number, y: number, rows: HTMLElement | null, config: ColConfigItem[]): string {
  const node = document.elementFromPoint(x, y)?.closest?.('[data-col-key]')
  if (!node || !rows || !rows.contains(node)) return ''
  const key = node.getAttribute('data-col-key') || ''
  return config.some(item => item.key === key) ? key : ''
}

/** 拖动手柄的六点图标（内联 SVG，与 icons.js 同款手法：不依赖字体字形） */
const GRIP_ICON = (
  <svg viewBox='0 0 24 24' width={13} height={13} fill='currentColor' aria-hidden='true' className='block'>
    <circle cx='9' cy='6' r='1.6' />
    <circle cx='15' cy='6' r='1.6' />
    <circle cx='9' cy='12' r='1.6' />
    <circle cx='15' cy='12' r='1.6' />
    <circle cx='9' cy='18' r='1.6' />
    <circle cx='15' cy='18' r='1.6' />
  </svg>
)

type PanelProps = {
  spec: TableEntry
  /** 锚点：那颗命令式建出来的齿轮按钮，浮层贴的就是它的矩形 */
  anchor: Anchor
  /** React 侧的重画入口：commit 通过它让浮层跟上配置（旧实现的 renderPanel） */
  repaintRef: { current: (() => void) | null }
  /** 收起浮层（由命令式外壳提供） */
  onClose: () => void
}

/**
 * 列设置面板。
 *
 * 配置本体**不放在组件 state 里**：那是能力层的东西（`spec.config`），四张表的
 * `apply()` 也读同一份。组件里存副本必然漂移，所以只用一个自增的版本号触发重画，
 * 渲染时直接读 `spec.config`。
 */
function ColSettingsPanel({ spec, anchor, repaintRef, onClose }: PanelProps) {
  const button = anchor
  const rowsRef = React.useRef<HTMLDivElement | null>(null)
  const [draggingKey, setDraggingKey] = React.useState<string | null>(null)
  const dragSessionRef = React.useRef<DragSession | null>(null)
  const [, setVersion] = React.useState(0)
  /** 强制重画：配置改了但引用没换（或换了）时都能让界面跟上 */
  const repaint = React.useCallback(() => setVersion(version => version + 1), [])

  // 把重画入口交给能力层的 commit（旧实现的 renderPanel 等价物）
  React.useLayoutEffect(() => {
    repaintRef.current = repaint
    return () => {
      repaintRef.current = null
    }
  }, [repaint, repaintRef])

  // 锚点按钮的「开着」态：挂到浮层的生命周期上 —— 浮层在，类与 aria 状态就在，卸掉才还原。
  // 关闭是「卸 root」一步到位（没有退出动画），所以这里不会提前摘掉（样式在 ui/css 里，照旧复用）
  React.useEffect(() => {
    button.classList.add(BUTTON_OPEN_CLASS)
    button.setAttribute('aria-expanded', 'true')
    return () => {
      button.classList.remove(BUTTON_OPEN_CLASS)
      button.setAttribute('aria-expanded', 'false')
    }
  }, [button])

  // 锚点滚出视口 / 被摘掉就收起：这是旧算法里唯一没交给浮层库的一条 ——
  // Positioner 只会「跟随 + 贴边」，锚点滚出视口后浮层会挂在视口边上、与按钮脱节
  React.useEffect(() => {
    const check = () => {
      const rect = button.getBoundingClientRect()
      if (!button.isConnected || !rect.width || rect.bottom <= 0 || rect.top >= window.innerHeight) onClose()
    }
    window.addEventListener('scroll', check, true)
    window.addEventListener('resize', check)
    return () => {
      window.removeEventListener('scroll', check, true)
      window.removeEventListener('resize', check)
    }
  }, [button, onClose])

  // 卸载时把拖动会话收干净（旧实现在这条路上会漏：监听与 body 类要等下一次 pointerup 才摘掉）。
  // 拖动期间已经改过内存里的顺序（拖动只重画、不写盘），所以补一次 commit —— 不补的话内存与
  // localStorage 会不一致（表按内存读，下次启动按盘读）
  React.useEffect(() => () => {
    const session = dragSessionRef.current
    if (!session) return
    session.teardown()
    commit(spec)
  }, [spec])

  /** 显隐开关 */
  function setVisible(key: string, next: boolean): void {
    const item = spec.config.find(entry => entry.key === key)
    if (!item) return
    // 最后一列不许关：全隐藏之后表格只剩空壳，用户得先想起「是我自己关的」才能从面板里找
    // 回来。关不掉比关得掉再懊恼一次好。旧实现是静默拒绝（把开关状态改回去），这里同样是
    // 静默不改 —— 受控开关不回跳，视觉上等价。
    if (!next && spec.config.filter(entry => entry.visible).length <= 1) return
    item.visible = next
    commit(spec)
  }

  /** 对齐三档 */
  function setAlign(key: string, next: Align): void {
    const item = spec.config.find(entry => entry.key === key)
    if (!item || item.align === next) return
    item.align = next
    commit(spec)
  }

  /** 恢复默认：顺序、显隐、对齐一起回到列声明（旧实现同） */
  function resetAll(): void {
    spec.config = defaultsOf(spec)
    commit(spec)
  }

  /**
   * 开始拖动排序。用指针事件而不是 HTML5 拖放（理由见文件头）。
   * 拖动过程中每跨过一行就重排一次配置并重画，但**不写盘、不通知表** ——
   * 每跨一行都重绘表格会很吵；松手（或浮层被关掉）才 commit。
   */
  function startDrag(key: string, event: React.PointerEvent<HTMLElement>): void {
    event.preventDefault()
    dragSessionRef.current?.teardown() // 理论上不会有上一段没收的会话，兜一手
    setDraggingKey(key)
    document.body.classList.add(DRAGGING_CLASS)
    const rows = rowsRef.current

    function teardown(): void {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      document.body.classList.remove(DRAGGING_CLASS)
      dragSessionRef.current = null
      setDraggingKey(null)
    }

    function onMove(moveEvent: PointerEvent): void {
      const overKey = rowKeyAt(moveEvent.clientX, moveEvent.clientY, rows, spec.config)
      if (!overKey || overKey === key) return
      const from = spec.config.findIndex(item => item.key === key)
      const to = spec.config.findIndex(item => item.key === overKey)
      if (from < 0 || to < 0) return
      spec.config = moveItem(spec.config, from, to)
      repaint()
    }

    function onUp(): void {
      teardown()
      commit(spec)
    }

    dragSessionRef.current = { teardown }
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
  }

  return (
    <Popover
      open
      // 非模态（Base UI 的默认档）：面板不该有遮罩、不该锁滚动 —— 它是「看着表格调表格」的浮层
      modal={false}
      // 收起的出口：点按钮（同表再点一次，走 makeButton 的处理器）、点面板外、Esc
      // 都汇到这里。eventDetails 只在「点面板外」时要 —— 把那次 click 记下来，
      // 好让按钮自己的处理器认出「这一下就是我」，不再把面板开回去（见文件头）
      onOpenChange={(next, eventDetails) => {
        if (next) return
        if (eventDetails?.reason === 'outside-press') outsidePressEvent = eventDetails.event ?? null
        onClose()
      }}
    >
      <PopoverContent
        // 贴锚点右下：align='end' = 浮层右缘对齐按钮右缘、side='bottom' = 排在按钮下方，
        // sideOffset 是旧算法的 6px 间距；贴边收 8px 用 collisionPadding 表达（= 旧算法的 EDGE）。
        // 翻向由 floating-ui 的 flip 决定，差别见文件头。
        // 锚点直接用那颗命令式按钮：没有 Trigger，所以要用 anchor 显式告诉浮层贴谁。
        // 层级（z-[35]）与卡片观感（底 / 描边 / 阴影 / 圆角 / 最大高度）都在组件库里
        anchor={button}
        side='bottom'
        align='end'
        sideOffset={6}
        collisionPadding={8}
        className='w-[336px]'
        aria-label={t('{name}的列设置', { name: spec.label || spec.id })}
      >
        {/* 表头：与数据行同一套网格，四列依次是 手柄 / 列名 / 显示 / 对齐 */}
        <div
          className={cn(
            ROW_GRID,
            'border-b border-hairline px-3 pt-[9px] pb-[7px]',
            'text-[10.5px] font-semibold tracking-[.05em] text-muted-foreground',
          )}
        >
          <span />
          <span className='text-center'>{t('列')}</span>
          <span className='text-center'>{t('显示')}</span>
          <span className='text-center'>{t('对齐')}</span>
        </div>

        {/* 行区滚动：列多时面板不跟着长，最大高度由外层（组件库）的 max-h 兜底 */}
        <div ref={rowsRef} className='min-h-0 flex-1 overflow-y-auto px-3 py-1'>
          {spec.config.map(item => {
            const label = labelOf(spec, item.key)
            const dragging = draggingKey === item.key
            return (
              <div
                key={item.key}
                data-col-key={item.key}
                className={cn(
                  ROW_GRID,
                  'rounded-sm py-[5px] hover:bg-nav-hover',
                  // 被拖动的那一行压淡：让下面的落点更醒目。节点本身不参与重排
                  //（重排是数据层的事 + repaint，见 startDrag）
                  dragging && 'opacity-50',
                )}
              >
                <span
                  className='flex cursor-grab items-center justify-center text-muted-foreground opacity-50 hover:opacity-100'
                  title={t('按住拖动调整列顺序')}
                  onPointerDown={event => startDrag(item.key, event)}
                >
                  {GRIP_ICON}
                </span>
                <span className='min-w-0 truncate text-xs text-foreground' title={label}>
                  {label}
                </span>
                <Switch
                  className='justify-self-center'
                  checked={item.visible}
                  onCheckedChange={next => setVisible(item.key, next)}
                  aria-label={t('显示「{label}」列', { label })}
                  title={item.visible ? t('这一列正在显示') : t('这一列已隐藏')}
                />
                <SegmentedControl
                  className='w-full justify-center'
                  options={ALIGNS}
                  value={item.align}
                  onValueChange={next => setAlign(item.key, next)}
                  aria-label={t('「{label}」列的对齐', { label })}
                />
              </div>
            )
          })}
        </div>

        <div className='flex items-center gap-2 border-t border-hairline px-3 pt-2 pb-[9px]'>
          <span className='flex-1 text-[11px] text-muted-foreground'>{t('拖动 ⋮⋮ 调整顺序')}</span>
          <Button variant='outline' size='sm' onClick={resetAll}>
            {t('恢复默认')}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}

/* ─── 静态表头的同步（模型管理 / 网关 Key）───── */
//
// 这两张表的 <colgroup> 与 <thead> 写在 index.html 里（不是每次渲染重画），
// 所以列顺序与显隐要**就地重排既有元素**，不能按字符串重建：
//   · <col> 上带着 table-columns.js 拖出来的 inline 宽度，重建会丢；
//   · <th> 里插着列宽把手（.col-grip），重建后要等它下次补偿。
// appendChild 对已存在的元素是「移动」，两者都自动跟着走。
// 提前 continue 的那一支很关键：隐藏的列**从 DOM 里摘掉**而不是 display:none ——
// <col> 的 display 在表布局里各浏览器行为不一致，摘掉才是可靠的「这一列不存在」。

/** 把一列的对齐写到表头 / 单元格上（三档互斥，先摘后加） */
function applyAlign(el: Element | undefined, align: Align): void {
  if (!el) return
  el.classList.remove(...ALIGN_CLASSES)
  el.classList.add(`ta-${align}`)
}

/** 静态表头的元素表：id → { table, row, group, ths, cols } */
type StaticHead = {
  table: Element
  row: Element
  group: Element | null
  ths: Map<string, Element>
  cols: Map<string, Element>
}

/**
 * 为什么必须缓存：隐藏一列是把它的 <th> / <col> 从 DOM 里**摘掉**（见 syncStaticHead），
 * 摘掉之后 `querySelectorAll` 就再也查不到它 —— 现查的话，用户重新勾上那一列会**放不回去**
 * （元素只在内存里飘着）。所以首次同步时记住这批元素，之后一直从缓存取。
 * 整张表被重建时（引用变了，或 thead 不在文档里了）缓存作废重取 —— 重建后 DOM 里本就只剩
 * 当时可见的列，这是可接受的退化：表格重建自己也会丢掉列宽之类的东西。
 */
const staticHeads = new Map<string, StaticHead>()

function staticHeadOf(id: string, table: Element | null | undefined): StaticHead | null {
  const cached = staticHeads.get(id)
  if (cached && cached.table === table && cached.row.isConnected) return cached
  const row = table?.querySelector('thead tr')
  if (!row) return null
  const group = table?.querySelector('colgroup') ?? null
  const entry: StaticHead = {
    table: table as Element,
    row,
    group,
    ths: new Map([...row.querySelectorAll('th[data-col]')].map(el => [el.getAttribute('data-col') || '', el])),
    cols: new Map([...(group?.querySelectorAll('col[data-col]') || [])].map(el => [el.getAttribute('data-col') || '', el])),
  }
  staticHeads.set(id, entry)
  return entry
}

/**
 * 让一张静态表（<colgroup> + <thead> 写死在 HTML 里）的表头跟上配置：
 * 按配置顺序重排、隐藏的摘掉、对齐类重新贴一遍。
 *
 * `viewHidden` 是**视图级**的额外隐藏集合（可选）：模型管理页选中自定义提供商时要把
 * 「倍率」「来源」两列收起来 —— 那是内置家清单的字段，自定义家没有。它与用户在列设置里的
 * 配置是两码事，所以叠加而不是改写配置：切回内置家时原样恢复（元素被摘掉但仍在 staticHeads
 * 的缓存里，放得回去）。数据行由各自的渲染函数按同一份可见列产出，表头与表体才不会各画一个样。
 */
function syncStaticHead(
  id: string,
  table: Element | null | undefined,
  viewHidden?: ReadonlySet<string> | null,
): void {
  const spec = registry.get(id)
  if (!spec || !table) return
  const head = staticHeadOf(id, table)
  if (!head) return
  const { row, group, ths, cols } = head

  for (const item of spec.config) {
    const th = ths.get(item.key)
    if (!th) continue
    const col = cols.get(item.key)
    if (!item.visible || viewHidden?.has(item.key)) {
      th.remove()
      col?.remove()
      continue
    }
    applyAlign(th, item.align)
    // 顺序即配置顺序：appendChild 把它们逐个移到末尾，最终顺序就是遍历顺序
    row.appendChild(th)
    if (col && group) group.appendChild(col)
  }
  // 表头换了一副样子，列宽那一层要跟着重新对一遍：把手是按「哪一列在最后」放的
  //（最后一列不给 —— 它钉在表格右缘会顶出横向滚动条），而最后是哪一列由列设置说了算。
  shared().wbTableColumns?.repaint?.(id)
}

/* ─── 按钮与注册 ────────────────────────────── */

/**
 * 「列设置」触发按钮：照旧用命令式建原生按钮并插进 legacy 页面骨架（理由见文件头）。
 * 齿轮图标来自 icons.js（运行期读 wbIcons），与旧实现逐字一致。
 *
 * 它同时就是浮层的锚点（PopoverContent 的 anchor 收它），所以不必再套包裹层。
 */
function makeButton(spec: TableEntry): Anchor {
  const button = document.createElement('button')
  button.type = 'button'
  button.className = `sm colset-btn${spec.buttonClass ? ` ${spec.buttonClass}` : ''}`
  button.id = `btn-colset-${spec.id}`
  button.title = t('调整「{name}」的列：显示 / 隐藏、顺序、对齐', { name: spec.label || spec.id })
  button.innerHTML = `${shared().wbIcons?.icon?.('settings', 14) || ''}<span>${t('列设置')}</span>`
  button.setAttribute('aria-haspopup', 'dialog')
  button.setAttribute('aria-expanded', 'false')
  button.addEventListener('click', event => {
    // 面板开着时点这颗按钮：Base UI 已在捕获阶段把它判成「点面板外」并关了窗，
    // 这一次冒泡到这里只需认领标记、不再开回去（否则按钮按下去关不掉，见文件头）
    if (outsidePressEvent === event) {
      outsidePressEvent = null
      return
    }
    togglePanel(spec, button)
  })
  return button
}

function mountOf(spec: TableSpec): Element | null {
  const target = typeof spec.mount === 'function' ? spec.mount() : spec.mount ? document.querySelector(spec.mount) : null
  return target || null
}

/**
 * 登记一张表：读回本地配置、把「列设置」按钮插进指定容器。
 *
 * `mount` 取该页的操作区（元素、选择器或返回元素的函数）：工具条右侧、卡片头的操作组、
 * 或批量栏那类只放按钮的容器都行。按钮默认插在**最前面**：它是「怎么看这张表」的开关，
 * 与旁边那些「对数据做什么」的操作按钮不是一类，排在前面不会被误当成主操作按钮。
 * `buttonPlacement: 'last'` 改成插到末尾 —— 模型管理页要求它排在操作按钮之后。
 *
 * 列的 key 与渲染顺序的对应关系只能有一处定义：`spec.columns` 的顺序。渲染方必须按这个
 * 顺序产出单元格（模型管理 / 网关 Key 的 DOM 同步按位置给 <td> 认列，靠的就是它）。
 */
function register(spec: TableSpec): ColSettingsHandle {
  const entry: TableEntry = { ...spec, config: load(spec) }
  registry.set(spec.id, entry)
  const host = mountOf(spec)
  const button = makeButton(entry)
  if (host) {
    if (spec.buttonPlacement === 'last') host.appendChild(button)
    else host.insertBefore(button, host.firstChild)
  }
  return {
    apply: columns => apply(spec.id, columns),
    config: () => entry.config,
  }
}

/* ─── 注册：对外契约 ─────────────────────────── */

window.wbColSettings = { register, apply, configOf, syncStaticHead, close: closePanel }

declare global {
  interface Window {
    /** 表格列设置（替换 ui/table-col-settings.js，接口与原实现一致） */
    wbColSettings?: {
      /** 登记一张表并注入触发按钮；表侧拿返回的 apply / config 渲染 */
      register(spec: TableSpec): ColSettingsHandle
      /** 把「按渲染顺序排好的列数组」投影成用户配置的顺序 / 显隐，并带上对齐 */
      apply<C extends { key: string }>(id: string, columns: C[]): (C & { align: Align })[]
      /** 直接拿有序配置（自己判断显隐与对齐） */
      configOf(id: string): ColConfigItem[] | null
      /** 静态表头就地重排（模型管理 / 网关 Key）；viewHidden 是视图级额外隐藏集合 */
      syncStaticHead(id: string, table: Element | null | undefined, viewHidden?: ReadonlySet<string> | null): void
      /** 收起浮层（切页时由 app.js 调） */
      close(): void
    }
  }
}
