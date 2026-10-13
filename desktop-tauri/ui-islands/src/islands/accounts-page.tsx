/**
 * Agent2API · 账号页（**按家分组**的一张表 + 批量栏 + 两个弹窗）—— React 岛。
 *
 * 替换 ui/accounts-table.js + ui/accounts-view.js + ui/accounts-filters.js +
 * ui/accounts-columns.js + ui/accounts-groups.js + ui/accounts-model.js +
 * ui/usage-actions.js + ui/account-panel.js + ui/proxy-form.js 九个文件。
 * 本文件只放**页面骨架与挂载**：行与单元格在 accounts-panels.tsx，两个弹窗在
 * accounts-dialogs.tsx，状态与动作在 accounts-data.ts，纯逻辑在 accounts-domain.ts，
 * 顺序拖拽在 accounts-queue-drag.ts。
 *
 * ── 对外契约（必须原样保留，调用点逐个 grep 确认过）─────────────
 *   · `window.wbAccountsView` —— app.js:147 syncConnections / app.js:575 render /
 *     app.js:622 refreshCaches / app.js:811 与 tasks-panel.tsx:559 syncBalancesSnapshot /
 *     providers.js:138 render
 *   · `window.wbAccountsModel` —— app.js:178 isRateLimited / app.js:663 isDesktopAccount /
 *     report.js:307 editionSuffix / models-fetch-modal.tsx:245 providerFeatures、
 *     :255 byPriorityOrder
 *   · `window.wbAccountPanel` —— app.js:623 invalidate / app.js:645 open
 *   注册都在 accounts-data.ts 的 installAccountsApi()（模块求值即完成）。
 *
 * ── 分组模式（2026-10）：一张表，**一组一个 `<tbody>`** ─────────
 * 账号按家分组显示：每个 `<tbody class="acct-group" data-provider=…>` = 一家，
 * 首行是通栏的分组带（`GroupBandRow`），其余行是这一家的账号。为什么仍然用一张
 * `<table>` 而不是改成 div 网格：列宽拖拽、列设置（显隐 / 顺序 / 对齐）都是按
 * `thead th[data-col]` 与 `colgroup col[data-col]` 就地重排既有元素实现的，
 * 换成网格等于把这两套机制全部重写一遍；多个 tbody 是 HTML 本来就允许的形状。
 *
 * 顺序也不再由「优先级」列承载，而是**拖出来的**：
 *   · 拖分组带（`.queue-grip`）= 改这一家的**组顺位**；
 *   · 拖行里的把手（`.order-grip`）= 改它在这一家内的**组内顺位**。
 * 两者合并成一条队列交给后端整队重编号（`POST /api/accounts/reorder`，见
 * accounts-data.ts 的 reorderQueue）。筛选 / 搜索 / 分页生效时顺序不可见（行被裁过，
 * 序号也断了），此时**锁定拖拽**并给一次提示 —— 拖动必然错位。
 *
 * ── 静态表头（本页最容易出错的地方）────────────────────
 * `<colgroup>` 的 `col[data-col]` 与 `<thead>` 的 `th[data-col]`（见 accounts-panels.tsx
 * 的 TableHead）是**字面量**渲染，永远按列定义的原始顺序渲染全部列、且不随任何状态
 * 变化：列的显隐与顺序靠 `wbColSettings.syncStaticHead` **就地重排既有元素**
 * （隐藏的列是从 DOM 里摘掉而不是 display:none），不能按状态重建 —— `<col>` 上带着列宽
 * 拖拽写进去的 inline 宽度，`<th>` 里插着列宽把手，重建会把两者一起丢掉。React 只在
 * 「同一位置、同一类型的子节点」上做属性 diff，这些节点的 props 与文本逐字不变，
 * 重渲染时一次 DOM 写都不会发生。数据行（tbody）相反：完全按 visibleColumns() 逐列渲染。
 *
 * ── 列宽（accounts-columns）与列设置（wbColSettings）是两套 ──────
 * 前者管宽度（拖表头右缘的把手），后者管显隐 / 顺序 / 对齐。列的集合只有一份
 * （accounts-columns.ts 的 ACCOUNT_COLUMNS），两套都按 `data-col` 认列。
 *
 * ── 挂载必须**同步**（flushSync）───────────────────────────
 * 账号页的「添加账号」按钮（`#btn-add-account-2`）由 ui/add-account.js:196 用
 * `$('btn-add-account-2').addEventListener(...)` **非可选链**地绑定，而那个脚本排在
 * islands/ui.js 之后、同步执行。React 的首次渲染默认是异步的（走 Scheduler 的宏任务），
 * 那时按钮还不存在 → 那一行会抛 TypeError，把 add-account.js 后面所有监听一起带崩。
 * 所以首次挂载用 flushSync 强制同步提交（DOM 与布局 effect 都落地），后续更新照常异步。
 *
 * ── 隐藏优先条件渲染 ───────────────────────────────────────
 * 组件库的工具类是**分层 + !important** 的，tokens.css 的 `[hidden] { display:none
 * !important }` 未分层；按 Cascade 5，important 的层序反转 —— 分层压过未分层。
 * 组件库 globals.css 已补同层的 `[hidden][hidden]` 兜底，属性式显隐因此可用；
 * 但能条件渲染就条件渲染（少一批常驻 DOM），属性式留给「节点必须常驻」的场合
 * （登录引擎的段落与取消按钮，见 add-provider-blocks.tsx）。
 */

import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { t } from '../i18n'
import { Button, Checkbox, Input, SegmentedControl, cn } from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { shared, toast, type AccountRecord, type ColSettingsHandle } from './accounts-shared'
import { isDesktopAccount, isEnabled, providerOf, supportsUsage } from './accounts-domain'
import { ACCOUNT_COLUMNS, bindColumnGrips, columnWidths, tableMinWidth } from './accounts-columns'
import {
  allAccounts, clearFilter, clearLimits, clearSelection, ensureProxyPoolOptions, filterActive,
  getStore, groupView, installAccountsApi, openBatchDialog, panelOpen, queryAllUsage, reorderQueue,
  rowContext, seats, segmentCounts, setAllGroupsCollapsed, setAllPicked, setSearchFilter,
  setSegmentFilter, snapshot, startConnectionsPolling, subscribe, tokenUsageOf, togglePick,
  usageEntryOf,
} from './accounts-data'
import {
  AccountCell, ActionsCell, ConnectionsCell, ExpiryCell, GroupBandRow, LimitsCell, OrderCell,
  PanelsRow, ProxyCell, StatusCell, TableHead, UsageCell,
} from './accounts-panels'
import { bindQueueDrag, type QueueDrop } from './accounts-queue-drag'
import { AccountsDialogs } from './accounts-dialogs'

/** 列设置的句柄（注册在挂载后的布局 effect 里，见下方说明） */
let colHandle: ColSettingsHandle | null = null
/** 列设置是否已经登记过（登记只做一次：它是全局注册表，重复登记会插出第二颗齿轮） */
let colSettingsRegistered = false
/**
 * 登记的重试定时器。
 *
 * 为什么需要重试：本岛的首帧是 `flushSync` 同步提交的（为了让 add-account-modal 在
 * **模块求值期**就能查到「添加账号」那颗按钮），而布局 effect 就挂在那次同步提交上 ——
 * 那一刻本模块还排在 `table-col-settings.tsx` 前面，`window.wbColSettings` 尚未挂上，
 * 一次性登记会静默落空、齿轮永远不出现（旧实现是加载期注册，没有这个时序问题）。
 * 所以拿不到就隔一拍再试，直到组件卸载。
 */
let colSettingsRetry = 0
/** 表格元素（静态表头的就地重排要用它；由 <table ref> 写入） */
let tableEl: HTMLTableElement | null = null
/** 表头已经同步过的「表格在不在 + 可见列与对齐」签名：只有它变了才重排表头（见 syncHead） */
let headSignature = ''

/** 当前可见列（顺序即配置顺序；列设置未就绪时退回全部列） */
function visibleColumns(): Array<(typeof ACCOUNT_COLUMNS)[number] & { align: 'left' | 'center' | 'right' }> {
  return colHandle ? colHandle.apply(ACCOUNT_COLUMNS) : ACCOUNT_COLUMNS
}

/** 触发一次重绘（store 换快照 → 订阅者重画） */
function requestRender(): void {
  window.wbAccountsView?.render?.()
}

/**
 * 表头就地重排（顺序 / 显隐 / 对齐）+ 把列宽把手挪到「不是最后一列」的格子上。
 *
 * 签名比对是必需的：这个方法要在**每次**提交后检查（注册完成、加载完成、配置变化都
 * 会改签名），而 `syncStaticHead` 内部是 `appendChild` 逐列移动既有元素 —— 无谓地每
 * 2 秒（连接数轮询）重排一次会不断产生 DOM 变动记录。
 */
function syncHead(): void {
  const columns = visibleColumns()
  const signature = `${tableEl ? 'T' : 'F'}|${columns.map(column => `${column.key}:${column.align}`).join('|')}`
  if (signature === headSignature) return
  headSignature = signature
  shared().wbColSettings?.syncStaticHead('accounts', tableEl)
  // 把手要放在「不是最后一列」的格子上：它绝对定位在右缘（right: -4px），钉在表格
  // 右缘会顶出一条横向滚动条。「哪一列在最后」是用户配置出来的，所以按渲染后的位置判。
  const ths = [...(tableEl?.querySelectorAll('thead th') || [])]
  ths.forEach((th, index) => {
    const grip = th.querySelector('.col-grip') as HTMLElement | null
    if (grip) grip.style.display = index === ths.length - 1 ? 'none' : ''
  })
}

/* ─── 页面 ─────────────────────────────────── */

function AccountsPage() {
  const store = React.useSyncExternalStore(subscribe, getStore)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  /** 登记列设置之后强制重画一次用的本地计数器（不依赖 store 订阅的时序，见下） */
  const [, forceTick] = React.useReducer((tick: number) => tick + 1, 0)

  const loaded = Boolean(snapshot())
  const all = allAccounts()
  // groups = 要渲染的（当前筛选下一条都看不见的组不留空壳）；allGroups = 全部组，
  // 拼队列与「折叠 / 展开全部」都用它
  const { groups, allGroups } = groupView(usageEntryOf, tokenUsageOf)
  // 限流维度的分段把「余额不足已跳过 / Token 限额已跳过」都算进已限流
  // （isLimitedNow 的口径），所以筛选与计数要带上余额与 Token 两份读数的查找口
  // —— 缓存每次 bump 后这里重算
  const counts = segmentCounts(usageEntryOf, tokenUsageOf)
  /** 真正会渲染出来的行（组序 → 组内序）。全选 / 计数 / 分页都以它为基准：
   *  被隐藏的组（一个启用的都没有）不该算进「当前看到的这些」—— 否则「全选（8 个）」
   *  会连看不见的行一起勾上，数字也对不上。分段计数（工具条那几枚徽标）另一回事：
   *  它说的是「一共有多少个」，不该被显示规则影响。 */
  const shown = groups.flatMap(group => group.accounts)
  const seatMap = seats()
  const columns = visibleColumns()
  /**
   * 客户端分页（通用表格外壳）：默认给「全部」这一档 —— 分组模式下按行分页会把
   * 一家的行切成两页（组头留在上一页），想分页的用户可以在页脚自己换档位。
   *
   * 分页生效时**顺序不可拖**（见 locked）：拖拽落点由渲染出来的行算，而重排要的是
   * 完整队列，两者在分页下对不上。所以它进了锁的判据（与筛选同一个口径）。
   */
  const paging = useClientPaging(shown.length, 'accounts', { defaultSize: 'all' })
  const pageRows = paging.paged ? paging.slice(shown) : shown
  const locked = filterActive() || paging.paged
  /** 筛选 / 搜索 / 分页任一在效时，顺序不可调整 —— 提示语要说清是卡在哪一条上 */
  const lockReason = filterActive() ? t('清除筛选') : t('把每页调回「全部」')

  // 代理列的选项（「网络代理」页的池条目）没就绪就补拉（节流 30 秒，失败不缓存）。
  // Clash 出口那一组已经不在这一页了 —— 出口统一由代理页的「同步 Clash Verge」
  // 导入池，这里只读池（见 accounts-panels 的 ProxyCell）
  React.useEffect(() => { ensureProxyPoolOptions() })

  // 列设置的注册必须在挂载后：按钮要插进本岛渲染出来的 `.batch-actions`，而
  // syncStaticHead 要求登记表已存在（它按 id 查配置）。用布局 effect 是为了让首屏的
  // 表头重排与按钮插入都赶在后续同步脚本之前完成（首次提交本身是 flushSync 的）。
  //
  // 登记完要立刻重画一次（forceTick，而不是等 store 的订阅回调 —— 那个订阅装在被动
  // effect 里，首屏这一刻还没生效）：登记之前那一次渲染读的是「全列」兜底值，用户存过的
  // 显隐 / 顺序必须在首屏就落到表头与数据行上（旧实现在加载期注册，所以首屏天然是对的）。
  React.useLayoutEffect(() => {
    if (!colSettingsRegistered) {
      // wbColSettings 还没挂上（本模块在 glob 里排在它前面，而首帧是同步提交的）：
      // 隔一拍再试，别把这次落空当成「登记过了」
      if (!shared().wbColSettings?.register) {
        window.clearTimeout(colSettingsRetry)
        colSettingsRetry = window.setTimeout(() => setRetryTick(tick => tick + 1), 50)
        return
      }
      colSettingsRegistered = true
      colHandle = shared().wbColSettings?.register({
        id: 'accounts',
        label: t('账号表'),
        columns: ACCOUNT_COLUMNS.map(column => ({
          key: column.key,
          label: column.label,
          align: column.align,
          legacyAlign: column.legacyAlign,
        })),
        // 挂载点是批量栏右侧的操作组（「批量操作 / 取消选择」那两颗）：齿轮插在最前，
        // 正好落在「批量操作」左边（列设置是「怎么看这张表」，与旁边那些「对数据做什么」
        // 的操作按钮不是一类，排头更顺）
        mount: () => document.querySelector('#batch-bar .batch-actions'),
        onChange: requestRender,
      }) || null
      if (colHandle) forceTick()
      return
    }
    syncHead()
  })
  // 登记重试的驱动：定时器到点改一次这个计数，让上面的布局 effect 再跑一遍
  const [, setRetryTick] = React.useState(0)
  React.useEffect(() => () => window.clearTimeout(colSettingsRetry), [])

  // 列宽拖拽（pointerdown 开拖 / dblclick 还原）委托在滚动容器上一次即可：
  // 表格被整表重建后监听仍然有效（委托到容器，不依赖具体节点）
  React.useEffect(() => {
    bindColumnGrips(listRef.current, requestRender)
  }, [])

  const visibleIds = shown.map(account => account.id)
  const allPicked = visibleIds.length > 0 && visibleIds.every(id => store.selected.has(id))
  const somePicked = visibleIds.some(id => store.selected.has(id))
  /** 被筛选隐藏但仍在勾选中的账号：操作会作用于它们，所以必须显式提示，不能静默生效 */
  const hiddenByFilter = [...store.selected].filter(id => !visibleIds.includes(id)).length
  const active = store.selected.size > 0
  // 「折叠全部 / 展开全部」看的是**全部**组（与按钮实际作用的对象同一份口径）：
  // 只看可见组的话，被筛掉的那几家会与标签对不上（见 setAllGroupsCollapsed 的说明）
  const allCollapsed = allGroups.length > 0 && allGroups.every(group => store.collapsed.has(group.provider))

  /**
   * 拖拽的落点处理：把「谁挪到了第几位」翻成一条**完整队列**再交给后端。
   *
   * 队列从分组的 `members`（全量，含折叠起来的组）拼，而不是从 DOM 现查：折叠的组
   * 在 DOM 里没有行，而后端要求「全部账号的一个排列」（少一个都会 400）。
   * 落点下标由拖拽模块给（按同级兄弟项的中点比出来），这里只做数组搬迁。
   */
  /** 拖拽要看两份分组：`all` 用来拼**完整**队列（被筛选挡住的组不渲染，但它家的账号
   *  仍在队列里，少一条就会被后端按「队列不完整」400 拒掉）；`displayed` 用来把落点
   *  下标（按渲染出来的组数出来的）换算成「插在哪个可见组之前」的锚点。 */
  const dragRef = React.useRef({ displayed: groups, all: allGroups })
  React.useEffect(() => { dragRef.current = { displayed: groups, all: allGroups } })
  /** 拖拽模块的处理器（绑定一次、读最新值，见下面的 effect） */
  const dragHandlers = React.useRef({ locked: false, lockReason: '', onDrop: (_drop: QueueDrop) => {} })
  const onDrop = React.useCallback((drop: QueueDrop) => {
    const { displayed, all: current } = dragRef.current
    if (drop.kind === 'group') {
      const from = current.findIndex(group => group.provider === drop.provider)
      if (from < 0) return
      const moved = current[from]
      const rest = current.filter((_, position) => position !== from)
      // 落点下标来自**渲染出来的**那些组（中点比出来的），而全量列表里还夹着
      // 不显示的组 —— 所以按「锚点」换算：插到第 N 个可见组之前 = 完整列表里
      // 那个组的下标之前；N 等于可见组数 = 插到队尾（不显示的组留在原处）
      const anchor = drop.index < displayed.length ? displayed[drop.index].provider : null
      const at = anchor === null ? -1 : rest.findIndex(group => group.provider === anchor)
      const to = anchor === null || at < 0 ? rest.length : at
      if (to === from) return
      const next = [...rest.slice(0, to), moved, ...rest.slice(to)]
      const queue = next.flatMap(group => group.members.map(account => account.id))
      void reorderQueue(queue, t('{name} 移到第 {rank} 顺位', { name: moved.label, rank: to + 1 }))
      return
    }
    const group = current.find(item => item.provider === drop.provider)
    if (!group) return
    const from = group.members.findIndex(account => account.id === drop.accountId)
    if (from < 0) return
    const moved = group.members[from]
    const rest = group.members.filter((_, position) => position !== from)
    const to = Math.max(0, Math.min(rest.length, drop.index))
    if (to === from) return
    const nextMembers = [...rest.slice(0, to), moved, ...rest.slice(to)]
    const next = current.map(item => item.provider === drop.provider ? { ...item, members: nextMembers } : item)
    const queue = next.flatMap(item => item.members.map(account => account.id))
    void reorderQueue(queue, t('{name} 移到 {group} 组内第 {rank} 位', {
      name: String(moved.name || moved.id), group: group.label, rank: to + 1,
    }))
  }, [])

  // 顺序拖拽（组顺位 / 组内顺位）：委托在滚动容器上，表格整表重画后监听仍然有效。
  // 绑定只做一次（与列宽拖拽同一手法）—— 处理器经 ref 读最新值：锁状态与分组
  // 每次渲染都会变，若把它们做进依赖数组，每变一次就往容器上再挂一份监听。
  React.useEffect(() => { dragHandlers.current = { locked, lockReason, onDrop } })
  React.useEffect(() => {
    bindQueueDrag(listRef.current, {
      isLocked: () => dragHandlers.current.locked,
      onLockedAttempt: () => toast(
        t('当前视图不能调整顺序：{reason} 之后再拖动', { reason: dragHandlers.current.lockReason }),
        'err',
      ),
      onDrop: drop => dragHandlers.current.onDrop(drop),
    })
  }, [])

  /** 单元格：按列 key 分发（勾选列的内容依赖行上下文，其余列是「账号 + 上下文」的纯函数） */
  function cell(key: string, account: AccountRecord, ctx: ReturnType<typeof rowContext>) {
    switch (key) {
      case 'pick':
        return (
          <Checkbox checked={ctx.picked} data-pick={account.id} title={t('勾选后可批量操作')}
            aria-label={t('勾选后可批量操作')}
            onCheckedChange={next => togglePick(account.id, next)} />
        )
      case 'order':
        return <OrderCell account={account} rank={ctx.rank} />
      case 'account':
        return <AccountCell account={account} namesHidden={store.namesHidden} />
      case 'proxy':
        return <ProxyCell account={account} />
      case 'connections':
        return <ConnectionsCell account={account} />
      case 'status':
        return <StatusCell account={account} />
      case 'limits':
        return <LimitsCell account={account} open={ctx.limitsOpen} />
      case 'expiry':
        return <ExpiryCell account={account} />
      case 'usage':
        return <UsageCell account={account} />
      case 'actions':
        return <ActionsCell account={account} atFront={ctx.seat.position <= 1} />
      default:
        return null
    }
  }

  return (
    <>
      <section className='panel account-table'>
        <div className='toolbar'>
          {/* 搜索：分组之后「哪一家的哪个账号」是明确的两级，搜索用来在整套账号里
              按名字 / 邮箱 / 标识定位。与两个筛选档位共用同一条可见性口径
              （accounts-domain 的 matchesFilter），命中的行留在它自己的组里 */}
          <div className='group'>
            <span className='label'>{t('搜索')}</span>
            <Input className='acct-search' type='search' value={store.filter.q}
              placeholder={t('账号名 / 邮箱 / 备注')} aria-label={t('搜索账号')}
              onChange={event => setSearchFilter(event.currentTarget.value)} />
          </div>
          <div className='divider' />

          <div className='group'>
            <span className='label'>{t('状态')}</span>
            {/* 两个筛选维度是**受控**的（取值与计数都以 store 为准），限流组在状态筛成
                「禁用」时置灰 —— 已禁用账号既不算正常也不算已限流 */}
            <SegmentedControl options={[
              { value: 'all', label: t('全部'), count: counts.enabledAll ?? 0 },
              { value: 'enabled', label: t('启用'), count: counts.enabled ?? 0 },
              { value: 'disabled', label: t('禁用'), count: counts.disabled ?? 0 },
            ]} value={store.filter.enabled} onValueChange={value => setSegmentFilter('enabled', value)}
              aria-label={t('启用状态')} />
          </div>

          <div className='group'>
            <span className='label'>{t('限额')}</span>
            <SegmentedControl options={[
              { value: 'all', label: t('全部'), count: counts.limitAll ?? 0 },
              { value: 'normal', label: t('正常'), count: counts.normal ?? 0, disabled: store.filter.enabled === 'disabled' },
              { value: 'limited', label: t('已限流'), count: counts.limited ?? 0, disabled: store.filter.enabled === 'disabled' },
            ]} value={store.filter.limit} onValueChange={value => setSegmentFilter('limit', value)}
              aria-label={t('限额状态')} />
          </div>

          <div className='actions'>
            {/* 折叠是「只看某几家」的入口：一家几十条时，收起不关心的那些比筛选更顺手。
                「全部」就是全部 —— 作用对象是全部组，不按当前筛选收窄（见 store 里
                setAllGroupsCollapsed 的说明） */}
            <Button id='btn-fold-groups' variant='outline'
              disabled={!allGroups.length}
              title={allCollapsed ? t('展开全部分组') : t('折叠全部分组（只留组头）')}
              onClick={() => setAllGroupsCollapsed(allGroups.map(group => group.provider), !allCollapsed)}>
              {allCollapsed ? t('展开全部') : t('折叠全部')}
            </Button>
            {/* 「添加账号」的打开逻辑归 add-account.js（它按同一个 id 绑了监听），这里再挂
                一次 onClick 是**冗余保险**：万一那个脚本的绑定因加载顺序没接上，按钮仍可用
                （openModal / resetAddStep 都是幂等的，重复调用无副作用） */}
            <Button id='btn-add-account-2' variant='default' title={t('登录 / 导入一个新账号')}
              onClick={() => {
                shared().wbAddAccountModal?.open?.()
                shared().wbAccountAddForms?.syncAddProvider?.()
              }}>{t('添加账号')}</Button>
            <Button id='btn-query-usage' variant='outline'
              disabled={store.usageBusy || !all.some(supportsUsage)}
              title={t('查询全部账号的余额（含已禁用账号 —— 禁用只表示不参与转发）')}
              onClick={() => void queryAllUsage()}>{store.usageBusy ? t('查询中…') : t('查询余额')}</Button>
          </div>
        </div>

        <div className={cn('batch-bar', active && 'active')} id='batch-bar'>
          <label className='batch-select-all'>
            <Checkbox id='batch-select-all' checked={allPicked} indeterminate={!allPicked && somePicked}
              disabled={!visibleIds.length} aria-label={t('全选当前筛选结果')}
              onCheckedChange={next => setAllPicked(visibleIds, next)} />
            <span id='batch-select-label'>
              {visibleIds.length ? t('全选当前筛选结果（{n} 个）', { n: visibleIds.length }) : t('没有可全选的账号')}
            </span>
          </label>
          <span className='batch-count'>
            {t('已选')} <b id='batch-count'>{store.selected.size}</b> {t('个 · 共')} <b id='accounts-count'>{all.length}</b> {t('个')}
          </span>
          {/* 分组已经逐段给出「哪家有几个」，这里只留一句「当前显示了几家的几行」 */}
          <span className='batch-summary' id='accounts-group-summary'>
            {groups.length
              ? t('{groups} 家 · 显示 {rows} 行', { groups: groups.length, rows: visibleIds.length })
              : ''}
          </span>
          {locked ? (
            <span className='batch-hidden' title={t('筛选 / 搜索 / 分页生效时顺序不可拖动（行被裁过，拖动会错位）')}>
              {t('顺序已锁定')}
            </span>
          ) : null}
          {filterActive() ? (
            <Button id='btn-clear-filter' variant='ghost' size='xs'
              title={t('清空搜索与两个筛选档位，回到全部账号')}
              onClick={() => clearFilter()}>{t('清除筛选')}</Button>
          ) : null}
          {hiddenByFilter ? (
            <span className='batch-hidden' id='batch-hidden-hint'>
              {t('另有 {n} 个已勾选账号被当前筛选隐藏，仍会参与操作', { n: hiddenByFilter })}
            </span>
          ) : null}
          <div className='batch-actions'>
            {/* 「列设置」按钮由 wbColSettings.register 插进这个容器的最前面（命令式，
                插入位置由那边决定，本岛只留容器） */}
            <Button id='btn-batch-open' variant='outline' disabled={!active}
              onClick={() => openBatchDialog([...store.selected], 'enable')}>{t('批量操作')}</Button>
            <Button id='btn-batch-clear' variant='outline' disabled={!active}
              onClick={() => clearSelection()}>{t('取消选择')}</Button>
          </div>
        </div>

        <div className='acct-scroll' id='account-list' ref={listRef}>
          {!loaded ? (
            <div className='empty'><span className='spinner' />{t('正在加载…')}</div>
          ) : !all.length ? (
            <div className='empty'>{t('暂无账号，请点击右上角「添加账号」')}</div>
          ) : !shown.length ? (
            <div className='empty'>
              {t('当前筛选条件下没有账号')}
              {filterActive() ? (
                <button type='button' className='link-btn' onClick={() => clearFilter()}>{t('清除筛选')}</button>
              ) : null}
            </div>
          ) : (
            <table className='acct-table' ref={setTableElement}
              style={{ minWidth: `${tableMinWidth(columns.map(column => column.key))}px` }}>
              {/* 上面那条 min-width 是「弹性列压到只剩 FLEX_MIN_WIDTH、再窄就横滚」的那条线，
                  由列宽算出（不再由 CSS 写死，见 accounts-columns 的 tableMinWidth） */}
              <ColGroup />
              <TableHead namesHidden={store.namesHidden} allPicked={allPicked} somePicked={somePicked}
                disabled={!visibleIds.length} onToggleAll={next => setAllPicked(visibleIds, next)} />
              {groups.map(group => {
                const collapsed = store.collapsed.has(group.provider)
                const rows = pageRows.filter(account => providerOf(account) === group.provider)
                return (
                  <tbody key={group.provider} className='acct-group' data-provider={group.provider}>
                    <GroupBandRow group={group} collapsed={collapsed} colSpan={columns.length} />
                    {collapsed ? null : rows.map(account => {
                      // 组内序号取**全量成员**里的位次（不是当前页 / 当前可见里的位次）：
                      // 筛选时号会跳，但号本身说的是「在这一家里的第几位」这个事实
                      const rank = group.members.findIndex(member => member.id === account.id) + 1
                      const ctx = rowContext(account, seatMap, rank > 0 ? rank : 1)
                      return (
                        <React.Fragment key={account.id}>
                          <tr className={cn('acct-row', !isEnabled(account) && 'disabled', ctx.picked && 'selected')}
                            data-id={account.id} data-provider={group.provider}
                            data-desktop={isDesktopAccount(account) ? '1' : undefined}>
                            {columns.map(column => (
                              <td key={column.key} className={`cell-${column.key} ta-${column.align}`}>
                                {cell(column.key, account, ctx)}
                              </td>
                            ))}
                          </tr>
                          <PanelsRow account={account} colSpan={columns.length}
                            limitsOpen={panelOpen(account.id, 'limits')}
                            onClear={(id, model) => void clearLimits(id, model)} />
                        </React.Fragment>
                      )
                    })}
                  </tbody>
                )
              })}
            </table>
          )}
        </div>

        {/* 页脚是纯控制条（读数 / 每页条数 / 跳页 / 翻页器，通用件 table-shell.tsx）。
            左侧原先那两句说明（优先级是全局队列 / 拖表头调列宽 / 设为首选的含义）
            按用户要求移除 —— 五张表的页脚现在都不带说明文字了。 */}
        <TableFooter
          total={shown.length}
          range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
          page={paging.page}
          pageCount={paging.pageCount}
          size={paging.size}
          onSizeChange={paging.setSize}
          onPageChange={paging.goto}
        />
      </section>
      <AccountsDialogs />
    </>
  )
}

/** 表格元素写进模块变量（供 syncStaticHead 用）。用模块级函数做 ref 回调，
    身份稳定 —— 内联箭头函数会在每次重渲染时先卸下再装上，白白多一轮空窗 */
function setTableElement(el: HTMLTableElement | null): void {
  tableEl = el
}

/** 列宽骨架（colgroup）：列的字面量顺序，`data-col` 与表头、列设置、列宽层三处同名 */
function ColGroup() {
  const widths = columnWidths()
  const col = (key: string) => (
    <col key={key} data-col={key} style={widths[key] ? { width: `${widths[key]}px` } : undefined} />
  )
  return (
    <colgroup>
      {col('pick')}
      {col('order')}
      {col('account')}
      {col('proxy')}
      {col('connections')}
      {col('status')}
      {col('limits')}
      {col('expiry')}
      {col('usage')}
      {col('actions')}
    </colgroup>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="accounts"]'

let root: ReturnType<typeof createRoot> | null = null

/**
 * 把 React root 直接建在 `.page[data-page="accounts"]` 上（不套宿主 div：页面 CSS 用
 * `.page[data-page="accounts"] .panel` 这组直接子选择器分配高度，中间插一层会打断它）。
 *
 * **首次提交必须是同步的**（flushSync，理由见文件头）：后续更新照常异步。
 */
function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  flushSync(() => { root!.render(<AccountsPage />) })
}

// 对外契约与连接数轮询都在模块求值时完成（app.js 的 refresh() 是异步的，但
// report.js / add-account.js 这些同步脚本紧随其后执行 —— 拿不到就会退化成静默不生效）
installAccountsApi()
startConnectionsPolling()

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
