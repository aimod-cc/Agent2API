/**
 * 账号表的**列定义（列宽口径）+ 列宽拖拽**（替换 ui/accounts-columns.js 与
 * ui/accounts-table.js 的 COLUMNS 常量）。
 *
 * ── 与列设置（wbColSettings）的分工 ────────────────────────────
 * 这是**两套**互不相干的机制，别混：
 *   · 本文件管**宽度** —— 表头右缘的把手拖动改 `<col>` 的 inline width，存 localStorage；
 *   · wbColSettings 管**显隐与顺序**（还带对齐）—— 它就地重排既有 `th[data-col]` /
 *     `col[data-col]`，并把隐藏的列从 DOM 里摘掉。
 * 两套共用同一份列集合：`ACCOUNT_COLUMNS` 的 key（= CSS 类后缀 `.cell-<key>`）。
 *
 * 表格是 table-layout: fixed，列宽由 `<colgroup>` 的 `<col>` 决定 —— 拖动只改被拖的
 * 那一列的 style.width，其余列不动。默认宽度只有 DEFAULTS 这一处（colgroup 逐列把它
 * 写成 inline width），page-accounts-table.css 的 `.cell-*` 是同一组数字的第二处声明，
 * 两处要同步（漂移的症状：用户双击把手「还原」后列宽跳到另一个值）。
 * 例外是弹性列（FLEX_COLUMNS）：它**不写宽度**，吃表格的剩余宽度。
 */

import { t } from '../i18n'
import type { Align } from './accounts-shared'

/** 列定义项（表头文案 / 小注 / 悬停说明 / 默认对齐 / 旧版默认对齐） */
export type AccountColumn = {
  key: string
  label: string
  /** 表头的悬停说明 */
  title?: string
  align: Align
  /** **上一版的默认对齐**，只被 table-col-settings 的 normalize 用来分辨旧存盘里那一档
   *  是「用户挑的」还是「旧默认值」—— 少了它，改过的默认对齐对老用户就不生效 */
  legacyAlign?: Align
}

/**
 * 列：勾选 / 顺序 / 账号 / 代理 / 连接数 / 状态 / 限流 / 有效期 / 余额 / 操作。
 *
 * ── 分组模式（2026-10）删掉了「优先级」与「提供商」两列 ──────────
 * 顺序改由**拖拽**表达，数字不再出现在表里：每一行是它所在组的组内序号（`#N`），
 * 组的先后由分组带的顺位表达 —— 于是「优先级」那一列（数字 + ↑↓）整体下线，
 * 「顺序」列顶上它的位置，并把拖动把手收进同一格（`⠿ #2`，见 OrderCell）。
 * 「提供商」列同时删掉：家名与图标已经在分组带上，行里再重复一遍是噪音。
 * 列设置里旧配置残留的这两个 key 由 table-col-settings 的归一化丢弃（认不出的列不认）。
 *
 * `key` 同时是 CSS 类名后缀（`cell-<key>`），默认列宽在 page-accounts-table.css 里按
 * 这些类名声明 —— 键名只有这一处定义。
 *
 * 勾选列的 label 给「选择」而不是空串：它在表格里确实没有表头文案（那一格是「全选」
 * 复选框），但列设置面板里必须有个名字 —— 面板按 label 显示，空串会退化成原始 key。
 *
 * 默认对齐：操作列居右（贴住表格右缘时整列有一条整齐的竖线），其余全部居中
 * （格子里装的是徽章 / 开关 / 序号 / 读数这类等宽或很短的内容，居中后同一列各行对齐
 * 到一条中轴，比左对齐更好扫读）。legacyAlign 只写在「上一版默认与新版不同」的列上。
 */
export const ACCOUNT_COLUMNS: AccountColumn[] = [
  { key: 'pick', label: t('选择'), align: 'center', legacyAlign: 'left' },
  {
    key: 'order', label: t('顺序'),
    title: t('组内先后：按住左边的把手上下拖动即可调整；整组的先后拖分组带的把手'),
    align: 'center', legacyAlign: 'left',
  },
  { key: 'account', label: t('账号'), align: 'center', legacyAlign: 'left' },
  {
    key: 'proxy', label: t('代理'),
    title: t('该账号出网走的代理（Clash 出口 / 自定义 / 直连）；点击可修改'),
    align: 'center',
  },
  {
    key: 'connections', label: t('连接数'),
    title: t('此刻正在使用这个账号的请求数（含还在下发内容的流式请求）；为 0 时不显示'),
    align: 'center',
  },
  { key: 'status', label: t('状态'), align: 'center', legacyAlign: 'left' },
  {
    key: 'limits', label: t('限流'),
    title: t('该账号当前限流中的模型；点徽章看明细'),
    align: 'center', legacyAlign: 'left',
  },
  { key: 'expiry', label: t('有效期'), align: 'center', legacyAlign: 'left' },
  // 「余额」列只放读数（查询按钮在操作列）：一个只显示余额数字的列叫「余额 / 积分」
  // 会让人以为这里还能点。而「余额」这个词也容得下各家的不同叫法（积分 / 余额）
  { key: 'usage', label: t('余额'), align: 'center', legacyAlign: 'left' },
  { key: 'actions', label: t('操作'), align: 'right' },
]

/* ─── 列宽 ─────────────────────────────────── */

const STORE_KEY = 'agent2api-accounts-col-widths'

/**
 * 默认列宽（px）：与 page-accounts-table.css 的 `.cell-*` 一一对应。
 *
 * 改这里**必须**同时改 CSS 与那份文件头的列宽预算说明 —— 三处是同一组数字。
 * 预算：九个固定列合计 1013px（勾选 47 / 顺序 84 / 代理 186 / 连接数 56 / 状态 80 /
 * 限流 148 / 有效期 80 / 余额 132 / 操作 200）。
 * 顺序列 84 是「拖动把手（20）+ 组内序号（如 #12，约 30）+ 间隔」的宽度：它替代了
 * 原先 132px 的优先级列（数字输入 + 两枚箭头），窄了 48px；
 * 代理列 186 是「节点名 :端口」的常见形态 + 选择器自带的约 41px 固定开销；
 * 余额列 132 是「主额度桶两行形态」的最小值（套餐名一行要放得下
 * 「ZCode Trust Build」，被省略号砍成「ZCode Tr…」这一列就白给了）；
 * 操作列 200 是四颗按钮并排的最坏情况（「已签到 / 余额 / 设置 / ⋯」），
 * 改按钮文案或增删按钮时重算一遍。
 *
 * **账号列不在这张表里** —— 它是唯一的弹性列，理由见 FLEX_COLUMNS。
 */
const DEFAULTS: Record<string, number> = {
  pick: 47,
  order: 84,
  proxy: 186,
  connections: 56,
  status: 80,
  limits: 148,
  expiry: 80,
  usage: 132,
  actions: 200,
}

/**
 * 弹性列的 key：**不写宽度**，由 table-layout: fixed 把剩余宽度全部给它。
 *
 * 只有账号列在这一组里：它是全表唯一内容长度不可控的列（昵称 / uid / 邮箱），
 * 该跟着窗口伸缩；其余十列装的是长度固定的控件（复选框 / 序号 / 开关 / 徽章 /
 * 四颗按钮），一起伸缩只会让同一列在不同窗口下忽宽忽窄，纵向对齐就失去意义。
 *
 * 这里**曾经**给账号列也写了 300px（DEFAULTS.account），于是 11 列全是定宽、一列弹性
 * 列都不剩：表格宽度被钉成 1493px 这个常数，而容器最宽只有 min(80vw, 页面限宽) −
 * 滚动槽，结果横向滚动条常驻、最右的「操作 / ⋯」列永远被切掉一截（按页脚量出来的
 * 溢出最少 104px）。而 page-accounts-table.css 那侧从始至终写着「.cell-account 不写
 * 宽度：table-layout: fixed 下它会自动吃掉剩下的全部」—— 与那一行 300px 正是两套
 * 互相矛盾的口径。
 *
 * 用户拖过之后它照旧是定宽（拖过就以用户的值为准），双击把手还原回弹性。
 */
const FLEX_COLUMNS = new Set(['account'])

/**
 * 弹性列的下限：容器窄到要把这一列压得比它还窄时，**撑宽表格交给横向滚动**，
 * 而不是继续挤它（page-accounts-table.css 的原则：不压缩列宽，宁可横滚）。
 *
 * 取 300 是「账号列原本的默认宽度」（分组化之前那一版的定宽值）。它同时是
 * **横向滚动的分界线**：容器窄于「定宽列合计（1013）+ 300 = 1313px」时表格原样横滚，
 * 宽于它则多余宽度全部归账号列 —— 表格不再恒定溢出、最右的「操作 / ⋯」列不再被切。
 */
const FLEX_MIN_WIDTH = 300

/** 拖动的下限：再窄就该点不准里面的控件了 */
const MIN_WIDTH = 56

/** 有宽度概念的列全集（定宽列的默认值 + 弹性列）：存盘回读的合法性校验按它认列 */
const KNOWN_COLUMNS = new Set([...Object.keys(DEFAULTS), ...FLEX_COLUMNS])

/** 用户改过的列宽（只有与默认不同的列才会有值），启动时从 localStorage 恢复 */
const overrides: Record<string, number> = (() => {
  try {
    const raw = JSON.parse(localStorage.getItem(STORE_KEY) || '{}') as Record<string, unknown>
    const clean: Record<string, number> = {}
    for (const [key, value] of Object.entries(raw || {})) {
      const width = Number(value)
      // 按「列存在与否」认列，不是按「有没有默认值」—— 弹性列没有默认值，但它可以有
      // 用户拖出来的覆盖（用真值判断会把账号列拖过的宽度在下次启动时悄悄丢掉）
      if (KNOWN_COLUMNS.has(key) && Number.isFinite(width) && width >= MIN_WIDTH) clean[key] = Math.round(width)
    }
    return clean
  } catch {
    return {}
  }
})()

function persist(): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(overrides))
  } catch { /* 隐私模式等存不了就算了：本次会话内仍然生效 */ }
}

/**
 * 渲染时的列宽表：定宽列的默认值 + 用户覆盖。
 *
 * 弹性列只在**被拖过**时才会出现在这里 —— 没有它的 key，`<ColGroup>` 就不给那一列写
 * inline width，它才真的是弹性的。这一条是整张表能不能吃满容器的关键，改动前务必
 * 先想清楚：把弹性列也塞进这个 map，表格宽度立刻退回一个常数，横滚条随之常驻。
 */
export function columnWidths(): Record<string, number> {
  return { ...DEFAULTS, ...overrides }
}

/**
 * 表格的宽度下限：`∑可见列宽 + 弹性列的下限`（已隐藏的列已从 DOM 摘掉，不计入）。
 *
 * 两个作用缺一不可：
 *   · table-layout: fixed 下弹性列吃剩余宽度，容器比「定宽列之和」还窄时它会被压成 0
 *     （表头直接看不见）—— 这个坑 ui/table-columns.js 的同名函数里记着实测；
 *   · 它同时是「表格开始横向滚动」的那一刻：容器宽于它就不滚、弹性列吃掉差额，
 *     窄于它才滚。所以要断言一张表在某宽度下会不会横滚，看的就是它。
 *
 * 因此这个值必须**从列宽算出来**，不能手写。原先它是 CSS 里一条写死的
 * `min-width: 1155px`（= 定宽列合计 1193 − 38），而账号列同时被钉着 300px ——
 * 表格真实需要 1493px，1155 永远碰不到：它没拦住任何东西，也没能让任何人注意到
 * 「账号页那条 1400px 的限宽已经比表格的下限还窄」（1193 + 300 + 滚动槽 11 = 1504 > 1400，
 * 所以那条限宽下面横滚条必然常驻）。
 */
export function tableMinWidth(visibleKeys: readonly string[]): number {
  let total = 0
  for (const key of visibleKeys) {
    const override = overrides[key]
    // 覆盖值不必再夹 MIN_WIDTH：拖动与读盘两处都已经夹过了
    if (override) total += override
    else if (FLEX_COLUMNS.has(key)) total += FLEX_MIN_WIDTH
    // 默认宽度**原样**计入，不要套 Math.max(MIN_WIDTH, …)：MIN_WIDTH 是「拖动能拖到
    // 多窄」的下限，不是布局地板 —— 勾选列 47px 就低于它，而 47 是算出来的对齐值
    // （16 + 15 + 16，见 page-accounts-table.css 的 .cell-pick），抬到 56 会让表格在
    // 还有余地时就先横滚
    else if (DEFAULTS[key]) total += DEFAULTS[key]
    else total += MIN_WIDTH
  }
  return Math.round(total)
}

/**
 * 第 index 个表头格对应的「列 key + 它的 `<col>`」。
 *
 * 列设置能藏列、能换顺序，所以**不能**只按位置认列：位置只是拿表头格用的，真正的
 * 身份是 `data-col`（表头 th 与 colgroup 的 col 上同名），拿到 key 之后再按 key 找
 * 那个 `<col>` —— 两处口径一致，用户拖过顺序之后也不会把宽度写到别的列上。
 */
function columnAt(table: Element | null, index: number): { key: string; col: Element } | null {
  const header = table?.querySelector(`thead th:nth-child(${index + 1})`)
  const key = header?.getAttribute('data-col') || header?.className.match(/cell-([a-z]+)/)?.[1]
  if (!key) return null
  const col = table?.querySelector(`colgroup col[data-col="${CSS.escape(key)}"]`)
    || table?.querySelectorAll('colgroup col')[index]
  return col ? { key, col } : null
}

/** 把一次宽度落进 `<col>`（拖动中实时调用的就是它） */
function applyWidth(col: Element, key: string, px: number): void {
  const width = Math.max(MIN_WIDTH, Math.round(px))
  ;(col as HTMLElement).style.width = width + 'px'
  // 定宽列拖回默认值就不必存盘；弹性列没有「默认宽度」可比 —— 只要拖过就存下来
  // （它由此从「吃剩余」变成定宽，双击把手再还原回弹性）
  if (FLEX_COLUMNS.has(key) || width !== DEFAULTS[key]) overrides[key] = width
  else delete overrides[key]
}

/** 列宽改动后请视图重绘（colgroup 由 widths() 统一生成，重绘让 DOM 与持久化状态对齐） */
let repaint = (): void => {}

/**
 * 委托绑定：pointerdown 开拖、dblclick 还原。挂在滚动容器上一次即可，
 * 表格被整表重绘后监听仍然有效（委托到容器，不依赖具体节点）。
 *
 * 与 table-columns.js 同一套写法（指针事件 + setPointerCapture + 两道收尾兜底）：
 * 松手若被浏览器丢掉（指针拖出窗口再松），拖动就永远不结束，鼠标一动列宽就跟着走。
 */
export function bindColumnGrips(host: HTMLElement | null, onChange: () => void): void {
  repaint = onChange
  if (!host) return
  let dragging: { col: Element; key: string; startX: number; startWidth: number; grip: Element; pointerId: number } | null = null

  host.addEventListener('pointerdown', event => {
    if (event.button !== 0) return
    const grip = (event.target as Element | null)?.closest?.('.col-grip')
    if (!grip) return
    event.preventDefault()
    const th = grip.closest('th')
    const table = th?.closest('table') || null
    const index = th?.parentElement ? [...th.parentElement.children].indexOf(th) : -1
    const column = columnAt(table, index)
    if (!column) return
    const startX = event.clientX
    const startWidth = column.col.getBoundingClientRect().width
    grip.classList.add('active')
    document.body.classList.add('col-resizing')
    try { (grip as Element & { setPointerCapture(id: number): void }).setPointerCapture(event.pointerId) } catch { /* 退回全局监听 */ }
    dragging = { ...column, startX, startWidth, grip, pointerId: event.pointerId }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
    window.addEventListener('pointercancel', up)
    window.addEventListener('blur', up)

    function move(moveEvent: PointerEvent): void {
      if (!dragging) return
      if (moveEvent.buttons === 0) { up(); return }
      applyWidth(dragging.col, dragging.key, dragging.startWidth + moveEvent.clientX - dragging.startX)
    }

    function up(): void {
      if (!dragging) return
      const { grip: activeGrip, pointerId } = dragging
      dragging = null
      activeGrip.classList.remove('active')
      document.body.classList.remove('col-resizing')
      try { (activeGrip as Element & { releasePointerCapture(id: number): void }).releasePointerCapture(pointerId) } catch { /* 已自动释放 */ }
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', up)
      window.removeEventListener('blur', up)
      persist()
      repaint()
    }
  })

  host.addEventListener('dblclick', event => {
    const grip = (event.target as Element | null)?.closest?.('.col-grip')
    if (!grip) return
    const th = grip.closest('th')
    const table = th?.closest('table') || null
    const index = th?.parentElement ? [...th.parentElement.children].indexOf(th) : -1
    const column = columnAt(table, index)
    if (!column) return
    delete overrides[column.key]
    persist()
    repaint()
  })
}
