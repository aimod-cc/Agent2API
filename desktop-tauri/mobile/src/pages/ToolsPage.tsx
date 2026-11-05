import { useEffect, useMemo, useState } from 'react'
import {
  Button, Card, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, MultiSelect, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch, confirmDialog,
} from '@ui'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { apiCall } from '../api/client'
import { store } from '../state/store'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 工具服务（手机端）。
 *
 * ── 手机能做什么、不能做什么 ────────────────────────────────
 * 服务开关、上游提供商密钥、提供商选择与视觉模型配置全部是**网关侧**的数据，
 * 手机上改完立刻生效；这部分与桌面端同一套接口。
 * 「本机接入」（把 Skill / MCP 配置写进客户端目录）作用在**网关所在机器**，
 * 手机上不做 —— 与「Agent 配置」页同一口径：手机上只读/远程操作，改本机文件回桌面端。
 */

type ToolProtocol = 'skill' | 'mcp' | 'api'
type ToolService = {
  id: 'web-search' | 'vision'; protocol: ToolProtocol; key: string; enabled: boolean
  capability_count: number; endpoint: string; version: string; timeout_seconds: number
}
type ProviderRow = {
  provider: string; display_name: string; configured: boolean; key_hint: string | null
  enabled: boolean; updated_at: number | null
  auth_type?: 'api_key' | 'account'; supports_fetch?: boolean; available?: boolean
  account_id?: string; account_name?: string; max_query_chars?: number | null; unavailable_reason?: string | null
  accounts?: Array<{ id: string; name: string; enabled: boolean }>
}
type Selection = { search_providers: string[]; fetch_provider: string }
type VisionConfig = {
  provider: string; provider_label: string; upstream_model: string; downstream_model: string
  reasoning: string; api_key_id: string; api_key_name: string; configured: boolean; key_available: boolean
}
type VisionKey = { id: string; name: string; key_prefix: string }
type ManageModel = { id: string; provider: string; enabled: boolean; kind?: string }
type ManageProvider = { provider: string; providerLabel?: string; count?: number }

const SERVICE_NAME: Record<string, string> = { 'web-search': t('网络搜索'), vision: t('视觉分析') }
const PROTOCOL_LABEL: Record<ToolProtocol, string> = { skill: 'Skill', mcp: 'MCP', api: 'API' }
const REASONING = ['off', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

function message(failure: unknown): string {
  return t(failure instanceof Error ? failure.message : String(failure))
}

export function ToolsPage() {
  const [services, setServices] = useState<ToolService[]>([])
  const [providers, setProviders] = useState<ProviderRow[]>([])
  const [selection, setSelection] = useState<Selection>({ search_providers: [], fetch_provider: 'tavily' })
  const [vision, setVision] = useState<{ config: VisionConfig | null; keys: VisionKey[] }>({ config: null, keys: [] })
  const [manage, setManage] = useState<{ models: ManageModel[]; providers: ManageProvider[] }>({ models: [], providers: [] })
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState('')
  const [editing, setEditing] = useState<ProviderRow | null>(null)
  const [visionOpen, setVisionOpen] = useState(false)
  const [selectionOpen, setSelectionOpen] = useState(false)

  const load = async () => {
    try {
      const [servicesData, providersData, selectionData, visionData, manageData] = await Promise.all([
        apiCall<ToolService[]>('/api/tools/servers'),
        apiCall<ProviderRow[]>('/api/tools/providers'),
        apiCall<Selection>('/api/tools/provider-selection'),
        apiCall<{ config: VisionConfig | null; keys: VisionKey[] }>('/api/tools/vision/config'),
        apiCall<{ models: ManageModel[]; providers: ManageProvider[]; reasoningLevels?: string[] }>('/api/models/manage'),
      ])
      setServices((servicesData || []).filter(service => service.protocol !== 'api'))
      setProviders((providersData || []).map(provider => ({ ...provider, display_name: t(provider.display_name) })))
      setSelection(selectionData)
      setVision(visionData)
      setManage({ models: (manageData.models || []).filter(model => model.enabled && model.kind !== 'image'), providers: manageData.providers || [] })
      setError('')
    } catch (failure) {
      setError(message(failure))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void load() }, [])
  usePolling(load, 120_000)
  const pull = usePullToRefresh({ onRefresh: load })

  const toggle = async (service: ToolService, enabled: boolean) => {
    setBusy(service.key)
    try {
      await apiCall(`/api/tools/servers/${service.id}`, { method: 'PATCH', body: { protocol: service.protocol, enabled } })
      setServices(current => current.map(item => item.key === service.key ? { ...item, enabled } : item))
      store.toast(t('服务状态已更新'))
    } catch (failure) {
      store.toast(message(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const toggleProvider = async (provider: ProviderRow, enabled: boolean) => {
    try {
      await apiCall(`/api/tools/providers/${provider.provider}`, { method: 'PUT', body: { enabled } })
      store.toast(t('已保存'))
      await load()
    } catch (failure) {
      store.toast(message(failure), 'err')
    }
  }

  const testProvider = async (provider: ProviderRow) => {
    setBusy(provider.provider)
    try {
      const result = await apiCall<{ success: boolean; result_count?: number; duration_ms?: number; error?: string; code?: string }>(
        `/api/tools/providers/${provider.provider}/test`, { method: 'POST' })
      if (result.success) store.toast(t('连接成功：{count} 条结果，耗时 {duration}ms', { count: result.result_count ?? 0, duration: result.duration_ms ?? 0 }))
      else store.toast(result.error ? t(result.error) : t('连接测试失败'), 'err')
    } catch (failure) {
      store.toast(message(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const clearProvider = async (provider: ProviderRow) => {
    try {
      await apiCall(`/api/tools/providers/${provider.provider}/key`, { method: 'DELETE' })
      store.toast(t('密钥已清除'))
      await load()
    } catch (failure) {
      store.toast(message(failure), 'err')
    }
  }

  const providerOptions = providers.map(provider => ({ value: provider.provider, label: provider.display_name }))
  const fetchOptions = providers.filter(provider => provider.supports_fetch !== false)
    .map(provider => ({ value: provider.provider, label: provider.display_name }))
  const selectedProviders = providers.filter(provider => selection.search_providers.includes(provider.provider))
  const readyProviders = selectedProviders.filter(provider => provider.enabled && provider.configured)
  const missingProviders = selectedProviders.filter(provider => !(provider.enabled && provider.configured))
  const searchReady = readyProviders.length > 0
  const readyNames = readyProviders.map(provider => provider.display_name).join(', ')
  const missingNames = missingProviders.map(provider => provider.display_name).join(', ')

  return <div className='m-page' ref={pull.bindRef}>
    <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
    <PageIntro subtitle={t('网络搜索与视觉分析：网关侧配置，改完立即生效。本机客户端的安装在桌面端完成。')}
      actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />} />
    {error ? <ErrorNote text={error} /> : null}
    {loading && !services.length ? <SkeletonList rows={4} height={90} /> : null}

    {!loading ? <>
      <div className='m-section'>
        <Card className='m-card m-card-pad'>
          <div style={{ fontSize: 12.5, lineHeight: 1.6, color: 'var(--m-text-2)' }}>
            {t('这里管理的是「客户端能用什么」：服务是否对外提供、上游提供商与密钥、视觉分析用哪个模型。')}
          </div>
        </Card>
      </div>

      <div className='m-section'>
        <div className='m-section-head'><h2>{t('服务与协议')}</h2></div>
        <Card className='m-card'>
          {services.map((service, index) => <div key={service.key} style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '12px 14px', borderTop: index ? '1px solid var(--m-line)' : undefined }}>
            <div style={{ minWidth: 0, flex: 1 }}>
              <div style={{ fontSize: 13, fontWeight: 600 }}>
                {SERVICE_NAME[service.id] || service.id}
                <span style={{ marginLeft: 8, fontSize: 11, color: 'var(--m-text-3)' }}>{PROTOCOL_LABEL[service.protocol]}</span>
              </div>
              <div style={{ marginTop: 2, fontSize: 11.5, color: 'var(--m-text-3)' }}>
                <code>{service.endpoint}</code> · v{service.version} · {t('超时 {seconds}s', { seconds: service.timeout_seconds })}
              </div>
            </div>
            <Switch checked={service.enabled} disabled={busy === service.key}
              onCheckedChange={checked => void toggle(service, checked === true)} aria-label={t('启用 / 停用此服务')} />
          </div>)}
          {!services.length ? <Empty text={t('没有可用的工具服务')} /> : null}
        </Card>
      </div>

      <div className='m-section'>
        <div className='m-section-head'>
          <h2>{t('上游提供商')}</h2>
          <Button size='sm' variant='link' type='button' className='m-link px-0' onClick={() => setSelectionOpen(true)}>{t('提供商选择')}</Button>
        </div>
        <Card className='m-card m-card-pad'>
          <div style={{ fontSize: 12, color: searchReady ? 'var(--m-ok)' : 'var(--m-warn)', marginBottom: 8 }}>
            {searchReady
              ? (missingProviders.length
                ? t('批量网页搜索可用：将使用 {ready}，未配置的 {missing} 会自动跳过。', { ready: readyNames, missing: missingNames })
                : t('批量网页搜索可用：所选来源均已启用并配置完成。'))
              : t('批量网页搜索不可用：请启用来源并配置密钥或绑定可用账号。')}
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 8 }}>
            {selection.search_providers.map(item => <Badge key={item} tone='brand'>{item}</Badge>)}
            <Badge tone='mute'>{t('抓取：{name}', { name: selection.fetch_provider })}</Badge>
          </div>
        </Card>
        {providers.map(provider => <Card key={provider.provider} className='m-card m-card-pad' style={{ marginTop: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{provider.display_name}</span>
            <Badge tone={provider.configured ? 'ok' : 'mute'}>{provider.configured ? t('已配置') : t('未配置')}</Badge>
            <span style={{ marginLeft: 'auto' }} />
            <Switch checked={provider.enabled} disabled={provider.available === false && !provider.enabled} onCheckedChange={checked => void toggleProvider(provider, checked === true)} aria-label={t('启用')} />
          </div>
          <div style={{ marginTop: 6, fontSize: 11.5, color: 'var(--m-text-3)' }}>
            {provider.auth_type === 'account' ? <>
              <div>{t('已有账号')} · {t('仅搜索')}</div>
              <div>{t('搜索账号')}: {provider.account_name || t('未选择账号')}</div>
              {provider.max_query_chars ? <div>{t('每条搜索词最多 {count} 字', { count: provider.max_query_chars })}</div> : null}
              {provider.unavailable_reason ? <div style={{ color: 'var(--m-warn)' }}>{t(provider.unavailable_reason)}</div> : null}
            </> : <>{t('密钥提示')}: {provider.key_hint || '-'}</>}
          </div>
          <div style={{ display: 'flex', gap: 6, marginTop: 8, flexWrap: 'wrap' }}>
            <Button size='sm' variant='outline' onClick={() => setEditing(provider)}>{t('编辑')}</Button>
            <Button size='sm' variant='outline' disabled={!provider.configured || busy === provider.provider} onClick={() => void testProvider(provider)}>{t('测试连接')}</Button>
            {provider.auth_type !== 'account' ? <Button size='sm' variant='ghost' disabled={!provider.configured} onClick={() => void confirmDialog(t('确定清除提供商 {name} 的 API Key？清除后该提供商将不可用，直到重新保存新密钥。', { name: provider.display_name }), { destructive: true }).then(ok => { if (ok) void clearProvider(provider) })}>{t('清除密钥')}</Button> : null}
          </div>
        </Card>)}
      </div>

      <div className='m-section'>
        <div className='m-section-head'>
          <h2>{t('视觉分析')}</h2>
          <Button size='sm' variant='link' type='button' className='m-link px-0' onClick={() => setVisionOpen(true)}>{t('配置')}</Button>
        </div>
        <Card className='m-card m-card-pad'>
          {!vision.config?.configured
            ? <div style={{ fontSize: 12.5, color: 'var(--m-warn)' }}>{t('尚未配置视觉模型，配置前调用方会收到“未配置”错误。')}</div>
            : vision.config.key_available === false
              ? <div style={{ fontSize: 12.5, color: 'var(--m-danger)' }}>{t('已配置的 API Key 已失效（被删除或禁用），请重新选择并保存。')}</div>
              : <div style={{ fontSize: 12.5, color: 'var(--m-text-2)' }}>{t('当前模型：{provider} / {model}', { provider: vision.config.provider_label || vision.config.provider, model: vision.config.upstream_model })}</div>}
          <div style={{ marginTop: 8, display: 'flex', gap: 8 }}>
            <Button size='sm' variant='outline' disabled={!vision.config?.configured} onClick={async () => {
              try {
                const result = await apiCall<{ success: boolean; duration_ms?: number; error?: string }>('/api/tools/vision/config/test', { method: 'POST' })
                if (result.success) store.toast(t('视觉链路连通：耗时 {duration}ms', { duration: result.duration_ms ?? 0 }))
                else store.toast(result.error || t('视觉链路测试失败'), 'err')
              } catch (failure) { store.toast(message(failure), 'err') }
            }}>{t('测试连接')}</Button>
          </div>
        </Card>
      </div>
    </> : null}

    {editing ? <ProviderDialog provider={editing} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); void load() }} /> : null}
    {selectionOpen ? <SelectionDialog
      providers={providerOptions}
      fetchProviders={fetchOptions}
      value={selection}
      onClose={() => setSelectionOpen(false)}
      onSaved={next => { setSelection(next); setSelectionOpen(false); void load() }}
    /> : null}
    {visionOpen ? <VisionDialog
      config={vision.config}
      keys={vision.keys}
      providers={(manage.providers || []).filter(item => (item.count ?? 0) > 0)}
      models={manage.models}
      onClose={() => setVisionOpen(false)}
      onSaved={() => { setVisionOpen(false); void load() }}
    /> : null}
  </div>
}

function ProviderDialog({ provider, onClose, onSaved }: { provider: ProviderRow; onClose: () => void; onSaved: () => void }) {
  const [apiKey, setApiKey] = useState('')
  const [accountId, setAccountId] = useState(provider.account_id || '')
  const isAccount = provider.auth_type === 'account'
  const accounts = (provider.accounts || []).filter(account => account.enabled)
  const accountLabel = accounts.find(account => account.id === accountId)?.name
    || (accountId ? t('已绑定账号不可用，请重新选择') : t('未选择账号'))
  const [enabled, setEnabled] = useState(provider.enabled)
  const [busy, setBusy] = useState(false)
  const save = async () => {
    setBusy(true)
    try {
      await apiCall(`/api/tools/providers/${provider.provider}`, { method: 'PUT', body: isAccount
        ? { account_id: accountId || null, enabled }
        : { api_key: apiKey.trim() || undefined, enabled } })
      store.toast(t('已保存'))
      onSaved()
    } catch (failure) { store.toast(message(failure), 'err') }
    finally { setBusy(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>{t('编辑 {name}', { name: provider.display_name })}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        {isAccount ? <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('搜索账号')}</div>
          <Select value={accountId || '__none__'} onValueChange={value => { if (typeof value === 'string') setAccountId(value === '__none__' ? '' : value) }}>
            <SelectTrigger className='w-full'><SelectValue>{accountLabel}</SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value='__none__'>{t('未选择账号')}</SelectItem>
              {accountId && !accounts.some(account => account.id === accountId) ? <SelectItem value={accountId} disabled>{accountLabel}</SelectItem> : null}
              {accounts.map(account => <SelectItem key={account.id} value={account.id}>{account.name}</SelectItem>)}
            </SelectContent>
          </Select>
          <p style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{t('复用「账号」页的登录态，不需要额外填写搜索密钥。账号停用或删除后不会自动换号。')}</p>
          {!accounts.length ? <ErrorNote text={t('没有可绑定的启用账号，请先到「账号」页添加或启用。')} /> : null}
          {provider.available === false && provider.unavailable_reason ? <ErrorNote text={t(provider.unavailable_reason)} /> : null}
        </div> : <label style={{ fontSize: 12.5 }}>{t('API Key')}
          <Input type='password' autoComplete='new-password' value={apiKey} placeholder={t('输入新密钥；留空保持不变')} onChange={event => setApiKey(event.currentTarget.value)} />
        </label>}
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 12.5 }}>
          {t('启用')}<Switch checked={enabled} disabled={isAccount && provider.available === false && !enabled} onCheckedChange={checked => setEnabled(checked === true)} aria-label={t('启用')} />
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

function SelectionDialog({ providers, fetchProviders, value, onClose, onSaved }: {
  providers: Array<{ value: string; label: string }>; fetchProviders: Array<{ value: string; label: string }>; value: Selection
  onClose: () => void; onSaved: (value: Selection) => void
}) {
  const [draft, setDraft] = useState(value)
  const [busy, setBusy] = useState(false)
  const save = async () => {
    if (!draft.search_providers.length) { store.toast(t('至少选择一个搜索提供商'), 'err'); return }
    setBusy(true)
    try {
      const next = await apiCall<Selection>('/api/tools/provider-selection', { method: 'PUT', body: draft })
      onSaved(next)
    } catch (failure) { store.toast(message(failure), 'err') }
    finally { setBusy(false) }
  }
  const fetchLabel = fetchProviders.find(item => item.value === draft.fetch_provider)?.label || draft.fetch_provider
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>{t('提供商选择')}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('批量网页搜索提供商')}</div>
          <MultiSelect value={draft.search_providers} onValueChange={items => setDraft(current => ({ ...current, search_providers: items }))}
            options={providers} placeholder={t('请选择提供商')} aria-label={t('批量网页搜索提供商')} />
        </div>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('批量网页抓取提供商')}</div>
          <Select value={draft.fetch_provider} onValueChange={next => { if (typeof next === 'string') setDraft(current => ({ ...current, fetch_provider: next })) }}>
            <SelectTrigger className='w-full'><SelectValue>{fetchLabel}</SelectValue></SelectTrigger>
            <SelectContent>{fetchProviders.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
          </Select>
          <p style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{t('已有账号搜索只支持搜索，不支持网页抓取；请求会使用绑定账号的额度与出网设置。')}</p>
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

function VisionDialog({ config, keys, providers, models, onClose, onSaved }: {
  config: VisionConfig | null; keys: VisionKey[]
  providers: ManageProvider[]; models: ManageModel[]
  onClose: () => void; onSaved: () => void
}) {
  const [provider, setProvider] = useState(config?.provider || '')
  const [model, setModel] = useState(config?.upstream_model || '')
  const [reasoning, setReasoning] = useState(config?.reasoning || '')
  const [keyId, setKeyId] = useState(config?.api_key_id || keys[0]?.id || '')
  const [busy, setBusy] = useState(false)

  const providerOptions = useMemo(() => providers.map(item => ({ value: item.provider, label: item.providerLabel || item.provider })), [providers])
  const modelOptions = useMemo(() => models.filter(item => item.provider === provider).map(item => ({ value: item.id, label: item.id })), [models, provider])
  const keyOptions = keys.map(key => ({ value: key.id, label: `${key.name}（${key.key_prefix}）` }))
  const savedKeyMissing = Boolean(config?.api_key_id) && !keys.some(key => key.id === config?.api_key_id)
  const keyLabel = keyOptions.find(item => item.value === keyId)?.label
    || (savedKeyMissing ? `${config?.api_key_name || config?.api_key_id}（${t('已失效')}）` : t('请选择 API Key'))

  const save = async () => {
    if (!provider || !model) { store.toast(t('请选择提供商与上游模型'), 'err'); return }
    if (!keyId) { store.toast(t('请选择内部调用 API Key'), 'err'); return }
    setBusy(true)
    try {
      await apiCall('/api/tools/vision/config', { method: 'PUT', body: { provider, upstream_model: model, reasoning_effort: reasoning, api_key_id: keyId } })
      store.toast(t('视觉服务配置已保存'))
      onSaved()
    } catch (failure) { store.toast(message(failure), 'err') }
    finally { setBusy(false) }
  }

  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>{t('视觉模型配置')}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('上游提供商')}</div>
          <Select value={provider} onValueChange={next => { if (typeof next === 'string') { setProvider(next); setModel('') } }}>
            <SelectTrigger className='w-full'><SelectValue>{providerOptions.find(item => item.value === provider)?.label || t('请选择提供商')}</SelectValue></SelectTrigger>
            <SelectContent>{providerOptions.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('上游模型')}</div>
          <Select value={model} onValueChange={next => { if (typeof next === 'string') setModel(next) }}>
            <SelectTrigger className='w-full' disabled={!provider}><SelectValue>{model || (provider ? t('请选择模型') : t('请先选择提供商'))}</SelectValue></SelectTrigger>
            <SelectContent>{modelOptions.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('思考等级')}</div>
          <Select value={reasoning} onValueChange={next => { if (typeof next === 'string') setReasoning(next) }}>
            <SelectTrigger className='w-full'><SelectValue>{reasoning || t('不指定')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value=''>{t('不指定')}</SelectItem>{REASONING.map(level => <SelectItem key={level} value={level}>{level}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <div>
          <div style={{ fontSize: 12.5, marginBottom: 6 }}>{t('内部调用 API Key')}</div>
          <Select value={keyId} onValueChange={next => { if (typeof next === 'string') setKeyId(next) }}>
            <SelectTrigger className='w-full'><SelectValue>{keyLabel}</SelectValue></SelectTrigger>
            <SelectContent>{keyOptions.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        {!providers.length ? <div style={{ fontSize: 11.5, color: 'var(--m-warn)' }}>{t('还没有可用的提供商，请先在桌面端添加账号并确认有已启用的模型。')}</div> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
