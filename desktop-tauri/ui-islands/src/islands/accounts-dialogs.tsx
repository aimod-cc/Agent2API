/**
 * 账号的两个弹窗：**账号设置**与**批量操作**（替换 ui/account-panel.js + ui/proxy-form.js）。
 *
 * 对外契约：`window.wbAccountPanel = { open, close, openBatch, closeBatch, syncAddProvider,
 * invalidate }`（app.js:623 调 invalidate、app.js:645 调 open）—— 注册在 accounts-data.ts，
 * 这里只提供界面。
 *
 * ── 代理表单为什么也在这里 ────────────────────────────────────
 * 它原先是一个可复用的命令式组件（ui/proxy-form.js：自己 innerHTML 建标记、多实例各用
 * 一套 id 命名空间），两个弹窗共用。迁到 React 之后「多实例 id 串台」这个问题天然消失
 * （每个实例的 DOM 归 React 管），表单状态直接提在弹窗的 state 里，`readProxy` 是一个
 * 纯函数 —— 于是不再需要「组件实例 + create/fill/read」那层协议。
 *
 * ── 优先级唯一约束的作用域是**同 provider 内** ─────────────────
 * 后端 `store_crud` 的 `provider_peers` 只在同一家内校验唯一与号段，于是
 * 「workbuddy 有 P100」与「小浣熊有 P100」合法并存。占用提示必须按同一口径过滤：
 * 跨家比较会把一个完全可以保存的数值报成「已被占用」，用户只能改用大号段。
 */

import * as React from 'react'
import {
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Input,
  Label,
  MultiSelect,
  NavItem,
  RadioGroup,
  RadioGroupItem,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@ui'
import { t } from '../i18n'
import {
  errorMessage, esc, poolItemLabel, shared, toast,
  type AccountRecord, type ClashSnapshot, type LimiterRule, type PoolItem,
} from './accounts-shared'
import {
  displayNameOf, providerFeatures, providerOf, supportsPlanChannel, supportsUsage,
  isOpencodeZenProvider, supportsProxyRotate,
  zcodePlanLabel, zcodePlanOf, ZCODE_PLAN_CODING, ZCODE_PLAN_START,
} from './accounts-domain'
import {
  readUsageChanges, UsageSettingsSection, usageDraftOf, type UsageDraft,
} from './accounts-dialog-usage'
import {
  BalanceQuerySection, balanceFieldConfigured, balanceQueryConfigured, balanceQueryDraftOf,
  balanceSpecOf, readBalanceQueryChanges, type BalanceFieldKey,
} from './balance-query'
import { limiterDraftOf, LimiterSection, readLimiterChanges } from './accounts-dialog-limiter'
import {
  allAccounts, clashOptions, closeDialog, findAccount, getStore, proxyPoolOptions,
} from './accounts-data'

/** 名称长度上限，与后端 custom_providers::MAX_NAME_CHARS 一致（前端先挡一次） */
const MAX_PROVIDER_NAME_CHARS = 64

type AccountLike = AccountRecord

/** 同一家的其它账号：自定义提供商那一段的「名下 N 个账号」按它算（级联删除会一起删） */
function peersOf(account: AccountLike, all: AccountLike[]): AccountLike[] {
  const provider = providerOf(account)
  return all.filter(item => item.id !== account.id && providerOf(item) === provider)
}

/** 账号设置弹窗的分组（左侧导航）。 */
type PaneId = 'basic' | 'credential' | 'query' | 'limiter' | 'network'

/** 并发上限的封顶值（与后端 apply_patch 的 MAX_CONCURRENT_LIMIT 一致） */
const CONC_MAX = 999

/**
 * 轮询档「测试出口」一次最多测几条。
 *
 * 每条候选都是一次真实出网（测试结果会记进那条代理的「上次测试」），几十条的
 * 池全测一遍要几十秒。8 条足够回答「这条轮询配置大致通不通」；要看单条结果，
 * 「网络代理」页每行都有独立的「测试」。
 */
const ROTATE_TEST_LIMIT = 8

function labelOf(account: AccountLike | null | undefined): string {
  // 与账号列同一取名口径（displayNameOf 内含 nameCustom / emailAsName 分流）
  return displayNameOf(account) || account?.id || ''
}

/* ─── 代理表单 ──────────────────────────────── */

export type ProxyPayload =
  | null
  | { source: 'pool'; proxyId: string }
  | { source: 'rotate'; all: boolean; proxyIds?: string[] }
  | { source: 'clash'; listenerUid: string }
  | { source: 'custom'; protocol: 'http' | 'socks5'; host: string; port: number; username: string; password: string }

export type ProxyDraft = {
  mode: 'none' | 'pool' | 'rotate' | 'clash' | 'custom'
  /** 池条目 id（mode === 'pool'） */
  proxyId: string
  /** 轮询档：true = 池里全部启用的出口（"全部"，默认档；false = 只在 rotateIds 里轮） */
  rotateAll: boolean
  /** 轮询档：指定要轮询的池条目 id（rotateAll 为 true 时不用） */
  rotateIds: string[]
  listenerUid: string
  protocol: 'http' | 'socks5'
  host: string
  port: string
  username: string
  password: string
}

/** 账号 proxy 字段 → 表单草稿（形态与后端 workbuddy-proxy.mjs 一致） */
export function draftOfProxy(proxy: AccountRecord['proxy']): ProxyDraft {
  const source = proxy?.config?.source || proxy?.source
  const config = proxy?.config || null
  const rotate = source === 'rotate'
  return {
    mode: source === 'pool' ? 'pool'
      : rotate ? 'rotate'
        : source === 'clash' ? 'clash' : source === 'custom' ? 'custom' : 'none',
    proxyId: source === 'pool' ? String(config?.proxyId || '') : '',
    // `all` 缺键按「全部」读（后端同一个口径：那是这一档的默认语义）
    rotateAll: rotate ? config?.all !== false : true,
    rotateIds: rotate && Array.isArray(config?.proxyIds) ? config.proxyIds.map(String) : [],
    listenerUid: config?.source === 'clash' || source === 'clash' ? String(config?.listenerUid || '') : '',
    protocol: config?.protocol === 'socks5' ? 'socks5' : 'http',
    host: config?.host || '',
    port: config?.port === undefined || config?.port === null ? '' : String(config.port),
    username: config?.username || '',
    password: config?.password || '',
  }
}

/** 草稿 → 接口 payload；非法输入抛出「面向用户」的错误（消息直接进 toast / 弹窗状态行） */
export function readProxyDraft(draft: ProxyDraft): ProxyPayload {
  if (draft.mode === 'none') return null
  if (draft.mode === 'pool') {
    if (!draft.proxyId) throw new Error(t('请先选择已保存的代理'))
    return { source: 'pool', proxyId: draft.proxyId }
  }
  if (draft.mode === 'rotate') {
    // 「全部」不写 proxyIds：池里以后新增的出口自动进轮询（后端归一也是这个形状）
    if (draft.rotateAll) return { source: 'rotate', all: true }
    if (!draft.rotateIds.length) throw new Error(t('轮询至少要选择一条代理（或改选「全部」）'))
    return { source: 'rotate', all: false, proxyIds: [...draft.rotateIds] }
  }
  if (draft.mode === 'clash') {
    if (!draft.listenerUid) throw new Error(t('请先选择 Clash Verge 出口'))
    return { source: 'clash', listenerUid: draft.listenerUid }
  }
  const host = draft.host.trim()
  const port = Number(draft.port)
  if (!host) throw new Error(t('请填写代理主机地址'))
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(t('代理端口必须是 1-65535 的整数'))
  return {
    source: 'custom',
    protocol: draft.protocol === 'socks5' ? 'socks5' : 'http',
    host,
    port,
    username: draft.username.trim(),
    password: draft.password,
  }
}

function clashOptionLabel(option: NonNullable<ClashSnapshot['options']>[number]): string {
  const inactive = option.profileActive === false ? t('（其他订阅，可能未生效）') : ''
  const disabled = option.enabled === false ? t('（已在 Clash 中禁用）') : ''
  return `${option.name} :${option.port}${inactive}${disabled}`
}

/**
 * 出网代理表单（账号设置与批量改代理共用）。刻意**不带** `data-island-input`：
 * 那是输入框岛（就地升级）的钩子，两个岛同时挂一个输入框会打架。
 *
 * 档位：
 *   · 无代理（直连）/ 自定义 —— 与改造前一致；
 *   · **已保存的代理** —— 引用「网络代理」页的池条目（推荐路径：出口在那里
 *     配一次、测一次，所有引用它的账号一起生效）。选项文案是
 *     「名字（协议 主机:端口）」，与账号页代理列同一格式（见 poolItemLabel）；
 *   · **轮询** —— 逐请求轮换池里的出口（换 IP）。**只在 `allowRotate` 为真时
 *     出现**（OpenCode Zen 免费档，判据见 accounts-domain 的 supportsProxyRotate
 *     —— 那个上游按 IP 限流）。默认「全部代理节点」（池里每个启用的出口轮流
 *     使用），取消勾选后从池里多选出要轮询的几条；
 *   · **Clash Verge** —— **只在账号当前就是直接引用 Clash 出口时出现**：
 *     那种存量记录要能改（切到别的档就消失，因为出口统一走池之后不再提供
 *     「新建一条 Clash 直引」的入口；想引用 Clash 出口请先到「网络代理」页
 *     点「同步 Clash Verge」，再从上面那一档选）。
 */
export function ProxyForm({
  draft,
  onChange,
  idPrefix,
  allowRotate = false,
}: {
  draft: ProxyDraft
  onChange: (next: ProxyDraft) => void
  idPrefix: string
  /** 是否给出「轮询」档（只有 OpenCode Zen 账号为真，见函数说明） */
  allowRotate?: boolean
}) {
  const store = getStore()
  const clash = store.clash
  const options = Array.isArray(clash?.options) ? clash.options : []
  /** Clash 直引档的可见性：见函数说明（存量记录专用） */
  const showClashMode = draft.mode === 'clash'
  const [testing, setTesting] = React.useState(false)
  const [testResult, setTestResult] = React.useState<React.ReactNode>(null)
  /** 池列表：「已保存的代理」与「轮询」两档共用的选项（模块级缓存，见 accounts-data） */
  const [pool, setPool] = React.useState<PoolItem[] | null>(null)
  const [poolError, setPoolError] = React.useState('')
  const set = (patch: Partial<ProxyDraft>): void => onChange({ ...draft, ...patch })

  // 出口列表只在 Clash 直引档出现时才需要（存量记录的下拉）—— 别的档不为它
  // 打这次请求
  React.useEffect(() => {
    if (showClashMode) void clashOptions().catch(() => { /* 画成「不可用」那一支 */ })
  }, [showClashMode])

  /** 读一次代理池（表单里两个入口：首次挂载与「重新读取」） */
  const loadPool = React.useCallback((force = false): void => {
    setPoolError('')
    void proxyPoolOptions({ force })
      .then(items => setPool(items))
      .catch(error => {
        setPool([])
        setPoolError(errorMessage(error))
      })
  }, [])
  // 只有选中（或初始就是）引用池的那两档时才真正去读 —— 其余档的用户不必为一次
  // 多余的请求买单；读完缓存住，切到这一档不会再打网络
  React.useEffect(() => {
    if ((draft.mode === 'pool' || draft.mode === 'rotate') && pool === null) loadPool()
  }, [draft.mode, pool, loadPool])

  /**
   * 轮询档当前会用到哪些条目（与后端 `proxy_rotate::candidate_ids` 同口径：
   * 「全部」= 池里启用的条目，顺序即池序；指定档 = 表单里选的那些）。
   */
  function rotateCandidateIds(): string[] {
    if (draft.rotateAll) {
      return (pool ?? []).filter(item => item.enabled !== false).map(item => item.id)
    }
    return [...draft.rotateIds]
  }

  /** 用当前表单内容测试出口连通性 */
  async function test(): Promise<void> {
    if (testing) return
    // 轮询档：逐条测候选（每条都走 testProxyPoolItem —— 结果会记进那条代理的
    // 「上次测试」，与代理页那颗按钮同一个动作）。候选可能几十条，全测一遍是
    // 几十次往返，所以只测前 ROTATE_TEST_LIMIT 条并把「还有几条没测」说出来；
    // 只为看某一条通不通时，用「网络代理」页的行内「测试」更直接。
    if (draft.mode === 'rotate') {
      const ids = rotateCandidateIds()
      if (!ids.length) {
        toast(t('轮询至少要选择一条代理（或改选「全部」）'), 'err')
        return
      }
      const targets = ids.slice(0, ROTATE_TEST_LIMIT)
      setTesting(true)
      setTestResult(t('正在连接上游…'))
      let ok = 0
      const failures: string[] = []
      for (const id of targets) {
        const name = pool?.find(item => item.id === id)?.name || id
        try {
          const data = await shared().workbuddyDesktop?.testProxyPoolItem?.(id)
          if (data?.success) ok += 1
          else failures.push(`${name}（${data?.error || t('连接失败')}）`)
        } catch (error) {
          failures.push(`${name}（${errorMessage(error)}）`)
        }
      }
      const rest = ids.length - targets.length
      const summary = t('✅ {ok}/{total} 个出口可用', { ok, total: targets.length })
        + (rest > 0 ? t('（只测了前 {limit} 个，还有 {rest} 个未测）', { limit: ROTATE_TEST_LIMIT, rest }) : '')
      setTestResult(
        <span className={failures.length ? 'text-destructive' : 'text-success'}>
          {summary}{failures.length ? `　${esc(failures.join(t('；')))}` : ''}
        </span>,
      )
      setTesting(false)
      return
    }
    // 池引用：测的是**那条已保存的条目**（testProxyPoolItem 会把结果记进它的
    // 「上次测试」，与代理页那一颗按钮同一个动作）—— 未选条目时给出提示
    if (draft.mode === 'pool') {
      if (!draft.proxyId) {
        toast(t('请先选择已保存的代理'), 'err')
        return
      }
      setTesting(true)
      setTestResult(t('正在连接上游…'))
      try {
        const data = await shared().workbuddyDesktop?.testProxyPoolItem?.(draft.proxyId)
        setTestResult(data?.success
          ? (
              <span className='text-success'>
                {t('✅ 出口可用')}
                {data.ip ? t('　出口 IP {ip}', { ip: esc(data.ip) }) : ''}
                {data.durationMs !== undefined && data.durationMs !== '' ? `　${esc(String(data.durationMs))}ms` : ''}
              </span>
            )
          : <span className='text-destructive'>❌ {esc(data?.error || t('连接失败'))}</span>)
      } catch (error) {
        setTestResult(<span className='text-destructive'>❌ {esc(errorMessage(error))}</span>)
      } finally {
        setTesting(false)
      }
      return
    }
    let proxy: ProxyPayload
    try {
      proxy = readProxyDraft(draft)
    } catch (error) {
      toast(errorMessage(error), 'err')
      return
    }
    setTesting(true)
    setTestResult(t('正在连接上游…'))
    try {
      const data = await shared().workbuddyDesktop?.testProxy?.({ proxy })
      if (data?.success) {
        setTestResult(
          <span className='text-success'>
            {t('✅ 出口可用')}
            {data.status !== undefined && data.status !== '' ? `　HTTP ${esc(String(data.status))}` : ''}
            {data.ip ? t('　出口 IP {ip}', { ip: esc(data.ip) }) : ''}
            {data.durationMs !== undefined && data.durationMs !== '' ? `　${esc(String(data.durationMs))}ms` : ''}
          </span>,
        )
      } else {
        setTestResult(<span className='text-destructive'>❌ {esc(data?.error || t('连接失败'))}</span>)
      }
    } catch (error) {
      setTestResult(<span className='text-destructive'>❌ {esc(errorMessage(error))}</span>)
    } finally {
      setTesting(false)
    }
  }

  return (
    <div className='flex flex-col'>
      <RadioGroup value={draft.mode} onValueChange={value => set({ mode: value as ProxyDraft['mode'] })}
        className='flex-row flex-wrap items-center gap-5' aria-label={t('代理方式')}>
        <Label className='inline-flex cursor-pointer items-center gap-2 font-normal'>
          <RadioGroupItem value='none' />{t('无代理（直连）')}
        </Label>
        <Label className='inline-flex cursor-pointer items-center gap-2 font-normal'>
          <RadioGroupItem value='pool' />{t('已保存的代理')}
        </Label>
        {/* 轮询档只对 OpenCode Zen 出现（见 allowRotate 的说明） */}
        {allowRotate ? (
          <Label className='inline-flex cursor-pointer items-center gap-2 font-normal'
            title={t('逐请求轮换出口（换 IP）：OpenCode Zen 免费档按 IP 限流，单个 IP 的额度用完会对它一律 429')}>
            <RadioGroupItem value='rotate' />{t('轮询（换 IP）')}
          </Label>
        ) : null}
        {/* Clash 直引档只对**已经是这种配置**的账号出现（见函数说明）：
            出口统一走代理池之后不再提供新建入口 */}
        {showClashMode ? (
          <Label className='inline-flex cursor-pointer items-center gap-2 font-normal'>
            <RadioGroupItem value='clash' />Clash Verge
          </Label>
        ) : null}
        <Label className='inline-flex cursor-pointer items-center gap-2 font-normal'>
          <RadioGroupItem value='custom' />{t('自定义')}
        </Label>
      </RadioGroup>

      {draft.mode === 'pool' ? (
        <div className='mt-3'>
          <div className='field-row'>
            <label htmlFor={`${idPrefix}-pool-exit`}>{t('代理')}</label>
            {/* value 恒为字符串（空串 = 还没选）：受控值从 undefined 切到字符串会被 Base UI
                当成「非受控 → 受控」的切换，所以不给 undefined */}
            <Select value={draft.proxyId} disabled={!pool?.length}
              onValueChange={value => set({ proxyId: String(value) })}>
              <SelectTrigger id={`${idPrefix}-pool-exit`} className='min-w-[220px]'>
                <SelectValue>
                  {pool?.find(item => item.id === draft.proxyId)
                    ? poolItemLabel(pool.find(item => item.id === draft.proxyId)!)
                    : (pool === null ? t('正在读取…') : pool.length ? t('请选择代理') : t('还没有保存的代理'))}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {(pool ?? []).map(item => (
                  <SelectItem key={item.id} value={item.id}>
                    {poolItemLabel(item)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button variant='outline' size='sm' onClick={() => loadPool(true)}>{t('重新读取')}</Button>
          </div>
          <div className='detail mt-1.5'>
            {poolError
              ? t('读取失败：{error}', { error: poolError })
              : pool === null
                ? t('正在读取代理列表…')
                : pool.length
                  ? t('地址与端口由「网络代理」页管理：那边改一次、测一次，所有引用它的账号一起生效')
                  : t('「网络代理」页还没有出口 —— 去那里新增，或在「订阅管理」里点「同步 Clash Verge」把 Clash 的出口导进来')}
          </div>
        </div>
      ) : null}

      {draft.mode === 'rotate' ? (
        <div className='mt-3'>
          {/* 「全部」是默认档：勾上 = 池里每个启用的出口轮流使用（新加的出口
              自动进轮询，不必回来改这条账号）。取消勾选才出现多选。
              这一行刻意**不走 .field-row**：账号设置弹窗里它的直接 label 子元素
              会被 `.acct-dlg-main .field-row > label` 强制成 76px 定宽字段标签，
              「全部代理节点（推荐）」比 76px 长，nowrap 溢出会压住后面的说明
              （实测叠字）—— 所以用普通 flex 行，文案跟在复选框后面自然排 */}
          <div className='flex flex-wrap items-center gap-x-2.5 gap-y-1'>
            <Checkbox id={`${idPrefix}-rotate-all`} checked={draft.rotateAll}
              aria-label={t('轮询全部代理节点')}
              onCheckedChange={value => set({ rotateAll: value === true })} />
            <label htmlFor={`${idPrefix}-rotate-all`} className='cursor-pointer text-xs text-subtle'>
              {t('全部代理节点（推荐）')}
            </label>
            <span className='detail'>
              {t('「网络代理」页里每个启用的出口轮流使用；之后新加的出口自动加入')}
            </span>
          </div>
          {!draft.rotateAll ? (
            <div className='field-row mt-2.5'>
              <label htmlFor={`${idPrefix}-rotate-exits`}>{t('轮询的代理')}</label>
              <MultiSelect id={`${idPrefix}-rotate-exits`}
                value={draft.rotateIds}
                onValueChange={values => set({ rotateIds: values.map(String) })}
                options={(pool ?? []).map(item => ({ value: item.id, label: poolItemLabel(item) }))}
                placeholder={t('选择要轮询的代理（可多选）')}
                aria-label={t('轮询的代理')} />
              <Button variant='outline' size='sm' onClick={() => loadPool(true)}>{t('重新读取')}</Button>
            </div>
          ) : null}
          <div className='detail mt-1.5'>
            {poolError
              ? t('读取失败：{error}', { error: poolError })
              : pool === null
                ? t('正在读取代理列表…')
                : pool.length
                  ? t('每条请求换一个出口（按顺序轮流），上游看到的 IP 因此是变化的；出口的地址与端口由「网络代理」页管理')
                  : t('「网络代理」页还没有出口 —— 去那里新增，或在「订阅管理」里点「同步 Clash Verge」把 Clash 的出口导进来')}
          </div>
        </div>
      ) : null}

      {draft.mode === 'clash' ? (
        <div className='mt-3'>
          <div className='field-row'>
            <label htmlFor={`${idPrefix}-clash-exit`}>{t('出口')}</label>
            {/* value 恒为字符串（空串 = 还没选）：受控值从 undefined 切到字符串会被 Base UI
                当成「非受控 → 受控」的切换，所以不给 undefined */}
            <Select value={draft.listenerUid} disabled={!options.length}
              onValueChange={value => set({ listenerUid: String(value) })}>
              <SelectTrigger id={`${idPrefix}-clash-exit`} className='min-w-[220px]'>
                <SelectValue>
                  {options.find(option => String(option.uid) === draft.listenerUid)
                    ? clashOptionLabel(options.find(option => String(option.uid) === draft.listenerUid)!)
                    : (clash?.available === false ? t('未检测到 Clash Verge') : options.length ? t('请选择出口') : t('没有可用出口'))}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {options.map(option => (
                  <SelectItem key={String(option.uid)} value={String(option.uid)}>
                    {clashOptionLabel(option)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button variant='outline' size='sm' onClick={() => {
              void clashOptions({ force: true })
                .then(() => toast(t('✅ 已重新读取 Clash Verge 配置')))
                .catch(error => toast(t('读取失败：{error}', { error: errorMessage(error) }), 'err'))
            }}>{t('重新读取')}</Button>
          </div>
          <div className='detail mt-1.5'>
            {clash === null
              ? t('正在读取 Clash Verge 配置…')
              : clash.available === false
                ? (clash.error ? t('不可用：{error}', { error: clash.error }) : t('未检测到 Clash Verge 配置'))
                : options.length
                  ? t('读取自 {dir}；端口由 Clash Verge 管理，这里实时同步', { dir: clash.dir || 'Clash Verge' })
                  : t('Clash Verge 里还没有配置混合监听器或节点端口')}
          </div>
          <p className='detail mt-1.5'>
            {t('这是既有配置（直接引用 Clash 出口，不经过代理池）。推荐改用上面的')}
            <b>{t('「已保存的代理」')}</b>
            {t('：到「网络代理」页的「订阅管理」里点「同步 Clash Verge」把出口导进池后即可选 —— 那样能集中测试、也能被多个账号共用。')}
          </p>
        </div>
      ) : null}

      {draft.mode === 'custom' ? (
        <div className='mt-3'>
          <div className='field-row'>
            <label htmlFor={`${idPrefix}-protocol`}>{t('协议')}</label>
            <Select value={draft.protocol} onValueChange={value => set({ protocol: value as 'http' | 'socks5' })}>
              <SelectTrigger id={`${idPrefix}-protocol`} className='min-w-[110px]'>
                <SelectValue>{draft.protocol === 'socks5' ? 'SOCKS5' : 'HTTP'}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='http'>HTTP</SelectItem>
                <SelectItem value='socks5'>SOCKS5</SelectItem>
              </SelectContent>
            </Select>
            <label htmlFor={`${idPrefix}-host`} className='ml-2'>{t('主机')}</label>
            <Input id={`${idPrefix}-host`} className='min-w-[140px]' placeholder='127.0.0.1'
              value={draft.host} onChange={event => set({ host: event.currentTarget.value })} />
            <label htmlFor={`${idPrefix}-port`}>{t('端口')}</label>
            <Input id={`${idPrefix}-port`} type='number' min={1} max={65535} className='max-w-[110px]'
              placeholder='7890' value={draft.port} onChange={event => set({ port: event.currentTarget.value })} />
          </div>
          <div className='field-row mt-2.5'>
            <label htmlFor={`${idPrefix}-user`}>{t('用户名')}</label>
            <Input id={`${idPrefix}-user`} placeholder={t('可选')} autoComplete='off'
              value={draft.username} onChange={event => set({ username: event.currentTarget.value })} />
            <label htmlFor={`${idPrefix}-pass`}>{t('密码')}</label>
            <Input id={`${idPrefix}-pass`} type='password' placeholder={t('可选')} autoComplete='new-password'
              value={draft.password} onChange={event => set({ password: event.currentTarget.value })} />
          </div>
        </div>
      ) : null}

      <div className='field-row mt-3'>
        <Button variant='outline' size='sm' disabled={testing} onClick={() => void test()}>
          {testing ? t('测试中…') : t('测试出口')}
        </Button>
        <span className='detail'>{testResult}</span>
      </div>
    </div>
  )
}

/* ─── 账号设置弹窗 ───────────────────────────── */

/**
 * 「使用哪个套餐」：**只有 ZCode 账号有这一项**（见 accounts-domain 的
 * `supportsPlanChannel`），因此按能力位条件渲染。
 *
 * 两条通道是**两份独立的额度**，上游对「套餐已到期」的拒绝只由其中一条给出，
 * 所以这里必须由用户明确指定走哪条（详见后端 `providers::zcode::plan` 的模块头）。
 * 没有套餐登录态（jwt）时活动套餐那一项**仍可选中**但带一句说明 —— 禁用选项
 * 会让用户以为「这个功能不存在」，而实际问题是他需要重新登录一次。
 */
function PlanChannelField({
  plan, hasJwt, onChange,
}: {
  plan: string
  hasJwt: boolean
  onChange: (next: string) => void
}) {
  const current = zcodePlanLabel(plan)
  const [pool, setPool] = React.useState<{ ready?: number; target?: number } | null>(null)
  // 只在真的要用活动套餐时才盯着令牌池：编码套餐那条路不需要验证码令牌。
  // 5 秒一轮（与后台铸造器同一量级），弹窗关掉就停
  React.useEffect(() => {
    if (plan !== ZCODE_PLAN_START) return () => { /* 不需要轮询 */ }
    let alive = true
    const tick = () => {
      void Promise.resolve(shared().workbuddyDesktop?.zcodeCaptchaStats?.())
        .then(stats => { if (alive && stats) setPool({ ready: Number(stats.ready) || 0, target: Number(stats.target) || 0 }) })
        .catch(() => { /* 桥不可用时不给状态，不报错 */ })
    }
    tick()
    const timer = window.setInterval(tick, 5000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [plan])
  return (
    <div className='field-row mt-2.5'>
      <label htmlFor='account-plan-channel'>{t('使用套餐')}</label>
      <Select value={plan} onValueChange={value => onChange(String(value))}>
        <SelectTrigger id='account-plan-channel' className='min-w-[220px]'>
          {/* 显式传当前项的展示文案（不依赖 value 自动显示，见工程约定） */}
          <SelectValue>{current}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ZCODE_PLAN_CODING}>
            {t('编码套餐（Coding Plan）— 走开放平台端点，消耗开放平台的额度（订阅套餐 / 资源包 / 充值余额）')}
          </SelectItem>
          <SelectItem value={ZCODE_PLAN_START}>
            {t('套餐池（Start Plan）— 走 zcode.z.ai 端点，消耗 ZCode 套餐池的额度（新用户体验额度 / 活动领取）')}
          </SelectItem>
        </SelectContent>
      </Select>
      {plan === ZCODE_PLAN_START && !hasJwt ? (
        <span className='detail text-destructive'>
          {t('该账号没有套餐登录态（jwt），套餐池通道会失败：请重新登录该账号')}
        </span>
      ) : null}
      {plan === ZCODE_PLAN_START ? (
        <span className='detail' title={t('套餐池的推理端点要求每条请求带一个阿里云验证码令牌；令牌由本应用在后台静默铸造，界面关闭时无法铸造')}>
          {t('验证码令牌：{counts}', { counts: pool ? `${pool.ready} / ${pool.target || 3}` : t('读取中…') })}
          {pool && pool.ready === 0 ? t('（库存为空，正在补；补不上时转发会失败）') : ''}
        </span>
      ) : null}
    </div>
  )
}

/** 余额凭证行：**只有 CatPaw 账号有这一项**，所以按 provider 条件渲染 */
function BalanceTokenField({
  configured, value, onChange, onClear, clearBusy,
}: {
  configured: boolean
  value: string
  onChange: (next: string) => void
  onClear: () => void
  clearBusy: boolean
}) {
  return (
    <div className='field-row mt-2.5'>
      <label htmlFor='account-balance-token-input'>{t('余额查询凭证')}</label>
      <Input id='account-balance-token-input' className='min-w-[220px]'
        placeholder={configured ? t('已配置，留空则不修改') : t('一般不用填')}
        title={t('积分查询已复用转发用的登录凭证，这里通常留空即可。只有旧版本填过、或从旧代理导入过凭证时才有值')}
        value={value} onChange={event => onChange(event.currentTarget.value)} />
      {configured ? (
        <Button variant='outline' size='sm' disabled={clearBusy} onClick={onClear}
          title={t('清除已配置的余额查询凭证')}>{t('清除')}</Button>
      ) : null}
    </div>
  )
}

/**
 * 自定义账号的**凭证段**（API Key / 无需鉴权）：只有自定义家渲染（内置八家的
 * 凭证各有自己的登录链路，改不了也不该在这儿改）。
 *
 * ── 为什么这里能改 Key（添加表单之外唯一的入口）──────────────
 * 添加表单只在「加账号」那一刻经过；建完账号后要换一把 Key、或者当初忘了勾
 * 「无需鉴权」（账号因此被选路跳过），都必须有地方改。改凭证**不动账号 id**
 * （id 是身份，见后端 `update_custom_credentials`），所以改完仍是同一条账号。
 *
 * ── 两个字段互斥（与后端同一不变量）─────────────────────────
 * 勾了「无需鉴权」就没有 Key 可填（输入框禁用、保存时后端也会把 key 清掉）；
 * 填了 Key 就自动摘掉「无需鉴权」。界面上不做花哨的联动提示，只把禁用的输入框
 * 与一句说明摆在那里 —— 两件事同时成立本来就是矛盾的输入。
 */
function CustomCredentialFields({
  hasApiKey, noAuth, value, onValue,
  onToggleNoAuth, onClear, clearBusy,
}: {
  hasApiKey: boolean
  noAuth: boolean
  value: string
  onValue: (next: string) => void
  onToggleNoAuth: (next: boolean) => void
  onClear: () => void
  clearBusy: boolean
}) {
  return (
    <>
      <div className='field-row mt-2.5'>
        <label htmlFor='account-api-key-input'>{t('API Key')}</label>
        <Input id='account-api-key-input' type='password' autoComplete='new-password'
          className='min-w-[220px]' disabled={noAuth}
          placeholder={hasApiKey ? t('已配置，留空则不修改') : 'sk-…'}
          title={t('上游的 API Key。留空 = 不修改（改凭证不会换账号 id）；要清掉已配的凭证用右边的「清除」')}
          value={value} onChange={event => onValue(event.currentTarget.value)} />
        {hasApiKey && !noAuth ? (
          <Button variant='outline' size='sm' disabled={clearBusy} onClick={onClear}
            title={t('清除已配置的 API Key（转发时会因缺少凭证被跳过）')}>{t('清除')}</Button>
        ) : null}
      </div>
      <div className='field-row mt-2.5'>
        <Label className='inline-flex cursor-pointer items-center gap-2.5 font-normal'>
          <Checkbox checked={noAuth} aria-label={t('该上游无需鉴权')}
            onCheckedChange={next => onToggleNoAuth(next === true)} />
          <span className='text-xs text-subtle'>{t('该上游无需鉴权（转发与拉取模型都不发送鉴权头）')}</span>
        </Label>
      </div>
      <p className='detail'>
        {t('上游本来不要凭证时（本地 Ollama、OpenCode Zen 的免费档）勾上这一项：账号会被正常选路；只把 Key 留空')}
        <strong>{t('又不勾')}</strong>
        {t('会被当成「未配置凭证」而跳过。')}
      </p>
    </>
  )
}

/** 自定义提供商的「提供商」一段（内置家不渲染：它们的协议与地址写在代码里） */
function ProviderSection({
  provider, name, protocol, baseUrl, count, emulation,
  onName, onProtocol, onBaseUrl, onEmulation, onRemove,
}: {
  provider: { id: string; name?: string; protocol?: string; baseUrl?: string }
  name: string
  protocol: string
  baseUrl: string
  count: number
  /** 客户端形态伪装（'' = 不伪装；'opencode' = 按官方 CLI 形状补齐请求） */
  emulation: string
  onName: (next: string) => void
  onProtocol: (next: string) => void
  onBaseUrl: (next: string) => void
  onEmulation: (next: string) => void
  onRemove: () => void
}) {
  const protocols = shared().wbProviders?.PROTOCOL_OPTIONS || [
    { value: 'openai', label: t('OpenAI 兼容') },
    { value: 'anthropic', label: 'Anthropic' },
  ]
  const current = protocols.find(option => option.value === protocol)?.label || protocol
  return (
    <DialogSection>
      <h3>{t('提供商')}</h3>
      <p>
        {t('这一栏改的是')}<strong>{t('「{name}」本身', { name: provider.name || provider.id })}</strong>
        {t('（名下 {count} 个账号共用）', { count })}
        {t('改协议 / Base URL 会改变它们的转发方式，正在进行的请求可能失败；模型清单与映射在「模型管理」页。')}
      </p>
      <div className='field-row'>
        <label htmlFor='account-provider-name'>{t('名称')}</label>
        <Input id='account-provider-name' maxLength={MAX_PROVIDER_NAME_CHARS} className='min-w-[220px]'
          placeholder={t('提供商显示名，1~{max} 个字符', { max: MAX_PROVIDER_NAME_CHARS })}
          value={name} onChange={event => onName(event.currentTarget.value)} />
      </div>
      <div className='field-row mt-2.5'>
        <label htmlFor='account-provider-protocol'>{t('协议')}</label>
        <Select value={protocol} onValueChange={value => onProtocol(String(value))}>
          <SelectTrigger id='account-provider-protocol' className='min-w-[180px]'>
            <SelectValue>{current}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {protocols.map(option => (
              <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className='field-row mt-2.5'>
        <label htmlFor='account-provider-baseurl'>{t('Base URL')}</label>
        <Input id='account-provider-baseurl' className='min-w-[260px]'
          placeholder={t('OpenAI 兼容填到 /v1；Anthropic 填根地址')}
          value={baseUrl} onChange={event => onBaseUrl(event.currentTarget.value)} />
      </div>
      <div className='field-row mt-2.5'>
        <Label className='inline-flex cursor-pointer items-center gap-2.5 font-normal'>
          <Checkbox checked={emulation === 'opencode'} aria-label={t('伪装 OpenCode 官方客户端')}
            onCheckedChange={next => onEmulation(next === true ? 'opencode' : '')} />
          <span className='text-xs text-subtle'>{t('伪装 OpenCode 官方客户端（OpenCode Zen 免费档要它才放行）')}</span>
        </Label>
      </div>
      <p className='detail'>
        {t('开启后，网关按 OpenCode 官方 CLI 的形状发请求：没填 Key 的账号用匿名凭证')}
        <code>{'public'}</code>
        {t('、补官方会话头、请求体补两个占位工具（上游免费档的三道校验）。')}
        <strong>{t('它会改写请求体')}</strong>
        {t('，只对 OpenCode Zen 这类上游有意义，别的家请关掉。')}
      </p>
      <div className='field-row mt-3'>
        <Button variant='destructive' onClick={onRemove}>{t('删除提供商')}</Button>
        <span className='detail'>{t('级联删除名下全部账号，不可恢复')}</span>
      </div>
    </DialogSection>
  )
}

export function AccountSettingsDialog({ id, onClose }: { id: string; onClose: () => void }) {
  const account = findAccount(id)
  const [enabled, setEnabled] = React.useState(account?.enabled !== false)
  const [name, setName] = React.useState(account?.name || '')
  const [proxyDraft, setProxyDraft] = React.useState<ProxyDraft>(() => draftOfProxy(account?.proxy))
  const [balanceToken, setBalanceToken] = React.useState('')
  // ZCode 的「使用哪个套餐」草稿（非 ZCode 账号恒为空串，那一段也不渲染）
  const [planChannel, setPlanChannel] = React.useState(() => zcodePlanOf(account))
  const [busy, setBusy] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>('')
  const [provider, setProvider] = React.useState<{
    id: string; name?: string; protocol?: string; baseUrl?: string; clientEmulation?: string
  } | null>(null)
  const [providerName, setProviderName] = React.useState('')
  const [providerProtocol, setProviderProtocol] = React.useState('chat_completions')
  const [providerBaseUrl, setProviderBaseUrl] = React.useState('')
  /** 客户端形态伪装草稿（'' = 不伪装；'opencode' = 按官方 CLI 形状补齐请求） */
  const [providerEmulation, setProviderEmulation] = React.useState('')
  /** 自定义账号的凭证草稿：新 Key（留空 = 不改，走「清除」按钮）与「无需鉴权」勾选 */
  const [apiKeyDraft, setApiKeyDraft] = React.useState('')
  const [noAuthDraft, setNoAuthDraft] = React.useState(account?.noAuth === true)
  // 「查询设置」段的草稿（自动查询间隔）：只在真的有余额概念的家渲染，草稿仍
  // 无条件初始化 —— 与 planChannel 同一形态，省一个条件分支
  const [usageDraft, setUsageDraft] = React.useState<UsageDraft>(() => usageDraftOf(account))
  // 「查询设置」段里**余额方案要的凭证**（只有登记了方案的自定义家才有字段）：
  // 草稿同样无条件初始化（没有方案时是一份空草稿，那一段也不渲染）
  const [balanceQueryDraft, setBalanceQueryDraft] = React.useState<Record<string, string>>(
    () => balanceQueryDraftOf(account?.balanceQuery),
  )
  // 「限制器」段的草稿（余额 / Token 规则列表）：与查询设置同一 gating
  const [limiterDraft, setLimiterDraft] = React.useState<LimiterRule[]>(() => limiterDraftOf(account))
  // 分组导航的当前页（弹窗按账号 id 挂载，切账号自然回到「基本」）
  const [pane, setPane] = React.useState<PaneId>('basic')
  // 并发上限草稿（原列表 ⋯ 菜单的 conc-dialog 迁入「基本」分组）：0~999，0 = 不限
  const [concDraft, setConcDraft] = React.useState(() => String(Number(account?.maxConcurrent) || 0))

  // 「提供商」那一段：目录里查得到才算自定义家（id 前缀只说明「长得像」，而记录本身
  // 才带着协议 / Base URL 的现值 —— 三个字段要拿它预填）。
  // 依赖只取 id：弹窗是按 id 挂载的（父组件给了 key），账号数据刷新不该重置用户的编辑
  React.useEffect(() => {
    let alive = true
    setProvider(null)
    const account = findAccount(id)
    if (!account) return () => { alive = false }
    void Promise.resolve(shared().wbCustomProvidersUi?.find?.(providerOf(account))).then(found => {
      if (!alive || !found) return
      setProvider(found)
      setProviderName(found.name || '')
      setProviderProtocol(found.protocol || 'chat_completions')
      setProviderBaseUrl(found.baseUrl || '')
      setProviderEmulation(typeof found.clientEmulation === 'string' ? found.clientEmulation : '')
    }).catch(() => { /* 目录不可用：退化成「不是自定义家」，不注入那一段 */ })
    return () => { alive = false }
  }, [id])

  if (!account) {
    return (
      <Dialog open onOpenChange={next => { if (!next) onClose() }}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t('账号设置')}</DialogTitle></DialogHeader>
          <DialogBody><p className='detail'>{t('账号不存在，请刷新后重试。')}</p></DialogBody>
          <DialogFooter><div className='mr-auto' /><Button variant='outline' onClick={onClose}>{t('关闭')}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    )
  }

  // 账号不存在时只给一块提示（列表可能刚被另一端删了）。用局部常量接住窄化后的账号：
  // 下面几个 async 函数声明会被提升，TS 不会把「早退之后的窄化」带进它们体内
  const target: AccountRecord = account
  const peers = peersOf(target, allAccounts())
  const isCatpaw = providerOf(target) === 'catpaw'
  // 自定义账号：凭证段（API Key / 无需鉴权）只对它渲染。判据用 provider 前缀 ——
  // 与后端 `custom_account_provider` 一致（提供商被删后残留的账号同样该能改凭证）
  const isCustomAccount = providerOf(target).startsWith('custom-')
  // 「轮询」档的可见性：只有 OpenCode Zen 免费档（按 IP 限流）。判据用**已读到的
  // 提供商记录**（上面那个 effect 拉的）而不是目录缓存 —— 这一段的上下文里就有它，
  // 且与「提供商」段编辑的是同一份现值（改了 emulation / baseUrl 未保存时，
  // 档位仍按盘上的值给，保存后才变 —— 与后端闸门读盘上记录同一口径）。
  // 记录本来就是这个档时**恒显示**（哪怕判据此刻为假：目录还没读到 / 提供商被
  // 改过）：否则用户看不到自己这条账号现在是什么状态；后端只挡「真的改成轮询」
  // 的保存，原样发回来不会 400。
  const allowRotate = isOpencodeZenProvider(provider, target.baseUrl) || proxyDraft.mode === 'rotate'
  // 余额方案那一段（「查询设置」里的查询凭证）：方案与「已配置」标记都由账号的
  // 公开形态回答 —— `scheme` 是这一家的方案 id，`has*` 是密字段的存在标记
  // （值永不回显，见后端 `store_view::balance_query_public`）
  const balanceSpec = balanceSpecOf(target.balanceQuery?.scheme)
  const balanceConfigured: Record<string, boolean> = {}
  for (const field of balanceSpec?.fields ?? []) {
    balanceConfigured[field.key] = balanceFieldConfigured(target.balanceQuery, field)
  }
  // 「要填但还没填全」时给一句中性提示（不是错误：余额列会显示「未配置」，
  // 用户在那一列看到的就是这句话的后果）
  const balanceNeedsCredentials = Boolean(balanceSpec && balanceSpec.fields.length)
    && !balanceQueryConfigured(target.balanceQuery)

  /** 清除余额凭证（立即落库，不走保存按钮 —— 它是一个独立的撤销动作） */
  async function clearBalanceToken(): Promise<void> {
    if (busy) return
    setBusy(true)
    try {
      await shared().workbuddyDesktop?.updateAccount?.(id, { balanceToken: null })
      setBalanceToken('')
      toast(t('✅ 已清除余额查询凭证'))
      await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  /**
   * 清除 API Key（与余额凭证同一形制：立即落库的独立撤销动作）。
   * 清掉之后账号变成「未配置凭证」，转发时会被跳过 —— 所以提示语点明这一点，
   * 而列表上那枚「未配置凭证」徽章会立刻出现，用户看得到后果。
   */
  async function clearApiKey(): Promise<void> {
    if (busy) return
    setBusy(true)
    try {
      await shared().workbuddyDesktop?.updateAccount?.(id, { apiKey: null })
      setApiKeyDraft('')
      toast(t('✅ 已清除 API Key（该账号现在没有凭证，转发会被跳过）'))
      await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  /**
   * 清除一个已配置的余额查询凭证字段（自定义家：NewAPI 的访问令牌 / 商汤的
   * 密码 / 智谱的控制台令牌）。
   *
   * 与清除 API Key 同一形制：**立即落库的独立撤销动作**（不进保存按钮那一批）
   * —— 密字段的值不回显，输入框留空是「不修改」，清除只能靠这个显式动作
   * （传 null 给后端，语义见 `normalize_balance_query_patch`）。
   * 清掉之后余额列会显示「未配置」，用户看得到后果。
   */
  async function clearBalanceQueryField(key: BalanceFieldKey): Promise<void> {
    if (busy) return
    setBusy(true)
    try {
      await shared().workbuddyDesktop?.updateAccount?.(id, { balanceQuery: { [key]: null } })
      setBalanceQueryDraft(prev => ({ ...prev, [key]: '' }))
      toast(t('✅ 已清除查询凭证'))
      await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  /**
   * 读「凭证」那一段的改动：没这一段（内置家）或什么都没改 → null。
   *
   * 三个字段的语义与后端逐条对齐（`update_custom_credentials`）：
   *   · 输入框填了 → 写新 Key（后端会顺手摘掉「无需鉴权」）；
   *   · 勾选态变了 → 写 noAuth（开=清 key，后端归一）；
   *   · 输入框留空且勾选态没变 → 什么都不发（留空是「不修改」，清除走按钮）。
   */
  function readCredentialPatch(): Record<string, unknown> | null {
    if (!isCustomAccount) return null
    const key = apiKeyDraft.trim()
    const patch: Record<string, unknown> = {}
    if (key) patch.apiKey = key
    else if (noAuthDraft !== (target.noAuth === true)) patch.noAuth = noAuthDraft
    return Object.keys(patch).length ? patch : null
  }

  /** 读「提供商」那一段的改动：没这一段（内置家）或字段都没动 → null（不提交） */
  function readProviderPatch(): { error: string } | { id: string; name: string; protocol: string; baseUrl: string; clientEmulation: string } | null {
    if (!provider) return null
    const nextName = providerName.trim()
    const nextBase = providerBaseUrl.trim()
    // 空名称 / 空 Base URL 后端会 400，而那时**账号已经存下了** —— 用户看到「保存失败」
    // 却发现账号的改动生效了。所以在账号落库之前先验一遍
    if (!nextName) return { error: t('请填写提供商名称') }
    if (!nextBase) return { error: t('请填写提供商的 Base URL') }
    const beforeEmulation = typeof provider.clientEmulation === 'string' ? provider.clientEmulation : ''
    if (
      provider.name === nextName && provider.protocol === providerProtocol
      && provider.baseUrl === nextBase && beforeEmulation === providerEmulation
    ) return null
    return {
      id: provider.id, name: nextName, protocol: providerProtocol,
      baseUrl: nextBase, clientEmulation: providerEmulation,
    }
  }

  async function save(): Promise<void> {
    if (busy) return
    let proxy: ProxyPayload
    try {
      proxy = readProxyDraft(proxyDraft)
    } catch (error) {
      toast(errorMessage(error), 'err')
      return
    }
    const providerPatch = readProviderPatch()
    if (providerPatch && 'error' in providerPatch) {
      setStatus(<span className='text-destructive'>{providerPatch.error}</span>)
      return
    }
    // 查询设置段（自动查询间隔）：与凭证段同一层，跟着这次保存一起落库；
    // 校验不过先在前端挡住（后端 apply_patch 对同一规则也会 400）
    const usageChanges = supportsUsage(target) ? readUsageChanges(target, usageDraft) : null
    if (usageChanges && 'error' in usageChanges) {
      setStatus(<span className='text-destructive'>{usageChanges.error}</span>)
      return
    }
    // 限制器段（余额 / Token 规则列表）：与查询设置同一层；编辑器保存时已校验过，
    // 这里只负责「真改了才带上」（对着现状保存不会凭空多一条变更日志，与后端一致）
    const limiterChanges = readLimiterChanges(target, limiterDraft)
    // 查询凭证段（自定义家的余额方案）：按方案读草稿 —— 密字段留空 = 不修改，
    // 非密字段原样提交（后端判无变化时不写日志）；这一家没有方案时恒 null
    const balanceQueryPatch = readBalanceQueryChanges(target.balanceQuery?.scheme, balanceQueryDraft)
    // 凭证段（自定义账号）：与代理 / 套餐同一层，跟着这次保存一起落库
    // （它走的是 PATCH /api/accounts 的自定义分支，见后端 `update_custom_credentials`）
    const credentialPatch = readCredentialPatch() || {}
    // 并发上限（「基本」分组）：本地先归一（number 输入挡不住手工脏值），后端
    // 400 只该是最后防线 —— 与原 conc-dialog 同一套 clamp（0~999，0 = 不限；
    // CodeArts 家 0 由上游解释成默认并发，见能力表 concurrencyDefault）
    const concRaw = Number(concDraft)
    if (!Number.isFinite(concRaw) || concRaw < 0) {
      toast(t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX }), 'err')
      return
    }
    const concNext = Math.min(CONC_MAX, Math.round(concRaw))
    const concCurrent = Number(target.maxConcurrent) || 0
    const concPatch: Record<string, unknown> = concNext !== concCurrent ? { maxConcurrent: concNext } : {}

    setBusy(true)
    try {
      // CatPaw 的余额凭证是**按需附加**的字段（别的家没有这一项，输入框也不存在）：
      // 空值 = 不修改（后端公开形态只给 hasBalanceToken 真假、不回显原值，所以这个
      // 输入框永远是空的；若把「空」解释成清除，用户每次保存设置都会把配好的凭证删掉）。
      // 清除走上面那个显式按钮。
      const balancePatch = isCatpaw && balanceToken.trim() ? { balanceToken: balanceToken.trim() } : {}
      // 套餐通道：只在**真的改了**时才带上（取值与后端 `zcode::normalize_plan`
      // 逐字一致）。带一个没变的键不会出错，但会让后端那句「套餐通道 → …」的
      // 变更提示在每次保存设置时都出现一次，看起来像刚改过
      const planPatch = supportsPlanChannel(target) && planChannel !== zcodePlanOf(target)
        ? { zcodePlan: planChannel }
        : {}
      // 顺序（priority）**不在这张表单里**：分组模式下它由拖拽整队写（见
      // accounts-data 的 reorderQueue）。设置弹窗只改「账号自身」的属性，
      // 一次「改备注名」不该顺手把顺序也提交一遍
      await shared().workbuddyDesktop?.updateAccount?.(id, {
        name: name.trim() || target.name,
        enabled,
        proxy,
        ...balancePatch,
        ...planPatch,
        ...credentialPatch,
        ...concPatch,
        ...(balanceQueryPatch ? { balanceQuery: balanceQueryPatch } : {}),
        ...(usageChanges && 'patch' in usageChanges ? usageChanges.patch : {}),
        ...(limiterChanges && 'patch' in limiterChanges ? limiterChanges.patch : {}),
      })
      // 提供商那一段排在账号之后（账号是本弹窗的主角，先落库）。它失败时账号已经存下了，
      // 所以留在弹窗里把那句话说清楚 —— 笼统报成「保存失败」会把两件事混成一件
      if (providerPatch) {
        try {
          await shared().wbCustomProvidersUi?.update?.(providerPatch)
        } catch (error) {
          setStatus(<span className='text-destructive'>{t('账号已保存，但提供商未更新：{error}', { error: errorMessage(error) })}</span>)
          await shared().wbApp?.refresh?.()
          setBusy(false)
          return
        }
      }
      // 先关窗并反馈成功：改动已经落库，刷新只是让列表跟上，不该让用户对着「保存中…」
      // 再多等一次网络往返
      onClose()
      toast(t('✅ 账号设置已保存'))
      // 保存后必须主动刷新列表：以前只关窗不刷新，行上的代理 / 优先级仍是旧数据，
      // 要等 20 秒那一轮轮询才更新 —— 用户看到的就是「保存完十几秒才变」。
      // 单独兜一层错：刷新失败只影响本次界面同步（后续轮询会自愈），不能掉进下面的
      // catch 被报成「保存失败」（保存其实已经成功了）
      try {
        await shared().wbApp?.refresh?.()
      } catch { /* 交给下一次轮询 */ }
    } catch (error) {
      // 后端校验失败（如优先级冲突）：留在弹窗里显示原因，方便直接改
      setStatus(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  // 「凭证与套餐」按家出现：有什么配什么（CatPaw 余额凭证 / 自定义账号凭证与
  // 提供商 / ZCode 套餐通道）。内置家什么都没有时整组不出现。
  const concDefault = Number(providerFeatures(providerOf(target)).concurrencyDefault) || 0
  const hasCredentialPane = isCatpaw || isCustomAccount || supportsPlanChannel(target)

  return (
    <Dialog open onOpenChange={next => {
      // 保存中不许关：关掉会让「到底存没存进去」变成未知状态
      if (next || busy) return
      onClose()
    }}>
      <DialogContent className='w-[min(900px,calc(100vw-48px))]'>
        <DialogHeader><DialogTitle>{t('账号设置 · {name}', { name: labelOf(account) })}</DialogTitle></DialogHeader>
        <DialogBody className='p-0'>
          <div className='acct-dlg'>
            {/* 分组导航复用模型管理左栏的形态（.prov-rail 容器 + NavItem，见 page-gateway.css）：
                选中态 / 悬浮态 / 字重都在组件里，这里只给弹窗内的栏宽 */}
            <aside className='prov-rail acct-dlg-rail' aria-label={t('账号设置分组')}>
              <div className='rail-scroll'>
                <div className='rail-label'>{t('账号设置')}</div>
                {([['basic', t('基本')], ...(hasCredentialPane ? [['credential', t('凭证与套餐')] as const] : []),
                  ...(supportsUsage(target) ? [['query', t('查询设置')] as const, ['limiter', t('限制器')] as const] : []),
                  ['network', t('网络')]] as Array<[PaneId, string]>).map(([id, label]) => (
                    <NavItem key={id} active={pane === id} title={label} className='shadow-none'
                      onClick={() => setPane(id)}>{label}</NavItem>
                  ))}
              </div>
            </aside>
            <div className='acct-dlg-main'>
              {pane === 'basic' ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('基本')}</h3>
                    <p>{t('这条账号是谁、排第几、能不能接请求。')}</p>
                  </div>
                  <div className='field-row'>
                    <label>{t('启用')}</label>
                    <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用该账号')} />
                    <span className='detail'>{t('关闭则不参与转发（余额查询照常，见「查询设置」）')}</span>
                  </div>
                  <div className='field-row'>
                    <label htmlFor='account-name-input'>{t('备注名')}</label>
                    <Input id='account-name-input' maxLength={100} placeholder={t('账号显示名称')} className='max-w-[280px]'
                      value={name} onChange={event => setName(event.currentTarget.value)} />
                  </div>
                  <div className='field-row'>
                    <label>{t('转发顺序')}</label>
                    <span className='detail'>
                      {t('在列表里拖：拖分组带调整整组顺位，拖行首把手调整组内先后（顺序由拖拽表达，不再填数字）')}
                    </span>
                  </div>
                  <div className='field-row'>
                    <label htmlFor='account-conc-input'>{t('并发上限')}</label>
                    <Input id='account-conc-input' type='number' min={0} max={CONC_MAX} step={1}
                      className='max-w-[110px]' value={concDraft}
                      onChange={event => setConcDraft(event.currentTarget.value)} />
                    <span className='detail'>{concDefault > 0 ? t('0 = 按本家默认 {default}', { default: concDefault }) : t('0 = 不限')}</span>
                  </div>
                  <p className='detail acct-dlg-hint'>
                    {concDefault > 0
                      ? t('并发上限 受上游硬顶约束，达到上限的账号跳过、请求转给其他账号（原列表 ⋯ 菜单的「并发上限」入口已并入这里）')
                      : t('并发上限 0~{max}；达到上限的账号跳过、请求转给其他账号（原列表 ⋯ 菜单的「并发上限」入口已并入这里）', { max: CONC_MAX })}
                  </p>
                </div>
              ) : null}

              {pane === 'credential' && hasCredentialPane ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('凭证与套餐')}</h3>
                    <p>{t('这一段按提供商出现：有什么配什么。')}</p>
                  </div>
                  {isCatpaw ? (
                    <BalanceTokenField configured={account.hasBalanceToken === true} value={balanceToken}
                      onChange={setBalanceToken} clearBusy={busy}
                      onClear={() => void clearBalanceToken()} />
                  ) : null}
                  {isCustomAccount ? (
                    <CustomCredentialFields
                      hasApiKey={target.hasApiKey === true}
                      noAuth={noAuthDraft}
                      value={apiKeyDraft}
                      onValue={setApiKeyDraft}
                      onToggleNoAuth={setNoAuthDraft}
                      onClear={() => void clearApiKey()}
                      clearBusy={busy} />
                  ) : null}
                  {supportsPlanChannel(target) ? (
                    <PlanChannelField plan={planChannel} hasJwt={account.canClaim === true}
                      onChange={setPlanChannel} />
                  ) : null}
                  {provider ? (
                    <ProviderSection provider={provider} count={peers.length + 1}
                      name={providerName} protocol={providerProtocol} baseUrl={providerBaseUrl}
                      emulation={providerEmulation}
                      onName={setProviderName} onProtocol={setProviderProtocol} onBaseUrl={setProviderBaseUrl}
                      onEmulation={setProviderEmulation}
                      onRemove={() => {
                        if (busy) return
                        // 删掉了就把本弹窗一起关掉（账号也没了）
                        void Promise.resolve(shared().wbCustomProvidersUi?.remove?.(provider.id)).then(removed => {
                          if (removed) onClose()
                        })
                      }} />
                  ) : null}
                </div>
              ) : null}

              {pane === 'query' && supportsUsage(target) ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('查询设置')}</h3>
                    <p>{t('余额列读数的自动刷新；也是「限制器」余额规则的数据来源。')}</p>
                  </div>
                  {/* 余额方案要的凭证（只有登记了方案的自定义家才有这一段；
                      DeepSeek 这类不需要额外信息的家只有一行方案说明） */}
                  {balanceSpec ? (
                    <BalanceQuerySection
                      spec={balanceSpec}
                      values={balanceQueryDraft}
                      configured={balanceConfigured}
                      onValue={(key, next) => setBalanceQueryDraft(prev => ({ ...prev, [key]: next }))}
                      onClear={key => void clearBalanceQueryField(key)}
                      clearBusy={busy}
                      idPrefix='account-balance-query'
                    />
                  ) : null}
                  {balanceNeedsCredentials ? (
                    <p className='detail mt-2.5 text-subtle'>
                      {t('查询凭证还没配齐：填好后余额列才有读数，在此之前会显示「未配置」。')}
                    </p>
                  ) : null}
                  <UsageSettingsSection account={target} draft={usageDraft} onChange={setUsageDraft} />
                </div>
              ) : null}

              {pane === 'limiter' && supportsUsage(target) ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('限制器')}</h3>
                    <p>
                      {t('按规则限制该账号参与转发：没有规则（或全部停用）= 不限制；跳过档自动恢复，禁用档需手动启用。')}
                    </p>
                  </div>
                  <LimiterSection account={target} draft={limiterDraft} onChange={setLimiterDraft} />
                </div>
              ) : null}

              {pane === 'network' ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('网络')}</h3>
                    <p>{t('这条账号访问上游走哪条线路。')}</p>
                  </div>
                  <ProxyForm draft={proxyDraft} onChange={setProxyDraft} idPrefix='account-proxy'
                    allowRotate={allowRotate} />
                  {account.proxy?.error ? (
                    <p className='detail text-destructive'>
                      {t('当前代理不可用：{error}（转发时会回退直连）', { error: account.proxy.error })}
                    </p>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
        </DialogBody>
        <DialogFooter>
          <div className='detail mr-auto' style={{ minHeight: 18 }}>{status}</div>
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 批量操作弹窗 ───────────────────────────── */

const BATCH_ACTIONS = [
  { value: 'enable', label: t('启用') },
  { value: 'disable', label: t('禁用') },
  { value: 'proxy', label: t('修改代理') },
  { value: 'concurrency', label: t('设置并发上限') },
  { value: 'remove', label: t('删除') },
] as const

function BatchDialog({ ids, action, onClose }: { ids: string[]; action: string; onClose: () => void }) {
  const [current, setCurrent] = React.useState(action)
  const [proxyDraft, setProxyDraft] = React.useState<ProxyDraft>(() => draftOfProxy(null))
  // 并发上限草稿：**不预填**。预填有两个坑：混选时每家现值不同（填谁都不对），
  // 而 CodeArts 的公开读数是「配置与上游硬顶取大」后的结果，预填再确认会把
  // 一个没配过的 3 物化进记录。空值 = 还没填，提交时显式拦下（见 run）。
  const [concDraft, setConcDraft] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [result, setResult] = React.useState<React.ReactNode>('')
  const accounts = ids.map(id => findAccount(id)).filter((account): account is AccountRecord => account !== null)
  /** 选中的账号里「本家没有『不限』这一档」的硬顶（CodeArts 3，其余 0）：只影响提示
   *  文案 —— 这些家即使填 0、或填一个比硬顶更小的数，实际并发仍由硬顶决定
   *  （见 store_util 的 `max_concurrent_public_for`）。空选区取 0 */
  const concHardCap = accounts.reduce(
    (cap, account) => Math.max(cap, Number(providerFeatures(providerOf(account)).concurrencyDefault) || 0),
    0,
  )
  /**
   * 「轮询」档在批量里只在**选中的账号全是 OpenCode Zen** 时给出。
   *
   * 后端的闸门是逐账号判的（非 Zen 账号存轮询会 400，见 apply_patch），所以
   * 混选时给出这一档等于让用户提交一次必然部分失败的批量操作。空选区不给
   * （那时连账号是谁都还不知道，见 supportsProxyRotate 的说明）。
   */
  const allowRotate = accounts.length > 0 && accounts.every(account => supportsProxyRotate(account))

  /** 批量删除是不可逆操作，按数量做二次确认（原生 confirm 在 Tauri WebView 里直接放行） */
  function confirmBatchRemove(count: number): Promise<boolean> {
    return Promise.resolve(shared().wbConfirm?.ask?.({
      title: t('删除选中的账号'),
      html: t('确定删除选中的 <strong>{count}</strong> 个账号？此操作不可恢复，账号的登录态会一并移除。', { count }),
      okText: t('删除'),
      okClass: 'danger',
    }) ?? false)
  }

  async function run(): Promise<void> {
    if (busy) return
    if (!ids.length) { toast(t('没有选中的账号'), 'err'); return }
    let proxy: ProxyPayload | undefined
    if (current === 'proxy') {
      try {
        proxy = readProxyDraft(proxyDraft)
      } catch (error) {
        toast(errorMessage(error), 'err')
        return
      }
    }
    // 并发上限：与设置弹窗同一套归一（0~999 的整数，0 = 不限）。**必须显式填**：
    // 空值不当 0 —— 否则「选错动作直接点执行」会把一批账号的并发限制全部清掉。
    // 后端还会再挡一次（同一份校验），这里先挡是为了不把 400 当交互反馈
    let maxConcurrent: number | undefined
    if (current === 'concurrency') {
      const raw = Number(concDraft.trim())
      if (!concDraft.trim() || !Number.isFinite(raw) || raw < 0) {
        toast(t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX }), 'err')
        return
      }
      maxConcurrent = Math.min(CONC_MAX, Math.round(raw))
    }
    if (current === 'remove' && !(await confirmBatchRemove(ids.length))) return

    // 批量禁用 / 删除可能把所有启用中的账号一起停掉，转发将无账号可用，提醒一下更稳妥。
    // 判据是「选中的账号是否已覆盖全部启用中的账号」，而不是数量对比 —— 否则勾了
    // 已禁用账号凑够数量也会误报
    if (current === 'disable' || current === 'remove') {
      const picked = new Set(ids)
      const survivors = allAccounts().filter(account => account.enabled !== false && !picked.has(account.id))
      if (!survivors.length) {
        const what = current === 'remove' ? t('删除') : t('禁用')
        const ok = await Promise.resolve(shared().wbConfirm?.ask?.({
          title: t('全部启用中的账号将被{what}', { what }),
          html: t('这会<strong>{what}</strong>所有启用中的账号，转发将不可用。确定继续？', { what }),
          okText: what,
          okClass: current === 'remove' ? 'danger' : 'primary',
        }) ?? false)
        if (!ok) return
      }
    }

    setBusy(true)
    setResult('')
    try {
      const payload: { action: string; ids: string[]; proxy?: unknown; maxConcurrent?: number } = { action: current, ids }
      if (current === 'proxy') payload.proxy = proxy
      if (current === 'concurrency') payload.maxConcurrent = maxConcurrent
      const data = await shared().workbuddyDesktop?.batchAccounts?.(payload)
      const okCount = (data?.ok || []).filter(item => item.changes?.length).length
      const removedCount = (data?.removed || []).length
      const failed = data?.failed || []
      const succeeded = current === 'remove' ? removedCount : okCount
      const verb = { enable: t('启用'), disable: t('禁用'), proxy: t('修改代理'), concurrency: t('设置并发上限'), remove: t('删除') }[current] || current
      if (failed.length) {
        const detail = failed.slice(0, 3).map(item => labelOf(findAccount(String(item.id))) || item.id).join(t('、'))
        toast(t('{verb}完成：成功 {ok} 个，失败 {fail} 个（{detail}{more}）', {
          verb, ok: succeeded, fail: failed.length, detail, more: failed.length > 3 ? t(' 等') : '',
        }), 'err')
        setResult(
          <span className='text-destructive'>{t('失败 {count} 个：', { count: failed.length })}
            {failed.map(item => `${labelOf(findAccount(String(item.id))) || item.id}（${item.error}）`).join(t('；'))}
          </span>,
        )
      } else {
        toast(t('✅ {verb}完成：共 {count} 个账号', { verb, count: succeeded }))
        onClose()
      }
      await shared().wbApp?.refresh?.()
    } catch (error) {
      setResult(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(t('批量操作失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

const verb = { enable: t('启用'), disable: t('禁用'), proxy: t('修改代理'), concurrency: t('设置并发上限'), remove: t('删除') }[current] || current

  return (
    <Dialog open onOpenChange={next => { if (next || busy) return; onClose() }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t('批量操作 · 已选 {count} 个账号', { count: ids.length })}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogSection>
            <h3>{t('将作用于以下账号')}</h3>
            <p style={{ maxHeight: 84, overflowY: 'auto' }}>{accounts.map(account => labelOf(account)).join(t('、'))}</p>
          </DialogSection>
          <DialogSection>
            <h3>{t('操作')}</h3>
            <RadioGroup value={current} onValueChange={setCurrent} className='flex-row flex-wrap items-center gap-5'
              aria-label={t('批量动作')}>
              {BATCH_ACTIONS.map(item => (
                <Label key={item.value} className='inline-flex cursor-pointer items-center gap-2 font-normal'>
                  <RadioGroupItem value={item.value} />{item.label}
                </Label>
              ))}
            </RadioGroup>
            {current === 'proxy' ? (
              <div className='mt-2.5'>
                <ProxyForm draft={proxyDraft} onChange={setProxyDraft} idPrefix='batch-proxy'
                  allowRotate={allowRotate} />
              </div>
            ) : null}
            {current === 'concurrency' ? (
              <div className='mt-2.5'>
                <div className='field-row'>
                  <label htmlFor='batch-conc-input'>{t('并发上限')}</label>
                  <Input id='batch-conc-input' type='number' min={0} max={CONC_MAX} step={1}
                    className='max-w-[110px]' placeholder={t('不限')} value={concDraft}
                    onChange={event => setConcDraft(event.currentTarget.value)} />
                  <span className='detail'>{t('0 = 不限')}</span>
                </div>
                <p className='detail acct-dlg-hint'>
                  {concHardCap > 0
                    ? t('并发上限 0~{max}；有上游硬顶的家（如 CodeArts）以它的硬顶为准；达到上限的账号会被跳过、请求转给其他账号', { max: CONC_MAX })
                    : t('并发上限 0~{max}；达到上限的账号会被跳过、请求转给其他账号', { max: CONC_MAX })}
                </p>
              </div>
            ) : null}
          </DialogSection>
          <div className='detail' style={{ minHeight: 18 }}>{result}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button variant={current === 'remove' ? 'destructive' : 'default'} disabled={busy} onClick={() => void run()}>
            {busy ? t('执行中…') : t('执行（{action}）', { action: verb })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 供页面调用的入口 ───────────────────────── */

/**
 * 两个弹窗的宿主：按 store 里的 dialog 状态挂载。
 * 弹窗走组件库的 Dialog（Esc / 点遮罩关闭、焦点陷阱、滚动锁定都内建），关闭即卸载 ——
 * index.html 里那两个常驻的 `#account-modal` / `#batch-modal` 因此不再需要。
 */
export function AccountsDialogs() {
  const store = getStore()
  if (!store.dialog) return null
  if (store.dialog.kind === 'settings') {
    return <AccountSettingsDialog key={store.dialog.id} id={store.dialog.id} onClose={closeDialog} />
  }
  return <BatchDialog key={store.dialog.ids.join(',')} ids={store.dialog.ids} action={store.dialog.action}
    onClose={closeDialog} />
}
