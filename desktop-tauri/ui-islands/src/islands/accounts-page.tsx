

import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { t } from '../i18n'
import { Button, Checkbox, Input, ResponsiveSheet, SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn, Card, Label, Empty, Table, TableBody, TableRow, TableCell } from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { shared, toast, type AccountRecord, type ColSettingsHandle } from './accounts-shared'
import { isDesktopAccount, isEnabled, providerOf, supportsUsage } from './accounts-domain'
import { ACCOUNT_COLUMNS, bindColumnGrips, columnWidths, tableMinWidth } from './accounts-columns'
import {
  allAccounts, bump, clearFilter, clearLimits, clearSelection, ensureProxyPoolOptions,
  filterActive, getStore, groupSortRules, groupView, hiddenAccounts, installAccountsApi, normalizeProviderFilter, openBatchDialog,
  providerSummaryList, queryAllUsage, refreshList, reorderQueue, rowContext, seats, segmentCounts,
  setAllGroupsCollapsed, setAllPicked, setProviderFilter, setSearchFilter, setSegmentFilter,
  snapshot, startConnectionsPolling, subscribe, subscribeProxyPool, syncGroupTotalPreference,
  tokenUsageOf, togglePick, usageEntryOf, GROUP_TOTAL_FILTERED_KEY,
} from './accounts-data'
import {
  AccountCell, ActionsCell, ConnectionsCell, ExpiryCell, GroupBandRow, LimitsCell, OrderCell,
  ProxyCell, StatusCell, TableHead, UsageCell,
} from './accounts-panels'
import { bindQueueDrag, type QueueDrop } from './accounts-queue-drag'
import { AccountsDialogs } from './accounts-dialogs'
import { HiddenAccountsNotice } from './accounts-hidden-notice'
import { activeGroupSortRules, manualOrderBlockedMessage } from './accounts-group-settings'
import { Badge, Spinner } from '@ui'
import { Alert } from '@ui'

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

/** 该账号所属组的排序规则是否生效（生效时组内把手锁定：拖了也会被规则排回去） */
function sortActiveIn(account: AccountRecord): boolean {
  const provider = providerOf(account)
  return activeGroupSortRules(provider, groupSortRules(provider)).length > 0
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
  const hidden = hiddenAccounts()
  // groups = 要渲染的（当前筛选下一条都看不见的组不留空壳）；allGroups = 全部组，
  // 拼队列与「折叠 / 展开全部」都用它
  const { groups, allGroups } = groupView(usageEntryOf, tokenUsageOf)
  // 限流维度的分段把「余额不足已跳过 / Token 限额已跳过」都算进已限流
  // （isLimitedNow 的口径），所以筛选与计数要带上余额与 Token 两份读数的查找口
  // —— 缓存每次 bump 后这里重算
  const counts = segmentCounts(usageEntryOf, tokenUsageOf)
  /**
   * 提供商下拉的选项：`全部（N）` + 每一家（含 0 条的家 —— 清单要完整）。
   * 数字是**家底的读数**（这家名下有几个账号），不随另外两档分段收窄：
   * 它与分段徽标回答的不是同一个问题 ——「这一家里有多少个」而不是「点进去能看到几条」，
   * 于是「哪家有账号、各有多少」不切页就能看到（家数动态，这也是它用下拉不用分段的原因）。
   */
  const providerOptions = [{ value: 'all', label: t('全部（{n}）', { n: all.length }) }]
    .concat(providerSummaryList().map(item => ({ value: item.id, label: t('{label}（{count}）', { label: t(item.label), count: item.count }) })))
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
   * 分页生效时**组顺位不可拖**（见 groupLocked）：整组落点在分页下难以定位到
   * 「哪个可见组」锚点（被分掉的组看不见），所以它进了锁的判据；组内行不受它
   * 限制 —— 落点按可见行换算（见 onDrop），当前页里照常拖得动。
   */
  const paging = useClientPaging(shown.length, 'accounts', { defaultSize: 'all' })
  const pageRows = paging.paged ? paging.slice(shown) : shown
  /** 整组允许仅按启用状态筛选；其他筛选或分页仍锁定，组内行不受筛选限制。 */
  const groupFilterLocked = store.filter.provider !== 'all'
    || store.filter.q.trim() !== ''
    || store.filter.limit !== 'all'
  const groupLocked = groupFilterLocked || paging.paged
  const lockReason = groupFilterLocked ? t('清除筛选') : t('把每页调回「全部」')

  // 代理列的选项（「网络代理」页的池条目）没就绪就补拉（节流 30 秒，失败不缓存）。
  // Clash 出口那一组已经不在这一页了 —— 出口统一由代理页的「同步 Clash Verge」
  // 导入池，这里只读池（见 accounts-panels 的 ProxyCell）
  React.useEffect(() => { ensureProxyPoolOptions() })

  // 池列表是账号页与「网络代理」页共用的一份缓存（见 proxy-pool-cache.ts）：那一页
  // 禁用节点 / 删除订阅 / 同步 Clash 之后会把新列表写进来，这里订阅到就重画 ——
  // 代理下拉的选项因此不会停在旧数据上（曾经它是一次性缓存，删掉的节点还留在
  // 下拉里，选中才被后端拒绝）
  React.useEffect(() => subscribeProxyPool(() => { bump() }), [])

  // 存盘里筛的那一家已经不在摘要里（账号删光、家也删了）就复位成「全部」—— 否则下拉
  // 找不到那一项、回落显示「全部」，列表却仍按它过滤（见 store 的 normalizeProviderFilter）
  React.useEffect(() => { normalizeProviderFilter() }, [store.version])

  // 设置页拨了「积分合计只统计当前筛选出来的账号」之后，本页要立刻按新口径重画。
  // 本页自己的写入走 store（同文档内 localStorage 不触发自己的 storage 事件），
  // 这条监听只接**别处**（设置页 / 另一个窗口）的写入。
  React.useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === null || event.key === GROUP_TOTAL_FILTERED_KEY) syncGroupTotalPreference()
    }
    window.addEventListener('storage', onStorage)
    return () => window.removeEventListener('storage', onStorage)
  }, [])

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
  /** 宽屏平铺、窄屏抽屉，筛选值始终由同一份 store 维护。 */
  const [filterOpen, setFilterOpen] = React.useState(false)
  /** 当前生效的筛选条数：给「筛选」按钮上的角标用，让用户知道列表被收窄了 */
  const activeFilterCount =
    (store.filter.provider !== 'all' ? 1 : 0)
    + (String(store.filter.q || '').trim() ? 1 : 0)
    + (store.filter.enabled !== 'all' ? 1 : 0)
    + (store.filter.limit !== 'all' ? 1 : 0)
  /**
   * 拖拽的落点处理：把「谁挪到了第几位」翻成一条**完整队列**再交给后端。
   *
   * 队列从分组的 `members`（全量，含折叠起来的组）拼，而不是从 DOM 现查：折叠的组
   * 在 DOM 里没有行，而后端要求「全部账号的一个排列」（少一个都会 400）。
   * 落点下标由拖拽模块给（按同级兄弟项的中点比出来），这里只做数组搬迁；
   * 筛选 / 搜索 / 分页裁行裁组时按锚点换算回全量（见两处分支的说明）。
   */
  /** 拖拽要看两份分组：`all` 用来拼**完整**队列（被筛选挡住的组不渲染，但它家的账号
   *  仍在队列里，少一条就会被后端按「队列不完整」400 拒掉）；`displayed` 用来把落点
   *  下标（按渲染出来的组数出来的）换算成「插在哪个可见组之前」的锚点；`renderedRows`
   *  给组内行做同一件事 —— 组内落点按**真画出来的行**（本页那一段）换算，见 onDrop。 */
  const renderedRows = new Map<string, string[]>()
  for (const group of groups) {
    renderedRows.set(group.provider,
      pageRows.filter(account => providerOf(account) === group.provider).map(account => account.id))
  }
  const dragRef = React.useRef({ displayed: groups, all: allGroups, renderedRows })
  React.useEffect(() => { dragRef.current = { displayed: groups, all: allGroups, renderedRows } })
  /** 拖拽模块的处理器（绑定一次、读最新值，见下面的 effect） */
  const dragHandlers = React.useRef({
    locked: false,
    lockReason: '',
    onDrop: (_drop: QueueDrop) => {},
  })
  const onDrop = React.useCallback((drop: QueueDrop) => {
    const { displayed, all: current } = dragRef.current
    if (drop.kind === 'group') {
      if (dragHandlers.current.locked) return
      const from = current.findIndex(group => group.provider === drop.provider)
      const visibleFrom = displayed.findIndex(group => group.provider === drop.provider)
      if (from < 0 || visibleFrom < 0) return
      const visible = displayed.filter(group => group.provider !== drop.provider)
      if (!visible.length || drop.index < 0 || drop.index > visible.length || drop.index === visibleFrom) return
      const moved = current[from]
      const rest = current.filter((_, position) => position !== from)
      // index 按移除被拖组后的可见列表计数；末尾落点只放在最后一个可见组之后。
      const anchor = visible[drop.index] || visible[visible.length - 1]
      const at = rest.findIndex(group => group.provider === anchor.provider)
      if (at < 0) return
      const to = at + (drop.index === visible.length ? 1 : 0)
      if (to === from) return
      const next = [...rest.slice(0, to), moved, ...rest.slice(to)]
      // 整组移动不覆盖组内手动兜底；自动消耗规则仍由网关动态执行。
      const queue = next.flatMap(group => group.queueMembers.map(account => account.id))
      void reorderQueue(queue, t('{name} 移到第 {rank} 顺位', { name: moved.label, rank: to + 1 }))
      return
    }
    const orderRules = activeGroupSortRules(drop.provider, groupSortRules(drop.provider))
    if (orderRules.length > 0) {
      toast(manualOrderBlockedMessage(orderRules), 'err')
      return
    }
    const group = current.find(item => item.provider === drop.provider)
    if (!group) return
    // 自动规则开启时不接受组内拖动；关闭后调整的是手动消耗顺序。
    const from = group.queueMembers.findIndex(account => account.id === drop.accountId)
    if (from < 0) return
    const moved = group.queueMembers[from]
    const rest = group.queueMembers.filter((_, position) => position !== from)
    // 落点下标来自**渲染出来的**那些行（筛选 / 搜索 / 分页会把组内的行裁掉一部分），
    // 而重排要的是完整 members —— 按锚点换算：插到第 N 个渲染行之前 = 完整 members
    // 里那行所在的下标之前；N 等于渲染行数 = 插到组尾（被裁掉的行留在原处）。
    // 渲染行从 dragRef 取（而不是 group.accounts 的全量可见行）：分页把一组的行切成
    // 两页时，DOM 里只有本页那一段，锚点必须按「真画出来的行」数，否则错一位。
    // 与组顺位分支（displayed → all）同一个手法：可见与全量的映射是确定的，错不了位。
    const visible = dragRef.current.renderedRows.get(drop.provider) || []
    const anchor = drop.index < visible.length ? visible[drop.index] : null
    const at = anchor === null ? -1 : rest.findIndex(account => account.id === anchor)
    const to = anchor === null || at < 0 ? rest.length : at
    if (to === from) return
    const nextMembers = [...rest.slice(0, to), moved, ...rest.slice(to)]
    const next = current.map(item => item.provider === drop.provider
      ? { ...item, members: nextMembers, queueMembers: nextMembers }
      : item)
    const queue = next.flatMap(item => item.queueMembers.map(account => account.id))
    void reorderQueue(queue, t('{name} 移到 {group} 组内第 {rank} 位', {
      name: String(moved.name || moved.id), group: t(group.label), rank: to + 1,
    }))
  }, [])

  // 顺序拖拽（组顺位 / 组内顺位）：委托在滚动容器上，表格整表重画后监听仍然有效。
  // 绑定只做一次（与列宽拖拽同一手法）—— 处理器经 ref 读最新值：锁状态与分组
  // 每次渲染都会变，若把它们做进依赖数组，每变一次就往容器上再挂一份监听。
  React.useEffect(() => { dragHandlers.current = { locked: groupLocked, lockReason, onDrop } })
  React.useEffect(() => {
    bindQueueDrag(listRef.current, {
      isLocked: () => dragHandlers.current.locked,
      onLockedAttempt: () => toast(
        t('当前视图不能调整组顺位：{reason} 之后再拖动', { reason: dragHandlers.current.lockReason }),
        'err',
      ),
      isRowLocked: provider => activeGroupSortRules(provider, groupSortRules(provider)).length > 0,
      onRowLockedAttempt: provider => toast(
        manualOrderBlockedMessage(activeGroupSortRules(provider, groupSortRules(provider))),
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
        return <OrderCell account={account} rank={ctx.rank} sortActive={sortActiveIn(account)} />
      case 'account':
        return <AccountCell account={account} namesHidden={store.namesHidden} />
      case 'proxy':
        return <ProxyCell account={account} />
      case 'connections':
        return <ConnectionsCell account={account} />
      case 'status':
        return <StatusCell account={account} />
      case 'limits':
        return <LimitsCell account={account} onClear={model => void clearLimits(account.id, model)} />
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
      <Card render={<section />} className='panel account-table'>
        <div className='toolbar'>
          {/* 窄屏「筛选」入口：只在 ≤767px 显示（桌面由 CSS 隐藏）。
              按钮上的角标是当前生效的筛选条数 —— 筛选条件收进抽屉后，
              列表被收窄这件事必须仍然可见，否则用户会以为账号丢了。 */}
          <Button size='default' variant='ghost' type='button' className='acct-filter-open hidden max-[767px]:inline-flex'
            onClick={() => setFilterOpen(true)}
            aria-label={t('打开筛选')}>
            {t('筛选')}
            {activeFilterCount > 0 ? <Badge variant='brand' shape='tag' className='acct-filter-badge'>{activeFilterCount}</Badge> : null}
          </Button>

          {/* 筛选值受控，切换布局不会改变条件。 */}
          <ResponsiveSheet open={filterOpen} onOpenChange={setFilterOpen} title={t('筛选')}
            inlineClassName='contents' bodyClassName='acct-filter-body'
            actions={<Button variant='ghost' size='sm' disabled={activeFilterCount === 0} onClick={() => clearFilter()}>{t('清空')}</Button>}>
          {/* 提供商：分组模式把「哪一家」画在了页面上（一家一段），但「只看某一家」仍是
              常用动作 —— 分组带收得掉一家、收不掉其余那几家。用下拉而不是分段按钮：家数
              是**动态**的（后端注册表加一家就多一项），分段按钮会随家数增长把工具条挤成
              一团；选项里带账号数（`WorkBuddy 国内版（14）`），于是「哪家有账号、各有多少」
              不用切页就能看到。取值与计数都以 store 为准（受控） */}
          <div className='group' data-provider-group='1'>
            <span className='label'>{t('提供商')}</span>
            <Select value={store.filter.provider} onValueChange={value => setProviderFilter(String(value))}>

              <SelectTrigger className='max-w-none min-w-0 w-full min-[768px]:max-w-[200px] min-[768px]:min-w-[130px] min-[768px]:w-auto' aria-label={t('按提供商筛选账号')}>
                <SelectValue>
                  {providerOptions.find(item => item.value === store.filter.provider)?.label || t('全部')}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {providerOptions.map(item => (
                  <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='divider' data-provider-divider='1' />

          {/* 搜索：分组之后「哪一家的哪个账号」是明确的两级，搜索用来在整套账号里
              按名字 / 邮箱 / 标识定位。与三个筛选档位共用同一条可见性口径
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
            {/* 两个分段维度（状态 / 限额）是**受控**的（取值与计数都以 store 为准）。
                计数按两级筛选分工（见 filterCounts 的说明）：状态是一级，徽标数全量
                （只随提供商与搜索收窄，切限额不动它）；限额是二级，徽标数「状态收窄
                后」的范围。限流组在状态筛成「禁用」时置灰 —— 已禁用账号既不算正常
                也不算已限流 */}
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
          </ResponsiveSheet>

          <div className='actions'>
            <Button id='btn-refresh-accounts' variant='outline' disabled={store.listBusy}
              title={t('重新读取账号列表和插件状态，不刷新 Token 或查询余额')}
              onClick={() => void refreshList({ feedback: true })}>
              {store.listBusy ? t('刷新中…') : t('刷新')}
            </Button>
            {/* 折叠是「只看某几家」的入口：一家几十条时，收起不关心的那些比筛选更顺手。
                「全部」就是全部 —— 作用对象是全部组，不按当前筛选收窄（见 store 里
                setAllGroupsCollapsed 的说明） */}
            <Button id='btn-fold-groups' variant='outline'
              disabled={!allGroups.length}
              title={allCollapsed ? t('展开全部分组') : t('折叠全部分组（只留组头）')}
              onClick={() => setAllGroupsCollapsed(allGroups.map(group => group.provider), !allCollapsed)}>
              {allCollapsed ? t('展开') : t('折叠')}
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

        {store.listError ? (
          <Alert variant='destructive' className='acct-load-error' role='alert'>
            {loaded
              ? t('刷新账号列表失败，当前仍显示上一次成功读取的数据：{message}', { message: store.listError })
              : t('账号列表加载失败，请点击「刷新」重试：{message}', { message: store.listError })}
          </Alert>
        ) : null}

        {loaded && all.length > 0 && hidden.total > 0 ? (
          <HiddenAccountsNotice hidden={hidden} compact />
        ) : null}


        <div className={cn('batch-bar', active && 'active')} id='batch-bar'>
          <Label className='batch-select-all'>
            <Checkbox id='batch-select-all' checked={allPicked} indeterminate={!allPicked && somePicked}
              disabled={!visibleIds.length} aria-label={t('全选当前筛选结果')}
              onCheckedChange={next => setAllPicked(visibleIds, next)} />
            <span id='batch-select-label'>
              {visibleIds.length ? t('全选当前筛选结果（{n} 个）', { n: visibleIds.length }) : t('没有可全选的账号')}
            </span>
          </Label>
          <span className='batch-count'>
            {t('已选')} <b id='batch-count'>{store.selected.size}</b> {t('个 · 共')} <b id='accounts-count'>{all.length}</b> {t('个')}
          </span>
          {/* 分组已经逐段给出「哪家有几个」，这里只留一句「当前显示了几家的几行」 */}
          <span className='batch-summary' id='accounts-group-summary'>
            {groups.length
              ? t('{groups} 家 · 显示 {rows} 行', { groups: groups.length, rows: visibleIds.length })
              : ''}
          </span>
          {/* 锁定条只管**组顺位**（组内行按可见行换算落点，筛选下照常拖得动） */}
          {groupLocked ? (
            <span className='batch-hidden' title={lockReason}>
              {t('组顺位已锁定')}
            </span>
          ) : null}
          {filterActive() ? (
            <Button id='btn-clear-filter' variant='ghost' size='xs'
              title={t('清空搜索与三个筛选档位，回到全部账号')}
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
            <Empty className='empty'>
              {store.listError
                ? t('账号列表加载失败，请点击「刷新」重试')
                : <><Spinner />{t('正在加载…')}</>}
            </Empty>
          ) : !all.length ? (
            hidden.total > 0
              ? <HiddenAccountsNotice hidden={hidden} />
              : <Empty className='empty'>{t('暂无账号，请点击右上角「添加账号」')}</Empty>
          ) : !shown.length ? (
            <Empty className='empty'>
              {t('当前筛选条件下没有账号')}
              {filterActive() ? (
                <Button size='default' variant='ghost' type='button' className='link-btn' onClick={() => clearFilter()}>{t('清除筛选')}</Button>
              ) : null}
            </Empty>
          ) : (
            <Table container={false} className='acct-table' ref={setTableElement}
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
                  <TableBody key={group.provider} className='acct-group' data-provider={group.provider}>
                    <GroupBandRow group={group} collapsed={collapsed} colSpan={columns.length} />
                    {collapsed ? null : rows.map(account => {
                      // 组内序号取**全量成员**里的位次（不是当前页 / 当前可见里的位次）：
                      // 筛选时号会跳，但号本身说的是「在这一家里的第几位」这个事实
                      const rank = group.members.findIndex(member => member.id === account.id) + 1
                      const ctx = rowContext(account, seatMap, rank > 0 ? rank : 1)
                      return (
                        <React.Fragment key={account.id}>
                          <TableRow className={cn('acct-row', !isEnabled(account) && 'disabled', ctx.picked && 'selected')}
                            data-id={account.id} data-provider={group.provider}
                            data-desktop={isDesktopAccount(account) ? '1' : undefined}>
                            {columns.map(column => (
                              <TableCell key={column.key} className={`cell-${column.key} ta-${column.align}`}>
                                {cell(column.key, account, ctx)}
                              </TableCell>
                            ))}
                          </TableRow>
                        </React.Fragment>
                      )
                    })}
                  </TableBody>
                )
              })}
            </Table>
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
      </Card>
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

// 首次切到本页时才挂载（见 island-lazy）：隐藏页不在首屏渲染，也不拉数据。
mountOnPage('accounts', PAGE_SELECTOR, mount)
