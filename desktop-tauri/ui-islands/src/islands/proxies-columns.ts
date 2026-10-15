/**
 * 代理池表的**列声明 + 列设置接入 + 列宽层接入**（非岛：`.ts` 不被当岛加载）。
 *
 * 从 proxies-page.tsx 拆出来：列这件事要三处对齐 —— 表格的渲染顺序、列设置
 * （显隐 / 顺序 / 对齐）与列宽（把手 + 覆盖值），三处读的必须是同一份列集合，
 * 所以列 key 只在本模块定义一次，页面与两个能力层都从这里取。
 *
 * ── 与另外五张表同一条口径 ────────────────────────────────────
 *   · **列设置**走 `window.wbColSettings`（islands/table-col-settings.tsx）：
 *     `register` 一次（顺手把「列设置」齿轮按钮插进面板头的操作组），页面用
 *     `visibleColumns()` 拿「按用户配置排好序的可见列」渲染表头与表体；
 *   · **列宽**走 `window.wbTableColumns`（ui/table-columns.js）：它按 `data-col`
 *     定位 `<col>` 与 `<th>`，默认列宽写在 `<col>` 的类上（没拖过的列走类里的
 *     默认值），拖过的列由它写 inline width、双击把手还原；
 *   · 本模块把**当前可见列**导出去（`window.wbProxiesPanel.visibleColumns`），
 *     列宽层要按它算「覆盖值落到哪一列 / 哪一列在最后（末列的把手往里挪）」。
 *
 * 隐藏某一列时列宽层的覆盖值不会丢：配置里只是 `visible: false`，key 还在，
 * 用户重新勾回来时宽度照旧。
 */

import { t } from '../i18n'

type Align = 'left' | 'center' | 'right'

/** 列声明（`key` 与渲染出的 `<th data-col>` / `<col data-col>` 三处同名） */
export type ProxyColumn = { key: string; label: string; align?: Align }

/**
 * 六列：勾选 / 节点 / 地址 / 启用 / 连通性 / 操作。
 *
 * 默认对齐照接入前的口径：勾选与启用居中（那两格装的是控件，居中的中轴更整齐）、
 * 操作居右（贴住表格右缘是一条整齐的竖线）、其余左对齐（地址与失败原因是长文本）。
 * 「勾选」在表头那一格是全选复选框、没有文案，但列设置面板里得有个名字 ——
 * 空串会退化成原始 key（与账号表的「选择」同一条约定）。
 */
export const PROXY_COLUMNS: readonly ProxyColumn[] = [
  { key: 'pick', label: t('选择'), align: 'center' },
  { key: 'name', label: t('节点') },
  { key: 'addr', label: t('地址') },
  { key: 'state', label: t('启用'), align: 'center' },
  { key: 'test', label: t('连通性') },
  { key: 'act', label: t('操作'), align: 'right' },
]

/**
 * 各列的**默认宽度类**（写在 `<col>` 上，没拖过的列走它）。
 *
 * 是普通 CSS 类而不是 Tailwind 工具类，位置在 css/page-gateway.css 的
 * `.proxies-table .p-*`：组件库的 Tailwind 是 `!important` 导出的，工具类的
 * `width: 196px !important` 会压住列宽层写下的 inline 宽度 —— 拖完一松手列宽
 * 就弹回原值。其余三张表的默认列宽（`.c-*` / `.k-*`）也是同一口径。
 *
 * 节点列**不给宽度**：它是全表唯一内容长度不可控的列（名字 + 协议 + 引用数），
 * 该吃掉剩余宽度；连通性列给 28% —— 失败原因有两行（结论 + 上游错误原文）。
 */
export const COL_CLASS: Record<string, string> = {
  pick: 'p-pick',
  name: '',
  addr: 'p-addr',
  state: 'p-state',
  test: 'p-test',
  act: 'p-act',
}

/** 列设置里的表 id：与 table-columns.js 的登记、wbTableColumns.repaint 同一个 */
export const COL_SETTINGS_ID = 'proxies'

export type ColSettingsHandle = {
  apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[]
  config(): unknown
}

/**
 * 两个能力层都是别的岛 / 脚本挂上来的共享桥，这里按「窄类型 + 转型」读，
 * 不往 Window 上加全局声明（多文件同名属性类型不一致会撞 TS2717）。
 */
type ColumnsWindow = {
  wbColSettings?: {
    register(spec: {
      id: string
      label?: string
      columns: readonly { key: string; label?: string; align?: Align }[]
      mount?: () => Element | null
      onChange?: () => void
    }): ColSettingsHandle
    close?(): void
  }
  wbTableColumns?: { repaint?: (id: string) => void }
}

function columnsWindow(): ColumnsWindow {
  return window as unknown as ColumnsWindow
}

/** 登记结果（拿到就不再重复登记；能力层未就绪时为 null） */
let handle: ColSettingsHandle | null = null

/** 列设置改动后的重画入口：页面挂载后登记（onChange 从 React 之外回调进来） */
let onColumnsChanged: (() => void) | null = null

/**
 * 该表当前可见的列（顺序即用户拖出来的顺序；列设置未就绪时退回全部列）。
 * 页面按它渲染，列宽层也按它算 —— 两边读同一份配置才不会各算一个样。
 */
export function visibleColumns(): (ProxyColumn & { align: Align })[] {
  if (handle) return handle.apply([...PROXY_COLUMNS])
  return PROXY_COLUMNS.map(column => ({ ...column, align: column.align ?? 'left' }))
}

/** 让列宽层重新对一遍把手与覆盖值（表头重画、列集合变化后调；幂等） */
export function repaintColumnWidths(): void {
  columnsWindow().wbTableColumns?.repaint?.(COL_SETTINGS_ID)
}

/**
 * 登记列设置（只做一次）并同步一次表头。
 *
 * `mount` 取面板头的操作组：按钮要插进**本岛渲染出来的**容器，所以得在 React
 * 提交之后调用；而本模块在 glob 里排在 table-col-settings 之前，首帧那一刻
 * `window.wbColSettings` 还没挂上 —— 拿不到就返回 false，由调用方隔一拍重试。
 *
 * @returns 已经登记好（true）还是能力层没就绪、需要重试（false）
 */
export function setupColumns(onChange: () => void): boolean {
  onColumnsChanged = onChange
  if (handle) return true
  const api = columnsWindow().wbColSettings
  if (!api?.register) return false
  handle = api.register({
    id: COL_SETTINGS_ID,
    label: t('代理池表'),
    columns: PROXY_COLUMNS.map(column => ({
      key: column.key,
      label: column.label,
      align: column.align ?? 'left',
    })),
    mount: () => document.querySelector('.page[data-page="proxies"] .panel-head .head-actions'),
    // 改动落地：整页重画一次，表头与表体按同一份可见列产出（列宽层由页面那侧的
    // repaintColumnWidths 跟上，见 proxies-page 的布局副作用）
    onChange: () => onColumnsChanged?.(),
  })
  return true
}

/** 收起列设置浮层（页面卸载时调，别让它孤零零留在屏幕上） */
export function closeColumnPanel(): void {
  columnsWindow().wbColSettings?.close?.()
}
