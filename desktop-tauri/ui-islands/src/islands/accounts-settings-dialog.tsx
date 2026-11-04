/** 单账号设置弹窗与提供商共用连接设置。 */

import * as React from 'react'
import {
  Badge, BadgeDot, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Input, NavList, NavItem, Sidebar, Switch,
  Field, FieldDescription, FieldLabel,
} from '@ui'
import '../styles/accounts-settings-dialog.css'
import { t } from '../i18n'
import {
  ACCOUNT_CONCURRENCY_MAX as CONC_MAX, errorMessage, esc, shared, toast,
  type AccountRecord, type LimiterGroup,
} from './accounts-shared'
import {
  displayNameOf, providerFeatures, providerOf, supportsPlanChannel, supportsUsage,
  isOpencodeZenProvider, zcodePlanOf,
} from './accounts-domain'
import { PlanChannelField } from './accounts-zcode-plan'
import {
  readUsageChanges, UsageSettingsSection, usageDraftOf, type UsageDraft,
} from './accounts-dialog-usage'
import {
  BalanceQuerySection, balanceFieldConfigured, balanceQueryConfigured, balanceQueryDraftOf,
  balanceSpecOf, readBalanceQueryChanges, type BalanceFieldKey,
} from './balance-query'
import { limiterDraftOf, LimiterSection, readLimiterChanges } from './accounts-dialog-limiter'
import { allAccounts, findAccount, syncBalancesSnapshot } from './accounts-data'
import { refreshSavedAccounts, updateAccountConfirmed } from './accounts-update'
import { draftOfProxy, ProxyForm, readProxyDraft, type ProxyDraft, type ProxyPayload } from './accounts-proxy-form'
import {
  AccountSettingsIcon, BalanceTokenField, CustomCredentialFields, ProviderSection, SettingGroup,
  type ProviderSettings,
} from './accounts-settings-fields'

type AccountLike = AccountRecord
type PaneId = 'basic' | 'credential' | 'query' | 'limiter' | 'network' | 'provider'

function peersOf(account: AccountLike, all: AccountLike[]): AccountLike[] {
  const provider = providerOf(account)
  return all.filter(item => item.id !== account.id && providerOf(item) === provider)
}

function labelOf(account: AccountLike | null | undefined): string {
  return displayNameOf(account) || account?.id || ''
}

export function AccountSettingsDialog({ id, onClose, initialAccount, initialPane = 'basic', routeContext }: {
  id: string; onClose: () => void
  initialAccount?: AccountRecord
  initialPane?: PaneId
  routeContext?: React.ReactNode
}) {
  const [account, setAccount] = React.useState(() => initialAccount || findAccount(id))
  const [enabled, setEnabled] = React.useState(account?.enabled !== false)
  const [allowRouteSkip, setAllowRouteSkip] = React.useState(account?.allowRouteSkip !== false)
  const [name, setName] = React.useState(account?.name || '')
  const [proxyDraft, setProxyDraft] = React.useState<ProxyDraft>(() => draftOfProxy(account?.proxy))
  const [balanceToken, setBalanceToken] = React.useState('')
  // ZCode 的「使用哪个套餐」草稿（非 ZCode 账号恒为空串，那一段也不渲染）
  const [planChannel, setPlanChannel] = React.useState(() => zcodePlanOf(account))
  const [busy, setBusy] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>('')
  const [provider, setProvider] = React.useState<ProviderSettings | null>(null)
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
  const [limiterDraft, setLimiterDraft] = React.useState<LimiterGroup[]>(() => limiterDraftOf(account))
  // 分组导航的当前页（弹窗按账号 id 挂载，切账号自然回到「基本」）
  const [pane, setPane] = React.useState<PaneId>(initialPane)
  // 并发上限草稿（原列表 ⋯ 菜单的 conc-dialog 迁入「基本」分组）：0~999，0 = 不限
  const [concDraft, setConcDraft] = React.useState(() => String(Number(account?.maxConcurrent) || 0))
  const [confirming, setConfirming] = React.useState(false)
  const actionLock = React.useRef(false)
  const paneScroll = React.useRef<HTMLDivElement | null>(null)
  const panePositions = React.useRef<Partial<Record<PaneId, number>>>({})
  const initial = React.useRef({
    enabled, allowRouteSkip, name, proxyDraft, planChannel, usageDraft,
    balanceQueryDraft, limiterDraft, concDraft, noAuthDraft,
  })
  const credentialLabel = providerOf(account).startsWith('custom-') ? t('凭证设置') : t('凭证与套餐')
  const paneInfo: Record<PaneId, { title: string; description: string; icon: string }> = {
    basic: { title: t('基本信息'), description: t('设置账号名称、启用状态与转发行为。'), icon: 'sliders' },
    credential: { title: credentialLabel, description: t('管理当前账号的凭证与可用套餐。'), icon: 'key' },
    query: { title: t('查询设置'), description: t('配置余额查询凭证与自动刷新间隔。'), icon: 'refresh' },
    limiter: { title: t('限制器'), description: t('用明确的规则保护当前账号的额度。'), icon: 'shield' },
    network: { title: t('网络'), description: t('设置当前账号访问上游的出网方式。'), icon: 'proxies' },
    provider: { title: t('连接设置'), description: t('统一管理提供商的协议、地址与兼容选项。'), icon: 'agent' },
  }
  const currentPane = paneInfo[pane]
  React.useLayoutEffect(() => {
    if (paneScroll.current) paneScroll.current.scrollTop = panePositions.current[pane] || 0
  }, [pane])

  function selectPane(next: PaneId): void {
    if (paneScroll.current) panePositions.current[pane] = paneScroll.current.scrollTop
    setPane(next)
  }

  // 「提供商」那一段：目录里查得到才算自定义家（id 前缀只说明「长得像」，而记录本身
  // 才带着协议 / Base URL 的现值 —— 三个字段要拿它预填）。
  // 依赖只取 id：弹窗是按 id 挂载的（父组件给了 key），账号数据刷新不该重置用户的编辑
  React.useEffect(() => {
    let alive = true
    setProvider(null)
    const account = initialAccount || findAccount(id)
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
  const providerCount = peers.length + 1
  const before = initial.current
  const same = (left: unknown, right: unknown): boolean => JSON.stringify(left) === JSON.stringify(right)
  const dirtyPanes: Record<PaneId, boolean> = {
    basic: enabled !== before.enabled || allowRouteSkip !== before.allowRouteSkip
      || (name.trim() || before.name) !== before.name || concDraft !== before.concDraft,
    credential: Boolean(balanceToken.trim() || apiKeyDraft.trim())
      || noAuthDraft !== before.noAuthDraft || planChannel !== before.planChannel,
    query: !same(usageDraft, before.usageDraft) || !same(balanceQueryDraft, before.balanceQueryDraft),
    limiter: !same(limiterDraft, before.limiterDraft),
    network: !same(proxyDraft, before.proxyDraft),
    provider: Boolean(provider && (
      providerName.trim() !== (provider.name || '') || providerProtocol !== (provider.protocol || 'chat_completions')
      || providerBaseUrl.trim() !== (provider.baseUrl || '') || providerEmulation !== (provider.clientEmulation || '')
    )),
  }
  const dirtyCount = Object.values(dirtyPanes).filter(Boolean).length
  const interactionBusy = busy || confirming

  async function askConfirmation(title: string, text: string, okText: string, danger = false): Promise<boolean> {
    if (actionLock.current) return false
    actionLock.current = true
    setConfirming(true)
    try {
      const ask = shared().wbConfirm?.ask
      if (!ask) throw new Error(t('确认弹窗不可用，请重新打开页面'))
      return await ask({ title, html: esc(text).replace(/\n/g, '<br>'), okText,
        okClass: danger ? 'danger' : 'primary', bodyClass: danger ? 'danger-zone' : '' })
    } catch (error) {
      toast(errorMessage(error), 'err')
      return false
    } finally {
      actionLock.current = false
      setConfirming(false)
    }
  }

  async function close(): Promise<void> {
    if (interactionBusy || actionLock.current) return
    if (dirtyCount && !await askConfirmation(t('放弃未保存的修改？'),
      t('所有分区中尚未保存的修改都会被放弃。已确认的独立清除操作不受影响。'), t('放弃修改'))) return
    onClose()
  }

  /** 清除余额凭证（立即落库，不走保存按钮 —— 它是一个独立的撤销动作） */
  async function clearBalanceToken(): Promise<void> {
    if (interactionBusy || actionLock.current) return
    if (!await askConfirmation(t('清除余额查询凭证？'),
      t('确认后立即生效，不受底部保存按钮控制。通常可继续复用转发用的登录凭证。'), t('清除'), true)) return
    actionLock.current = true
    setBusy(true)
    try {
      const saved = await updateAccountConfirmed(id, { balanceToken: null })
      setAccount(saved)
      setBalanceToken('')
      toast(t('✅ 已清除余额查询凭证'))
      try {
        await refreshSavedAccounts()
      } catch (error) {
        toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
      setBusy(false)
    }
  }

  /**
   * 清除 API Key（与余额凭证同一形制：立即落库的独立撤销动作）。
   * 清掉之后账号变成「未配置凭证」，转发时会被跳过 —— 所以提示语点明这一点，
   * 而列表上那枚「未配置凭证」徽章会立刻出现，用户看得到后果。
   */
  async function clearApiKey(): Promise<void> {
    if (interactionBusy || actionLock.current) return
    if (!await askConfirmation(t('清除已存 API Key？'),
      t('确认后立即生效，不受底部保存按钮控制。没有凭证且未开启「无需鉴权」时，此账号将被转发跳过。'), t('清除'), true)) return
    actionLock.current = true
    setBusy(true)
    try {
      const saved = await updateAccountConfirmed(id, { apiKey: null })
      setAccount(saved)
      setApiKeyDraft('')
      toast(t('✅ 已清除 API Key（该账号现在没有凭证，转发会被跳过）'))
      try {
        await refreshSavedAccounts()
      } catch (error) {
        toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
      setBusy(false)
    }
  }

  /**
   * 清除这个账号单独保存的上游地址，改回跟随提供商的 Base URL
   * （与清除 API Key 同一形制：立即落库的独立撤销动作）。
   *
   * 早先版本新建提供商时会把当时的地址抄进首个账号，而账号上的地址优先于
   * 提供商 —— 于是改提供商地址后请求仍打旧地址，界面上还看不到那份旧值
   * （issue #455）。这个按钮是存量账号的修复入口；新建的提供商不再产生
   * 这种覆盖项，改提供商地址时后端也会顺手清掉等于旧地址的那份。
   */
  async function clearAccountBaseUrl(): Promise<void> {
    if (interactionBusy || actionLock.current) return
    const nextBase = providerBaseUrl.trim()
    if (!await askConfirmation(t('改回跟随提供商？'),
      t('这个账号将改用提供商当前保存的 Base URL：{url}。确认后立即生效，不受底部保存按钮控制。',
        { url: nextBase || '—' }), t('改回跟随'))) return
    actionLock.current = true
    setBusy(true)
    try {
      const saved = await updateAccountConfirmed(id, { baseUrl: null })
      setAccount(saved)
      toast(t('✅ 已改回跟随提供商的 Base URL'))
      try {
        await refreshSavedAccounts()
      } catch (error) {
        toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
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
    if (interactionBusy || actionLock.current) return
    if (!await askConfirmation(t('清除查询凭证？'),
      t('确认后立即生效，余额查询可能显示「未配置」。取消账号设置不会恢复已清除的凭证。'), t('清除'), true)) return
    actionLock.current = true
    setBusy(true)
    try {
      const saved = await updateAccountConfirmed(id, { balanceQuery: { [key]: null } })
      setAccount(saved)
      setBalanceQueryDraft(prev => ({ ...prev, [key]: '' }))
      toast(t('✅ 已清除查询凭证'))
      try {
        await refreshSavedAccounts()
      } catch (error) {
        toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
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
    if (noAuthDraft) {
      if (target.noAuth !== true) patch.noAuth = true
    } else if (key) patch.apiKey = key
    else if (target.noAuth === true) patch.noAuth = false
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
    try {
      if (!['http:', 'https:'].includes(new URL(nextBase).protocol)) throw new Error('invalid protocol')
    } catch {
      return { error: t('请填写有效的 HTTP 或 HTTPS 地址') }
    }
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
    if (interactionBusy || actionLock.current || !dirtyCount) return
    setStatus('')
    let proxy: ProxyPayload
    try {
      proxy = readProxyDraft(proxyDraft)
    } catch (error) {
      selectPane('network')
      setStatus(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(errorMessage(error), 'err')
      return
    }
    const providerPatch = readProviderPatch()
    if (providerPatch && 'error' in providerPatch) {
      selectPane('provider')
      setStatus(<span className='text-destructive'>{providerPatch.error}</span>)
      return
    }
    // 查询设置段（自动查询间隔）：与凭证段同一层，跟着这次保存一起落库；
    // 校验不过先在前端挡住（后端 apply_patch 对同一规则也会 400）
    const usageChanges = supportsUsage(target) ? readUsageChanges(target, usageDraft) : null
    if (usageChanges && 'error' in usageChanges) {
      selectPane('query')
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
      selectPane('basic')
      setStatus(<span className='text-destructive'>{t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX })}</span>)
      toast(t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX }), 'err')
      return
    }
    const concNext = Math.min(CONC_MAX, Math.round(concRaw))
    const concCurrent = Number(target.maxConcurrent) || 0
    const concPatch: Record<string, unknown> = concNext !== concCurrent ? { maxConcurrent: concNext } : {}
    if (providerPatch && !await askConfirmation(t('保存提供商共用配置？'),
      t('「{name}」名下 {count} 个账号共用这些连接设置。修改协议或 Base URL 可能中断正在进行的请求。',
        { name: provider?.name || provider?.id || '', count: providerCount }), t('确认保存'))) return

    actionLock.current = true
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
      const nextName = name.trim() || target.name
      const saved = await updateAccountConfirmed(id, {
        ...(nextName !== target.name ? { name: nextName } : {}),
        enabled,
        allowRouteSkip,
        proxy,
        ...balancePatch,
        ...planPatch,
        ...credentialPatch,
        ...concPatch,
        ...(balanceQueryPatch ? { balanceQuery: balanceQueryPatch } : {}),
        ...(usageChanges && 'patch' in usageChanges ? usageChanges.patch : {}),
        ...(limiterChanges && 'patch' in limiterChanges ? limiterChanges.patch : {}),
      })
      if ((nextName !== target.name || target.nameCustom === true) && saved.nameCustom !== true) {
        throw new Error(t('账号更新结果未确认，请刷新后核对设置'))
      }
      setAccount(saved)
      // 提供商那一段排在账号之后（账号是本弹窗的主角，先落库）。它失败时账号已经存下了，
      // 所以留在弹窗里把那句话说清楚 —— 笼统报成「保存失败」会把两件事混成一件
      if (providerPatch) {
        try {
          const updateProvider = shared().wbCustomProvidersUi?.update
          if (!updateProvider) throw new Error(t('账号更新接口不可用，请重新打开页面'))
          await updateProvider(providerPatch)
        } catch (error) {
          selectPane('provider')
          setStatus(<span className='text-destructive'>{t('账号已保存，但提供商未更新：{error}', { error: errorMessage(error) })}</span>)
          try { await refreshSavedAccounts() }
          catch (refreshError) { toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(refreshError) }), 'err') }
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
        await refreshSavedAccounts()
        await syncBalancesSnapshot()
      } catch (error) {
        toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      // 后端校验失败（如优先级冲突）：留在弹窗里显示原因，方便直接改
      setStatus(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
      setBusy(false)
    }
  }

  // 内置账号仅在有余额凭证或套餐通道时显示凭证分区。
  const concDefault = Number(providerFeatures(providerOf(target)).concurrencyDefault) || 0
  const hasCredentialPane = isCatpaw || isCustomAccount || supportsPlanChannel(target)
  const accountPanes: PaneId[] = ['basic', ...(hasCredentialPane ? ['credential' as const] : []),
    ...(supportsUsage(target) ? ['query' as const] : []), 'limiter', 'network']
  const providerLabel = provider?.name || shared().wbProviders?.labelOf?.(providerOf(target)) || providerOf(target)

  async function removeProvider(): Promise<void> {
    if (interactionBusy || actionLock.current || !provider) return
    actionLock.current = true
    setConfirming(true)
    try {
      const remove = shared().wbCustomProvidersUi?.remove
      if (!remove) throw new Error(t('账号更新接口不可用，请重新打开页面'))
      if (await remove(provider.id)) onClose()
    } catch (error) {
      toast(t('删除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      actionLock.current = false
      setConfirming(false)
    }
  }

  async function openModels(): Promise<void> {
    if (interactionBusy || actionLock.current) return
    if (dirtyCount && !await askConfirmation(t('放弃未保存的修改？'),
      t('前往模型管理会关闭账号设置，尚未保存的修改不会生效。'), t('前往模型管理'))) return
    onClose()
    shared().wbApp?.showPage?.('gateway')
  }

  return (
    <Dialog open onOpenChange={(next, details) => {
      if (next) return
      details.cancel()
      void close()
    }}>
      <DialogContent className='acct-settings-dialog' showCloseButton={!interactionBusy}>
        <DialogHeader className='acct-dlg-header'>
          <div className='acct-dlg-avatar'><AccountSettingsIcon name={isCustomAccount ? 'agent' : 'accounts'} size={23} /></div>
          <div className='acct-dlg-identity'>
            <div className='acct-dlg-title-row'><DialogTitle>{t('账号设置')}</DialogTitle>
              {isCustomAccount ? <Badge variant='secondary' shape='tag'>{t('自定义 API')}</Badge> : null}
            </div>
            <p><span>{labelOf(account)}</span><span className='acct-dlg-title-dot'>·</span>
              <span className={`acct-dlg-account-state${account.enabled === false ? ' off' : ''}`}>
                <BadgeDot />{account.enabled === false ? t('已停用') : t('已启用')}
              </span>
            </p>
          </div>
        </DialogHeader>
        <DialogBody className='acct-dlg-body'>
          <div className='acct-dlg'>
            <Sidebar className='acct-dlg-rail' aria-label={t('账号设置分组')}>
              <NavList className='acct-dlg-nav'>
                <div className='acct-dlg-nav-label'>{t('当前账号')}</div>
                {accountPanes.map(id => <NavItem key={id} active={pane === id} title={paneInfo[id].title}
                  disabled={interactionBusy} className='shadow-none'
                  icon={<AccountSettingsIcon name={paneInfo[id].icon} />}
                  trailing={dirtyPanes[id] ? <span className='acct-dlg-dirty-dot' aria-label={t('有未保存的修改')} /> : undefined}
                  onClick={() => selectPane(id)}>{paneInfo[id].title}</NavItem>)}
                {provider ? <>
                  <div className='acct-dlg-nav-divider' />
                  <div className='acct-dlg-nav-label'>{t('提供商')}<span>{t('共用')}</span></div>
                  <NavItem active={pane === 'provider'} disabled={interactionBusy} title={paneInfo.provider.title}
                    icon={<AccountSettingsIcon name={paneInfo.provider.icon} />}
                    trailing={dirtyPanes.provider ? <span className='acct-dlg-dirty-dot' aria-label={t('有未保存的修改')} /> : undefined}
                    onClick={() => selectPane('provider')}>{paneInfo.provider.title}</NavItem>
                </> : null}
              </NavList>
              {provider ? <div className='acct-dlg-rail-context'><strong><AccountSettingsIcon name='accounts' size={14} />{providerLabel}</strong>
                <p>{t('{count} 个账号共用连接配置', { count: providerCount })}</p>
              </div> : null}
            </Sidebar>
            <div className='acct-dlg-main'>
              <div className='acct-dlg-pane-head'>
                <div><h3>{currentPane.title}</h3><p>{currentPane.description}</p></div>
                <span className={`acct-dlg-scope${pane === 'provider' ? ' shared' : ''}`}>
                  <AccountSettingsIcon name='accounts' size={13} />
                  {pane === 'provider' ? t('{count} 个账号共用', { count: providerCount }) : t('仅当前账号')}
                </span>
              </div>
              <div ref={paneScroll} className='acct-dlg-scroll' aria-busy={busy}>
                <div className='acct-dlg-pane' inert={interactionBusy ? true : undefined}>
                  {pane === 'basic' ? (
                    <>
                      <SettingGroup title={t('账号信息')}>
                        <Field className='acct-dlg-field'>
                          <FieldLabel htmlFor='account-name-input'>{t('备注名')}</FieldLabel>
                          <Input id='account-name-input' maxLength={100} placeholder={t('账号显示名称')}
                            value={name} onChange={event => setName(event.currentTarget.value)} />
                        </Field>
                        <div className='acct-dlg-readonly'><span>{t('账号 ID')}</span><code>{target.id}</code></div>
                        <Field className='acct-dlg-toggle-field'>
                          <div><FieldLabel htmlFor='account-enabled'>{t('启用账号')}</FieldLabel>
                            <FieldDescription>{t('关闭后不参与转发，余额查询不受影响。')}</FieldDescription>
                          </div>
                          <Switch id='account-enabled' checked={enabled} onCheckedChange={setEnabled} />
                        </Field>
                      </SettingGroup>
                      <SettingGroup title={t('转发行为')}>
                        <Field className='acct-dlg-toggle-field'>
                          <div><FieldLabel htmlFor='account-route-skip'>{t('失败后允许换号')}</FieldLabel>
                            <FieldDescription>{t('关闭后，该账号转发失败时返回本次错误，不再顺延其他账号。禁用、无凭证和限制器等保护仍生效。')}</FieldDescription>
                          </div>
                          <Switch id='account-route-skip' checked={allowRouteSkip} onCheckedChange={setAllowRouteSkip} />
                        </Field>
                        <Field className='acct-dlg-concurrency'>
                          <div><FieldLabel htmlFor='account-conc-input'>{t('并发上限')}</FieldLabel>
                            <FieldDescription>{t('达到上限后，将请求交给其他账号。')}</FieldDescription>
                          </div>
                          <Input id='account-conc-input' type='number' min={0} max={CONC_MAX} step={1}
                            value={concDraft} onChange={event => setConcDraft(event.currentTarget.value)} />
                        </Field>
                      </SettingGroup>
                      <p className='detail acct-dlg-hint'>{concDefault > 0
                        ? t('0 = 按本家默认 {default}', { default: concDefault }) : t('0 = 不限')}</p>
                      {routeContext ? <div className='acct-dlg-route-context'>{routeContext}</div>
                        : <p className='detail acct-dlg-group-note'><AccountSettingsIcon name='route' size={14} />
                          {t('在账号列表中拖动分组或行首把手，调整转发顺序。')}</p>}
                    </>
                  ) : null}

                  {pane === 'credential' && hasCredentialPane ? <>
                    {isCatpaw ? <BalanceTokenField configured={account.hasBalanceToken === true} value={balanceToken}
                      onChange={setBalanceToken} clearBusy={interactionBusy} onClear={() => void clearBalanceToken()} /> : null}
                    {isCustomAccount ? <CustomCredentialFields hasApiKey={target.hasApiKey === true} noAuth={noAuthDraft}
                      value={apiKeyDraft} clearBusy={interactionBusy} onClear={() => void clearApiKey()}
                      onValue={next => { setApiKeyDraft(next); if (next.trim()) setNoAuthDraft(false) }}
                      onToggleNoAuth={next => { setNoAuthDraft(next); if (next) setApiKeyDraft('') }} /> : null}
                    {supportsPlanChannel(target) ? <SettingGroup title={t('使用套餐')}>
                      <PlanChannelField plan={planChannel} hasJwt={account.canClaim === true} onChange={setPlanChannel} />
                    </SettingGroup> : null}
                    {provider ? <div className='acct-dlg-related'>
                      <AccountSettingsIcon name='agent' size={18} />
                      <div><h3>{t('要修改连接地址或协议？')}</h3><p>{t('{count} 个账号共用提供商的连接配置。', { count: providerCount })}</p></div>
                      <Button variant='link' size='xs' onClick={() => selectPane('provider')}>{t('连接设置')}<AccountSettingsIcon name='caretRight' size={12} /></Button>
                    </div> : null}
                  </> : null}

                  {pane === 'query' && supportsUsage(target) ? <>
                    <SettingGroup title={t('自动查询余额')}>
                      <UsageSettingsSection draft={usageDraft} onChange={setUsageDraft} />
                    </SettingGroup>
                    {balanceSpec ? <SettingGroup title={t('查询凭证')} action={<Badge variant='secondary' shape='tag'>{balanceSpec.label}</Badge>}>
                      <BalanceQuerySection spec={balanceSpec} values={balanceQueryDraft} configured={balanceConfigured}
                        onValue={(key, next) => setBalanceQueryDraft(prev => ({ ...prev, [key]: next }))}
                        onClear={key => void clearBalanceQueryField(key)} clearBusy={interactionBusy} idPrefix='account-balance-query' />
                      {balanceNeedsCredentials ? <p className='detail acct-dlg-inline-note'>{t('查询凭证还没配齐：填好后余额列才有读数，在此之前会显示「未配置」。')}</p> : null}
                    </SettingGroup> : null}
                    <p className='detail acct-dlg-group-note'><AccountSettingsIcon name='info' size={14} />{t('余额读数也是限制器余额规则的数据来源。')}</p>
                  </> : null}

                  {pane === 'limiter' ? <>
                    <LimiterSection account={target} draft={limiterDraft} onChange={setLimiterDraft} allowBalance={supportsUsage(target)} />
                    <p className='detail acct-dlg-group-note'><AccountSettingsIcon name='info' size={14} />
                      {t('没有规则或全部停用时不限制；跳过后自动恢复，禁用后需手动启用。')}</p>
                  </> : null}

                  {pane === 'network' ? <>
                    <SettingGroup title={t('出网方式')}>
                      <ProxyForm draft={proxyDraft} onChange={setProxyDraft} idPrefix='account-proxy' allowRotate={allowRotate} />
                      {account.proxy?.error ? <p className='detail text-destructive acct-dlg-block'>
                        {t('当前代理不可用：{error}（转发时会回退直连）', { error: account.proxy.error })}
                      </p> : null}
                    </SettingGroup>
                    <p className='detail acct-dlg-group-note'><AccountSettingsIcon name='shield' size={14} />{t('代理只作用于当前账号，不改变其他账号的出网方式。')}</p>
                  </> : null}
                  {pane === 'provider' && provider ? <ProviderSection provider={provider} count={providerCount}
                    name={providerName} protocol={providerProtocol} baseUrl={providerBaseUrl}
                    accountBaseUrl={target.baseUrl || ''} emulation={providerEmulation}
                    onName={setProviderName} onProtocol={setProviderProtocol} onBaseUrl={setProviderBaseUrl}
                    onEmulation={setProviderEmulation} onClearAccountBaseUrl={() => void clearAccountBaseUrl()}
                    disabled={interactionBusy} onRemove={() => void removeProvider()} onModels={() => void openModels()} /> : null}
                </div>
              </div>
            </div>
          </div>
        </DialogBody>
        <DialogFooter className='acct-dlg-footer'>
          <div className={`acct-dlg-save-state${dirtyCount ? ' dirty' : ''}`} role='status'>
            <AccountSettingsIcon name={status ? 'info' : dirtyCount ? 'timer' : 'shield'} size={16} />
            <div><strong>{status || (busy ? t('正在保存…') : dirtyCount
              ? t('{count} 个分区有未保存的更改', { count: dirtyCount }) : t('没有未保存的更改'))}</strong>
              <p>{dirtyPanes.provider ? t('包含提供商共用配置，将影响 {count} 个账号', { count: providerCount }) : t('各分区的更改将统一保存')}</p>
            </div>
          </div>
          <div className='acct-dlg-footer-actions'>
            <Button variant='outline' disabled={interactionBusy} onClick={() => void close()}>{t('取消')}</Button>
            <Button variant='default' disabled={interactionBusy || !dirtyCount} onClick={() => void save()}>{busy ? t('保存中…') : t('保存更改')}</Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
