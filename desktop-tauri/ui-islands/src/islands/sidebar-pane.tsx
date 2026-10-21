/**
 * Agent2API · 设置 → 显示 → 侧边栏菜单（React 岛）
 *
 * 让用户控制主侧边栏：**显隐 / 顺序 / 分组**。参考了 OmniProxy 的
 * 「设置 → 外观与布局 → 侧边栏菜单显示」（一组开关），但这里多了三件事：
 * 拖拽排序（含跨组）、自定义分组（新建 / 改名 / 删除）、整组隐藏。
 *
 * ── 数据口径不在这里 ─────────────────────────────────────────
 * 布局的读写、校验、默认值、升级补位全部在 `ui/sidebar-layout.js`
 * （经典脚本，经 `window.wbSidebarLayout` 取用）。为什么不分两套：
 * 侧边栏由 `ui/app.js` 渲染、编辑面板是 React 岛，模型放任一侧另一侧就得抄一份
 * 规则，而「默认分组 + 键校验 + 新菜单补位」抄两遍必然漂。本文件只做三件事：
 *   ① 把那份模型包成 React 状态；② 画界面；③ 改完写盘并派 `wb:sidebar-layout`
 *   事件让 app.js 重画侧边栏。
 *
 * ── 拖拽为什么不用拖拽库 / HTML5 拖放 ────────────────────────
 * 与账号页分组模式、表格列设置同一取舍：拖动过程中配置一变就要重画，而 HTML5
 * 拖放的会话绑在**那个节点**上，一重画就断；指针事件的状态只在 JS 变量里，
 * 节点换掉不影响会话。视觉照旧：被拖项压淡、跨过哪一行就在哪一行落位、
 * 松手才写盘。
 *
 * ── 为什么不在面板里再画一个侧边栏预览 ───────────────────────
 * 主侧边栏就在同一屏的左边，改动即时生效 —— 再画一个小预览是重复信息，
 * 还会把面板挤窄（这一页是两栏布局）。所以这里只做「右侧面板 + 左侧真实侧边栏」。
 *
 * ── 与同事改动的边界 ────────────────────────────────────────
 * 本文件只被 settings-page.tsx 的 DisplayPane 引用一行；不碰 settings-state /
 * settings-model 的快照（侧边栏布局不进后端配置），也不碰「云同步」「版本信息」
 * 那两个分类。
 */

import * as React from 'react'
import { Badge, Button, Input, Popover, PopoverContent, PopoverTrigger, Switch, cn } from '@ui'
import { t } from '../i18n'

/* ─── 从经典脚本取模型 ─────────────────────── */

/**
 * `ui/sidebar-layout.js` 挂上来的接口。这里手写一份类型，而不是 import 那个 .js：
 * 它是经典脚本（给 app.js 用），没有模块导出。两边字段名必须一致 ——
 * 改那边时这里跟着改，TypeScript 会在每个调用点报错，不会静默漂。
 */
type MenuDef = { key: string; label: string; icon: string; fixed?: boolean }
type LayoutGroup = { id: string; label: string; custom: boolean; hidden: boolean; items: string[] }
type Layout = { groups: LayoutGroup[]; hidden: string[] }
type RenderGroup = { id: string; label: string; items: MenuDef[] }

type LayoutApi = {
  STORAGE_KEY: string
  MENUS: MenuDef[]
  MENU_KEYS: string[]
  defaultLayout(): Layout
  normalizeLayout(raw: unknown): Layout
  readLayout(): Layout
  writeLayout(layout: Layout): void
  cloneLayout(layout: Layout): Layout
  menuOf(key: string): MenuDef | undefined
  /** 分组名的展示文本：内置分组查词典，用户自建的原样返回 */
  groupLabel(label: string): string
  isFixedMenu(key: string): boolean
  locate(layout: Layout, key: string): { gi: number; index: number } | null
  groupHasFixed(group: LayoutGroup): boolean
  visibleCount(group: LayoutGroup, hidden: string[]): number
  moveItem(layout: Layout, key: string, toGi: number, toIndex: number): Layout | null
  moveGroup(layout: Layout, gid: string, toIndex: number): Layout | null
  setItemHidden(layout: Layout, key: string, hidden: boolean): Layout | null
  setGroupHidden(layout: Layout, gid: string, hidden: boolean): Layout | null
  renameGroup(layout: Layout, gid: string, label: string): Layout | null
  addGroup(layout: Layout, label: string): Layout | null
  removeGroup(layout: Layout, gid: string): Layout | null
  renderGroups(layout: Layout): RenderGroup[]
  firstVisibleKey(layout: Layout): string | null
  isVisible(layout: Layout, key: string): boolean
}

function layoutApi(): LayoutApi | null {
  return (window as unknown as { wbSidebarLayout?: LayoutApi }).wbSidebarLayout ?? null
}

/** 图标（icons.js 的内联 SVG 串）：与设置页左栏分类图标同一注入手法 */
function iconHtml(name: string, size: number): string {
  return (window as unknown as { wbIcons?: { icon?: (n: string, s?: number) => string } })
    .wbIcons?.icon?.(name, size) || ''
}

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构 —— 与 settings-model 同口径） */
function toast(message: string, kind?: 'err' | 'ok'): void {
  (window as unknown as { wbApp?: { toast?: (m: string, k?: string) => void } })
    .wbApp?.toast?.(message, kind)
}

/* ─── 图标 ─────────────────────────────────── */

/**
 * 面板自用的两枚图标（icons.js 里还没有，落地后可以补进去）。
 * 几何与 icons.js 那组描边图标同一套约定（1.8 粗细、圆头圆角），
 * 所以并排放在一起不会觉得是两套风格。
 */
const PLUS_ICON = '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" '
  + 'stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>'
const FOLDER_PLUS_ICON = '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" '
  + 'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'
  + '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/>'
  + '<path d="M12 11v6M9 14h6"/></svg>'

const GRIP_ICON = '<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true">'
  + '<circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/>'
  + '<circle cx="9" cy="12" r="1.4"/><circle cx="15" cy="12" r="1.4"/>'
  + '<circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/></svg>'

/** 一行菜单 / 一个分组头的网格：把手 / 图标 / 名称 / 尾巴 */
const ITEM_GRID = 'grid grid-cols-[18px_17px_minmax(0,1fr)_auto] items-center gap-2'

/* ─── 主组件 ───────────────────────────────── */

/**
 * 侧边栏菜单编辑面板。
 *
 * 状态策略：`layout` 是唯一受控状态，所有变更都走 `apply(next)` ——
 * 那里统一做「空结果 = 无变化」判定、写盘、派事件、刷新本地状态。
 * 编辑过程里的临时态（改名中的输入框、拖拽会话）另用 useState / useRef，
 * 不进 layout，避免把「未提交的编辑」混进存盘。
 */
export function SidebarPane() {
  const api = layoutApi()

  // 模型没加载（index.html 里 sidebar-layout.js 被挪走 / 加载失败）：
  // 面板退化为一句话说明。不抛错、不白屏 —— 设置页其余分类照常可用。
  if (!api) {
    return (
      <section className='panel'>
        <div className='panel-head'><h2>{t('侧边栏菜单')}</h2></div>
        <div className='panel-body'>
          <div className='settings-state'>{t('侧边栏布局模块未加载（ui/sidebar-layout.js 应在 app.js 之前引入）。')}</div>
        </div>
      </section>
    )
  }

  // 上面已收窄：把 api 作为 prop 传给内层组件，闭包里就不必到处判空
  return <SidebarPaneBody api={api} />
}

/**
 * 面板主体。拆出来只为一件事：让 `api` 在类型上确定非空 ——
 * 拖动 / 键盘排序这些闭包里每一处都要用它，逐处写 `api?.` 会把真正的
 * 「没加载」分支掩盖成静默无操作。
 */
function SidebarPaneBody({ api }: { api: LayoutApi }) {
  const [layout, setLayout] = React.useState<Layout>(() => api.readLayout())
  /** 正在改名的分组 id（null = 没有在改） */
  const [renaming, setRenaming] = React.useState<string | null>(null)
  /** 改名草稿：只在 renaming 期间有值 */
  const [renameDraft, setRenameDraft] = React.useState('')
  /** 新建分组的输入框是否展开 */
  const [adding, setAdding] = React.useState(false)
  const [addDraft, setAddDraft] = React.useState('')
  /** 被拖动的对象（拖动期间只用于压淡原项，不参与数据） */
  const [dragId, setDragId] = React.useState<string | null>(null)
  const dragRef = React.useRef<{ kind: 'group' | 'item'; id: string } | null>(null)
  const dragSessionRef = React.useRef<{ teardown: () => void } | null>(null)
  /** 最新布局：拖动中的 pointermove 闭包拿不到重绘后的 state，只能从这里读 */
  const layoutRef = React.useRef<Layout>(layout)

  /**
   * 提交一次布局变更。
   *
   * 三条约定：
   *   · 变更函数返回 null = 什么都没变（拖回原位 / 改名成同一个字）——
   *     不写盘、不重绘、不派事件，避免无意义的重画把拖拽会话打断；
   *   · 写盘之后立刻派 `wb:sidebar-layout`，app.js 据此重画左侧真实侧边栏；
   *   · 本地 state 用**写盘后规范化过的那份**（api.readLayout），
   *     而不是变更函数的返回值 —— 存盘时会补位 / 去重，两边必须一致。
   */
  const apply = React.useCallback((next: Layout | null): boolean => {
    if (!next) return false
    api.writeLayout(next)
    const normalized = api.readLayout()
    // ref 立刻更新：拖动中下一次 pointermove 可能早于 React 重绘，
    // 读到旧值就会按过期的下标再排一次，把刚放好的项又挪走
    layoutRef.current = normalized
    setLayout(normalized)
    window.dispatchEvent(new CustomEvent('wb:sidebar-layout'))
    return true
  }, [api])

  // 别的窗口 / 标签页改了布局（同一台机器上开了两个面板）：跟着刷新
  React.useEffect(() => {
    const sync = () => {
      const next = api.readLayout()
      layoutRef.current = next
      setLayout(next)
    }
    window.addEventListener('storage', sync)
    window.addEventListener('wb:sidebar-layout', sync)
    return () => {
      window.removeEventListener('storage', sync)
      window.removeEventListener('wb:sidebar-layout', sync)
    }
  }, [api])

  // 卸载时把没收尾的拖动会话收干净（与 table-col-settings 同一处理）
  React.useEffect(() => () => { dragSessionRef.current?.teardown() }, [])

  const menus = api.MENUS
  const visibleMenus = menus.filter(menu => !layout.hidden.includes(menu.key)).length
  const hiddenGroups = layout.groups.filter(group => group.hidden).length

  /* ── 变更动作 ── */

  const toggleItem = (key: string, visible: boolean) => {
    apply(api.setItemHidden(layout, key, !visible))
  }

  const toggleGroup = (gid: string) => {
    const group = layout.groups.find(item => item.id === gid)
    if (!group) return
    if (!group.hidden && api.groupHasFixed(group)) {
      // 不置灰按钮：置灰的按钮点下去毫无反应，用户只会以为界面坏了。
      // 允许点，但把「为什么不行、该怎么办」说清楚。
      toast(t('「设置」是固定项，含它的分组不能整组隐藏；可以单独隐藏组内其它菜单'))
      return
    }
    apply(api.setGroupHidden(layout, gid, !group.hidden))
  }

  const commitRename = (gid: string) => {
    const next = api.renameGroup(layout, gid, renameDraft)
    setRenaming(null)
    if (next) apply(next)
  }

  const commitAdd = () => {
    const next = api.addGroup(layout, addDraft)
    if (!next) { toast(t('请填一个分组名'), 'err'); return }
    setAdding(false)
    setAddDraft('')
    apply(next)
    toast(t('已新建分组「{name}」，点组头的 ＋ 把菜单移进来', { name: addDraft.trim() }))
  }

  const removeGroup = (gid: string) => {
    const group = layout.groups.find(item => item.id === gid)
    if (!group) return
    const target = layout.groups[0].id === gid ? layout.groups[1] : layout.groups[0]
    const ask = (window as unknown as {
      wbConfirm?: { ask?: (options: Record<string, unknown>) => Promise<boolean> }
    }).wbConfirm?.ask
    const message = group.items.length
      ? t('删除分组「{name}」？组内 {n} 个菜单会移到「{target}」，不会丢。', {
          name: group.label, n: group.items.length, target: target?.label ?? '',
        })
      : t('删除空分组「{name}」？', { name: group.label })
    const finish = (ok: boolean) => {
      if (!ok) return
      if (apply(api.removeGroup(layout, gid))) toast(t('已删除分组「{name}」', { name: group.label }))
    }
    // 没有确认框（极端时序）时直接删：删分组不销毁任何菜单，是可撤销的低危操作
    if (ask) void ask({ title: t('删除分组'), text: message, okText: t('删除'), bodyClass: '' }).then(finish)
    else finish(true)
  }

  const resetAll = () => {
    const ask = (window as unknown as {
      wbConfirm?: { ask?: (options: Record<string, unknown>) => Promise<boolean> }
    }).wbConfirm?.ask
    const finish = (ok: boolean) => {
      if (!ok) return
      apply(api.defaultLayout())
      toast(t('已恢复默认分组：概览 / 转发链路 / 诊断 / 系统'))
    }
    if (ask) {
      void ask({
        title: t('恢复默认侧边栏'),
        text: t('会恢复默认的分组、顺序与显隐设置。你自建的分组会被删除（组内菜单不会丢）。'),
        okText: t('恢复默认'),
        bodyClass: '',
      }).then(finish)
    } else finish(true)
  }

  /* ── 拖拽 ── */

  /**
   * 开始拖动。用指针事件而不是 HTML5 拖放（理由见文件头）。
   *
   * 落点判定：菜单行优先（落在某一行上 = 插到它前 / 后），否则退到分组卡片
   * （落在组头或组内空白 = 追加到该组末尾）。用 `elementFromPoint` 而不是
   * 事件 target —— 拖动中指针下方是原节点自己，事件 target 不可靠。
   */
  function startDrag(kind: 'group' | 'item', id: string, event: React.PointerEvent<HTMLElement>) {
    if (event.button !== 0) return
    event.preventDefault()
    dragSessionRef.current?.teardown()
    dragRef.current = { kind, id }
    setDragId(id)
    document.body.classList.add('sc-dragging')
    // 同步给源节点挂上「拖动中」的类，不等 React 重绘。
    //
    // 为什么必须同步：落点判定用 elementFromPoint，而指针底下压着的**正是源节点
    // 自己** —— 它要是不吃指针事件（.sc-item.is-dragging / .sc-group.is-dragging
    // 都是 pointer-events:none），elementFromPoint 永远返回它，落点算不出来，
    // 拖动就成了「看着能拖、松手没反应」。setDragId 触发的重绘是异步的，
    // 第一次 pointermove 很可能赶在重绘之前，所以这里直接改 DOM。
    const source = event.currentTarget.closest<HTMLElement>(
      kind === 'group' ? '[data-sc-group]' : '[data-sc-item]',
    )
    source?.classList.add('is-dragging')
    // 最新布局放 ref：拖动中每次重排都会换一份 layout，闭包捕获的是开始拖动
    // 那一刻的旧值 —— 拿旧值算下标，第二下就落错位置。
    layoutRef.current = layout

    const onMove = (moveEvent: PointerEvent) => {
      const current = dragRef.current
      if (!current) return
      const el = document.elementFromPoint(moveEvent.clientX, moveEvent.clientY) as HTMLElement | null
      if (!el) return
      const live = layoutRef.current

      if (current.kind === 'group') {
        const card = el.closest<HTMLElement>('[data-sc-group]')
        if (!card) return
        const gid = card.dataset.scGroup || ''
        if (!gid || gid === current.id) return
        const targetIndex = live.groups.findIndex(group => group.id === gid)
        if (targetIndex < 0) return
        const rect = card.getBoundingClientRect()
        const after = moveEvent.clientY > rect.top + rect.height / 2
        apply(api.moveGroup(live, current.id, after ? targetIndex + 1 : targetIndex))
        return
      }

      const row = el.closest<HTMLElement>('[data-sc-item]')
      if (row) {
        const targetKey = row.dataset.scItem || ''
        const gid = row.dataset.scGid || ''
        if (!targetKey || targetKey === current.id) return
        const gi = live.groups.findIndex(group => group.id === gid)
        if (gi < 0) return
        const index = live.groups[gi].items.indexOf(targetKey)
        if (index < 0) return
        const rect = row.getBoundingClientRect()
        const after = moveEvent.clientY > rect.top + rect.height / 2
        apply(api.moveItem(live, current.id, gi, after ? index + 1 : index))
        return
      }

      // 不在任何行上：落到分组卡片就追加到该组末尾
      const card = el.closest<HTMLElement>('[data-sc-group]')
      if (!card) return
      const gi = live.groups.findIndex(group => group.id === (card.dataset.scGroup || ''))
      if (gi < 0) return
      apply(api.moveItem(live, current.id, gi, live.groups[gi].items.length))
    }

    const teardown = () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
      document.body.classList.remove('sc-dragging')
      // 源节点可能已被重绘换掉，按选择器找当前那一个摘类（找不到也无所谓）
      document.querySelectorAll('.is-dragging').forEach(node => node.classList.remove('is-dragging'))
      dragRef.current = null
      dragSessionRef.current = null
      setDragId(null)
    }
    const onUp = () => { teardown() }

    dragSessionRef.current = { teardown }
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
  }

  /**
   * 键盘排序：把手聚焦后 Alt+↑/↓ 移动一格，跨过组边界时落到相邻组。
   * 不做这一条的话，「调整顺序」对只能键盘操作的用户就是不可用的 ——
   * 而侧边栏顺序恰恰是这类偏好的典型场景。
   */
  function moveByKeyboard(kind: 'group' | 'item', id: string, up: boolean) {
    if (kind === 'group') {
      const index = layout.groups.findIndex(group => group.id === id)
      if (index < 0) return
      apply(api.moveGroup(layout, id, up ? index : index + 2))
      return
    }
    const from = api.locate(layout, id)
    if (!from) return
    let toGi = from.gi
    let toIndex = from.index + (up ? 0 : 1)
    const group = layout.groups[from.gi]
    if (up && from.index === 0 && from.gi > 0) {
      toGi = from.gi - 1
      toIndex = layout.groups[toGi].items.length
    } else if (!up && from.index === group.items.length - 1 && from.gi < layout.groups.length - 1) {
      toGi = from.gi + 1
      toIndex = 0
    }
    apply(api.moveItem(layout, id, toGi, toIndex))
  }

  /* ── 视图 ── */

  return (
    <>
      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('侧边栏菜单')}</h2>
          <span
            className='tip-q'
            data-tip={t('控制主侧边栏显示哪些菜单、按什么顺序、怎么分组。隐藏只影响导航显示，不会撤销页面访问权限；被隐藏的页面仍可通过地址直接打开。')}
          />
          <div className='head-actions'>
            <Button variant='outline' size='sm' onClick={resetAll}>{t('恢复默认')}</Button>
          </div>
        </div>

        <div className='panel-body'>
          <div className='settings-state'>
            {t('拖动左侧 ⠿ 调整顺序（菜单可以跨组拖）；拨开关控制显隐；点分组名可重命名；组头的 ＋ 把别的菜单移进来；眼睛按钮隐藏整组。改动即时生效并记住，左侧侧边栏就是结果。键盘也能排序：把手聚焦后按 Alt+↑ / ↓。')}
          </div>

          <div className='sc-groups'>
            {layout.groups.map(group => {
              const shown = api.visibleCount(group, layout.hidden)
              const locked = api.groupHasFixed(group)
              const isRenaming = renaming === group.id
              return (
                <div
                  key={group.id}
                  data-sc-group={group.id}
                  className={cn('sc-group', group.hidden && 'is-hidden', dragId === group.id && 'is-dragging')}
                >
                  <div className='sc-group-head'>
                    <span
                      className='sc-grip'
                      role='button'
                      tabIndex={0}
                      title={t('拖动调整分组顺序（Alt+↑ / ↓ 也可）')}
                      aria-label={t('拖动分组 {name}', { name: group.label })}
                      onPointerDown={event => startDrag('group', group.id, event)}
                      onKeyDown={event => {
                        if (!event.altKey) return
                        if (event.key === 'ArrowUp') { event.preventDefault(); moveByKeyboard('group', group.id, true) }
                        if (event.key === 'ArrowDown') { event.preventDefault(); moveByKeyboard('group', group.id, false) }
                      }}
                      dangerouslySetInnerHTML={{ __html: GRIP_ICON }}
                    />

                    {isRenaming ? (
                      <Input
                        className='sc-rename'
                        value={renameDraft}
                        maxLength={12}
                        autoFocus
                        aria-label={t('分组名')}
                        onChange={event => setRenameDraft(event.target.value)}
                        onBlur={() => commitRename(group.id)}
                        onKeyDown={event => {
                          if (event.key === 'Enter') { event.preventDefault(); commitRename(group.id) }
                          if (event.key === 'Escape') { event.preventDefault(); setRenaming(null) }
                        }}
                      />
                    ) : (
                      <button
                        type='button'
                        className='sc-group-title'
                        title={t('点击重命名')}
                        onClick={() => { setRenaming(group.id); setRenameDraft(group.label) }}
                      >
                        {api.groupLabel(group.label)}
                      </button>
                    )}

                    <span className='sc-group-meta'>{t('{n} 项 · {shown} 显示', { n: group.items.length, shown })}</span>

                    <div className='sc-group-actions'>
                      <GroupPicker
                        layout={layout}
                        api={api}
                        group={group}
                        onPick={key => apply(api.moveItem(layout, key, layout.groups.findIndex(item => item.id === group.id), group.items.length))}
                      />
                      <button
                        type='button'
                        className='sc-icon-btn'
                        title={locked
                          ? t('含固定项「设置」，不可整组隐藏（可以单独隐藏组内其它菜单）')
                          : (group.hidden ? t('显示这一组') : t('隐藏这一组'))}
                        aria-label={group.hidden ? t('显示分组 {name}', { name: group.label }) : t('隐藏分组 {name}', { name: group.label })}
                        aria-pressed={!group.hidden}
                        onClick={() => toggleGroup(group.id)}
                        dangerouslySetInnerHTML={{ __html: iconHtml(group.hidden ? 'eyeOff' : 'eye', 15) }}
                      />
                      <button
                        type='button'
                        className='sc-text-btn'
                        disabled={layout.groups.length <= 1}
                        title={layout.groups.length <= 1 ? t('至少要保留一个分组') : t('删除这个分组（组内菜单移到第一组）')}
                        onClick={() => removeGroup(group.id)}
                      >
                        {t('删除')}
                      </button>
                    </div>
                  </div>

                  <div className='sc-items'>
                    {group.items.length === 0 ? (
                      <div className='sc-empty'>{t('空分组 · 点组头的 ＋ 把菜单移进来')}</div>
                    ) : group.items.map(key => {
                      const menu = api.menuOf(key)
                      if (!menu) return null
                      const isHidden = layout.hidden.includes(key)
                      return (
                        <div
                          key={key}
                          data-sc-item={key}
                          data-sc-gid={group.id}
                          className={cn('sc-item', ITEM_GRID, isHidden && 'is-hidden', dragId === key && 'is-dragging')}
                        >
                          <span
                            className='sc-grip'
                            role='button'
                            tabIndex={0}
                            title={t('拖动调整顺序，可跨组（Alt+↑ / ↓ 也可）')}
                            aria-label={t('拖动菜单 {name}', { name: menu.label })}
                            onPointerDown={event => startDrag('item', key, event)}
                            onKeyDown={event => {
                              if (!event.altKey) return
                              if (event.key === 'ArrowUp') { event.preventDefault(); moveByKeyboard('item', key, true) }
                              if (event.key === 'ArrowDown') { event.preventDefault(); moveByKeyboard('item', key, false) }
                            }}
                            dangerouslySetInnerHTML={{ __html: GRIP_ICON }}
                          />
                          <span className='sc-item-ico' dangerouslySetInnerHTML={{ __html: iconHtml(menu.icon, 15) }} />
                          <span className='sc-item-label' title={menu.label}>{menu.label}</span>
                          {menu.fixed ? (
                            <Badge variant='outline' shape='tag' title={t('设置不可隐藏（隐藏了只能清缓存找回来）')}>
                              {t('固定')}
                            </Badge>
                          ) : (
                            <Switch
                              checked={!isHidden}
                              onCheckedChange={next => toggleItem(key, next)}
                              aria-label={isHidden ? t('显示 {name}', { name: menu.label }) : t('隐藏 {name}', { name: menu.label })}
                              title={isHidden ? t('这一项已隐藏') : t('这一项正在显示')}
                            />
                          )}
                        </div>
                      )
                    })}
                  </div>
                </div>
              )
            })}
          </div>

          <div className='sc-add'>
            {adding ? (
              <div className='sc-add-form'>
                <Input
                  value={addDraft}
                  maxLength={12}
                  autoFocus
                  placeholder={t('分组名，例如「常用」')}
                  aria-label={t('新分组名')}
                  onChange={event => setAddDraft(event.target.value)}
                  onKeyDown={event => {
                    if (event.key === 'Enter') { event.preventDefault(); commitAdd() }
                    if (event.key === 'Escape') { event.preventDefault(); setAdding(false); setAddDraft('') }
                  }}
                />
                <Button size='sm' onClick={commitAdd}>{t('添加')}</Button>
                <Button variant='ghost' size='sm' onClick={() => { setAdding(false); setAddDraft('') }}>{t('取消')}</Button>
              </div>
            ) : (
              <Button variant='dashed' className='w-full' onClick={() => setAdding(true)}>
                <span dangerouslySetInnerHTML={{ __html: FOLDER_PLUS_ICON }} />
                {t('新建分组')}
              </Button>
            )}
          </div>

          <div className='settings-state'>
            {t('{total} 个菜单 · 显示 {shown} · 隐藏 {hidden} · {groups} 个分组', {
              total: menus.length,
              shown: visibleMenus,
              hidden: menus.length - visibleMenus,
              groups: layout.groups.length,
            })}
            {hiddenGroups ? t('（{n} 个整组隐藏）', { n: hiddenGroups }) : ''}
          </div>
        </div>
      </section>
    </>
  )
}

/* ─── 组头「＋」：把别的菜单移进这一组 ─────────── */

/**
 * 一个 Popover，列出「当前不在这一组」的菜单，并标出它们现在在哪一组。
 *
 * 为什么入口放在组头而不是每行一枚「移到分组」：13 个菜单每行再放一枚图标会挤，
 * 而「往这个组里加东西」本来就是组级的动作。拖拽也能做到同一件事，
 * 但拖动跨组需要先把目标组滚进视野，菜单多时并不好用 —— 两条路都留着。
 */
function GroupPicker({ layout, api, group, onPick }: {
  layout: Layout
  api: LayoutApi
  group: LayoutGroup
  onPick: (key: string) => void
}) {
  const [open, setOpen] = React.useState(false)
  const candidates = api.MENU_KEYS.filter(key => !group.items.includes(key))

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <button
            type='button'
            className='sc-icon-btn'
            title={t('把菜单移入「{name}」', { name: group.label })}
            aria-label={t('把菜单移入 {name}', { name: group.label })}
          />
        }
      >
        <span dangerouslySetInnerHTML={{ __html: PLUS_ICON }} />
      </PopoverTrigger>
      <PopoverContent align='end' side='bottom' className='w-[228px] p-1'>
        <div className='px-2 pt-1.5 pb-1.5 text-[10.5px] font-semibold tracking-[.04em] text-muted-foreground'>
          {t('移入「{name}」', { name: api.groupLabel(group.label) })}
        </div>
        {candidates.length === 0 ? (
          <div className='px-2 py-2 text-center text-[11px] text-muted-foreground'>
            {t('所有菜单都已在这一组')}
          </div>
        ) : (
          <div className='max-h-[280px] overflow-y-auto'>
            {candidates.map(key => {
              const menu = api.menuOf(key)
              if (!menu) return null
              const from = api.locate(layout, key)
              return (
                <button
                  key={key}
                  type='button'
                  className='flex h-7 w-full items-center gap-2 rounded-xs px-2 text-left text-[12.5px] text-foreground hover:bg-nav-hover'
                  onClick={() => { setOpen(false); onPick(key) }}
                >
                  <span className='grid size-[15px] flex-none place-items-center text-muted-foreground'
                    dangerouslySetInnerHTML={{ __html: iconHtml(menu.icon, 14) }} />
                  <span className='min-w-0 flex-1 truncate'>{menu.label}</span>
                  <span className='flex-none text-[10.5px] text-muted-foreground'>
                    {from ? api.groupLabel(layout.groups[from.gi].label) : ''}
                  </span>
                </button>
              )
            })}
          </div>
        )}
      </PopoverContent>
    </Popover>
  )
}
