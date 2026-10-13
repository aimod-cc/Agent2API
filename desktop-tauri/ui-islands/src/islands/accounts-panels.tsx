/**
 * 账号表的**行与单元格**（替换 ui/accounts-table.js 的单元格渲染 + ui/accounts-model.js
 * 的 ⋯ 菜单 / 行内面板 HTML + ui/accounts-view.js 的菜单交互）。
 *
 * ── 为什么还留着 legacy 类名 ─────────────────────────────────
 * 表格的每一格都有 page-accounts-table.css 里量出来的尺寸与排版（列宽预算、
 * `.acct-actions` 的四颗按钮预算…）。迁移只换**控件**，布局类名原样保留：
 * 换掉的控件走组件库（Button / Badge / Switch / Checkbox / Select / Popover），
 * 而 `.usage-sum`（读数，不可点）与 `.order-cell`（分组模式的拖动把手 + 组内序号）
 * 这两处组件库没有对应件，保持原标记形态 —— 见最终报告的「组件库缺口」。
 *
 * ── 分组化（2026-10）删掉的两块 ────────────────────────────────
 *   · `PriorityStepper`（数字框 + 两枚箭头的优先级控件）：顺序改由拖拽表达，
 *     连它承载的「↑ = 数值变小」那套方向语义一起下线 —— 现在只有「拖到哪一行」
 *     一种表达（落在目标行上方 / 下方，由 accounts-queue-drag 判定）。
 *   · `ProviderCell`（按 provider 上色的徽章）：家名与图标搬到了分组带
 *     （`GroupBandRow`），行里不再重复。`.pbadge` 这一族样式留在 CSS 里 ——
 *     报表页的账号行还在用（report-page.tsx 自己的那一枚）。
 */

import * as React from 'react'
import {
  Badge,
  BadgeDot,
  Button,
  Checkbox,
  Popover,
  PopoverContent,
  PopoverTrigger,
  Progress,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Tooltip,
  TooltipArrow,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
  cn,
} from '@ui'
import { formatTime, poolItemLabel, POOL_VALUE_PREFIX, shared, type AccountRecord, type LimiterRule, type TokenReading, type UsageEntry } from './accounts-shared'
import {
  accountTags, activeLimits, balanceBlockedOf, claimDoneTitle, claimedToday,
  displayNameOf, expiryMillis, formatIntervalSeconds, formatResetText,
  formatTokenCount, identifierOf, isDesktopAccount, isEnabled, isFreePlan, planBadgeLabel,
  providerFeatures,
  providerOf, RESET_UNKNOWN, supportsClaim, supportsUsage, supportsUsageDetail,
  tokenBlockedOf, tokenCountdownText, tokenDisableTriggeredOf, tokenReadingForRule,
  tokenRulesOf, tokenWindowInfo, zcodePlanStateLabel,
} from './accounts-domain'
import {
  PROXY_CUSTOM_CURRENT, PROXY_CUSTOM_EDIT, applyProxyPick,
  connectionsOf, maskName, openBatchDialog, openSettingsDialog, openZcodePlans,
  poolError, proxyPoolSnapshot, queryUsageOnce, setAccountEnabled, setGroupEnabled, setPanelOpen,
  tokenUsageOf, toggleGroupCollapsed, toggleNamesHidden, usageEntryOf, usageFailureOf,
} from './accounts-data'
import type { AccountGroup } from './accounts-domain'
import { PROVIDER_ICONS } from './add-provider-pick'
import { t } from '../i18n'

/** 图标（icons.js 的内联 SVG 串）：整站共用一份图标集，这里只做注入 */
function iconHtml(name: string, size: number): string {
  return shared().wbIcons?.icon?.(name, size) || ''
}

/* ─── 顺序列（组内）──────────────────────────── */

/**
 * 顺序格：`⠿ #N` —— 左边是拖动把手，右边是**组内序号**。
 *
 * ── 为什么不再有数字输入与 ↑↓ ────────────────────────────────
 * 顺序改由拖拽表达（组序拖分组带、组内拖这一格），数字本身不再是用户要维护的东西：
 * 全局 `priority` 由「组顺位 → 组内顺位」编译出来（后端 `reorder_accounts` 整队重编号），
 * 界面上留一个可编辑的号码栏，等于给同一件事留了两个口径。旧的优先级列（数字 + 两枚
 * 箭头）因此整体下线，`commitPriority` / `moveAccount` 两个动作也随之不再被调用。
 *
 * 把手是**唯一**的拖动起点（不是整行都能拖）：行里还有开关、下拉、按钮，整行可拖会
 * 让「点一下开关」变成「拖走了一行」。序号显示的是它在**本组内**的位次，组与组之间的
 * 先后由分组带的顺位表达 —— 两者合起来才是真实的尝试顺序。
 */
export function OrderCell({ account, rank }: { account: AccountRecord; rank: number }) {
  const label = t('组内第 {rank} 位', { rank })
  return (
    <div className='order-cell'>
      <span className='order-grip' role='button' tabIndex={-1} aria-label={t('拖动调整组内顺序')}
        title={t('按住拖动，调整这个账号在本组内的顺序')}
        dangerouslySetInnerHTML={{ __html: iconHtml('gripVertical', 14) }} />
      <span className='order-rank' title={label}>{`#${rank}`}</span>
    </div>
  )
}

/* ─── 分组带 ────────────────────────────────── */

/**
 * 分组带的 ⋯ 菜单：整组启用 / 禁用、批量操作本组、折叠本组。
 *
 * 「整组启用 / 禁用」直接落批量端点（一次改一组，与单行开关同一口径）；
 * 「批量操作本组…」打开既有的批量弹窗并**预选本组**（改代理这类要挑参数的动作用它，
 * 不必再新造一套界面）；折叠那一项与组头那颗箭头同一个状态 —— **它是个开关**，
 * 所以文案必须跟着当前状态变：已经收起的那一组，这一项写「展开这一组」才是它真做的事。
 */
function GroupMenu({ group, ids, collapsed }: { group: AccountGroup; ids: string[]; collapsed: boolean }) {
  const [open, setOpen] = React.useState(false)
  const itemClass = 'w-full justify-start px-2 font-normal'
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<button type='button' className='grp-iconbtn' title={t('本组操作')} />}>⋯</PopoverTrigger>
      <PopoverContent align='end' sideOffset={4} className='w-[196px] p-1.5'>
        <div className='flex flex-col gap-0.5'>
          <Button variant='ghost' size='sm' className={itemClass}
            title={t('启用这一组里的全部账号（含被筛选隐藏的）')}
            onClick={() => { setOpen(false); void setGroupEnabled(ids, true) }}>
            {t('全部启用（{n}）', { n: ids.length })}
          </Button>
          <Button variant='ghost' size='sm' className={itemClass}
            title={t('禁用这一组里的全部账号（含被筛选隐藏的）')}
            onClick={() => { setOpen(false); void setGroupEnabled(ids, false) }}>
            {t('全部禁用（{n}）', { n: ids.length })}
          </Button>
          <Button variant='ghost' size='sm' className={itemClass}
            title={t('打开批量操作弹窗，并预选这一组的账号（改代理等动作在弹窗里挑参数）')}
            onClick={() => { setOpen(false); openBatchDialog(ids, 'proxy') }}>
            {t('批量操作本组…')}
          </Button>
          <div className='my-1 h-px bg-hairline' />
          <Button variant='ghost' size='sm' className={itemClass}
            onClick={() => { setOpen(false); toggleGroupCollapsed(group.provider) }}>
            {collapsed ? t('展开这一组') : t('折叠这一组')}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}

/**
 * 分组带：一家的分隔横幅（`<tbody>` 的第一行，通栏一格）。
 *
 * 从左到右：拖动把手（整组顺位的唯一拖动起点）→ 顺位 → 家图标 + 家名 → 统计
 * （N 个账号 · 启用 x · 禁用 y · 限流 z）→ 右侧折叠箭头与 ⋯ 菜单。
 * 统计读的是**整组**（不受筛选影响）：它回答「这一家有多少账号」，不是「当前显示几行」。
 *
 * 样式刻意保持中性（surface 底 + 一条 hairline）：一家一色会让整页像调色板，
 * 而「哪一段是哪一家」由家名与图标已经说清楚了。
 */
export function GroupBandRow({ group, collapsed, colSpan }: {
  group: AccountGroup
  collapsed: boolean
  colSpan: number
}) {
  const icon = PROVIDER_ICONS[group.provider]
  return (
    <tr className='grp-band' data-provider={group.provider}>
      <td colSpan={colSpan}>
        <div className='grp-band-inner'>
          <span className='queue-grip' role='button' tabIndex={-1} aria-label={t('拖动调整整组顺序')}
            title={t('按住拖动，调整这一家的组顺位（整组一起动）')}
            dangerouslySetInnerHTML={{ __html: iconHtml('gripVertical', 14) }} />
          <span className='grp-rank' title={t('组顺位：越小越先用（拖动分组带调整）')}>
            {t('第 {rank} 顺位', { rank: group.rank })}
          </span>
          <span className='grp-logo'>
            {icon
              ? <img src={icon} alt='' loading='lazy' />
              : <span className='grp-logo-txt'>{String(group.label || '?').trim().slice(0, 1).toUpperCase()}</span>}
          </span>
          <span className='grp-name'>{group.label}</span>
          <span className='grp-stat'>{t('{total} 个账号 · 启用 {enabled} · 禁用 {disabled}{limited}', {
            total: group.members.length,
            enabled: group.enabled,
            disabled: group.disabled,
            limited: group.limited > 0 ? t(' · 限流 {n}', { n: group.limited }) : '',
          })}</span>
          <span className='grp-spacer' />
          <button type='button' className='grp-iconbtn grp-fold' aria-expanded={!collapsed}
            title={collapsed ? t('展开这一组') : t('折叠这一组')}
            onClick={() => toggleGroupCollapsed(group.provider)}
            dangerouslySetInnerHTML={{ __html: iconHtml('chevronDown', 14) }} />
          <GroupMenu group={group} ids={group.accounts.map(account => account.id)} collapsed={collapsed} />
        </div>
      </td>
    </tr>
  )
}

/* ─── 状态列 ────────────────────────────────── */

/**
 * 状态：启用 / 禁用开关（+ 需要留意时的健康徽章）。
 * 开关直接落 `PATCH { enabled }`，不做二次确认 —— 这个动作可逆，且关掉后账号记录仍在
 * 列表里。徽章只在需要留意时出现（一切正常时 accountTags 返回空）—— 启用状态由开关的
 * 轨道位置与滑块表达，再补一枚「启用」是同一格里的第二次说明。
 */
export function StatusCell({ account }: { account: AccountRecord }) {
  const enabled = isEnabled(account)
  const tags = accountTags(account)
  // Token 限额的禁用档命中且账号已被禁用：补一枚红色徽章回答「为什么是禁的」
  // —— 禁用动作本身不带「为什么」的标记，当前窗口的消耗读数就是证据（与后端
  // 自动禁用钩子同判据，见 tokenDisableTriggeredOf）。
  if (!enabled && tokenDisableTriggeredOf(account, tokenUsageOf(account))) {
    tags.push({
      text: t('Token 限额 · 已禁用'),
      kind: 'bad' as const,
      title: t('本周期 Token 消耗达到限制器的上限，账号已被自动禁用（窗口重置也不恢复，需手动启用）'),
    })
  }
  const who = displayNameOf(account) || account.id
  return (
    <>
      <Switch checked={enabled} aria-label={t('{action}{who}', { action: enabled ? t('禁用') : t('启用'), who })}
        title={enabled ? t('已启用，点击禁用（不参与转发）') : t('已禁用，点击启用')}
        onCheckedChange={next => void setAccountEnabled(account.id, next)} />
      {tags.length ? (
        <div className='status-tags'>
          {tags.map(tag => (
            <Badge key={tag.text} shape='tag' variant={tag.kind === 'bad' ? 'destructive' : 'secondary'}
              title={tag.title}>{tag.text}</Badge>
          ))}
        </div>
      ) : null}
    </>
  )
}

/* ─── 限流列 ────────────────────────────────── */

/**
 * 限流：这个账号**当前限流中的模型**。限额在后端按「账号 × 模型」记，四家通用 ——
 * 所以这一列对四家都成立，不再需要「选个模型看队列」的筛选器。
 * 有限流时是一枚可点的黄色徽章（点开 / 收起行下的明细面板），正常时是绿点「正常」。
 *
 * 「已限流」分段把限制器的跳过档也算进来（isLimitedNow 的口径），这一列必须同口径：
 * 没有模型限额但被限制器拦下的账号不亮绿点，给一枚黄色「余额不足 / Token 限额」
 * 说明它为何归入已限流（跳过的细节仍由余额列的徽章承载，这里不复述）。
 */
export function LimitsCell({ account, open }: { account: AccountRecord; open: boolean }) {
  const entries = activeLimits(account)
  if (!entries.length) {
    if (balanceBlockedOf(account, usageEntryOf(account))) {
      return (
        <Badge variant='warning' shape='tag'
          title={t('余额低于限制器的阈值，转发时会跳过该账号（余额回升自动恢复）')}>
          {t('余额不足')}
        </Badge>
      )
    }
    if (tokenBlockedOf(account, tokenUsageOf(account))) {
      return (
        <Badge variant='warning' shape='tag'
          title={t('本周期 Token 消耗达到限制器的上限，转发时会跳过该账号（窗口重置自动恢复）')}>
          {t('Token 限额')}
        </Badge>
      )
    }
    return (
      <Badge variant='success' shape='tag' title={t('当前没有任何模型处于限流中')}>
        <BadgeDot />{t('正常')}
      </Badge>
    )
  }
  const soonest = formatResetText(entries[0].resetAt)
  return (
    <>
      <Badge variant='warning' shape='tag' render={<button type='button' />}
        className={open ? 'ring-2 ring-warning-soft' : undefined}
        title={t('点击{action}各模型的限流明细', { action: open ? t('收起') : t('查看') })}
        onClick={() => setPanelOpen(account.id, 'limits', !open)}>
        {t('{n} 个模型 ▾', { n: entries.length })}
      </Badge>
      <span className='lim-sub' title={t('最早恢复')}>
        {t('最早 {soonest} 恢复', { soonest: soonest === RESET_UNKNOWN ? t('待定') : soonest })}
      </span>
    </>
  )
}

/* ─── 有效期 / 连接数 / 余额列 ──────────────── */

/**
 * 有效期：按「这家有没有版本概念」选字段（workbuddy 是 expiresAt，其余是
 * tokenExpiresAt），与域层的 tokenExpiryOf 同口径。文案收短成「30 天后」，
 * 完整句留在 title —— 列宽有限。
 *
 * 读数经域层 `expiryMillis` **先归一到毫秒**再判：落盘的到期值在秒与毫秒之间漂过
 * （Trae 的凭据是从 CPA 的 auth 文件与手工粘贴进来的，那里是 10 位秒；别家与上游
 * 刷新响应都是 13 位毫秒），不归一时 `1791009732` 当毫秒读就是 1970-01-21，账号一进
 * 面板就红着显示「已过期」。归一规则与理由写在 accounts-domain.ts 那一处，
 * 后端 `providers::trae::Credential::expires_at_ms` 是同一个口径。
 */
export function ExpiryCell({ account }: { account: AccountRecord }) {
  const features = providerFeatures(providerOf(account))
  const expiresAt = expiryMillis(features.edition ? account.expiresAt : account[features.expiry])
  if (!expiresAt) return <span className='muted' title={t('记录里没有过期时间')}>—</span>
  const left = expiresAt - Date.now()
  if (left <= 0) return <Badge variant='destructive' shape='tag' title={t('凭证已过期，转发时会先刷新')}>{t('已过期')}</Badge>
  const text = left < 3600e3 ? t('{n} 分钟后', { n: Math.max(1, Math.round(left / 60e3)) })
    : left < 48 * 3600e3 ? t('{n} 小时后', { n: (left / 3600e3).toFixed(1) })
      : t('{n} 天后', { n: Math.floor(left / 24 / 3600e3) })
  // 完整时间点只在解析得出时补进 title：formatTime 对非法时间戳返回空串，
  // 直接拼会留下一个空的「（）」
  const full = formatTime(expiresAt)
  return <span title={full ? t('{text}过期（{full}）', { text, full }) : t('{text}过期', { text })}>{text}</span>
}

/**
 * 连接数：此刻正在使用这个账号的请求数（2 秒一轮的实时计数）。
 * 口径与 OmniProxy 上游管理页的「连接」列一致 —— 有连接时显示数字、为 0 时**什么都不
 * 显示**（留空）。满屏的 0 会把少数几个真正在跑的账号淹没；要看「谁是 0」时空白本身
 * 就是答案。计数缺失（还没拉到、后端不可达）与 0 同样处理：把一个尚未知的值渲染成 0
 * 会读成「这个账号没在用」，而事实可能是「数据还没到」。
 */
export function ConnectionsCell({ account }: { account: AccountRecord }) {
  const value = connectionsOf(account.id)
  if (value <= 0) return null
  return (
    <span className='conn-count' title={t('{n} 个请求正在使用该账号（含还在下发内容的流式请求）', { n: value })}>
      {value}
    </span>
  )
}

/** 数值 → 展示串（与余额列摘要同口径，取不到给「—」） */
function numberText(value: unknown): string {
  if (value === null || value === undefined || value === '') return '—'
  const number = Number(value)
  return Number.isFinite(number) ? String(number) : String(value)
}

/**
 * 订阅信息 → 摘要 title 里的一段文字（统一形状的 `subscription`）。
 * `expireAt` 各家的类型不同（小浣熊给的是上游原样的字符串日期，AutoClaw 可能给时间戳）：
 * 能解析成日期的按本地时间格式化，否则原样显示 —— 不猜、不丢。
 */
function subscriptionText(subscription: unknown): string {
  if (!subscription || typeof subscription !== 'object') return ''
  const info = subscription as Record<string, unknown>
  const parts: string[] = []
  if (info.planName) parts.push(t('套餐 {name}', { name: String(info.planName) }))
  if (info.status) parts.push(t('状态 {status}', { status: String(info.status) }))
  // 三态里的非「生效中」档要说清楚：代表套餐可能是**待生效**的（刚领到还没到生效
  // 时间的活动包，见后端 `plan_state`），只写「套餐 X 到期 …」会被读成现在就在用
  if (info.state && String(info.state) !== 'active') {
    parts.push(zcodePlanStateLabel(info.state))
  }
  const expireAt = info.expireAt
  if (expireAt !== null && expireAt !== undefined && expireAt !== '') {
    const asNumber = Number(expireAt)
    const text = Number.isFinite(asNumber) && asNumber > 1e11 ? formatTime(asNumber) : String(expireAt)
    if (text) parts.push(t('到期 {time}', { time: text }))
  }
  if (Number.isFinite(Number(info.remainQuota))) parts.push(t('余量 {value}', { value: numberText(info.remainQuota) }))
  if (Number.isFinite(Number(info.totalQuota))) parts.push(t('总量 {value}', { value: numberText(info.totalQuota) }))
  return parts.join(' ')
}

/**
 * 读数里**待生效**的套餐（`plans` 数组里 `state === 'pending'` 的那些）。
 * 没有这个数组的家（其余全部）与旧后端一律返回空数组 —— 判据只看字段，不看 provider。
 *
 * 为什么单独把这一档拎出来：活动套餐常常不是立刻生效的（官方客户端显示
 * 「待生效 今天 23:00 · 过期时间 10月12日 09:00」），刚领到的那笔额度还没开始算。
 * 余额列上什么都不显示，用户会以为没领上；一条徽章回答「领到了、还没到生效时间」。
 */
export function pendingPlans(entry: UsageEntry): Array<Record<string, unknown>> {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return []
  const plans = (entry as Record<string, unknown>).plans
  if (!Array.isArray(plans)) return []
  return plans.filter(item => item && typeof item === 'object'
    && String((item as Record<string, unknown>).state || '') === 'pending') as Array<Record<string, unknown>>
}

/**
 * 待生效套餐的悬停说明：逐份列「名字（生效时间）」——时间可能在读数里缺失，那就只说名字。
 *
 * 用后端的 `pendingUntil`（最早一条还没到点的**权益生效时间**，口径见后端
 * `plan_effective_times`）；它缺失时才退回 `startsAt`（那是领取时间，只在没有
 * 权益的套餐上才等于生效时间）。
 */
export function pendingPlansTitle(plans: Array<Record<string, unknown>>): string {
  const lines = plans.map(plan => {
    const name = String(plan.name || plan.planId || t('套餐'))
    const starts = Number(plan.pendingUntil ?? plan.startsAt)
    return Number.isFinite(starts) && starts > 0
      ? t('{name}（{time} 起生效）', { name, time: formatTime(starts * 1000) })
      : name
  })
  return t('已领到、还没到生效时间的套餐（生效后余额列会按它显示）：\n{lines}', { lines: lines.join('\n') })
}

/**
 * 余额结果 → 一行摘要（`{text, kind, title}`）。
 *
 * 形状探测按**字段**而不是按 provider（`totalLeft` 键 = workbuddy 既有形状，否则看
 * `available` / `wallets`）：provider 只决定「谁去查」，不决定「查回来长什么样」。
 * 失败与「未配置」的分流走 usageFailureOf（判据的唯一入口，与查询动作那边的 toast 同源）。
 * 摘要文案刻意压到「数字 + 单位」，完整句（各钱包 / 套餐 / 到期）放进 title ——
 * 余额列是这张表里最窄的几列之一。
 */
export function usageSummary(entry: UsageEntry): { text: string; kind: string; title: string } {
  if (entry === undefined) return { text: t('未查询'), kind: 'muted', title: t('尚未查询该账号的余额') }
  if (entry === null) return { text: t('查询中…'), kind: 'muted', title: t('正在查询') }
  const failure = usageFailureOf(entry)
  if (failure) {
    return failure.notConfigured
      ? { text: t('未配置'), kind: 'muted', title: t('{message}（去该账号的「设置」里填上查询凭证即可）', { message: failure.message }) }
      : { text: t('查询失败'), kind: 'bad', title: failure.message }
  }
  if (typeof entry !== 'object' || entry === null) return { text: t('无数据'), kind: 'muted', title: String(entry) }
  const data = entry as Record<string, unknown>
  if (Object.prototype.hasOwnProperty.call(data, 'totalLeft')) {
    const total = data.unlimited ? '∞' : numberText(data.totalLeft)
    return {
      text: t('可用 {total}', { total }),
      kind: 'ok',
      title: t('总剩余 {total} · 套餐 {plan} · 奖励 {bonus}', {
        total, plan: numberText(data.planLeft), bonus: numberText(data.bonusLeft),
      }),
    }
  }
  if (Object.prototype.hasOwnProperty.call(data, 'available') || Array.isArray(data.wallets)) {
    const unit = String(data.unit || t('积分'))
    const wallets = Array.isArray(data.wallets) ? data.wallets as Array<Record<string, unknown>> : []
    // 上游给的展示串优先（带千分位 / 单位的格式化），没有才按数值拼
    const detail = wallets
      .map(wallet => `${wallet?.displayName || wallet?.type || t('明细')} `
        + `${wallet?.balanceView ? String(wallet.balanceView) : numberText(wallet?.balance)}`)
      .join(' · ')
    const subscription = subscriptionText(data.subscription)
    // ── 没有额度桶时的口径说明（两档，判据都是**结构**不是文案）───────
    // 这种账号（额度记在活动套餐那条通道上）余额列会退回套餐权益：权益里带着
    // 已用/总量就画真条，只有发放额就报「权益 x」不画条 —— 两种情况都要说明，
    // 否则「权益 3亿 token」会被读成「还剩 3 亿」。
    // 另外：额度窗口若是从开放平台监控通道补来的（`walletsFrom`），也要说一句
    // ——那是**另一条通道**的口径（每 N 小时 / 每周的剩余比例），与套餐到期无关。
    const notes: string[] = []
    if (wallets.length === 0 && Array.isArray(data.plans) && data.plans.length > 0) {
      const plan = leadPlan(data)
      notes.push(plan && grantUsage(plan)
        ? t('读数里没有额度桶，这里按套餐权益里的已用 / 总量显示（上游给的数）')
        : t('上游的余额读数里没有这个套餐的额度桶，剩余量读不到；余额列显示的是套餐发放的权益额度，不是剩余量'))
    }
    if (String(data.walletsFrom || '') === 'monitor') {
      notes.push(t('额度窗口读数来自开放平台监控通道（套餐 JWT 那条通道没有额度桶）：读的是每 N 小时 / 每周窗口的剩余比例'))
    }
    // 展示串优先：ZCode 的额度单位是 token（1 亿 = 9 位数字），而余额列只有几十
    // 像素宽 —— 后端因此给了 `availableView`（`1亿 token` 这类紧凑串）。没有这个
    // 字段的家（其余全部）走的仍是「数值 + 单位」那条老路，行为一字未变。
    const available = data.availableView
      ? String(data.availableView)
      : t('可用 {value} {unit}', { value: numberText(data.available), unit })
    // ── 部分失败：一份账读到了、另一份没读到 ────────────────────
    // CodeArts 的余额是**两台网关**（订阅统计 + 福利网关，见后端
    // `providers::codearts::balance` 的模块头），后端把失败的一侧写进
    // `statisticsError` / `benefitError` 而不是整次失败。这时读数是真的、
    // 但**不完整**：显示成一片绿「可用 —」会被读成「额度用完了」，
    // 而实际是那半边根本没读到。判据仍然只在 `usageFailureOf` 那一处
    // （整次失败的入口），这里只补「半次失败」。
    const missing = [
      data.statisticsError ? t('订阅统计未读到：{error}', { error: String(data.statisticsError) }) : '',
      data.benefitError ? t('福利网关未读到：{error}', { error: String(data.benefitError) }) : '',
    ].filter(Boolean)
    // ── 「没有福利」不是「没读到」────────────────────────────────
    // 上游对没有福利池的账号（福利按限时活动下发，Free 账号常常没有）回
    // `4004 benefit not found`，后端把它翻成 `benefitAbsent` 而不是错误。
    // 这里只在中性说明里提一句 —— 不动 kind、不加 ⚠：它回答的是「为什么
    // 这行没有福利读数」，不是一个需要用户去查的问题。
    const absent = data.benefitAbsent
      ? [t('该账号没有福利模型额度（福利按活动下发，不是每个账号都有）')]
      : []
    return {
      text: available + (missing.length ? ' ⚠' : ''),
      kind: missing.length ? 'warn' : 'ok',
      title: [available, detail, subscription, ...notes, ...absent, ...missing].filter(Boolean).join(' · '),
    }
  }
  return { text: t('无数据'), kind: 'muted', title: t('未返回可识别的余额数据') }
}

/**
 * 读数里「代表这个账号」的套餐：生效中优先，其次待生效（与后端 `pick_plan` 同一
 * 优先级 —— 一份刚领到、还没生效的套餐比一份已经不提供权益的旧套餐更该代表这个
 * 账号）。认不出状态的条目（缺失 / 上游新取值）放在最后 —— 有总比没有强。
 */
function leadPlan(data: Record<string, unknown>): Record<string, unknown> | null {
  const plans = Array.isArray(data.plans) ? data.plans as Array<Record<string, unknown>> : []
  const stateOf = (plan: Record<string, unknown>) => String(plan?.state || '')
  return plans.find(plan => stateOf(plan) === 'active')
    || plans.find(plan => stateOf(plan) === 'pending')
    || plans[0]
    || null
}

/**
 * 套餐里最大的一笔**发放额度**（`{units, unit}`；一笔可读的都没有时 null）。
 *
 * 只用来给「没有额度桶」的账号在余额列兜一个读数 —— 那是**权益额度**不是剩余量，
 * 所以调用处把它标成「权益」并且不画进度条（见 `usagePool` 的兜底分支）。
 */
function biggestGrant(plan: Record<string, unknown>): { units: number; unit: string } | null {
  const items = Array.isArray(plan.entitlements) ? plan.entitlements as Array<Record<string, unknown>> : []
  let best: { units: number; unit: string } | null = null
  for (const item of items) {
    const units = Number(item?.grantUnits)
    if (!Number.isFinite(units) || units <= 0) continue
    const unit = String(item?.unitType || '').trim()
    if (!best || units > best.units) best = { units, unit }
  }
  return best
}

/**
 * 套餐权益里**能算比例**的用量（`{used, total, unit}`；给不出时 null）。
 *
 * 上游并不一定给（`used_units` / `total_units` / `remaining_units` 都是可选键，
 * 后端原样带出来，缺失是 null）—— 给了就画一条真进度条，没给就退回「权益额度」
 * 那种干读数（见 `usagePool` 的兜底分支）。取**第一笔总量为正**的权益：
 * 多笔权益各画各的条是明细弹窗的事，这一列只回答「主额度还剩多少」。
 */
function grantUsage(plan: Record<string, unknown>): { used: number; total: number; unit: string } | null {
  const items = Array.isArray(plan.entitlements) ? plan.entitlements as Array<Record<string, unknown>> : []
  for (const item of items) {
    const total = numberOrNull(item?.totalUnits)
    if (total === null || total <= 0) continue
    const used = numberOrNull(item?.usedUnits)
    const remaining = numberOrNull(item?.remainingUnits)
    const usedValue = used === null
      ? (remaining === null ? null : Math.max(0, total - remaining))
      : used
    if (usedValue === null) continue
    return { used: usedValue, total, unit: String(item?.unitType || '').trim() }
  }
  return null
}

/**
 * 余额里的「主额度桶」：界面用它画套餐名与进度条（`UsageCell` 的两行形态）。
 *
 * ── 三档取值（按「能画出什么」排）────────────────────────────
 *   ① **有总量的桶**里取总量最大的那个（现状）；
 *   ② 一个「有总量」的都没有时，取**只有比例**的窗口里剩余最少（最紧张）的那个
 *      —— 监控通道那条链回的常是纯比例（2026-02 起上游大多不再给绝对量，
 *      见后端 `monitor.rs`），而这一列问的是「还够不够用」，最紧张的那个才是答案；
 *   ③ 连比例都没有（活动套餐的额度不落在任何桶里）时，退回套餐的权益：
 *      权益里带着已用/总量就画**真**进度条（读数「剩 x / y」），只有发放额
 *      就只报「权益 x」并且**不画条** —— 那是发放额不是剩余量，画一条满格的
 *      条会被读成「还剩这么多」。
 * 三档都取不到（免费账号没领过）返回 null，那一列显示「无额度」是对的。
 *
 * ── 为什么取「总量最大的那个桶」──────────────────────────────
 * 一个账号常常同时挂着好几份额度：活动发的大额包（ZCode Trust Build 的 1 亿）
 * 与每天续发的小额包（Start Plan 的 300 万）。用户点开这一列想看的是**大额那份**
 * 还剩多少，而「总量最大」正是它 —— 也顺带避开了每日桶在一天之内反复回满
 * 导致进度条乱跳。总量缺一个都不参与这一档的比较（没有总量就画不出「用了多少」），
 * 于是没有结构化验数据的家（其余全部）自然退回上面那套纯读数呈现。
 *
 * ── 缺失一律当「不知道」─────────────────────────────────────
 * `remainingPercent` 缺失时**不画进度条**（但名字与读数照给），
 * 不拿 0 或 100 冒充 —— 一条满格的进度条与一条空进度条读起来是相反的意思，
 * 而它们都可能是错的。
 */
export function usagePool(entry: UsageEntry): { planName: string; text: string; percent: number | null } | null {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null
  const data = entry as Record<string, unknown>
  const wallets = Array.isArray(data.wallets) ? data.wallets as Array<Record<string, unknown>> : []
  let lead: Record<string, unknown> | null = null
  for (const wallet of wallets) {
    const total = Number(wallet?.total)
    if (!Number.isFinite(total) || total <= 0) continue
    if (!lead || total > Number(lead.total)) lead = wallet
  }
  const subscription = (data.subscription && typeof data.subscription === 'object'
    ? data.subscription
    : {}) as Record<string, unknown>
  if (!lead) {
    // ② 只有比例的窗口（监控通道）：取剩余最少的那个
    let tightest: { wallet: Record<string, unknown>; percent: number } | null = null
    for (const wallet of wallets) {
      const percent = numberOrNull(wallet?.remainingPercent)
      if (percent === null) continue
      if (!tightest || percent < tightest.percent) tightest = { wallet, percent }
    }
    if (tightest) {
      const percent = Math.max(0, Math.min(100, tightest.percent))
      // 单元格只要**前半句**：监控通道的展示串是「剩 72% · 10-11 03:00 重置」，
      // 重置时刻那一半在悬停提示里（`usageSummary` 的 detail 用完整串）——
      // 这一格只有几十像素宽，塞进去会把列挤变形。分隔符「 · 」是后端
      // `monitor::wallet_view` 拼的（那两段之间唯一的那个），前面那段就是读数。
      const view = String(tightest.wallet.balanceView || '').split(' · ')[0].trim()
      return {
        planName: String(tightest.wallet.planName || subscription.planName || ''),
        text: view || t('剩余 {percent}%', { percent: Math.round(percent) }),
        percent,
      }
    }
    // ③ 连比例都没有：退回套餐权益（有已用/总量就画真条，只有发放额就只报数）
    const plan = leadPlan(data)
    if (!plan) return null
    const usage = grantUsage(plan)
    const planName = String(plan.name || plan.planId || subscription.planName || '')
    if (usage) {
      const remaining = Math.max(0, usage.total - usage.used)
      return {
        planName,
        text: t('剩 {left} / {total}{unit}', {
          left: compactNumber(remaining),
          total: compactNumber(usage.total),
          unit: usage.unit ? ` ${usage.unit}` : '',
        }),
        percent: Math.max(0, Math.min(100, (remaining / usage.total) * 100)),
      }
    }
    const grant = biggestGrant(plan)
    if (!grant) return null
    return {
      planName,
      text: grant.unit
        ? t('权益 {value} {unit}', { value: compactNumber(grant.units), unit: grant.unit })
        : t('权益 {value}', { value: compactNumber(grant.units) }),
      // 权益额度不是剩余量：不画条（口径见上面那段文档）
      percent: null,
    }
  }
  // 套餐名优先取这个桶自己的（`wallet.planName`），退回账号级那份 ——
  // 桶认不出归属时（上游没给 plan_id）至少还有订阅里的名字可显示
  const planName = String(lead.planName || subscription.planName || '')
  const percent = numberOrNull(lead.remainingPercent)
  return {
    planName,
    text: String(lead.balanceView || numberText(lead.balance)),
    percent: percent === null ? null : Math.max(0, Math.min(100, percent)),
  }
}

/**
 * 数值字段 → number 或 null。
 *
 * 必须显式判 null/undefined/空串：`Number(null)` 是 0（不是 NaN），照直转换会把
 * 「上游没给这个数」变成「这个数是 0」—— 进度条会画成一条空条，与「剩余 0%」
 * 读起来一模一样，而那是两件事（见 `usagePool` 的缺失口径）。
 */
function numberOrNull(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

/* ─── 余额明细弹层（usageDetail 能力位的那几家，现役只有 CodeArts）─── */

/** 明细弹层里的一行读数（`UsageDetailCell` 的渲染输入） */
type UsageDetailRow = {
  name: string
  /** 右侧读数：积分类「剩 x / y」，token 类用后端的 balanceView（口径在后端） */
  view: string
  tone: 'ok' | 'warn' | 'bad' | 'muted'
  /** **剩余**百分比（条画的是剩余，与 usage-pool 同方向）；null = 不画条 */
  percent: number | null
  /** 超额：条画满走红 + 数值变红（上游到达上限后仍继续计数，如实显示） */
  over: boolean
  /** 悬停说明（为什么没条 / 超额口径 / 4004 的含义 —— 界面上不放长句的原因） */
  title?: string
}

/** token 组的平铺行（余额格里直接显示的那两行；比弹层行更紧凑） */
type UsageFlatRow = {
  /** 短标签（余额列只有 132px，「福利模型日额度」放不下，平铺用短名） */
  name: string
  /** 紧凑读数（万/亿缩写；「剩 0 / 1000 万」比 8 位原数好读得多） */
  view: string
  tone: 'ok' | 'warn' | 'bad' | 'muted'
  /** **剩余**百分比（画条用，与积分明细同向）；null = 无真总量，不画条 */
  percent: number | null
  /** 超额：条画满走红 */
  over: boolean
  /** 悬停全文（后端 balanceView + 行的口径说明） */
  title?: string
}

/** 明细弹层的数据形状（`usageDetailOf` 的产物） */
type UsageDetail = {
  credit: UsageDetailRow[]
  /** token 组：**平铺在余额格里**（对话 token / 福利日额度是转发可用的直接标志，
   *  不该折叠进弹层；积分类的构成才是「要看才展开」的） */
  token: UsageFlatRow[]
  /** 可用合计（各积分池剩余之和；读不出数时 null，不画合计行） */
  total: number | null
  unit: string
  /** 套餐徽标文案（试用版 / 免费版 / 原名）与积分重置日期 */
  plan: string
  /** 上游原始套餐名是否 free（`plan-chip.free` 样式类按它判 —— 不能比 plan 展示文案，
   *  后者随界面语言变，见 accounts-domain 的 isFreePlan） */
  free: boolean
  resetDate: string
  /** 半次失败（两台网关有一台没回话）与「没有福利池」的中性说明 */
  missing: string[]
  benefitAbsent: boolean
}

/** units.js 的公开面（本文件只读 formatTokens）：量级词（万 / 亿 与 k / M）已按界面
 *  语言在那边一处处理，这里不再自己拼字面量。局部窄类型 + 转型而不是 declare global
 *  —— wbUnits 是多页共享的桥，各岛各 declare 一份会撞 TS2717（同 report-charts 的处理）。 */
type UnitsBridge = { formatTokens?: (value: unknown) => string }

/**
 * 数值 → 紧凑读数（token 数常是七位八位的整数，余额列只有 132px，读的是量级）：
 * 量级词与分档全交给 units.js 的 formatTokens（简体「354万」、英文「354k」，
 * 设置页那个开关拨一下两处一起变）；拿不到桥时退回裸数字。
 */
function compactNumber(value: number): string {
  const api = (window as unknown as { wbUnits?: UnitsBridge }).wbUnits
  return api?.formatTokens ? api.formatTokens(value) : String(value)
}

/** 后端 displayName → 平铺短标签（映射不到的原样显示，不硬造）。匹配仍按上游给的中文
 *  名称判定（那是数据），输出的短标签才是界面文案，走 t() */
function flatMeterName(name: string): string {
  if (name.includes('福利')) return t('福利日额度')
  if (name.includes('对话')) return t('对话 token')
  return name
}

/**
 * 余额文档 → 明细弹层 + 平铺行的数据。**只做形状归一，不做 provider 判断**（那在能力位）。
 *
 * ── 为什么积分类的读数在前端拼 ─────────────────────────────
 * 后端 `Meter::view()` 对「只有剩余没有总量」的行（赠送积分常这样）会回落成
 * 「已用 0%」—— 那是它的已知边界（见 balance.rs 那边的 TODO 注释）。弹层这边
 * 自行按 creditTotal / creditRemaining 拼「剩 x / y」，把口径收拢在积分字段上，
 * 不依赖那条展示串。
 *
 * ── token 组为什么单独一份紧凑形状 ─────────────────────────
 * 平铺在格子里读，空间只有 132px：短标签 + 万/亿缩写的读数放得下；
 * 后端的 balanceView 是全量串（「剩 0 / 10000000 token」），留给悬停。
 * 条只在**真总量**（allowance > 1，与后端 view() 的占位包口径一致）时画 ——
 * 上游给过 `package_token_amount=1` 的占位形状，照它画条是「永远满格」的假信息。
 *
 * ── 语义色的口径 ───────────────────────────────────────────
 * 条画**剩余**比例（与 usage-pool 同向）：≥20% 绿、<20% 黄（贴底要留意）、
 * 超额红。剩 0 而没有超额的（按需付费积分未开通）是灰 —— 「0 但不是故障」；
 * 无总量不画条（画不出比例就不装作有），上游没给就是没给。
 */
function usageDetailOf(entry: UsageEntry): UsageDetail | null {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null
  const data = entry as Record<string, unknown>
  const wallets = Array.isArray(data.wallets) ? data.wallets as Array<Record<string, unknown>> : []
  if (!wallets.length) return null
  const unit = String(data.unit || t('积分'))
  const credit: UsageDetailRow[] = []
  const token: UsageFlatRow[] = []
  for (const wallet of wallets) {
    const name = String(wallet.displayName || wallet.type || t('明细'))
    const remaining = numberOrNull(wallet.creditRemaining)
    const total = numberOrNull(wallet.creditTotal)
    const view = String(wallet.balanceView || '') || numberText(wallet.balance)
    const usedPercent = numberOrNull(wallet.usedPercent)
    if (remaining !== null) {
      // 积分类（套餐赠送积分口径）：总量缺失时不画条
      const hasTotal = total !== null && total > 0
      const percent = hasTotal ? Math.max(0, Math.min(100, remaining / total * 100)) : null
      credit.push({
        name,
        view: hasTotal
          ? t('剩 {remaining} / {total}', { remaining: numberText(remaining), total: numberText(total) })
          : t('剩 {remaining}', { remaining: numberText(remaining) }),
        tone: remaining <= 0 ? 'muted' : percent !== null && percent < 20 ? 'warn' : 'ok',
        percent,
        over: false,
        title: hasTotal ? undefined : t('上游未给总量，画不出剩余比例'),
      })
      continue
    }
    if (usedPercent !== null || numberOrNull(wallet.allowanceTokens) !== null) {
      // token 类：对话 token / 福利模型日额度 —— 平铺到格子里直接看
      const used = numberOrNull(wallet.usedTokens) ?? 0
      const allowance = numberOrNull(wallet.allowanceTokens)
      const hasTotal = allowance !== null && allowance > 1
      const over = hasTotal && used > allowance
      const unlimited = wallet.unlimited === true
      // 条与积分明细同款：画**剩余**比例，口径自算（used / allowance，与读数同一
      // 来源，不依赖上游的 value 百分比）；无真总量（含占位包 ≤1、无上限）不画 ——
      // 画不出比例就不装作有
      const percent = hasTotal
        ? Math.max(0, Math.min(100, (1 - used / allowance) * 100))
        : null
      const flat = unlimited
        ? t('无上限 · 已用 {used}', { used: compactNumber(used) })
        : hasTotal
          ? t('剩 {remaining} / {total}', {
            remaining: compactNumber(Math.max(0, allowance - used)), total: compactNumber(allowance),
          })
          : used > 0
            ? t('已用 {used}', { used: compactNumber(used) })
            : t('未使用')
      token.push({
        name: flatMeterName(name),
        view: flat,
        tone: over ? 'bad' : !hasTotal && !unlimited ? 'muted'
          : percent !== null && percent < 20 ? 'warn' : 'ok',
        percent,
        over,
        title: [view, unlimited ? t('无上限按日计（上游到达上限后仍继续计数，超额如实显示）') : null]
          .filter(Boolean).join(' · '),
      })
      continue
    }
    // 既无积分也无 token 量（理论不该出现）：退回一句读数，不静默丢行
    token.push({ name, view, tone: 'muted', percent: null, over: false, title: view })
  }
  const subscription = (data.subscription && typeof data.subscription === 'object'
    ? data.subscription : {}) as Record<string, unknown>
  const missing = [
    data.statisticsError ? t('订阅统计未读到：{error}', { error: String(data.statisticsError) }) : '',
    data.benefitError ? t('福利网关未读到：{error}', { error: String(data.benefitError) }) : '',
  ].filter(Boolean)
  return {
    credit,
    token,
    total: numberOrNull(data.available),
    unit,
    plan: planBadgeLabel(subscription.planName),
    free: isFreePlan(subscription.planName),
    resetDate: String(subscription.resetDate || ''),
    missing,
    benefitAbsent: data.benefitAbsent === true,
  }
}

/**
 * 余额列：**只放读数**（不可点）—— 查询按钮住在操作列，这一列纯粹是
 * 「一眼看出还剩多少」。刻意不换成组件库的 Badge：它是读数而不是状态徽章，
 * 样式全在 `.usage-sum` 里（四档语义色：ok / bad / muted / warn —— `warn` 是
 * 「读到了但不完整」那一档，见上面 `usageSummary` 的半次失败分支）。
 *
 * ── 两行形态一：主额度桶（`usagePool`，ZCode 那家）──────────
 * 第一行是套餐名（用户问得最多的一句是「这 1 亿是哪个活动给的」），
 * 第二行是进度条 + 「剩余 / 总量」。进度条画的是**剩余**比例 ——
 * 与相邻的读数同一方向，否则「条快满了」与「剩 8800 万」会互相打架。
 * 完整明细（每个桶、到期、可用合计）仍在悬停提示里，这里只抢最基本的两个问题：
 * 哪个套餐、还剩多少。
 *
 * ── 两行形态二：套餐徽标 + 明细弹层（`usageDetail` 能力位，CodeArts）──
 * 本家的余额是两台网关两组口径的**加总**，一行读数分不出「试用版」与「免费版」
 * 的权益差别。第一行徽标标套餐，第二行「可用 N 积分」可点击，点开逐项读数
 * （积分池 / token 池各一组，见 `UsageDetailCell`）。悬停提示保留 —— 弹层是
 * 「要看构成」的入口，悬停仍是「顺眼一看」的那份摘要。
 */
/**
 * 余额格里 Token 限制器的**第二行读数**：配了 Token 规则的账号显示
 * 「周期 Token 已用 / 上限 · 剩 N 分」。多条规则取「最值得关注的那条」——
 * 已触发的优先，否则取用量比例最高的一条；其余规则在设置弹窗里看全貌。
 */
function prominentTokenLimit(account: AccountRecord): React.ReactNode {
  const readings = tokenUsageOf(account)
  let best: { rule: LimiterRule; reading: TokenReading; ratio: number } | null = null
  for (const rule of tokenRulesOf(account)) {
    if (rule.enabled === false) continue
    const reading = tokenReadingForRule(rule, readings)
    if (!reading) continue
    const ratio = rule.threshold > 0 ? reading.used / rule.threshold : 0
    if (ratio >= 1) {
      best = { rule, reading, ratio }
      break
    }
    if (!best || ratio > best.ratio) best = { rule, reading, ratio }
  }
  if (!best) return null
  const { rule, reading } = best
  const hit = reading.used >= rule.threshold
  const info = tokenWindowInfo(rule)
  const countdown = !hit ? ` · ${tokenCountdownText(info.remainingMs, info.daily)}` : ''
  const scope = info.daily ? t('今日已用') : t('周期 Token')
  const windowText = rule.reset === 'daily' ? t('今天（本地 0 点重置）') : t('{period}窗口', { period: formatIntervalSeconds(rule.period ?? 0) })
  const title = t('限制器：{window}已用 {used} / {threshold} Token', {
    window: windowText,
    used: Math.round(reading.used).toLocaleString('en-US'),
    threshold: Math.round(rule.threshold).toLocaleString('en-US'),
  })
    + (hit ? t('，已达到上限') : t('，{countdown}重置', { countdown: tokenCountdownText(info.remainingMs, info.daily) }))
  return (
    <span className='usage-token-sub' title={title}>
      {scope} <span className={hit ? 'hit' : undefined}>{formatTokenCount(reading.used)}</span>
      {' '}/ {formatTokenCount(rule.threshold)}{countdown}
    </span>
  )
}

export function UsageCell({ account }: { account: AccountRecord }) {
  if (!supportsUsage(account)) {
    return <span className='muted' title={t('该提供商没有余额查询')}>—</span>
  }
  // 读入口走 usageEntryOf（不是裸的 usageEntries().get）：它会作废「比账号记录还旧」
  // 的失败结论，理由与后端快照出口一致
  const entry = usageEntryOf(account)
  const summary = usageSummary(entry)
  // 限制器的跳过徽章：与后端选路过滤同一判据（balanceBlockedOf / tokenBlockedOf），
  // 让「为什么这个账号不接请求」在界面上有处可看。禁用档不标 —— 那一档
  // 状态列的「已禁用」开关就是答案；跳过档账号仍是启用的，不标就看不出。
  const blockedBadge = balanceBlockedOf(account, entry) ? (
    <Badge variant='warning' shape='tag'
      title={t('余额低于限制器的阈值，转发时会跳过该账号（余额回升自动恢复）')}>
      {t('余额不足 · 已跳过')}
    </Badge>
  ) : tokenBlockedOf(account, tokenUsageOf(account)) ? (
    <Badge variant='warning' shape='tag'
      title={t('本周期 Token 消耗达到限制器的上限，转发时会跳过该账号（窗口重置自动恢复）')}>
      {t('Token 限额 · 已跳过')}
    </Badge>
  ) : null
  const tokenSub = prominentTokenLimit(account)
  // 「待生效 N 份」徽章：注意力只在读数可用（ok / warn）时才有意义 —— 查询失败 /
  // 未配置那几档连读数都没有，挂一条「领到了几份」反而更让人糊涂。usageDetail 那一支
  // （CodeArts）没有套餐清单，不画。
  const pending = summary.kind === 'ok' || summary.kind === 'warn' ? pendingPlans(entry) : []
  const pendingBadge = pending.length ? (
    <Badge variant='warning' shape='tag' title={pendingPlansTitle(pending)}>
      {t('待生效 {n} 份', { n: pending.length })}
    </Badge>
  ) : null
  if (supportsUsageDetail(account)) {
    const usable = summary.kind === 'ok' || summary.kind === 'warn'
    const detail = usable ? usageDetailOf(entry) : null
    if (detail) {
      return <UsageDetailCell summary={summary} detail={detail} blockedBadge={blockedBadge} tokenSub={tokenSub} />
    }
    // 还没查到 / 查询失败 / 没拼出明细：维持单行读数的老样子
    return (
      <span className='usage-sum-wrap'>
        <span className={`usage-sum ${summary.kind}`} title={summary.title}>{summary.text}</span>
        {blockedBadge}
        {pendingBadge}
        {tokenSub}
      </span>
    )
  }
  // 失败 / 未配置那些档不画进度条：读数本身就不是「还剩多少」，
  // 给它配个进度条会把一句错误装饰成一条可信的读数
  const pool = summary.kind === 'ok' || summary.kind === 'warn' ? usagePool(entry) : null
  if (!pool) {
    return (
      <span className='usage-sum-wrap'>
        <span className={`usage-sum ${summary.kind}`} title={summary.title}>{summary.text}</span>
        {blockedBadge}
        {pendingBadge}
        {tokenSub}
      </span>
    )
  }
  return (
    <span className='usage-pool' title={summary.title}>
      {pool.planName ? <span className='usage-pool-name'>{pool.planName}</span> : null}
      <span className='usage-pool-line'>
        {pool.percent !== null ? <Progress value={pool.percent} className='usage-pool-bar' /> : null}
        <span className={`usage-pool-view ${summary.kind}`}>{pool.text}</span>
      </span>
      {blockedBadge}
      {pendingBadge}
      {tokenSub}
    </span>
  )
}

/**
 * 余额格的明细形态（`usageDetail` 能力位）。
 *
 * hooks 只住在这个子组件里：`UsageCell` 对失败 / 未查询档有早退分支，
 * 把 useState 留在这里才不会踩「条件渲染后调 hook」的规则。
 *
 * ── 什么平铺、什么进弹层 ──────────────────────────────────
 * **token 组平铺**（对话 token / 福利模型日额度）：它们是「这账号现在能不能跑」
 * 的直接标志——转发的失败多半是这两行的问题，折进弹层等于让人每回都多点一下；
 * **积分类进弹层**（外面只留总额）：积分池的构成（套餐/基础包/赠送）是「对账才看」
 * 的信息，总额 + 弹层足够。弹层走组件库的 Popover（Portal + 贴边翻转都是现成的），
 * 锚点就是那颗积分读数按钮，与 ⋯ 菜单同一个交互语言。
 */
function UsageDetailCell({ summary, detail, blockedBadge, tokenSub }: {
  summary: { text: string; kind: string; title: string }
  detail: UsageDetail
  blockedBadge: React.ReactNode
  /** Token 限制器的周期读数（余额格第二行；配了规则才有） */
  tokenSub: React.ReactNode
}) {
  const [open, setOpen] = React.useState(false)
  const usable = summary.kind === 'ok' || summary.kind === 'warn'
  return (
    <span className={open ? 'usage-cell open' : 'usage-cell'}>
      <span className='usage-plan'>
        {detail.plan
          ? <span className={`plan-chip${detail.free ? ' free' : ''}`}>{detail.plan}</span>
          : null}
        {detail.resetDate
          ? <span className='plan-sub' title={t('套餐积分的重置日期')}>{t('重置 {date}', { date: detail.resetDate })}</span>
          : null}
      </span>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger
          render={<button type='button' className={`usage-toggle ${summary.kind}`}
            title={summary.title} aria-label={t('积分明细')} />}
        >
          <span className='val'>{summary.text}</span>
          <span className='caret' aria-hidden>▾</span>
        </PopoverTrigger>
        <PopoverContent align='end' sideOffset={6} className='usage-pop w-[288px]'>
          {usable && detail.missing.length
            ? detail.missing.map(line => <div key={line} className='usage-pop-missing' title={line}>{line}</div>)
            : null}
          {detail.credit.length ? (
            <>
              <div className='usage-pop-group'>{t('积分')}</div>
              {detail.credit.map(row => <UsageDetailRowView key={row.name} row={row} />)}
            </>
          ) : null}
          {detail.total !== null ? (
            <div className='usage-pop-total'>
              <span className='k'>{t('可用合计')}</span>
              <span className='v'>{numberText(detail.total)} <small>{detail.unit}</small></span>
            </div>
          ) : null}
          {/* 兜底：没有积分类读数的账号（免费版的统计里没有积分类计量表，额度全在
              福利 token 池 —— available 为 null，摘要那行显示「可用 —」）不加这句
              弹层就是一个空框，「弹不出来」的观感比缺数据更让人怀疑是故障 */}
          {!detail.credit.length && detail.total === null ? (
            <div className='usage-pop-empty'
              title={t('该账号的额度只有福利 token 池（见余额格那两行），订阅统计没有积分类计量表')}>
              {t('该账号没有积分类读数（额度只有福利 token 池）')}
            </div>
          ) : null}
        </PopoverContent>
      </Popover>
      {/* token 组平铺（对话 token / 福利模型日额度）：转发能不能跑，看这两行。
          条与积分明细同款（画剩余比例）；无真总量不画 —— 上游没给就不装作有 */}
      {detail.token.map(row => (
        <span className='usage-flat-line' key={row.name} title={row.title}>
          <span className='usage-flat-head'>
            <span className='usage-flat-name'>{row.name}</span>
            <span className={`usage-flat-view ${row.tone}`}>{row.view}</span>
          </span>
          {row.percent !== null ? (
            <span className={`usage-flat-track${row.over ? ' over' : ''}`}>
              <i style={{ width: `${row.percent}%` }} aria-label={t('剩余 {percent}%', { percent: Math.round(row.percent) })} />
            </span>
          ) : null}
        </span>
      ))}
      {detail.benefitAbsent ? (
        <span className='usage-flat-line'
          title={t('福利按限时活动下发，不是每个账号都有 —— 正常状态，不是查询失败')}>
          <span className='usage-flat-name'>{t('福利日额度')}</span>
          <span className='usage-flat-view muted'>{t('无福利额度')}</span>
        </span>
      ) : null}
      {blockedBadge}
      {tokenSub}
    </span>
  )
}

/** 明细弹层里的一行（名称 + 读数 + 可选的剩余进度条；长说明只在悬停里） */
function UsageDetailRowView({ row }: { row: UsageDetailRow }) {
  return (
    <div className='usage-pop-row' title={row.title}>
      <div className='usage-pop-line'>
        <span className='usage-pop-name'>{row.name}</span>
        <span className={`usage-pop-view ${row.tone}`}>{row.view}</span>
      </div>
      {row.percent !== null ? (
        <span className={`usage-pop-track${row.over ? ' over' : ''}`}>
          <i style={{ width: `${row.percent}%` }} aria-label={t('剩余 {percent}%', { percent: Math.round(row.percent) })} />
        </span>
      ) : null}
    </div>
  )
}

/* ─── 账号 / 提供商 / 代理列 ─────────────────── */

/**
 * 账号：第一行名称，第二行邮箱（有才渲染），第三行只在异常时出现（代理不可用原因）。
 *
 * 主名走 displayNameOf 的纯 nameCustom 分流：显式设置过备注名（打标）的账号
 * 备注名恒为主名；未打标的账号维持历史口径 —— 邮箱系三家（Qoder / AutoClaw
 * 国际版 / Accio）邮箱当主名，其余昵称优先。更新前设置的旧备注没有标记，
 * 到设置里把备注名改一次值（同值提交不打标）即生效。
 *
 * 悬停气泡就是这一列的「详细信息」面板：**一行一条**，组件库 Tooltip 即现
 * （原生 title 由浏览器控制出现时机与断行，两样都不合用），带指向箭头 ——
 * 邮箱、标识、备注名（未生效时气泡可查）、上游昵称、更新时间、来源。
 * 标识（UID / userId）与 Token 尾号不上屏也不进气泡：对「这条账号能不能用」
 * 没有信息量（Token 尾号曾试过放在气泡里，用户实测反馈去掉）。
 * 隐藏账号名开关打开时邮箱 / 昵称在气泡里同样打码 ——
 * 不给「悬停一下就绕过打码」的口子。
 */
export function AccountCell({ account, namesHidden }: { account: AccountRecord; namesHidden: boolean }) {
  const ident = identifierOf(account)
  const features = providerFeatures(providerOf(account))
  const name = displayNameOf(account) || t('未命名账号')
  const email = String(account.email || '').trim()
  const nickname = String(account.nickname || '').trim()
  // 记录里原样的备注名（未经 displayNameOf 的兜底链）：未设备注时它建号时就有种子值，
  // 主名被邮箱 / 昵称占着，这里让它在气泡里可查
  const rawName = String(account.name || '').trim()
  const mask = (value: string): string => (namesHidden ? maskName(value) : value)
  const titleLines = [
    email && email !== name ? t('邮箱 {email}', { email: mask(email) }) : '',
    ident ? `${features.identifier} ${ident}` : '',
    !account.nameCustom && rawName && rawName !== email && rawName !== nickname
      ? t('备注名 {name}', { name: mask(rawName) })
      : '',
    nickname && nickname !== name && nickname !== email ? t('昵称 {name}', { name: mask(nickname) }) : '',
    isDesktopAccount(account) ? t('桌面端实时登录态（凭证每次从客户端登录态文件读取）') : '',
    account.updatedAt ? t('更新于 {time}', { time: formatTime(account.updatedAt) }) : '',
    account.source ? t('来源 {source}', { source: account.source === 'imported' ? t('旧数据导入') : t('手动添加') }) : '',
  ].filter(Boolean)

  const showEmail = email && email !== name
  const proxyError = account.proxy?.error
  // 原生 title 的出现时机由浏览器/系统定（悬停约一秒才出，改不了），换成组件库
  // Tooltip：Provider delay=0 悬停即现；Portal 渲染不被表格滚动容器裁剪；
  // 内容一行一个 div（用户要的「一行一个信息」）。
  const nameNode = titleLines.length ? (
    <TooltipProvider delay={0}>
      <Tooltip>
        <TooltipTrigger render={<div className='acct-name' />}>
          <span className='name'>{mask(name)}</span>
        </TooltipTrigger>
        <TooltipContent>
          <TooltipArrow />
          {titleLines.map((line, index) => <div key={index}>{line}</div>)}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  ) : (
    <div className='acct-name'>
      <span className='name'>{mask(name)}</span>
    </div>
  )
  return (
    <>
      {nameNode}
      {showEmail ? (
        <div className='acct-sub'>
          <span className='acct-email' title={t('账号邮箱')}>{mask(email)}</span>
        </div>
      ) : null}
      {proxyError ? (
        <div className='acct-note bad'
          title={t('代理不可用：{error}', { error: proxyError })}>{t('代理不可用：{error}', { error: proxyError })}</div>
      ) : null}
    </>
  )
}

/**
 * 代理：这个账号出网走哪条线路。一格一个下拉，**选中即保存**。
 *
 * 选项**只有**直连 + 「网络代理」页的代理池条目（值是 `pool:<proxyId>`）——
 * 出口统一在那一页配 / 命名 / 测试（Clash 的出口由「同步 Clash Verge」整体
 * 镜像进池），这里只做「选哪一条」。「自定义代理…」是**动作项**（不是一种
 * 配置）—— 选中它打开账号设置弹窗，下拉随即恢复原值（它是受控的，重绘即回原值）。
 * 池列表来自 accounts-data 的模块级缓存（同步读，纯渲染不发请求）；没就绪时
 * 先只有「直连 + 当前值 + 自定义…」，页面的自愈 effect 补拉一次再重画。
 *
 * 存量记录（直接引用 Clash 出口的 `{source:'clash', listenerUid}`、或自定义
 * 形状）在这里显示为**补位项**（带来源前缀），照原样转发；用户在这个下拉里
 * 选一条池条目或切回直连就会改写它。
 */
export function ProxyCell({ account }: { account: AccountRecord }) {
  const proxy = account.proxy
  const source = proxy?.config?.source || proxy?.source
  const label = proxy?.label || (source === 'custom' ? t('自定义代理') : t('已设置'))
  const broken = proxy?.error
  const pool = proxyPoolSnapshot()
  const poolItems = Array.isArray(pool) ? pool : []

  // 当前值映射回下拉的值域：池条目用 `pool:<id>`、其余（Clash 直引 / custom /
  // 坏形状）落到补位项 —— 绝不能回落成「直连」：那会把「配置坏了」显示成「没配」
  let current = ''
  if (source === 'pool' && proxy?.config?.proxyId) current = `${POOL_VALUE_PREFIX}${proxy.config.proxyId}`
  else if (proxy) current = PROXY_CUSTOM_CURRENT

  const items: Array<{ value: string; label: string; disabled?: boolean }> = [{ value: '', label: t('直连') }]
  for (const item of poolItems) {
    // 文案是「名字（协议 主机:端口）」—— 名字是用户在「网络代理」页起的，
    // 地址是后端解析出来的实时值（见 poolItemLabel）。同一格里多条目同名时
    // 地址是唯一能分辨它们的读数，不能只写名字
    items.push({ value: `${POOL_VALUE_PREFIX}${item.id}`, label: poolItemLabel(item) })
  }
  if (pool === null) {
    // 还没读到（首帧 / 自愈 effect 尚未跑完）：给一句「读取中」而不是
    // 「还没有代理」—— 后者会让用户以为池是空的
    items.push({ value: '__hint_pool__', label: t('正在读取代理列表…'), disabled: true })
  } else if (!poolItems.length) {
    // 池为空 / 读取失败各说明一句：后者是故障（页面侧在节流重试），前者是
    // 「还没配」—— 两种都不该静默成「只有直连」
    items.push({
      value: '__hint_pool__',
      label: poolError() ? t('代理列表读取失败（重试中）') : t('还没有代理（去「网络代理」页添加）'),
      disabled: true,
    })
  }
  if (current === PROXY_CUSTOM_CURRENT) {
    // 补位项：不带动任何写操作（PROXY_CUSTOM_CURRENT 在 applyProxyPick 里被忽略），
    // 只是把当前值原样显示出来
    const prefix = source === 'clash' ? t('Clash 出口：') : source === 'custom' ? t('自定义：') : ''
    items.push({ value: PROXY_CUSTOM_CURRENT, label: `${prefix}${label}${broken ? t('（不可用）') : ''}` })
  } else if (source === 'pool' && !poolItems.some(item => `${POOL_VALUE_PREFIX}${item.id}` === current)) {
    // 池引用但条目已不在池里（被删 / Clash 侧删了出口）：补位显示当前值
    items.push({ value: current, label: `${label}${broken ? t('（不可用）') : ''}` })
  }
  items.push({ value: PROXY_CUSTOM_EDIT, label: t('自定义代理…') })

  const title = broken
    ? t('代理不可用：{error}（转发时会回退直连）；选「自定义代理…」去修改', { error: broken })
    : source === 'clash' || source === 'custom'
      ? t('当前：{label}（未经过代理池 —— 可在「网络代理」页把出口同步进池后来这里改选）', { label })
      : t('当前：{label}；选项来自「网络代理」页；「自定义代理…」打开完整设置', {
        label: proxy ? label : t('直连'),
      })
  const selected = items.find(item => item.value === current)

  return (
    <Select value={current} onValueChange={value => void applyProxyPick(account.id, String(value), current)}>
      {/* 代理解析失败时整格标红：旧实现靠 `.cell-proxy.err .select-trigger` 那条 CSS，
          换成组件库的 Select 之后触发器没有 .select-trigger 这个类名（它走 data-slot），
          所以把描边色直接写在工具类上（见最终报告里变死的 CSS） */}
      <SelectTrigger className={cn('w-full', broken && 'border-destructive-bd')}
        title={title} aria-label={t('出网代理')}>
        <SelectValue>{selected?.label || t('直连')}</SelectValue>
      </SelectTrigger>
      <SelectContent>
        {items.map(item => (
          <SelectItem key={item.value} value={item.value} disabled={item.disabled}>{item.label}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

/* ─── 操作列 ────────────────────────────────── */

/**
 * 操作：领套餐 / 余额 / 设置 / ⋯，顺序固定。
 *
 * 顺序按「点的频次」排，按钮的显隐会随账号状态变，但**顺序不跟着变**。
 * 「设为首选」不在这里：它在 ⋯ 菜单的第二项（行上留一颗按钮去重复隔壁优先级列的
 * 信息，代价是操作列多留 50px，而那 50px 全是从账号列挤出来的）。
 *
 * （签到曾是这排的第一颗按钮，已随签到功能整体迁到「签到中心」——
 * checkin-page.tsx 的每日签到卡承接了它的职责，含单账号签到与重签。
 * CodeArts 的「领福利」同一场收敛：入口只留签到中心的「活动福利」卡
 * （领取状态也在那里标记），账号页不再放第二颗一样的按钮。）
 */
export function ActionsCell({ account, atFront }: { account: AccountRecord; atFront: boolean }) {
  const [usageBusy, setUsageBusy] = React.useState(false)
  const canUsage = supportsUsage(account)
  const canClaim = supportsClaim(account)

  return (
    <div className='acct-actions'>
      {canClaim ? (
        // 点开**套餐明细**弹窗（zcode-plans-modal.tsx）：可领取的逐份列出（标「今日已领」、
        // 逐份「领取」），名下的套餐也逐份列出（含**待生效**与已过期）。
        // 按钮**不因「今天领过」置灰**：上游的「已领取过」是按套餐判的，领了 A 之后
        // B 照样能领 —— 还能领哪几份只有弹窗里逐份比对才说得清，所以每次点都是打开它。
        <Button variant='outline' size='xs'
          title={claimedToday(account)
            ? claimDoneTitle(account)
            : t('查看可领取的套餐与名下已有的套餐（每天一期，领取需过一次人机验证）')}
          onClick={() => openZcodePlans(account.id)}>{t('领套餐')}</Button>
      ) : null}
      {canUsage ? (
        <Button variant='outline' size='xs' disabled={usageBusy}
          title={t('查询该账号剩余余额（读数显示在余额列）')}
          onClick={() => {
            setUsageBusy(true)
            void queryUsageOnce(account.id).finally(() => setUsageBusy(false))
          }}>{t('余额')}</Button>
      ) : null}
      <Button variant='outline' size='xs' title={t('备注名 / 启用 / 代理')}
        onClick={() => openSettingsDialog(account.id)}>{t('设置')}</Button>
      <MoreMenu account={account} atFront={atFront} />
    </div>
  )
}

/* ─── ⋯ 菜单 ───────────────────────────────── */

/**
 * ⋯ 菜单（原先是「点击才把 div.more-menu 插进 .cell-actions」的命令式实现，还要自己
 * 量几何决定向上还是向下弹）。现在走组件库的 Popover：锚点、翻转、贴边、点外部关闭、
 * Esc、焦点归位都由 Base UI 的 floating-ui 那层负责，滚动时自动跟位。
 *
 * 菜单按「对转发的影响面」从大到小排：启用/禁用最重，故在最前；「设为首选」只改队列
 * 顺序（不改启用状态），排在它之后；「并发上限」是账号属性（标签里带当前值）；
 * 「删除账号」同样最重，排在最后并加一条分隔线。首尾两项都标 danger：它们会立刻改变
 * 转发可用性。
 * 「刷新 Token」按 `hasRefreshToken` 决定（桌面端账号的记录里不落 refreshToken，
 * 所以这一项对它不出现 —— 手动刷新走的是「不过期就原样返回」的路径）。
 */
export function MoreMenu({ account, atFront }: { account: AccountRecord; atFront: boolean }) {
  const [open, setOpen] = React.useState(false)
  const enabled = isEnabled(account)
  const maxConcurrent = Number(account.maxConcurrent) || 0
  const itemClass = 'w-full justify-start px-2 font-normal'
  const dangerClass = `${itemClass} text-destructive hover:text-destructive`

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant='outline' size='xs' title={t('更多操作')} />}>⋯</PopoverTrigger>
      <PopoverContent align='end' sideOffset={4} className='w-[172px] p-1.5'>
        <div className='flex flex-col gap-0.5'>
          <Button variant='ghost' size='sm' className={dangerClass}
            onClick={() => { setOpen(false); void setAccountEnabled(account.id, !enabled) }}>
            {enabled ? t('禁用') : t('启用')}
          </Button>
          <Button variant='ghost' size='sm' className={itemClass} disabled={atFront}
            title={atFront ? t('已在全局队列第一位') : t('仅将优先级调整到全局第一位，不改变启用状态')}
            onClick={() => { setOpen(false); shared().wbApp?.runAccountAction?.('switch', account.id) }}>
            {t('设为首选')}
          </Button>
          <Button variant='ghost' size='sm' className={itemClass}
            title={t('在账号设置的「基本」分组里设置该账号同时最多处理的请求数')}
            onClick={() => { setOpen(false); openSettingsDialog(account.id) }}>
            {t('并发上限：{value}', { value: maxConcurrent > 0 ? maxConcurrent : t('不限') })}
          </Button>
          {account.hasRefreshToken ? (
            <Button variant='ghost' size='sm' className={itemClass}
              onClick={() => { setOpen(false); shared().wbApp?.runAccountAction?.('refresh', account.id) }}>
              {t('刷新 Token')}
            </Button>
          ) : null}
          <div className='my-1 h-px bg-hairline' />
          <Button variant='ghost' size='sm' className={dangerClass}
            title={isDesktopAccount(account)
              ? t('删除这条账号记录（不会影响客户端自己的登录态；之后可再点「导入桌面端登录态」加回来）')
              : undefined}
            onClick={() => { setOpen(false); shared().wbApp?.runAccountAction?.('remove', account.id) }}>
            {t('删除账号')}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}

/* ─── 展开的明细行（限流）──────────────────── */

/**
 * 限流明细面板：这个账号**当前限流中的模型**逐行列出 —— 模型名、恢复时间、上游给的
 * 原因，以及「清除标记」动作（单条）与「全部清除」。
 *
 * 为什么值得一整块面板：限流是按模型的（一个账号完全可能 A 模型限流、B 模型正常），
 * 把模型名列出来才能回答「到底是谁把我限了」；而「清除标记」是真实动作，悬浮层里
 * 放不下也点不稳。「清除标记」只作用于本机这份冷却标记（下一次请求若上游仍限流会
 * 再次被标记），所以它是安全且可逆的，不需要二次确认。
 */
function LimitPanel({ account, onClose, onClear }: {
  account: AccountRecord
  onClose: () => void
  onClear: (model: string) => void
}) {
  const entries = activeLimits(account)
  const close = (
    <Button variant='ghost' size='icon-xs' className='panel-close' title={t('收起')} onClick={onClose}>✕</Button>
  )
  if (!entries.length) {
    // 已展开但记录恰好全部过期时给一句中性说明 —— 数据是两次读盘之间变了的，
    // 不该渲染成一块空面板
    return <div className='row-panel limit-panel'>{t('当前没有限流中的模型。')}{close}</div>
  }
  return (
    <div className='row-panel limit-panel'>
      {close}
      <div className='lp-head'>
        <b>{displayNameOf(account)}</b>
        <span className='muted'>{t('{n} 个模型限流中 · 记录来自上游 429 / 限额码，到恢复时间自动解除', { n: entries.length })}</span>
        <Button variant='outline' size='xs' title={t('清掉该账号全部模型的限流标记')}
          onClick={() => onClear('')}>{t('全部清除')}</Button>
      </div>
      {entries.map(entry => {
        const reset = formatResetText(entry.resetAt)
        const reason = entry.message || (entry.status ? t('上游返回 {status}', { status: entry.status }) : '')
        return (
          <div className='lp-row' key={entry.model}>
            <span className='lp-model' title={entry.model}>{entry.model}</span>
            <Badge variant='warning' shape='tag'>{t('限流中')}</Badge>
            <span className='lp-reset' title={t('到恢复时间后自动解除，无需手动操作')}>
              {reset === RESET_UNKNOWN ? t('恢复时间未知') : t('{time} 恢复', { time: reset })}
            </span>
            <span className='lp-reason' title={reason}>{reason}</span>
            <Button variant='outline' size='2xs'
              title={t('清掉本机的限流标记，立刻重新尝试该模型（上游若仍在限流会再次被标记）')}
              onClick={() => onClear(entry.model)}>{t('清除标记')}</Button>
          </div>
        )
      })}
    </div>
  )
}

export function PanelsRow({ account, colSpan, limitsOpen, onClear }: {
  account: AccountRecord
  colSpan: number
  /** 展开态由页面从 store 读出来传进来（这一层只负责画） */
  limitsOpen: boolean
  onClear: (id: string, model: string) => void
}) {
  if (!limitsOpen) return null
  return (
    <tr className='acct-panels' data-panels-for={account.id}>
      <td colSpan={colSpan}>
        <LimitPanel account={account} onClose={() => setPanelOpen(account.id, 'limits', false)}
          onClear={model => onClear(account.id, model)} />
      </td>
    </tr>
  )
}

/* ─── 表头 ──────────────────────────────────── */

/**
 * 表头行。**字面量 JSX、顺序固定、不随任何状态变化**：列的显隐与顺序由
 * `wbColSettings.syncStaticHead` 就地重排既有元素（隐藏的列是从 DOM 里摘掉而不是
 * display:none），不能按状态重建 —— `<col>` 上带着列宽拖拽的 inline 宽度，`<th>` 里
 * 插着列宽把手，重建会把两者一起丢掉。React 只在「同一位置、同一类型的子节点」上做
 * 属性 diff，这些 th 的 props 与文本逐字不变，重渲染时一次 DOM 写都不会发生。
 *
 * 各列的对齐类（ta-*）由 syncStaticHead 统一贴（它按用户配置给），所以这里不写 ——
 * 写了反而会与它的结果打架。数据行相反：完全按 visibleColumns() 逐列渲染。
 */
export function TableHead({ namesHidden, allPicked, somePicked, disabled, onToggleAll }: {
  namesHidden: boolean
  allPicked: boolean
  somePicked: boolean
  disabled: boolean
  onToggleAll: (picked: boolean) => void
}) {
  const grip = <span className='col-grip' title={t('拖动调整列宽（双击还原）')} />
  return (
    <thead>
      <tr>
        <th className='cell-pick' data-col='pick'>
          {/* 表头这颗「全选」与批量栏那颗是**同一个选择**（表头入口是表格化之后补的） */}
          <Checkbox id='acct-select-all' checked={allPicked} indeterminate={!allPicked && somePicked}
            disabled={disabled} aria-label={t('全选当前筛选结果')}
            title={t('全选 / 取消全选当前筛选结果（与批量栏同一个选择）')}
            onCheckedChange={next => onToggleAll(next)} />
        </th>
        <th className='cell-order' data-col='order'>
          <span className='th-label' title={t('组内先后：按住左边的把手上下拖动即可调整；整组的先后拖分组带的把手')}>
            {t('顺序')}
          </span>
          {grip}
        </th>
        <th className='cell-account' data-col='account'>
          <span className='th-label'>
            {t('账号')}
            <NameEyeButton hidden={namesHidden} />
          </span>
          {grip}
        </th>
        <th className='cell-proxy' data-col='proxy'>
          <span className='th-label' title={t('该账号出网走的代理（Clash 出口 / 自定义 / 直连）；点击可修改')}>{t('代理')}</span>
          {grip}
        </th>
        <th className='cell-connections' data-col='connections'>
          <span className='th-label' title={t('此刻正在使用这个账号的请求数（含还在下发内容的流式请求）；为 0 时不显示')}>{t('连接数')}</span>
          {grip}
        </th>
        <th className='cell-status' data-col='status'>
          <span className='th-label'>{t('状态')}</span>{grip}
        </th>
        <th className='cell-limits' data-col='limits'>
          <span className='th-label' title={t('该账号当前限流中的模型；点徽章看明细')}>
            {t('限流')}
          </span>
          {grip}
        </th>
        <th className='cell-expiry' data-col='expiry'>
          <span className='th-label'>{t('有效期')}</span>{grip}
        </th>
        <th className='cell-usage' data-col='usage'>
          <span className='th-label'>{t('余额')}</span>{grip}
        </th>
        {/* 最后一列不给把手：它绝对定位在右缘，钉在表格右缘会顶出一条横向滚动条 */}
        <th className='cell-actions' data-col='actions'><span className='th-label'>{t('操作')}</span></th>
      </tr>
    </thead>
  )
}

/**
 * 表头的「隐藏账号名」眼睛（只在账号列出现）：点一下整列名字变星号，
 * 截图 / 演示时不必逐个打码。图标随状态换睁眼 / 闭眼，隐藏生效时常亮主色 ——
 * 「现在处于打码状态」不用悬停就能看出来。
 */
export function NameEyeButton({ hidden }: { hidden: boolean }) {
  const label = hidden ? t('显示账号名') : t('隐藏账号名（名字显示为星号）')
  return (
    <button type='button' className={`name-eye${hidden ? ' on' : ''}`} title={label} aria-label={label}
      aria-pressed={hidden} onClick={() => toggleNamesHidden()}
      dangerouslySetInnerHTML={{ __html: iconHtml(hidden ? 'eyeOff' : 'eye', 13) }} />
  )
}
