/**
 * 「添加账号」弹窗右侧非主分栏（转发行为 / 查询设置 / 限制器 / 网络）及共享表单状态。
 *
 * 与「账号设置」弹窗共用同一套 SettingGroup 排版和子控件（UsageSettingsSection /
 * LimiterSection / BalanceQuerySection / ProxyForm / UserAgentInput），让用户
 * 在创建账号时就能一次性配齐所有属性，无需添加完再去账号设置补填。
 */

import * as React from 'react'
import { Badge, Field, FieldDescription, FieldLabel, Input, Switch } from '@ui'
import { t } from '../i18n'
import { AccountSettingsIcon, SettingGroup } from './accounts-settings-fields'
import { readUsageDraft, usageDraftOf, UsageSettingsSection, type UsageDraft } from './accounts-dialog-usage'
import { LimiterSection } from './accounts-dialog-limiter'
import type { LimiterGroup } from './accounts-limiter-types'
import { ProxyForm } from './accounts-proxy-form'
import { useAddAccountProxy, useAddAccountProxyDraft } from './add-account-proxy'
import { UserAgentInput, useUserAgentPresets, userAgentEditable } from './add-account-user-agent'
import { BalanceQuerySection, balanceSpecOf, type BalanceFieldKey } from './balance-query'
import { accountRequest } from './add-account-bridge'
import { ACCOUNT_CONCURRENCY_MAX as CONC_MAX } from './accounts-shared'

export type AddPaneId = 'basic' | 'forwarding' | 'query' | 'limiter' | 'network'

type BuildSettingsOptions = {
  supportsUsage: boolean
  uaEditable: boolean
  balanceScheme?: string
}

type BuildSettingsResult =
  | { error: string; pane: AddPaneId }
  | { payload: Record<string, unknown> }

type AddAccountSettingsValue = {
  pane: AddPaneId
  setPane: (pane: AddPaneId) => void
  enabled: boolean
  setEnabled: (next: boolean) => void
  allowRouteSkip: boolean
  setAllowRouteSkip: (next: boolean) => void
  concDraft: string
  setConcDraft: (next: string) => void
  userAgent: string
  setUserAgent: (next: string) => void
  usageDraft: UsageDraft
  setUsageDraft: (next: UsageDraft) => void
  balanceValues: Record<string, string>
  setBalanceValue: (key: BalanceFieldKey, next: string) => void
  limiterDraft: LimiterGroup[]
  setLimiterDraft: (next: LimiterGroup[]) => void
  dirtyPanes: Record<AddPaneId, boolean>
  readSettingsPayload: (options: BuildSettingsOptions) => BuildSettingsResult
  applySettingsAfterCreate: (
    accountId: string | undefined,
    provider: string,
    options: Omit<BuildSettingsOptions, 'balanceScheme'>,
  ) => Promise<string | undefined>
  resetAll: () => void
}

const Context = React.createContext<AddAccountSettingsValue | null>(null)

function readOptionalBalanceQuery(
  scheme: string | undefined,
  values: Record<string, string>,
): Record<string, string> | undefined {
  const spec = balanceSpecOf(scheme)
  if (!spec || !spec.fields.length) return undefined
  const patch: Record<string, string> = {}
  for (const field of spec.fields) {
    const value = String(values[field.key] ?? '').trim()
    if (value) patch[field.key] = value
  }
  return Object.keys(patch).length ? patch : undefined
}

export function AddAccountSettingsProvider({
  contextKey,
  children,
}: {
  contextKey: string
  children: React.ReactNode
}) {
  const proxy = useAddAccountProxy()
  const [pane, setPane] = React.useState<AddPaneId>('basic')
  const [enabled, setEnabled] = React.useState(true)
  const [allowRouteSkip, setAllowRouteSkip] = React.useState(true)
  const [concDraft, setConcDraft] = React.useState('0')
  const [userAgent, setUserAgent] = React.useState('')
  const [usageDraft, setUsageDraft] = React.useState<UsageDraft>(() => usageDraftOf(null))
  const [balanceValues, setBalanceValues] = React.useState<Record<string, string>>({})
  const [limiterDraft, setLimiterDraft] = React.useState<LimiterGroup[]>([])
  const [limiterDirty, setLimiterDirty] = React.useState(false)

  const initialUsageRef = React.useRef<UsageDraft>(usageDraftOf(null))
  const prevKeyRef = React.useRef(contextKey)

  const resetAll = React.useCallback(() => {
    setPane('basic')
    setEnabled(true)
    setAllowRouteSkip(true)
    setConcDraft('0')
    setUserAgent('')
    const freshUsage = usageDraftOf(null)
    initialUsageRef.current = freshUsage
    setUsageDraft(freshUsage)
    setBalanceValues({})
    setLimiterDraft([])
    setLimiterDirty(false)
    proxy.resetProxy()
  }, [proxy])

  // 切换提供商时重置账号级附加配置与侧栏页签，避免把上一家的参数带入新表单。
  React.useEffect(() => {
    if (prevKeyRef.current === contextKey) return
    prevKeyRef.current = contextKey
    resetAll()
  }, [contextKey, resetAll])

  const setBalanceValue = React.useCallback((key: BalanceFieldKey, next: string) => {
    setBalanceValues(prev => ({ ...prev, [key]: next }))
  }, [])

  const updateLimiter = React.useCallback((next: LimiterGroup[]) => {
    setLimiterDraft(next)
    setLimiterDirty(true)
  }, [])

  const latestRef = React.useRef({
    enabled,
    allowRouteSkip,
    concDraft,
    userAgent,
    usageDraft,
    balanceValues,
    limiterDraft,
    limiterDirty,
  })
  latestRef.current = {
    enabled,
    allowRouteSkip,
    concDraft,
    userAgent,
    usageDraft,
    balanceValues,
    limiterDraft,
    limiterDirty,
  }

  const readSettingsPayload = React.useCallback((options: BuildSettingsOptions): BuildSettingsResult => {
    const state = latestRef.current
    const concText = state.concDraft.trim()
    const concNum = Number(concText)
    if (!concText || !Number.isFinite(concNum) || !Number.isInteger(concNum) || concNum < 0 || concNum > CONC_MAX) {
      return { error: t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX }), pane: 'forwarding' }
    }

    const payload: Record<string, unknown> = {}
    if (!state.enabled) payload.enabled = false
    if (!state.allowRouteSkip) payload.allowRouteSkip = false
    if (concNum > 0) payload.maxConcurrent = concNum
    if (options.uaEditable && state.userAgent.trim()) {
      payload.userAgent = state.userAgent.trim()
    }

    if (options.supportsUsage) {
      const usageResult = readUsageDraft(state.usageDraft)
      if ('error' in usageResult) {
        return { error: usageResult.error, pane: 'query' }
      }
      const initialUsage = readUsageDraft(initialUsageRef.current)
      const isDefaultUsage = 'value' in initialUsage
        && JSON.stringify(usageResult.value) === JSON.stringify(initialUsage.value)
      if (!isDefaultUsage) {
        payload.usageQuery = usageResult.value
      }
    }

    const balanceQuery = readOptionalBalanceQuery(options.balanceScheme, state.balanceValues)
    if (balanceQuery) payload.balanceQuery = balanceQuery

    if (state.limiterDirty || state.limiterDraft.length > 0) {
      payload.limiterGroups = state.limiterDraft
    }

    return { payload }
  }, [])

  const applySettingsAfterCreate = React.useCallback(async (
    accountId: string | undefined,
    provider: string,
    options: Omit<BuildSettingsOptions, 'balanceScheme'>,
  ): Promise<string | undefined> => {
    if (!accountId) return undefined
    const effectiveUaEditable = options.uaEditable && userAgentEditable(provider)
    const built = readSettingsPayload({
      supportsUsage: options.supportsUsage,
      uaEditable: effectiveUaEditable,
    })
    if ('error' in built) return built.error
    if (!Object.keys(built.payload).length) return undefined
    try {
      await accountRequest('PATCH', `/api/accounts/${encodeURIComponent(accountId)}`, built.payload)
      return undefined
    } catch {
      return t('账号已添加，但部分附加设置未能同步保存，请在账号设置中重新确认。')
    }
  }, [readSettingsPayload])

  const dirtyPanes: Record<AddPaneId, boolean> = React.useMemo(() => ({
    basic: false,
    forwarding: !enabled || !allowRouteSkip || concDraft.trim() !== '0' || Boolean(userAgent.trim()),
    query: JSON.stringify(usageDraft) !== JSON.stringify(initialUsageRef.current)
      || Object.values(balanceValues).some(value => Boolean(value.trim())),
    limiter: limiterDirty || limiterDraft.length > 0,
    network: proxy.dirty,
  }), [enabled, allowRouteSkip, concDraft, userAgent, usageDraft, balanceValues, limiterDirty, limiterDraft.length, proxy.dirty])

  const value = React.useMemo<AddAccountSettingsValue>(() => ({
    pane,
    setPane,
    enabled,
    setEnabled,
    allowRouteSkip,
    setAllowRouteSkip,
    concDraft,
    setConcDraft,
    userAgent,
    setUserAgent,
    usageDraft,
    setUsageDraft,
    balanceValues,
    setBalanceValue,
    limiterDraft,
    setLimiterDraft: updateLimiter,
    dirtyPanes,
    readSettingsPayload,
    applySettingsAfterCreate,
    resetAll,
  }), [
    pane,
    enabled,
    allowRouteSkip,
    concDraft,
    userAgent,
    usageDraft,
    balanceValues,
    setBalanceValue,
    limiterDraft,
    updateLimiter,
    dirtyPanes,
    readSettingsPayload,
    applySettingsAfterCreate,
    resetAll,
  ])

  return <Context.Provider value={value}>{children}</Context.Provider>
}

export function useAddAccountSettings(): AddAccountSettingsValue {
  const value = React.useContext(Context)
  if (!value) throw new Error('Add account settings context is unavailable')
  return value
}

export function AddAccountForwardingPane({
  concDefault = 0,
  uaEditable,
  busy,
}: {
  concDefault?: number
  uaEditable: boolean
  busy: boolean
}) {
  const settings = useAddAccountSettings()
  const presets = useUserAgentPresets()
  return (
    <div inert={busy ? true : undefined}>
      <SettingGroup title={t('转发行为')}>
        <Field className='acct-dlg-toggle-field'>
          <div>
            <FieldLabel htmlFor='add-account-enabled'>{t('启用账号')}</FieldLabel>
            <FieldDescription>{t('关闭后不参与转发，余额查询不受影响。')}</FieldDescription>
          </div>
          <Switch id='add-account-enabled' checked={settings.enabled} onCheckedChange={settings.setEnabled} />
        </Field>
        <Field className='acct-dlg-toggle-field'>
          <div>
            <FieldLabel htmlFor='add-account-route-skip'>{t('失败后允许换号')}</FieldLabel>
            <FieldDescription>{t('关闭后，该账号转发失败时返回本次错误，不再顺延其他账号。禁用、无凭证和限制器等保护仍生效。')}</FieldDescription>
          </div>
          <Switch id='add-account-route-skip' checked={settings.allowRouteSkip} onCheckedChange={settings.setAllowRouteSkip} />
        </Field>
        <Field className='acct-dlg-concurrency'>
          <div>
            <FieldLabel htmlFor='add-account-conc-input'>{t('并发上限')}</FieldLabel>
            <FieldDescription>{t('达到上限后，将请求交给其他账号。')}</FieldDescription>
          </div>
          <Input
            id='add-account-conc-input'
            type='number'
            min={0}
            max={CONC_MAX}
            step={1}
            value={settings.concDraft}
            onChange={event => settings.setConcDraft(event.currentTarget.value)}
          />
        </Field>
      </SettingGroup>
      <p className='detail acct-dlg-hint'>
        {concDefault > 0 ? t('0 = 按本家默认 {default}', { default: concDefault }) : t('0 = 不限')}
      </p>
      {uaEditable ? (
        <SettingGroup title={t('请求标识')}>
          <Field className='acct-dlg-field'>
            <FieldLabel htmlFor='add-account-user-agent'>User-Agent</FieldLabel>
            <UserAgentInput
              id='add-account-user-agent'
              value={settings.userAgent}
              onChange={settings.setUserAgent}
              presets={presets}
              disabled={busy}
            />
            <FieldDescription>
              {t('用于当前账号的对话、余额查询、模型拉取、模型测试、签到及凭证刷新等上游请求。留空使用提供商默认值；可选预置或直接填写。')}
            </FieldDescription>
          </Field>
        </SettingGroup>
      ) : (
        <p className='detail acct-dlg-group-note'>
          <AccountSettingsIcon name='info' size={14} />
          {t('该提供商使用固定客户端请求标识，暂不支持自定义 User-Agent。')}
        </p>
      )}
    </div>
  )
}

export function AddAccountQueryPane({
  supportsUsage,
  balanceScheme,
  busy,
}: {
  supportsUsage: boolean
  balanceScheme?: string
  busy: boolean
}) {
  const settings = useAddAccountSettings()
  const spec = balanceSpecOf(balanceScheme)
  return (
    <div inert={busy ? true : undefined}>
      {supportsUsage ? (
        <SettingGroup title={t('余额查询')}>
          <UsageSettingsSection
            draft={settings.usageDraft}
            onChange={settings.setUsageDraft}
            idPrefix='add-account-usage'
          />
        </SettingGroup>
      ) : null}
      {spec ? (
        <SettingGroup
          title={t('查询凭证')}
          action={<Badge variant='secondary' shape='tag'>{spec.label}</Badge>}
        >
          <BalanceQuerySection
            spec={spec}
            values={settings.balanceValues}
            configured={{}}
            onValue={settings.setBalanceValue}
            idPrefix='add-account-balance-query'
          />
        </SettingGroup>
      ) : null}
      <p className='detail acct-dlg-group-note'>
        <AccountSettingsIcon name='info' size={14} />
        {t('余额读数也是限制器余额规则的数据来源。')}
      </p>
    </div>
  )
}

export function AddAccountLimiterPane({
  provider,
  allowBalance,
  busy,
}: {
  provider: string
  allowBalance: boolean
  busy: boolean
}) {
  const settings = useAddAccountSettings()
  return (
    <div inert={busy ? true : undefined}>
      <LimiterSection
        provider={provider}
        draft={settings.limiterDraft}
        onChange={settings.setLimiterDraft}
        allowBalance={allowBalance}
      />
      <p className='detail acct-dlg-group-note'>
        <AccountSettingsIcon name='info' size={14} />
        {t('没有规则或全部停用时不限制；跳过后自动恢复，禁用后需手动启用。')}
      </p>
    </div>
  )
}

export function AddAccountNetworkPane({
  idPrefix,
  busy,
  allowRotate = false,
}: {
  idPrefix: string
  busy: boolean
  allowRotate?: boolean
}) {
  const proxy = useAddAccountProxy()
  const { draft, change } = useAddAccountProxyDraft()
  return (
    <div inert={busy ? true : undefined}>
      <SettingGroup title={t('出网方式')}>
        <ProxyForm
          draft={draft}
          onChange={change}
          idPrefix={idPrefix}
          allowSystemImport
          allowRotate={allowRotate}
          onSystemImportBusy={proxy.setBusy}
        />
      </SettingGroup>
      <p className='detail acct-dlg-group-note'>
        <AccountSettingsIcon name='shield' size={14} />
        {t('所选代理只作用于当前账号，后续转发与余额查询继续使用。')}
      </p>
    </div>
  )
}
