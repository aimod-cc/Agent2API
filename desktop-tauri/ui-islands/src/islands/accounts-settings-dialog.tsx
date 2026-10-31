/** 单账号设置弹窗：基本、凭证与套餐、查询设置、限制器和网络。 */

import * as React from 'react'
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogSection, DialogTitle, Input, Label, NavList, NavItem, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue, Switch,
  Field, FieldGroup, FieldLabel,
} from '@ui'
import { t } from '../i18n'
import {
  ACCOUNT_CONCURRENCY_MAX as CONC_MAX, errorMessage, shared, toast,
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
import { Sidebar } from '@ui'

/** 与后端 custom_providers::MAX_NAME_CHARS 一致。 */
const MAX_PROVIDER_NAME_CHARS = 64

type AccountLike = AccountRecord
type PaneId = 'basic' | 'credential' | 'query' | 'limiter' | 'network'

function peersOf(account: AccountLike, all: AccountLike[]): AccountLike[] {
  const provider = providerOf(account)
  return all.filter(item => item.id !== account.id && providerOf(item) === provider)
}

function labelOf(account: AccountLike | null | undefined): string {
  return displayNameOf(account) || account?.id || ''
}

/* ─── 账号设置弹窗 ───────────────────────────── */

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
    <Field orientation='horizontal' className='field-row mt-2.5'>
      <FieldLabel htmlFor='account-balance-token-input'>{t('余额查询凭证')}</FieldLabel>
      <Input id='account-balance-token-input' className='min-w-[220px]'
        placeholder={configured ? t('已配置，留空则不修改') : t('一般不用填')}
        title={t('积分查询已复用转发用的登录凭证，这里通常留空即可。只有旧版本填过、或从旧代理导入过凭证时才有值')}
        value={value} onChange={event => onChange(event.currentTarget.value)} />
      {configured ? (
        <Button variant='outline' size='sm' disabled={clearBusy} onClick={onClear}
          title={t('清除已配置的余额查询凭证')}>{t('清除')}</Button>
      ) : null}
    </Field>
  )
}

/**
 * Qoder 网页 Cookie（积分到期明细）。
 *
 * 网页接口只认网页会话 Cookie，openapi 的 accessToken 读不到逐份积分包；
 * 所以这里给桌面端一个「打开页面自动获取」的入口。手动兜底仍在添加账号/
 * 重新导入表单的 `网页 Cookie（可选）` 字段里。
 */
function QoderWebCookieField({ account, configured }: { account: AccountRecord; configured: boolean }) {
  const [busy, setBusy] = React.useState(false)
  const [saved, setSaved] = React.useState(configured)
  const captureFn = shared().workbuddyDesktop?.captureQoderWebCookie
  const canCapture = Boolean(captureFn)

  const capture = async () => {
    if (!captureFn) {
      toast(t('当前环境不支持自动获取；请在添加账号/重新导入时粘贴网页 Cookie'), 'err')
      return
    }
    setBusy(true)
    try {
      const result = await captureFn(account.id, providerOf(account))
      setSaved(result?.configured !== false)
      toast(t('✅ 已获取 Qoder 网页 Cookie'))
      try { await refreshSavedAccounts() } catch { /* 列表刷新失败不影响已保存的 Cookie */ }
    } catch (error) {
      toast(t('获取 Cookie 失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className='mt-3 rounded-sm border border-hairline px-2.5 py-2.5'>
      <div className='flex items-center gap-2'>
        <span className='text-xs font-medium'>{t('网页 Cookie（积分到期明细）')}</span>
        <span className={saved ? 'text-[10.5px] text-success' : 'text-[10.5px] text-subtle'}>
          {saved ? t('已配置') : t('未配置')}
        </span>
      </div>
      <p className='detail mt-1'>
        {t('用于读取 Qoder 积分包的到期时间，不影响聊天登录。打开窗口登录一次即可；Cookie 失效后重新获取。')}
      </p>
      <div className='mt-2 flex items-center gap-2'>
        <Button size='sm' disabled={busy || !canCapture} onClick={() => void capture()}>
          {busy ? t('等待登录完成…') : saved ? t('重新获取') : t('打开页面获取')}
        </Button>
        {!canCapture ? <span className='detail'>{t('当前环境不支持自动获取，请用重新导入时的网页 Cookie 字段')}</span> : null}
      </div>
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
      <Field orientation='horizontal' className='field-row mt-2.5'>
        <FieldLabel htmlFor='account-api-key-input'>{t('API Key')}</FieldLabel>
        <Input id='account-api-key-input' type='password' autoComplete='new-password'
          className='min-w-[220px]' disabled={noAuth}
          placeholder={hasApiKey ? t('已配置，留空则不修改') : 'sk-…'}
          title={t('上游的 API Key。留空 = 不修改（改凭证不会换账号 id）；要清掉已配的凭证用右边的「清除」')}
          value={value} onChange={event => onValue(event.currentTarget.value)} />
        {hasApiKey && !noAuth ? (
          <Button variant='outline' size='sm' disabled={clearBusy} onClick={onClear}
            title={t('清除已配置的 API Key（转发时会因缺少凭证被跳过）')}>{t('清除')}</Button>
        ) : null}
      </Field>
      <Field orientation='horizontal' className='field-row mt-2.5'>
        <FieldLabel className='inline-flex cursor-pointer items-center gap-2.5 font-normal'>
          <Checkbox checked={noAuth} aria-label={t('该上游无需鉴权')}
            onCheckedChange={next => onToggleNoAuth(next === true)} />
          <span className='text-xs text-subtle'>{t('该上游无需鉴权（转发与拉取模型都不发送鉴权头）')}</span>
        </FieldLabel>
      </Field>
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
      <Field orientation='horizontal' className='field-row'>
        <FieldLabel htmlFor='account-provider-name'>{t('名称')}</FieldLabel>
        <Input id='account-provider-name' maxLength={MAX_PROVIDER_NAME_CHARS} className='min-w-[220px]'
          placeholder={t('提供商显示名，1~{max} 个字符', { max: MAX_PROVIDER_NAME_CHARS })}
          value={name} onChange={event => onName(event.currentTarget.value)} />
      </Field>
      <Field orientation='horizontal' className='field-row mt-2.5'>
        <FieldLabel htmlFor='account-provider-protocol'>{t('协议')}</FieldLabel>
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
      </Field>
      <Field orientation='horizontal' className='field-row mt-2.5'>
        <FieldLabel htmlFor='account-provider-baseurl'>{t('Base URL')}</FieldLabel>
        <Input id='account-provider-baseurl' className='min-w-[260px]'
          placeholder={t('OpenAI 兼容填到 /v1；Anthropic 填根地址')}
          value={baseUrl} onChange={event => onBaseUrl(event.currentTarget.value)} />
      </Field>
      <Field orientation='horizontal' className='field-row mt-2.5'>
        <FieldLabel className='inline-flex cursor-pointer items-center gap-2.5 font-normal'>
          <Checkbox checked={emulation === 'opencode'} aria-label={t('伪装 OpenCode 官方客户端')}
            onCheckedChange={next => onEmulation(next === true ? 'opencode' : '')} />
          <span className='text-xs text-subtle'>{t('伪装 OpenCode 官方客户端（OpenCode Zen 免费档要它才放行）')}</span>
        </FieldLabel>
      </Field>
      <p className='detail'>
        {t('开启后，网关按 OpenCode 官方 CLI 的形状发请求：没填 Key 的账号用匿名凭证')}
        <code>{'public'}</code>
        {t('、补官方会话头、请求体补两个占位工具（上游免费档的三道校验）。')}
        <strong>{t('它会改写请求体')}</strong>
        {t('，只对 OpenCode Zen 这类上游有意义，别的家请关掉。')}
      </p>
      <FieldGroup orientation='horizontal' className='field-row mt-3'>
        <Button variant='destructive' onClick={onRemove}>{t('删除提供商')}</Button>
        <span className='detail'>{t('级联删除名下全部账号，不可恢复')}</span>
      </FieldGroup>
    </DialogSection>
  )
}

export function AccountSettingsDialog({ id, onClose, initialAccount, initialPane = 'basic', routeContext }: {
  id: string; onClose: () => void
  initialAccount?: AccountRecord
  initialPane?: PaneId
  routeContext?: React.ReactNode
}) {
  const account = initialAccount || findAccount(id)
  const [enabled, setEnabled] = React.useState(account?.enabled !== false)
  const [allowRouteSkip, setAllowRouteSkip] = React.useState(account?.allowRouteSkip !== false)
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
  const [limiterDraft, setLimiterDraft] = React.useState<LimiterGroup[]>(() => limiterDraftOf(account))
  // 分组导航的当前页（弹窗按账号 id 挂载，切账号自然回到「基本」）
  const [pane, setPane] = React.useState<PaneId>(initialPane)
  // 并发上限草稿（原列表 ⋯ 菜单的 conc-dialog 迁入「基本」分组）：0~999，0 = 不限
  const [concDraft, setConcDraft] = React.useState(() => String(Number(account?.maxConcurrent) || 0))

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
  const isQoder = ['qoder', 'qoder-intl'].includes(providerOf(target))
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
      await updateAccountConfirmed(id, { balanceToken: null })
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
      await updateAccountConfirmed(id, { apiKey: null })
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
      await updateAccountConfirmed(id, { balanceQuery: { [key]: null } })
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
      // 提供商那一段排在账号之后（账号是本弹窗的主角，先落库）。它失败时账号已经存下了，
      // 所以留在弹窗里把那句话说清楚 —— 笼统报成「保存失败」会把两件事混成一件
      if (providerPatch) {
        try {
          const updateProvider = shared().wbCustomProvidersUi?.update
          if (!updateProvider) throw new Error(t('账号更新接口不可用，请重新打开页面'))
          await updateProvider(providerPatch)
        } catch (error) {
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
      setBusy(false)
    }
  }

  // 「凭证与套餐」按家出现：有什么配什么（CatPaw 余额凭证 / 自定义账号凭证与
  // 提供商 / ZCode 套餐通道）。内置家什么都没有时整组不出现。
  const concDefault = Number(providerFeatures(providerOf(target)).concurrencyDefault) || 0
  const hasCredentialPane = isCatpaw || isQoder || isCustomAccount || supportsPlanChannel(target)

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
            <Sidebar className='prov-rail acct-dlg-rail' aria-label={t('账号设置分组')}>
              <NavList className='rail-scroll'>
                <div className='rail-label'>{t('账号设置')}</div>
                {([['basic', t('基本')], ...(hasCredentialPane ? [['credential', t('凭证与套餐')] as const] : []),
                  ...(supportsUsage(target) ? [['query', t('查询设置')] as const] : []),
                  ['limiter', t('限制器')], ['network', t('网络')]] as Array<[PaneId, string]>).map(([id, label]) => (
                    <NavItem key={id} active={pane === id} title={label} className='shadow-none'
                      onClick={() => setPane(id)}>{label}</NavItem>
                  ))}
              </NavList>
            </Sidebar>
            <div className='acct-dlg-main'>
              {pane === 'basic' ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('基本')}</h3>
                    <p>{t('这条账号是谁、排第几、能不能接请求。')}</p>
                  </div>
                  <Field orientation='horizontal' className='field-row'>
                    <FieldLabel>{t('启用')}</FieldLabel>
                    <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用该账号')} />
                    <span className='detail'>{t('关闭则不参与转发（余额查询照常，见「查询设置」）')}</span>
                  </Field>
                  <Field orientation='horizontal' className='field-row'>
                    <FieldLabel htmlFor='account-name-input'>{t('备注名')}</FieldLabel>
                    <Input id='account-name-input' maxLength={100} placeholder={t('账号显示名称')} className='max-w-[280px]'
                      value={name} onChange={event => setName(event.currentTarget.value)} />
                  </Field>
                  {routeContext || <FieldGroup orientation='horizontal' className='field-row'>
                    <Label>{t('转发顺序')}</Label>
                    <span className='detail'>
                      {t('在列表里拖：拖分组带调整整组顺位，拖行首把手调整组内先后（顺序由拖拽表达，不再填数字）')}
                    </span>
                  </FieldGroup>}
                  <Field orientation='horizontal' className='field-row'>
                    <FieldLabel htmlFor='account-route-skip'>{t('失败后允许换号')}</FieldLabel>
                    <Switch id='account-route-skip' checked={allowRouteSkip} onCheckedChange={setAllowRouteSkip} />
                  </Field>
                  <p className='detail'>{t('关闭后，该账号转发失败时返回本次错误，不再顺延其他账号。禁用、无凭证和限制器等保护仍生效。')}</p>
                  <Field orientation='horizontal' className='field-row'>
                    <FieldLabel htmlFor='account-conc-input'>{t('并发上限')}</FieldLabel>
                    <Input id='account-conc-input' type='number' min={0} max={CONC_MAX} step={1}
                      className='max-w-[110px]' value={concDraft}
                      onChange={event => setConcDraft(event.currentTarget.value)} />
                    <span className='detail'>{concDefault > 0 ? t('0 = 按本家默认 {default}', { default: concDefault }) : t('0 = 不限')}</span>
                  </Field>
                  <p className='detail acct-dlg-hint'>
                    {concDefault > 0
                      ? t('并发上限 填 0 即按本家默认 {default}；填 1 或 2 就按填的算，更大的数会被本家上限截断（原列表 ⋯ 菜单的「并发上限」入口已并入这里）', { default: concDefault })
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
                  {isQoder ? <QoderWebCookieField account={target} configured={target.hasWebCookie === true} /> : null}
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
                  <UsageSettingsSection draft={usageDraft} onChange={setUsageDraft} />
                </div>
              ) : null}

              {pane === 'limiter' ? (
                <div>
                  <div className='acct-dlg-pane-head'>
                    <h3>{t('限制器')}</h3>
                    <p>
                      {t('按规则限制该账号参与转发：没有规则（或全部停用）= 不限制；跳过档自动恢复，禁用档需手动启用。')}
                    </p>
                  </div>
                  <LimiterSection account={target} draft={limiterDraft} onChange={setLimiterDraft} allowBalance={supportsUsage(target)} />
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
