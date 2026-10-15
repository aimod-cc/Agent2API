/**
 * Agent2API · 网络代理页（代理池：列表 / 新增 / 编辑 / 删除 / 启停 / 连通性测试
 * / 同步 Clash Verge / 勾选批量测试与批量删除）。
 *
 * 对照 OmniProxy 的 ProxiesPage（那张带 CRUD + 测试 + Clash 同步 + 批量的代理表），
 * 存储与解析在 Rust 侧：`core::proxy_pool` 是唯一事实来源（`kv` 的 `proxyPool`
 * 键），HTTP 契约见 `api::proxies` 的模块头，桥方法见 `src/bridge.rs` 的
 * 「代理池」一段。
 *
 * ── 数据层在隔壁 ─────────────────────────────────────────────
 * 本文件只放**视图层**（页面骨架 + 工具条 / 批量条 + 分组表格 + 弹窗挂载）；
 * 状态、取数、单条与批量动作都在 `proxies-state.ts`，两个配置弹窗在
 * `proxies-dialogs.tsx`，订阅相关在 `proxies-sub-view.tsx`（都是普通模块 /
 * 同目录组件，不是岛 —— 文件名是 .ts / 不是页面入口）。
 *
 * ── 这一页解决什么（为什么不能只在账号弹窗里配代理）────────────
 * 改造前代理只能逐个账号配：同一出口用在三个账号上要填三遍、改端口要改三处，
 * 也看不到「这出口通不通、出口 IP 是什么」。现在出口在这里配一次：
 * 账号弹窗的「出网代理」多一档「已保存的代理」，选这里的一条即可。
 *
 * ── 三种条目（只读语义照 OmniProxy）──────────────────────────
 *   · 手动：这里新建 / 编辑 / 删除，字段全在本地；
 *   · Clash Verge 同步来的：**只读镜像** —— 名称 / 端口 / 启用状态全部跟随
 *     Clash，本页不给编辑与删除入口，要改去 Clash Verge 改、要删去 Clash Verge
 *     删（下次同步自然摘掉）；
 *   · 订阅节点：**只读镜像** —— 由内核起成本地监听器后镜像进来，入口在
 *     「订阅管理」弹窗里（见下）。
 *   两种只读来源在「操作」列**只有测试**（外加订阅孤儿的删除）：去哪儿改由只读
 *   开关的悬浮提示说明，不再摆两颗永远点不动的「去 X 改」按钮占地方。
 *   同步有两处触发：进页面时的列表接口（后端自动同步）与面板头的
 *   「同步 Clash Verge」按钮（手动，会报本次变更数）。
 *
 * ── 一页只有一张表：订阅与内核去哪了 ──────────────────────────
 * 改造前订阅区块是代理池**上方**的第二个面板：两张表叠在一页，页面被拉得很长，
 * 而订阅表多半是「配完就不常动」的低频内容。现在订阅表整体收进「订阅管理」
 * 弹窗（面板头的按钮带订阅条数），页面只剩这一张池表；**内核状态条**提到页面
 * 级（`KernelBar`）—— 它讲的是「本机内核 / 节点端口」这件全局的事，端口用尽的
 * 报错也属于它，跟着订阅表一起被收起来就看不见了。内核的完整生命周期都在那条
 * 状态条上：未装是「下载内核」，装好后是「启动 / 重启内核」+「删除内核」。
 *
 * ── 分组带 + 分页 ─────────────────────────────────────────────
 * 表体按来源分三组（手动 / Clash Verge / 订阅），每组一条 `.tr-group` 带
 * （类名与模型页共用，见 page-gateway.css）—— 分组带是「这一组的只读口径」唯一
 * 的落点，几十条订阅节点混在一起也看得出哪些能改、要改去哪儿改。
 * 但几百条平铺会把页面撑到几千像素，所以行数交给页脚的**分页栏**（通用表格外壳
 * useClientPaging + TableFooter，与账号 / 网关 Key / 事件日志同一套控件）：
 * 原先那套「组内只铺 N 条 + 展开其余」的折叠取消了 —— 同一件事（几千像素的列表
 * 怎么看）在两处有两种解法时，用户得先判断「这回该点展开还是翻页」。
 * 分页按**整份命中列表**切片（组序 → 组内序），页内再铺各自的分组带：订阅那组
 * 跨页时，每一页都能看到它是哪来的、能不能改。
 *
 * ── 工具条与批量条共用一个槽位 ────────────────────────────────
 * 勾选后原地换成批量条（`.batch-bar`，账号页那套类名），取消选择换回工具条 ——
 * 两条不是上下叠着，表格不会因为多出一条状态带而整体下移一行。全选框在表头，
 * 作用域是**当前筛出来的行**（搜索后全选就只选看得见的那些）。
 *
 * ── 按「提供方」筛选（下拉）────────────────────────────────────
 * 来源分段控件只到「手动 / Clash Verge / 订阅」这一层，而订阅那档底下常有好几家
 * 机场（每家几十上百条）——「只想看魔戒的节点」在那层筛不出来。工具条里因此多一个
 * **提供方下拉**（排在来源分段控件之前）：手动 / Clash Verge / 全部订阅 / 各家订阅
 * （名字与条数都来自订阅清单本身，不靠节点名猜）/ 已移除的订阅（孤儿节点）。
 * 两个控件**联动而不是各筛各的**：选中某一家时来源自动落到「订阅」，点来源分段时
 * 提供方回到「全部提供方」—— 否则会出现「手动 + 魔戒」这种永远筛不出东西、
 * 也看不出是谁在挡着的组合。
 *
 * ── 测试的判据与账号弹窗同一套 ────────────────────────────────
 * 判据是「能否连上**上游**」（不是能否访问第三方站点）、出口 IP 是附加信息、
 * 失败也返回 200 + `success:false`，见 `core::egress::test_connectivity`。
 * 结果会落进条目（`lastTest`）：重启 / 换设备打开都还能看到上次测出来是什么。
 * 失败原因（上游错误原文）直接铺在第二行并截断，完整文本在 title 里 ——
 * 只放在 title 里等于要用户逐行悬停去猜「为什么不通」。
 *
 * ── 地址单独一列（带复制）────────────────────────────────────
 * 地址不跟名字挤同一格：它是用户要拿去**核对**（这条连的到底是哪儿）和**贴到别处**
 * 的值（客户端配置、curl 命令），跟名字放一起既读不清也没地方安复制按钮。
 * 名称列因此只剩「名字 + 协议 + 被几个账号引用」。复制走 clipboard.js 的全局
 * 委托（`data-copy` 给文本、`.copy-btn` 负责「⧉ → ✓」反馈，见 ui/clipboard.js），
 * 与订阅表的链接列、Keys 页同一套 —— 岛内不自己写剪贴板逻辑。
 * 只读形态（「解析失败」/「未分配端口」）没有可复制的地址，就不摆那颗按钮。
 *
 * ── 列设置与列宽拖动（与另外五张表同一套）─────────────────────
 * 面板头那颗齿轮 + 列设置浮层（显隐 / 拖动排序 / 对齐）来自 table-col-settings，
 * 表头右缘的列宽把手来自 ui/table-columns.js —— 六张表共用的两个能力层，本页的
 * 登记与列集合都在 `proxies-columns.ts`。六列虽然都是核心信息，但「先看哪几列」
 * 随用法变（排查连通性时只留地址 + 连通性，批量清理时只看勾选 + 名字），
 * 所以不再替用户把列钉死。
 *
 * ── 页面骨架的类名照旧 ────────────────────────────────────────
 * `.panel` / `.panel-head` / `.models-table-wrap` / `.models-table` 都是既有全局
 * 样式（layout.css / page-gateway.css），批量条用账号页那组 `.batch-bar` 类，
 * 分组带用模型页那组 `.tr-group` / `.prov-tag`，页脚分页栏是通用表格外壳
 * （table-shell）—— 这一页没有新增任何 CSS 文件；只有默认列宽那一组
 * `.proxies-table .p-*` 加在 page-gateway.css 里（与网关 Key 的 `.k-*` 并排）。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button,
  Checkbox,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  cn,
} from '@ui'
import { poolItemAddress } from './accounts-shared'
import {
  batchRemove, batchTest, clearSelection, durationText, formatTime, getSnapshot,
  isOrphanSubscriptionItem, isPicked, isReadonlyPoolItem, isUnprovisionedItem, items,
  loadPanel, readonlyHint, removeItem, setAllPicked, subscribe, syncClash, testItem,
  toggleItem, togglePick, wb, type ProxyPoolItem,
} from './proxies-state'
import {
  getSnapshot as getSubSnapshot, kernel as subKernel, loadSection, subscribe as subscribeSubs,
  type SubscriptionItem,
} from './proxies-sub-state'
import { KernelBar, SubscriptionManagerDialog } from './proxies-sub-view'
import { ProxyModal, ProxySettingsModal } from './proxies-dialogs'
import {
  COL_CLASS, closeColumnPanel, repaintColumnWidths, setupColumns, visibleColumns,
} from './proxies-columns'
import { TableFooter, useClientPaging } from './table-shell'
import { t } from '../i18n'

/* ─── 页面 ───────────────────────────────────── */

function ProxiesPage() {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  // 订阅清单：条数给「订阅管理」按钮带读数，id / 名字给「提供方」下拉认家
  //（下拉的档位不靠节点名猜，就是这份清单 + 池里的 providerId）
  const subs = React.useSyncExternalStore(subscribeSubs, getSubSnapshot).data?.items ?? []
  const [modal, setModal] = React.useState<{ item: ProxyPoolItem | null } | null>(null)
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [subsOpen, setSubsOpen] = React.useState(false)
  const [kw, setKw] = React.useState('')
  const [srcFilter, setSrcFilter] = React.useState('all')
  const [vendorFilter, setVendorFilter] = React.useState('all')
  const [stFilter, setStFilter] = React.useState('all')
  /** 列设置登记的「拿不到就重试」驱动（本岛排在 table-col-settings 之前，见 proxies-columns） */
  const [colRetry, setColRetry] = React.useState(0)
  /** 列设置改动后强制重画（登记前那一拍读的是「全列」兜底值） */
  const [, forceTick] = React.useReducer((tick: number) => tick + 1, 0)

  const rows = items()
  const ready = snapshot.data !== null
  const selectedCount = snapshot.selected.size
  const clashPicked = rows.filter(item => snapshot.selected.has(item.id) && isReadonlyPoolItem(item)).length
  const progress = snapshot.batchProgress

  // 内核状态与节点端口范围在页面顶部的那条状态条上（它自己也订阅 sub-state）
  React.useEffect(() => { void loadSection() }, [])

  /**
   * 列设置的登记（拿不到能力层就隔一拍重试）。挂载后登记而不是模块顶层：齿轮按钮
   * 要插进本岛渲染出来的操作组，那个容器只有 React 提交之后才存在。
   */
  React.useLayoutEffect(() => {
    if (!setupColumns(() => forceTick())) {
      const timer = window.setTimeout(() => setColRetry(tick => tick + 1), 50)
      return () => window.clearTimeout(timer)
    }
    forceTick()
    return undefined
  }, [colRetry])

  // 表头 / 表体每次重画后让列宽层重新对一遍：把手挂在表头格上、覆盖宽度写在 <col> 上，
  // 两者都可能被 React 的重建带走（列集合变化、强制重画），repaint 是幂等的兜底
  React.useLayoutEffect(() => { repaintColumnWidths() })

  // 本页卸载（切换页面不会卸载，留着兜底）：列设置浮层与它的锚点按钮一起走
  React.useEffect(() => () => { closeColumnPanel() }, [])

  /** 状态档：未就绪（还没分配端口）单独一档 —— 它既不是「可用」也不是「失败」 */
  function stateKey(item: ProxyPoolItem): 'ok' | 'fail' | 'wait' | 'none' {
    if (item.port === null || item.port === undefined) return 'wait'
    if (!item.lastTest?.at) return 'none'
    return item.lastTest.success ? 'ok' : 'fail'
  }

  const keyword = kw.trim().toLowerCase()
  /** 提供方下拉的档位（顺序固定：粗档在前，各家订阅按池里条数多的排前面） */
  const vendors = vendorOptions(rows, subs)

  /**
   * 提供方是否命中：'all' 不筛；manual / clash / subscription 三个粗档按来源组比；
   * 孤儿档看后端标的 orphan；其余按条目上的 providerId 比（就是那个订阅的 id）。
   */
  function vendorHit(item: ProxyPoolItem): boolean {
    if (vendorFilter === 'all') return true
    if (vendorFilter === 'manual' || vendorFilter === 'clash' || vendorFilter === 'subscription') {
      return groupOf(item) === vendorFilter
    }
    if (vendorFilter === ORPHAN_VENDOR) return Boolean(item.orphan)
    return item.source === 'subscription' && !item.orphan && item.providerId === vendorFilter
  }

  function matches(item: ProxyPoolItem): boolean {
    if (srcFilter !== 'all' && groupOf(item) !== srcFilter) return false
    if (!vendorHit(item)) return false
    if (stFilter !== 'all' && stateKey(item) !== stFilter) return false
    if (keyword) {
      const hay = [item.name, poolItemAddress(item), item.host, item.protocol, String(item.port ?? '')]
        .join(' ').toLowerCase()
      if (!hay.includes(keyword)) return false
    }
    return true
  }

  /**
   * 选提供方：来源那一档跟着走（选具体某家 = 来源落到「订阅」；选手动 / Clash
   * = 来源落到同名那档）。两个控件各筛各的会筛出「手动 + 魔戒」这种恒空的组合，
   * 而用户看不出是谁在挡着 —— 联动之后任一个控件都随时给出一个有效视图。
   */
  function pickVendor(next: string | null): void {
    // 组件库的 Select 允许回 null（清除档位）：当作「全部提供方」
    const value = next ?? 'all'
    setVendorFilter(value)
    if (value === 'all') setSrcFilter('all')
    else if (value === 'manual' || value === 'clash') setSrcFilter(value)
    else setSrcFilter('subscription') // 全部订阅 / 某一家订阅 / 已移除的订阅
  }

  /** 选来源粗档：提供方细筛回到「全部提供方」（同上，避免恒空的组合） */
  function pickSource(next: string): void {
    setSrcFilter(next)
    setVendorFilter('all')
  }

  const shown = rows.filter(matches)
  /** 表头全选的**作用域**：当前筛出来的行（搜索后全选就只选看得见的那些） */
  const shownIds = shown.map(item => item.id)
  const allPicked = shownIds.length > 0 && shownIds.every(id => snapshot.selected.has(id))
  const somePicked = shownIds.some(id => snapshot.selected.has(id)) && !allPicked
  /** 当前可见列（顺序 / 显隐 / 对齐都跟着列设置走；表头、表体、空态 colspan 读它） */
  const columns = visibleColumns()
  /**
   * 命中行的**渲染顺序**：组序（手动 → Clash → 订阅）→ 组内保持后端给的顺序。
   * 分组带与分页切片都按它 —— 不先排好，第 2 页可能又冒出一条「手动」的带。
   */
  const ordered: ProxyPoolItem[] = []
  for (const group of groups()) ordered.push(...shown.filter(item => groupOf(item) === group.key))
  // 分页按整份命中列表切（客户端分页，与账号 / 网关 Key 同一套控件与存盘口径）
  const paging = useClientPaging(shown.length, 'proxies')
  const pageRows = paging.paged ? paging.slice(ordered) : ordered
  /** 各组在**整个命中集**里的条数：分组带报的是这一组有多少条，不是这一页有多少条 */
  const groupCounts = new Map<string, number>()
  for (const item of shown) groupCounts.set(groupOf(item), (groupCounts.get(groupOf(item)) ?? 0) + 1)

  // 筛选 / 搜索换了结果集：回到第 1 页（停在第 5 页再筛成三条，看到的会是尾部，
  // 与签到记录 / 请求日志同一条口径；越界由 useClientPaging 自己夹回）
  React.useEffect(() => { paging.goto(1) }, [keyword, srcFilter, vendorFilter, stFilter])

  function testCell(item: ProxyPoolItem): React.ReactNode {
    const last = item.lastTest
    if (!last || !last.at) return <span className='text-subtle'>{t('未测试')}</span>
    const suffix = durationText(last.durationMs)
    return (
      <div className='flex flex-col gap-0.5 leading-[1.5] min-w-0'>
        {/* 失败原因是上游错误原文（后端数据）原样透出；耗时只有数字单位，不包 */}
        <span className={last.success ? 'text-success' : 'text-destructive'}>
          {last.success ? t('✅ {ip}', { ip: last.ip || t('连接成功') }) : t('❌ 连接失败')}
          {suffix ? `　${suffix}` : ''}
          <span className='text-[11px] text-subtle'>　{formatTime(last.at)}</span>
        </span>
        {last.success ? null : (
          <span className='overflow-hidden text-ellipsis whitespace-nowrap text-[11px] text-subtle'
            title={String(last.error || t('连接失败'))}>
            {String(last.error || t('连接失败'))}
          </span>
        )}
      </div>
    )
  }

  /** 名称列第二行：协议 + 引用它的账号数（地址在隔壁的「地址」列，不在这里挤） */
  function nameMeta(item: ProxyPoolItem): React.ReactNode {
    const used = item.usedBy || []
    const parts: React.ReactNode[] = []
    if (item.protocol) parts.push(item.protocol)
    if (used.length) {
      parts.push(
        <span className='text-warn'
          title={used.map(entry => String(entry.name || entry.id || '')).join(t('、'))}>
          {t('{n} 个账号在用', { n: used.length })}
        </span>,
      )
    }
    if (!parts.length) return null
    return (
      <span className='text-[11px] text-subtle'>
        {parts.map((part, index) => (
          <React.Fragment key={index}>
            {index ? '　·　' : ''}
            {part}
          </React.Fragment>
        ))}
      </span>
    )
  }

  /**
   * 地址列：本地监听地址 + 复制。
   *
   * 前两种形态没有可复制的文本，就不摆那颗按钮 —— 一颗点下去没反应的复制按钮
   * 比没有更糟（用户会以为是自己点歪了）。剪贴板交给 clipboard.js 的全局委托：
   * `data-copy` 给文本，`.copy-btn` 负责「⧉ → ✓」那一下反馈。
   */
  function addrCell(item: ProxyPoolItem): React.ReactNode {
    // 解析失败：错误原文挂在 title 上（这一格铺不下详情，但用户需要知道为什么）
    if (item.resolveError) {
      return <span className='text-destructive' title={item.resolveError}>{t('解析失败')}</span>
    }
    // lazy listeners：订阅节点还没有监听端口（未被账号引用，或刚引用上、内核还没补建完）
    if (isUnprovisionedItem(item)) {
      // 「点测试会自动补建」只在**有内核**时成立：内核没装（没下过 / 刚被删）
      // 时补建无从谈起，该说的是「先去下载内核」—— 与后端 pool_test 的判据同口径
      const noKernel = subKernel()?.installed === false
      return (
        <span className='text-subtle'
          title={noKernel
            ? t('代理内核未安装（在「订阅管理」里下载内核后，节点会自动分配监听端口）')
            : t('还没有监听端口（点「测试」会自动补建并等它就绪；或在账号里引用它，保存后自动补建）')}>
          {poolItemAddress(item) || t('未分配端口')}
        </span>
      )
    }
    const address = poolItemAddress(item)
    return (
      // 地址**不截断**：Clash 那类的 resolved 是「名字（协议 主机:端口）」这种长标签，
      // 截断会把端口吃掉 —— 而看端口正是这一列的用途。固定列宽下换成折行（break-all
      // 兜住没有空格的长主机名），行高让给这一条，完整文本同时挂在 title 上。
      <div className='flex min-w-0 items-center gap-1'>
        <span className='min-w-0 flex-1 font-mono text-[12px] leading-[1.45] text-text-2 break-all'
          title={address || undefined}>{address || '—'}</span>
        {address ? (
          <Button variant='ghost' size='icon-xs' className='copy-btn flex-none'
            data-copy={address} title={t('复制地址')}>⧉</Button>
        ) : null}
      </div>
    )
  }

  function cell(column: string, item: ProxyPoolItem, busy: boolean, picked: boolean): React.ReactNode {
    // 批量在跑时行内一切写入口都停掉：批量测试会把 testing 指向「当前那一条」，
    // 这时用户点编辑 / 删除 / 启停会和正在跑的流程抢同一份数据
    const locked = busy || snapshot.batchBusy
    switch (column) {
      case 'pick':
        return (
          <Checkbox checked={picked} title={t('勾选后可批量操作')}
            aria-label={t('勾选「{name}」', { name: item.name || t('未命名') })}
            onCheckedChange={next => togglePick(item.id, next)} />
        )
      case 'name':
        return (
          <div className='flex flex-col gap-0.5 leading-[1.5] min-w-0'>
            <b>{item.name || t('未命名')}</b>
            {nameMeta(item)}
          </div>
        )
      case 'addr':
        return addrCell(item)
      case 'state': {
        // 只读镜像：开关照常显示真实状态（Clash 里禁用了这里也看得见），但不给改
        const readonly = isReadonlyPoolItem(item)
        const hint = readonlyHint(item)
        return (
          <Switch checked={item.enabled} disabled={locked || readonly}
            aria-label={t('启用「{name}」', { name: item.name || t('未命名') })}
            title={readonly ? hint : undefined}
            onCheckedChange={next => void toggleItem(item, next)} />
        )
      }
      case 'test':
        return testCell(item)
      case 'act': {
        // 手动条目给编辑 / 删除；只读条目只留测试（去哪儿改在只读开关的悬浮提示里）；
        // 孤儿订阅节点（所属订阅已删）**删除放行** —— 归属订阅已不存在，
        // 删除是清理它们的唯一出路（见 isWritablePoolItem）
        const orphan = isOrphanSubscriptionItem(item)
        const readonly = isReadonlyPoolItem(item)
        const hint = readonlyHint(item)
        const testing = snapshot.testing === item.id
        return (
          <div className='row-actions'>
            <Button size='sm' variant='ghost' disabled={locked || snapshot.testing !== null}
              onClick={() => void testItem(item)}>
              {testing ? t('测试中…') : t('测试')}
            </Button>
            {item.source !== 'clash' && item.source !== 'subscription' ? (
              <>
                <Button size='sm' variant='ghost' disabled={locked}
                  onClick={() => setModal({ item })}>{t('编辑')}</Button>
                <Button size='sm' variant='destructive' disabled={locked}
                  onClick={() => void removeItem(item)}>{t('删除')}</Button>
              </>
            ) : null}
            {orphan ? (
              <Button size='sm' variant='destructive' disabled={locked} title={hint}
                onClick={() => void removeItem(item)}>{t('删除')}</Button>
            ) : null}
          </div>
        )
      }
      default:
        return null
    }
  }

  /** 一条数据行 */
  function row(item: ProxyPoolItem): React.ReactNode {
    const busy = snapshot.pending.has(item.id)
    const picked = isPicked(item.id)
    return (
      <tr key={item.id} className={cn(item.enabled ? '' : 'off', picked && 'bg-primary-soft')}
        data-id={item.id}>
        {columns.map(column => (
          <td key={column.key} className={`ta-${column.align}`}>
            {cell(column.key, item, busy, picked)}
          </td>
        ))}
      </tr>
    )
  }

  /**
   * 表体：**本页**的行 + 它们各自的分组带。
   *
   * 分组带夹在行的中间（组一变就插一条），所以带子是「当前这一页里出现过的组」——
   * 订阅那组跨页时，第 2 页开头仍会看到它的带与只读口径。带上的条数是该组在
   * 整个命中集里的总数（不是这一页的条数）：它讲的是「这一组有多少条」，
   * 「这一页多少条」是页脚分页栏的事（两者报同一个数只会让人误读）。
   */
  function body(): React.ReactNode {
    if (!rows.length) {
      return (
        <tr>
          <td colSpan={columns.length} className='empty'>
            {ready
              ? t('还没有代理。点右上角「＋ 新增代理」手填一个，点「同步 Clash Verge」把 Clash 的出口导进来，或在「订阅管理」里导入订阅节点。')
              : t('加载中…')}
          </td>
        </tr>
      )
    }
    if (!shown.length) {
      return (
        <tr>
          <td colSpan={columns.length} className='empty'>
            {keyword
              ? t('没有匹配「{kw}」的出口', { kw: kw.trim() })
              : t('没有匹配当前筛选的出口')}
          </td>
        </tr>
      )
    }
    const blocks: React.ReactNode[] = []
    let current = ''
    for (const item of pageRows) {
      const key = groupOf(item)
      if (key !== current) {
        current = key
        const group = groups().find(entry => entry.key === key)
        blocks.push(
          <tr className='tr-group' key={`group:${key}:${blocks.length}`}>
            <td colSpan={columns.length}>
              <span className='prov-tag'>{group?.label ?? key}</span>
              {t('{n} 个', { n: groupCounts.get(key) ?? 0 })}
              {group?.note ? <span className='font-normal'>　·　{group.note}</span> : null}
            </td>
          </tr>,
        )
      }
      blocks.push(row(item))
    }
    return <>{blocks}</>
  }

  return (
    <>
      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('代理池')}</h2>
          <span className='panel-sub'>
            {ready ? t('{n} 个出口', { n: rows.length }) : t('加载中…')}
          </span>
          {/* 来源说明：本表的条目不止手动添加的 —— 订阅与 Clash 的出口自动
              镜像进来（只读），放在面板头让「源 → 消费」的关系一眼可见，
              不必等用户看到页脚的 Clash 说明才反应过来 */}
          <span className='text-[11px] text-subtle'>{t('订阅与 Clash 的出口自动镜像进来')}</span>
        <div className='head-actions'>
          {/* 设置放在最左：低频配置项（端口范围 / 测试网址），不必挤在主操作旁边。
              弹窗是打开时才读设置（loadSettings 在弹窗内部），这里只负责开门 */}
          <Button variant='outline' disabled={snapshot.batchBusy}
            onClick={() => setSettingsOpen(true)}>
            {t('设置')}
          </Button>
          {/* 订阅管理：订阅表的唯一入口（页面上不再有第二个面板）。
              带订阅条数 —— 0 条时只写按钮名，免得出现「订阅管理（0）」这种读数 */}
          <Button variant='outline' disabled={snapshot.batchBusy}
            onClick={() => setSubsOpen(true)}>
            {subs.length ? t('订阅管理（{n}）', { n: subs.length }) : t('订阅管理')}
          </Button>
          {/* 同步放在刷新左边：它做的事比「刷新」多一层（把 Clash 的出口集合
              镜像进池），而进页面时的那次自动同步是静默的 —— 用户想在
              「我刚在 Clash 里加了出口」之后立刻见效时点它。
              批量在跑时两个都停：它们会把正在被批量流程改写的列表整份换掉 */}
          <Button variant='outline' disabled={snapshot.syncing || snapshot.batchBusy}
            onClick={() => void syncClash()}>
            {snapshot.syncing ? t('同步中…') : t('同步 Clash Verge')}
          </Button>
          <Button variant='outline' disabled={snapshot.loading || snapshot.batchBusy}
            onClick={() => { void loadPanel(); void loadSection() }}>
            {snapshot.loading ? t('刷新中…') : t('刷新')}
          </Button>
          <Button variant='default' disabled={snapshot.batchBusy}
            onClick={() => setModal({ item: null })}>{t('＋ 新增代理')}</Button>
        </div>
      </div>

      {/* 内核状态条：本机内核与节点端口是全局的事（端口用尽也报在这里），
          不跟着订阅表一起收进弹窗 */}
      <div className='px-3 pt-3'>
        <KernelBar showPortRange />
      </div>

      {/* 工具条与批量条**共用一个槽位**：勾选后原地换成批量条，取消选择换回来 ——
          两条不叠着，表格不会因为多出一条状态带而整体下移。
          批量条与账号页同一套类名（page-accounts.css），勾选后整条点亮成主色底 */}
      {selectedCount > 0 ? (
        <div className='batch-bar active'>
          <span className='batch-count'>
            {t('已选')} <b>{selectedCount}</b> {t('个 · 共')} <b>{shown.length}</b> {t('个')}
          </span>
          {clashPicked ? (
            <span className='batch-hidden'>
              {t('其中 {n} 条是同步来的只读条目（Clash / 订阅）：可参与批量测试，删除会跳过', { n: clashPicked })}
            </span>
          ) : null}
          <div className='batch-actions'>
            <Button variant='outline' disabled={snapshot.batchBusy}
              onClick={() => void batchTest()}>
              {progress ? t('测试中 {done}/{total}', { done: progress.done, total: progress.total }) : t('批量测试')}
            </Button>
            <Button variant='destructive' disabled={snapshot.batchBusy}
              onClick={() => void batchRemove()}>
              {t('批量删除')}
            </Button>
            <Button variant='outline' disabled={snapshot.batchBusy}
              onClick={() => clearSelection()}>
              {t('取消选择')}
            </Button>
          </div>
        </div>
      ) : (
        /* 内边距与 .batch-bar 的 10px 16px 对齐：两条同高，互为替换时表格不下移 */
        <div className='flex flex-wrap items-center gap-2.5 border-b border-border px-4 py-[10px]'>
          <InputGroup className='w-[clamp(150px,22vw,220px)]'>
            <InputGroupInput type='search' value={kw} aria-label={t('搜索节点')}
              placeholder={t('搜索名称 / 地址 / 端口')}
              onChange={event => setKw(event.currentTarget.value)} />
            <InputGroupAddon align='inline-start' aria-hidden='true'>
              <SearchIcon />
            </InputGroupAddon>
          </InputGroup>
          {/* 提供方下拉排在来源分段之前：它比来源更「具体」（来源只到手动 / Clash /
              订阅这一层，而订阅底下是各家机场）。SelectValue 显式给当前档的文案 ——
              组件库的 Select 不靠 value 反查展示文本（见 AGENTS 的 SelectValue 约定） */}
          <Select value={vendorFilter} onValueChange={pickVendor}>
            <SelectTrigger className='w-[clamp(150px,18vw,210px)]' aria-label={t('按提供方筛选')}>
              <SelectValue>{vendors.find(option => option.value === vendorFilter)?.label ?? t('全部提供方')}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {vendors.map(option => (
                <SelectItem key={option.value} value={option.value}>
                  {/* 0 条的档弱化显示但**留着**（与来源分段的约定一致）：订阅还在、
                      节点还没落地时，档位凭空消失会让人以为是没订阅上 */}
                  <span className='flex w-full items-center gap-2'>
                    <span className={'min-w-0 flex-1 truncate' + (option.count ? '' : ' text-subtle')}>
                      {option.label}
                    </span>
                    <span className='flex-none text-[11px] text-subtle'>{option.count}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <SegmentedControl options={sourceOptions(rows)} value={srcFilter}
            onValueChange={pickSource} aria-label={t('按来源筛选')} />
          <SegmentedControl options={stateOptions(rows)} value={stFilter}
            onValueChange={setStFilter} aria-label={t('按状态筛选')} />
          <span className='ml-auto text-[11px] text-subtle'>
            {t('显示 {n} / 共 {total} 个出口', { n: shown.length, total: rows.length })}
          </span>
        </div>
      )}

      <div className='models-table-wrap'>
        <table className='models-table proxies-table'>
          {/* colgroup 与 thead 的顺序都跟着列设置走，data-col 一个不少：列设置的就地
              重排、列宽把手（table-columns.js 按 data-col 定位 <col> 与 <th>）都要读它。
              默认列宽写在 <col> 的类上（没拖过的列走它），拖过的列由列宽层写 inline width */}
          <colgroup>
            {columns.map(column => (
              <col key={column.key} data-col={column.key}
                className={COL_CLASS[column.key] || undefined} />
            ))}
          </colgroup>
          <thead>
            <tr>
              {columns.map(column => (
                <th key={column.key} data-col={column.key} className={`ta-${column.align}`}>
                  {column.key === 'pick' ? (
                    /* 全选在表头（表格的常规位置），作用域是当前筛出来的行 */
                    <Checkbox checked={allPicked} indeterminate={somePicked}
                      disabled={!shownIds.length} title={t('全选当前筛选出的出口')}
                      aria-label={t('全选当前筛选出的出口')}
                      onCheckedChange={next => setAllPicked(shownIds, next)} />
                  ) : column.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>{body()}</tbody>
        </table>
      </div>

      <TableFooter
        leading={(
          /* 脚注只留**状态**（Clash 没同步上 / 列表读失败）：那两句用法说明
             （出口怎么被账号引用、只读镜像去哪儿改）已经收进各自的入口 ——
             账号弹窗的「出网代理」、面板头的「订阅管理」与「同步 Clash Verge」，
             页脚常驻一大段解释只会把读数挤掉 */
          <>
            {/* Clash 状态常驻在页脚：同步是静默的（进页面自动跑一次），没有这句话时
                「为什么一条 Clash 条目都没有」只能靠点按钮 + 看 toast 才知道 */}
            {snapshot.data?.clash && snapshot.data.clash.available === false ? (
              <span className='text-subtle'>
                {snapshot.data.clash.error
                  ? t('未检测到 Clash Verge：{reason}', { reason: snapshot.data.clash.error })
                  : t('未检测到 Clash Verge')}
              </span>
            ) : null}
            {snapshot.error ? (
              <span className='text-destructive'>{t('读取失败：{reason}', { reason: snapshot.error })}</span>
            ) : null}
          </>
        )}
        total={shown.length}
        page={paging.page}
        pageCount={paging.pageCount}
        size={paging.size}
        onSizeChange={paging.setSize}
        onPageChange={paging.goto}
        range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
      />

      {modal ? (
        <ProxyModal
          item={modal.item}
          onClose={() => setModal(null)}
        />
      ) : null}
      {settingsOpen ? (
        <ProxySettingsModal onClose={() => setSettingsOpen(false)} />
      ) : null}
      {subsOpen ? (
        <SubscriptionManagerDialog onClose={() => setSubsOpen(false)} />
      ) : null}
      </section>
    </>
  )
}

/* ─── 来源分组 ───────────────────────────────── */

/** 条目 → 组键（未知来源按手动处理：它一定不是那两类只读镜像） */
function groupOf(item: ProxyPoolItem): string {
  if (item.source === 'clash') return 'clash'
  if (item.source === 'subscription') return 'subscription'
  return 'manual'
}

/**
 * 三个来源组（顺序固定：可写的在最上，镜像来的按「配置成本」排在后面）。
 * note 就是这一组的只读口径 —— 分组带是用户唯一会逐组读一行文字的地方。
 */
function groups(): Array<{ key: string; label: string; note: string }> {
  return [
    { key: 'manual', label: t('手动'), note: t('本页可编辑、可删除') },
    { key: 'clash', label: 'Clash Verge', note: t('只读：在 Clash Verge 里改') },
    { key: 'subscription', label: t('订阅'), note: t('只读：在「订阅管理」里改') },
  ]
}

/** 分段档位带上各来源的条数（0 条的档仍然保留，只是弱化显示 —— 与账号页同一约定） */
function sourceOptions(rows: ProxyPoolItem[]): Array<{ value: string; label: string; count: number }> {
  const count = (key: string) => rows.filter(item => groupOf(item) === key).length
  return [
    { value: 'all', label: t('全部'), count: rows.length },
    { value: 'manual', label: t('手动'), count: count('manual') },
    { value: 'clash', label: 'Clash', count: count('clash') },
    { value: 'subscription', label: t('订阅'), count: count('subscription') },
  ]
}

/** 状态档位：未就绪（订阅节点还没有监听端口）单独一档 */
function stateOptions(rows: ProxyPoolItem[]): Array<{ value: string; label: string; count: number }> {
  const key = (item: ProxyPoolItem) => {
    if (item.port === null || item.port === undefined) return 'wait'
    if (!item.lastTest?.at) return 'none'
    return item.lastTest.success ? 'ok' : 'fail'
  }
  const count = (target: string) => rows.filter(item => key(item) === target).length
  return [
    { value: 'all', label: t('全部状态'), count: rows.length },
    { value: 'ok', label: t('可用'), count: count('ok') },
    { value: 'fail', label: t('失败'), count: count('fail') },
    { value: 'wait', label: t('未就绪'), count: count('wait') },
  ]
}

/* ─── 提供方档位 ─────────────────────────────── */

/** 「已移除的订阅」那一档的值：孤儿节点（所属订阅已删）单列一档，别混进各家 */
const ORPHAN_VENDOR = '__orphan'

/**
 * 提供方下拉的档位：全部 / 手动 / Clash Verge / 全部订阅 / 各家订阅 / 已移除的订阅。
 *
 * ── 家名从哪儿来（不猜）────────────────────────────────────────
 * 名字与 id 都取**订阅清单**（`getSubscriptions` 的 items），池条目只提供
 * `providerId` —— 节点名里带不带机场名是各家自己的命名习惯（「魔戒 · 香港-01」
 * 是同步时拼的展示名），拿它反推归属一改命名就错。清单里有的订阅一律列出
 *（哪怕这一家在池里 0 条：先有订阅、内核还没起监听器时就是这样，「有这家但没节点」
 * 比「下拉里找不到这家」好解释）。
 *
 * 各家按池里条数从多到少排：点这个下拉的人多半想快速切到「那一大坨」。
 */
function vendorOptions(
  rows: ProxyPoolItem[],
  subs: SubscriptionItem[],
): Array<{ value: string; label: string; count: number }> {
  const subscription = rows.filter(item => groupOf(item) === 'subscription')
  const named = subs
    .map(sub => ({
      value: sub.id,
      label: sub.name || sub.id,
      count: subscription.filter(item => !item.orphan && item.providerId === sub.id).length,
    }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label))
  const orphanCount = subscription.filter(item => item.orphan).length
  return [
    { value: 'all', label: t('全部提供方'), count: rows.length },
    { value: 'manual', label: t('手动'), count: rows.filter(item => groupOf(item) === 'manual').length },
    { value: 'clash', label: 'Clash Verge', count: rows.filter(item => groupOf(item) === 'clash').length },
    { value: 'subscription', label: t('全部订阅'), count: subscription.length },
    ...named,
    ...(orphanCount ? [{ value: ORPHAN_VENDOR, label: t('已移除的订阅'), count: orphanCount }] : []),
  ]
}

/** 搜索框左侧的放大镜（内联 SVG，照 models-page 的做法 —— 岛内不引图标库） */
function SearchIcon(): React.ReactNode {
  return (
    <svg width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor'
      strokeWidth='2' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <circle cx='11' cy='11' r='8' />
      <path d='m21 21-4.3-4.3' />
    </svg>
  )
}

/* ─── 挂载与对外契约 ─────────────────────────── */

/** app.js 切到本页时调它（同名契约见 ui/app.js 的 showPage） */
declare global {
  interface Window {
    wbProxiesPanel?: {
      load: () => Promise<void>
      /** 当前可见列（顺序即配置顺序）：列宽层按它算覆盖值落到哪一列、哪一列在最后 */
      visibleColumns: () => Array<{ key: string }>
    }
  }
}

window.wbProxiesPanel = {
  load: async () => { await loadPanel(); await loadSection() },
  // 与另外五张表同一个契约名（table-columns.js 的 columnsOf 读它）
  visibleColumns: () => visibleColumns().map(column => ({ key: column.key })),
}

const PAGE_SELECTOR = '.page[data-page="proxies"]'

/**
 * 把 React root 直接建在 `.page[data-page="proxies"]` 上（不套宿主 div：页面 CSS 用
 * `.page` 的直接子选择器分配布局，中间插一层会打断它）。先清掉骨架里的静态子节点，
 * 下面的 JSX 会按同样的类名重新渲染它。
 */
function mount(): void {
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section || section.dataset.mounted === '1') return
  section.dataset.mounted = '1'
  section.replaceChildren()
  createRoot(section).render(<ProxiesPage />)
  // 首屏若正停在本页就自拉一次（app.js 启动时的 showPage 早于本岛加载，
  // 那一次 load() 会丢；与 keys / models 各岛同一处兜底）
  if (wb().wbApp?.currentPage === 'proxies') { void loadPanel(); void loadSection() }
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
