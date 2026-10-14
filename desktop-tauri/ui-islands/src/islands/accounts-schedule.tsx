/**
 * 账号页 · 「模型调度」作用域视图（**只读**）—— React 岛。
 *
 * ── 这一版回答什么 ──────────────────────────────────────────
 * 「这个模型的下一个请求会先用谁、后面依次是谁、谁被跳过、为什么被跳过」。
 * 它**什么都不改** —— 可写覆盖（按模型排除账号 / 按模型设策略）刻意留到下一版，
 * 理由见文件末尾「下一版」那一段。
 *
 * ── 排序一行都不自己写（本岛最要紧的一条）────────────────────
 * 行序直接取 `GET /api/routing/queue?model=` 的 `ordered`。那个端点的排序实现
 * 就是转发选路用的**同一个函数**（后端 `routing::plan_route`，见
 * `api/routing_api.rs` 的模块头）—— 于是「界面画的顺序」与「转发实际走的顺序」
 * 不可能分叉。渲染层绝不自己再排一遍：`accounts-domain.accountGroups` 已经是
 * 后端 `priority::grouped_queue_order` 的**第二份同构实现**（那份自带「改一处
 * 必须同步另一处」的欠条），这里不加第三份。
 *
 * ── 厂商 = 分组（文件夹）、账号 = 行（文件）──────────────────
 * 队列里同一家的账号**必然连续** —— 这是账号页整队重排写优先级时保证的不变量
 * （见后端 `priority::grouped_queue_order` 的模块说明），所以「按首次出现归组」
 * 得到的组序就等于真实队列里的组序，不需要任何额外排序。
 *
 * ── 倍率为什么在分组带上、而不是一列 ─────────────────────────
 * 倍率（`credits`）是 (提供商, 模型) 级属性：在「同一个模型」这个作用域下，全表
 * 每一行都是同一个数，做成一列就是 N 行重复同一个数。它属于「这一家在这个模型
 * 上的单价」，位置就是分组带。数据源是 `GET /api/models/manage` 的
 * `models[].credits`，与模型管理页「倍率」列同一份数据、同一段解析口径
 * （`x0.7 credits` → `0.7x`）。
 *
 * ── 桥 ─────────────────────────────────────────────────────
 * `getRoutingQueue` / `getModelManage` 都已由壳提供（`src-tauri/src/bridge.rs`
 * 与 `server/src/web_shim.rs` **两处成对**实现）；本岛只声明自己用到的那几个
 * 成员，不往 Window 上加全局类型（与 accounts-shared 的 SharedWindow 同一手法，
 * 避免同名属性类型不一致撞出 TS2717）。
 */

import * as React from 'react'
import { t } from '../i18n'
import { cn, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { displayNameOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'

/* ─── 桥（只声明用到的成员）────────────────────── */

/** 队列里的一条候选（`api/routing_api.rs` 的 entry_json 形状） */
type QueueEntry = {
  id?: string
  name?: string
  provider?: string
  providerLabel?: string
  /** usable / tried / disabled / cooldown / concurrency / limiter / no-credentials */
  state?: string
  /** 一句「为什么排这 / 为什么跳过」 */
  reason?: string
}

/** `GET /api/routing/queue?model=` 的响应 */
type QueueSnapshot = {
  strategy?: string
  sticky?: string
  model?: string | null
  lastRequestModel?: string | null
  pickedId?: string
  ordered?: QueueEntry[]
  blocked?: QueueEntry[]
}

/** `/api/models/manage` 里我们只读这两样 */
type ManageModel = { id?: string; provider?: string; credits?: unknown }

/** `/api/session` 快照里我们只读这两样（models 是广告中的模型清单） */
type SessionState = {
  models?: Array<{ id?: string; provider?: string; providerLabel?: string }>
  accounts?: { accounts?: AccountRecord[] }
  lastRequestModel?: string | null
}

function bridge(): {
  workbuddyDesktop?: {
    getRoutingQueue?: (model?: string) => Promise<QueueSnapshot | null | undefined>
    getModelManage?: () => Promise<{ models?: ManageModel[] } | null | undefined>
  }
  wbApp?: { getState?: () => SessionState | null | undefined }
} {
  return window as unknown as ReturnType<typeof bridge>
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/* ─── 作用域（全局 / 单模型）的共享状态 ─────────────────
   切换器在工具条里、视图在列表区，是同一棵 React 树上的两个位置；而
   accounts-page 自己也要读它（决定「渲染账号表还是调度视图」），所以状态放在
   模块级、两边都用 useSyncExternalStore 订阅 —— 不把 state 提在 accounts-page
   里再往下传（那会让它多背一份与它无关的状态）。 */

/** 全局作用域：'' —— 「模型名」不会是空串，所以用它做哨兵值 */
export const SCOPE_ALL = ''

let scope = SCOPE_ALL
const scopeListeners = new Set<() => void>()

function subscribeScope(listener: () => void): () => void {
  scopeListeners.add(listener)
  return () => { scopeListeners.delete(listener) }
}

/** getSnapshot 必须返回稳定值：`scope` 是字符串，直接返回它即可 */
function scopeSnapshot(): string {
  return scope
}

/** 当前作用域（'' = 全局账号表）。accounts-page 用它决定渲染哪一份内容 */
export function useScheduleScope(): string {
  return React.useSyncExternalStore(subscribeScope, scopeSnapshot)
}

export function setScheduleScope(model: string): void {
  const next = String(model || '')
  if (scope === next) return
  scope = next
  scopeListeners.forEach(listener => listener())
}

/* ─── 模型清单（作用域选择器的选项）─────────────── */

/**
 * 作用域选项 =「全局」+ 广告中的模型，**按模型名去重**。
 *
 * 必须去重：`/api/session` 的 `models` 是「模型 × 承载家」的清单，同一个模型被
 * 多家同时提供就会出现多条（如 `kimi-k3` 同时挂在 CatPaw 与小浣熊下）。作用域是
 * **模型名**（后端 `plan_route` 的候选收窄走 `route_for_forward(model)`），所以
 * 同名只该有一个选项 —— 承载它的家写进副标题，正好回答「这个模型是谁家在供」。
 */
function scopeOptions(): Array<{ value: string; label: string }> {
  const carriers = new Map<string, string[]>()
  for (const model of bridge().wbApp?.getState?.()?.models || []) {
    const id = String(model?.id || '').trim()
    if (!id) continue
    const label = String(model?.providerLabel || model?.provider || '').trim()
    const list = carriers.get(id) || []
    if (label && !list.includes(label)) list.push(label)
    carriers.set(id, list)
  }
  const options = [{ value: SCOPE_ALL, label: t('全局（账号表）') }]
  for (const [id, providers] of carriers) {
    options.push({ value: id, label: providers.length ? `${id} · ${providers.join(' / ')}` : id })
  }
  return options
}

/**
 * 工具条里的作用域选择器（由 accounts-page 渲染）。
 *
 * 选项在**每次渲染时现算**：`getState()` 是同步取内存快照，而 accounts-page 因
 * 连接数轮询（2 秒一轮）会持续重渲染，清单变化自然跟上 —— 不值得为它再挂一份订阅。
 */
export function ScopeSwitcher() {
  const model = useScheduleScope()
  const options = scopeOptions()
  return (
    <div className='group'>
      <span className='label'>{t('作用域')}</span>
      <Select value={model} onValueChange={value => setScheduleScope(String(value))}>
        <SelectTrigger className='max-w-[320px] min-w-[180px]' aria-label={t('选择调度视图的作用域')}>
          <SelectValue>
            {options.find(item => item.value === model)?.label || t('全局（账号表）')}
          </SelectValue>
        </SelectTrigger>
        <SelectContent>
          {options.map(item => (
            <SelectItem key={item.value || '__all__'} value={item.value}>{item.label}</SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

/* ─── 倍率（分组带上的读数）─────────────────────── */

type RateMap = Map<string, string>

/**
 * `(提供商, 模型名) → 倍率短文案`。
 *
 * 键按小写归一：后端清单里同一家的模型 id 大小写可能不一致（各家目录原样透传），
 * 而作用域传进来的是广告名 —— 不归一就会出现「模型页显示 0.7x、这里显示 —」。
 * 解析口径与模型管理页 `formatCredits` 逐字相同（`x0.7 credits` → `0.7x`），
 * 两处不会对同一个数给出两种写法。
 */
function rateMapOf(view: { models?: ManageModel[] } | null | undefined): RateMap {
  const map: RateMap = new Map()
  for (const model of view?.models || []) {
    const id = String(model?.id || '').trim().toLowerCase()
    const provider = String(model?.provider || '').trim()
    if (!id || !provider) continue
    const text = String(model?.credits ?? '').trim()
    if (!text) continue
    const match = /x\s*([\d.]+)/i.exec(text)
    map.set(`${provider}::${id}`, match ? `${match[1]}x` : text)
  }
  return map
}

/* ─── 状态徽章 ─────────────────────────────────── */

/** 队列状态 → 徽章文案与色调（state 取值见后端 `routing::PlanEntry.state`） */
const STATE_META: Record<string, { label: string; cls: string }> = {
  usable: { label: '可用', cls: 'bg-success-soft text-success' },
  tried: { label: '本轮已试', cls: 'bg-muted text-muted-foreground' },
  disabled: { label: '已禁用', cls: 'bg-muted text-muted-foreground' },
  cooldown: { label: '限流冷却', cls: 'bg-warning-soft text-warning' },
  concurrency: { label: '并发已满', cls: 'bg-muted text-muted-foreground' },
  limiter: { label: '限制器拦下', cls: 'bg-warning-soft text-warning' },
  'no-credentials': { label: '无凭证', cls: 'bg-destructive-soft text-destructive' },
}

function StateBadge({ state }: { state: string | undefined }) {
  const key = String(state || '')
  const meta = STATE_META[key] || { label: key || '—', cls: 'bg-muted text-muted-foreground' }
  return (
    <span className={cn('inline-flex rounded px-1.5 py-0.5 text-[11px] leading-4 whitespace-nowrap', meta.cls)}>
      {t(meta.label)}
    </span>
  )
}

/* ─── 策略 / 粘性的只读文案 ─────────────────────── */

function strategyLabel(value: unknown): string {
  switch (String(value || '')) {
    case 'order': return t('顺序优先')
    case 'rotate': return t('负载均衡')
    default: return String(value || '—')
  }
}

function stickyLabel(value: unknown): string {
  switch (String(value || '')) {
    case 'auto': return t('自动')
    case 'session': return t('会话内')
    case 'turn': return t('每轮')
    case 'off': return t('关闭')
    default: return String(value || '—')
  }
}

/* ─── 按家归堆（厂商 = 分组）───────────────────── */

type EntryGroup = {
  provider: string
  label: string
  entries: QueueEntry[]
}

/**
 * 按**首次出现**归堆 —— 不排序（见文件头：组序就是队列里的组序）。
 * `providers` 的顺序因此原样保留，组内也保持传入顺序。
 */
function groupByProvider(entries: QueueEntry[]): EntryGroup[] {
  const groups: EntryGroup[] = []
  const index = new Map<string, EntryGroup>()
  for (const entry of entries) {
    const provider = String(entry.provider || '')
    let group = index.get(provider)
    if (!group) {
      group = { provider, label: String(entry.providerLabel || provider || '—'), entries: [] }
      index.set(provider, group)
      groups.push(group)
    }
    group.entries.push(entry)
  }
  return groups
}

/* ─── 调度视图（列表区）────────────────────────── */

/**
 * 模型调度视图：整块替换列表区的账号表（由 accounts-page 在作用域非全局时渲染）。
 *
 * 数据一次拉两样：队列（顺序 + 原因）与模型清单（只取倍率）。两样都不依赖账号表
 * 的 store —— 这一块是**只读旁路**，账号表那份状态一律不碰。
 */
export function SchedulePane({ model }: { model: string }) {
  const [queue, setQueue] = React.useState<QueueSnapshot | null>(null)
  const [rates, setRates] = React.useState<RateMap>(() => new Map())
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  /** 手动刷新：改一次它就重跑下面的取数 effect */
  const [reload, setReload] = React.useState(0)

  // 倍率表：与作用域无关，进页面拉一次即可（失败就退化成「—」，不挡住主内容）
  React.useEffect(() => {
    let alive = true
    void Promise.resolve(bridge().workbuddyDesktop?.getModelManage?.())
      .then(view => { if (alive) setRates(rateMapOf(view)) })
      .catch(() => { if (alive) setRates(new Map()) })
    return () => { alive = false }
  }, [])

  React.useEffect(() => {
    let alive = true
    setBusy(true)
    setError('')
    const request = bridge().workbuddyDesktop?.getRoutingQueue?.(model)
    if (!request) {
      setBusy(false)
      setError(t('当前环境不支持读取调度队列'))
      return
    }
    void Promise.resolve(request)
      .then(next => { if (!alive) return; setQueue(next || null); setBusy(false) })
      .catch(err => { if (!alive) return; setError(errorMessage(err)); setBusy(false) })
    return () => { alive = false }
  }, [model, reload])

  /** id → 账号记录：队列条目只有 id，展示名与备注名要从账号表同一份数据里取 */
  const records = React.useMemo(() => {
    const map = new Map<string, AccountRecord>()
    for (const record of bridge().wbApp?.getState?.()?.accounts?.accounts || []) {
      if (record?.id) map.set(String(record.id), record)
    }
    return map
  }, [queue])

  const ordered = queue?.ordered || []
  const blocked = queue?.blocked || []
  const orderedGroups = React.useMemo(() => groupByProvider(ordered), [queue])
  const blockedGroups = React.useMemo(() => groupByProvider(blocked), [queue])

  function displayOf(entry: QueueEntry): string {
    const id = String(entry.id || '')
    const record = id ? records.get(id) : undefined
    if (record) return displayNameOf(record)
    return String(entry.name || id || '—')
  }

  function rateOf(provider: string): string {
    return rates.get(`${provider}::${model.toLowerCase()}`) || '—'
  }

  /** 位次：`ordered` 里的下标（+1）。被跳过者没有位次，拿 '' 让那一列空着 */
  const seatOf = new Map<string, number>()
  ordered.forEach((entry, position) => {
    if (entry.id) seatOf.set(String(entry.id), position + 1)
  })

  const pickedId = String(queue?.pickedId || '')

  return (
    <div className='flex min-h-0 flex-col' data-sched-pane='1'>
      {/* 只读头：当前策略 / 粘性 / 这一份队列是算给哪个模型的 */}
      <div className='flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline bg-surface px-3 py-2 text-xs text-muted-foreground'>
        <span>{t('策略')}<b className='ml-1 text-foreground'>{strategyLabel(queue?.strategy)}</b></span>
        <span>{t('会话粘性')}<b className='ml-1 text-foreground'>{stickyLabel(queue?.sticky)}</b></span>
        <span>{t('模型')}<b className='ml-1 text-foreground'>{model}</b></span>
        <span>{t('队列 {n} 个可用 · 跳过 {m} 个', { n: ordered.length, m: blocked.length })}</span>
        <span className='flex-1' />
        <span className='text-muted-foreground'>{t('只读：这里的顺序就是转发会走的顺序')}</span>
        <button type='button' className='rounded border border-hairline px-2 py-0.5 hover:bg-control-hover'
          onClick={() => setReload(count => count + 1)}>{t('刷新')}</button>
      </div>

      <div className='min-h-0 flex-1 overflow-auto'>
        {busy && !queue ? (
          <div className='p-4 text-sm text-muted-foreground'>{t('正在读取调度队列…')}</div>
        ) : error ? (
          <div className='p-4 text-sm text-destructive'>{error}</div>
        ) : (
          <>
            {/* ── 调度顺序：会被用到的那些 ── */}
            <div className='px-3 pt-3 pb-1 text-xs font-medium text-muted-foreground'>
              {t('调度顺序（从上往下依次尝试）')}
            </div>
            {ordered.length ? orderedGroups.map(group => (
              <ScheduleGroup key={group.provider} group={group} rate={rateOf(group.provider)}
                seatOf={seatOf} displayOf={displayOf} pickedId={pickedId} />
            )) : (
              <div className='px-3 py-2 text-sm text-muted-foreground'>
                {t('这个模型当前没有可用账号（全部被跳过，原因见下）')}
              </div>
            )}

            {/* ── 被跳过者：原因逐条摆出来，用户不用猜 ── */}
            {blocked.length ? (
              <>
                <div className='px-3 pt-4 pb-1 text-xs font-medium text-muted-foreground'>
                  {t('被跳过（{n} 个）', { n: blocked.length })}
                </div>
                {blockedGroups.map(group => (
                  <ScheduleGroup key={group.provider} group={group} rate={rateOf(group.provider)}
                    seatOf={seatOf} displayOf={displayOf} pickedId='' blocked />
                ))}
              </>
            ) : null}

            {queue?.lastRequestModel ? (
              <div className='px-3 py-3 text-xs text-muted-foreground'>
                {t('最近一次请求用的是：{model}', { model: String(queue.lastRequestModel) })}
              </div>
            ) : null}
          </>
        )}
      </div>
    </div>
  )
}

/* ─── 一组（一家）──────────────────────────────── */

/**
 * 分组带 + 这一家的行。分组带上的倍率是「这一家在这个模型上的单价」——
 * 模型级属性，所以只在带上出现一次（理由见文件头）。
 */
function ScheduleGroup({ group, rate, seatOf, displayOf, pickedId, blocked }: {
  group: EntryGroup
  rate: string
  seatOf: Map<string, number>
  displayOf: (entry: QueueEntry) => string
  pickedId: string
  blocked?: boolean
}) {
  return (
    <div className='mb-2 overflow-hidden rounded border border-hairline bg-card' data-sched-group={group.provider}>
      <div className='flex items-center gap-3 border-b border-hairline bg-surface px-3 py-1.5'>
        <span className='text-sm font-medium text-foreground'>{group.label}</span>
        <span className='text-xs text-muted-foreground'>
          {t('{n} 个账号', { n: group.entries.length })}
        </span>
        <span className='flex-1' />
        <span className='text-xs text-muted-foreground' data-sched-rate='1' title={t('这一家在这个模型上的倍率（与模型管理页同一份数据）')}>
          {t('倍率')}<b className='ml-1 text-foreground'>{rate}</b>
        </span>
      </div>
      <table className='w-full table-fixed border-collapse text-sm'>
        <tbody>
          {group.entries.map((entry, index) => {
            const id = String(entry.id || '')
            const seat = seatOf.get(id)
            const picked = !blocked && pickedId !== '' && id === pickedId
            return (
              <tr key={id || `${group.provider}-${index}`} data-sched-row={id}
                className={cn('border-t border-hairline first:border-t-0', picked && 'bg-primary-soft')}>
                <td className='w-12 px-2 py-1.5 text-center text-xs text-muted-foreground'>
                  {seat ? `#${seat}` : '—'}
                </td>
                {/* 宽度写死前两列、把余量留给「原因」：原因句是这一格唯一的长文本，
                    让它吃剩余宽度；账号名列不写宽度会与它平分 —— 名字很短，
                    多出来的宽度只会把原因挤出视野 */}
                <td className='w-56 px-2 py-1.5'>
                  <span className='text-foreground'>{displayOf(entry)}</span>
                  {picked ? (
                    <span className='ml-2 rounded bg-primary-tint px-1.5 py-0.5 text-[11px] text-primary-fg'
                      data-sched-picked='1'>
                      {t('下一次优先用它')}
                    </span>
                  ) : null}
                </td>
                <td className='w-24 px-2 py-1.5'><StateBadge state={entry.state} /></td>
                <td className='px-2 py-1.5 text-xs text-muted-foreground'>{String(entry.reason || '')}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

/*
 * ── 下一版（可写覆盖）为什么先不做 ──────────────────────────
 * 这一版刻意只读，是因为「局部设置」到底该可写什么，答案要先看过真实队列才拍得准：
 *   · 若语义是「这个模型单独一套调度参数（顺序优先 / 轮换）」——落点是全仓库 3 处
 *     构造 `PlanMeta` 的地方（`rotate.rs` 的 `select_target_account` 与
 *     `pick_next_account`、`routing_api.rs` 的队列端点），把
 *     `config::routing_settings()` 换成按模型解析；而轮转游标本来就按请求模型分键
 *     （`rotate_take(keys.request_model(), …)`）、会话粘性本来就是模型感知的，
 *     运行时状态层不需要新造。
 *   · 若语义是「这个模型别用某些账号」——落点在 `plan_route` 的准入循环一处，
 *     改动更小，但语义弱（只能减不能调）。
 * 两者都**必须两处调用点同源**（`pick_next_account` 是 429 顺延那条路，漏改就会
 * 「首轮排除、顺延又塞回来」）。先把只读视图跑起来、确认要哪一样，再动那一步。
 */
