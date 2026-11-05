import * as React from 'react'
import {
  Alert, Button, Card, CardContent, CardHeader, CardTitle, Dialog, DialogBody, DialogContent,
  DialogFooter, DialogHeader, DialogTitle, Empty, Field, FieldDescription, FieldLabel, Input,
  MultiSelect, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner, Switch,
  confirmDialog,
} from '@ui'
import { t } from '../i18n'
import { callJson } from './assistant-state'
import { openExternal } from './update-shared'
import {
  PROVIDER_KEY_PAGES, clearProviderKey, clearVision, fetchProviders, fetchSelection, fetchVision,
  formatTime, saveProvider, saveSelection, saveVision, testProvider, testVision,
  type ProviderRow, type ProviderSelection, type ProviderTestResult, type VisionConfig as VisionConfigRow,
  type VisionKeyOption,
} from './tools-data'

/* ─── 网络搜索：上游提供商 ─────────────────────────────────── */

/** 稳定错误码 → 界面文案（代码是后端口径，文案是中文键，便于扫描进词典） */
const TEST_FAILURE_TEXT: Record<string, string> = {
  invalid_provider_key: t('密钥或账号登录态无效'),
  rate_limited: t('触发上游限流'),
  timed_out: t('上游请求超时'),
  no_results: t('上游没有返回结果'),
  provider_unavailable: t('提供商暂不可用或配置有误'),
  configuration: t('搜索来源配置不完整'),
  permission_denied: t('当前网关 Key 无权使用所选账号搜索来源'),
  invalid_input: t('请求参数无效'),
}

function testFailureText(result: ProviderTestResult): string {
  const text = result.code ? TEST_FAILURE_TEXT[result.code] : ''
  return result.error ? t(result.error) : text || t('连接测试失败')
}

export function WebSearchConfig() {
  const [providers, setProviders] = React.useState<ProviderRow[]>([])
  const [selection, setSelection] = React.useState<ProviderSelection>({ search_providers: [], fetch_provider: 'tavily' })
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const [testing, setTesting] = React.useState('')
  const [editing, setEditing] = React.useState<ProviderRow | null>(null)

  /**
   * `silent` = 只刷新数据、不切换到「正在加载」骨架。
   *
   * 开关提供商、清密钥这类「就地更新」必须走 silent：切骨架会把整块配置
   * 卸载重挂，页面高度瞬间归零，浏览器就把滚动位置拉回顶部 —— 用户点一下
   * 开关，页面自己跳回最上面。
   */
  const load = React.useCallback(async (silent = false) => {
    if (!silent) setLoading(true)
    setError('')
    try {
      const [providersResult, selectionResult] = await Promise.all([fetchProviders(), fetchSelection()])
      setProviders(providersResult || [])
      setSelection(selectionResult)
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      if (!silent) setLoading(false)
    }
  }, [])

  React.useEffect(() => { void load() }, [load])

  const options = providers.map(provider => ({ value: provider.provider, label: provider.display_name }))
  const fetchOptions = providers.filter(provider => provider.supports_fetch !== false)
    .map(provider => ({ value: provider.provider, label: provider.display_name }))
  const selected = new Set(selection.search_providers)
  // 选择范围和可用状态分开：密钥或账号配置失效时，明确显示会跳过哪些来源。
  const readySelected = providers.filter(provider => selected.has(provider.provider) && provider.enabled && provider.configured)
  const missing = providers.filter(provider => selected.has(provider.provider) && !(provider.enabled && provider.configured))
  const aggregateReady = readySelected.length > 0
  const readyNames = readySelected.map(provider => provider.display_name).join(', ')
  const missingNames = missing.map(provider => provider.display_name).join(', ')

  const saveProviders = async () => {
    if (!selection.search_providers.length) { setError(t('批量网页搜索至少选择一个提供商')); return }
    setSaving(true); setError(''); setNotice('')
    try {
      const next = await saveSelection(selection)
      setSelection(next)
      setNotice(t('能力提供商配置已保存'))
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      setSaving(false)
    }
  }

  const toggle = async (provider: ProviderRow, enabled: boolean) => {
    setError('')
    try {
      await saveProvider(provider.provider, { enabled })
      await load(true)
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    }
  }

  const test = async (provider: ProviderRow) => {
    setTesting(provider.provider); setError(''); setNotice('')
    try {
      const result = await testProvider(provider.provider)
      if (result.success) {
        setNotice(t('连接成功：{count} 条结果，耗时 {duration}ms', { count: result.result_count ?? 0, duration: result.duration_ms ?? 0 }))
      } else {
        setError(testFailureText(result))
      }
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      setTesting('')
    }
  }

  const clearKey = async (provider: ProviderRow) => {
    setError(''); setNotice('')
    try {
      await clearProviderKey(provider.provider)
      setNotice(t('密钥已清除'))
      await load(true)
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    }
  }

  if (loading) return <div className='flex items-center gap-2 p-4 text-sm text-subtle'><Spinner />{t('正在加载工具服务配置…')}</div>

  return <Card>
    <CardHeader><CardTitle>{t('上游提供商配置')}</CardTitle></CardHeader>
    <CardContent className='flex flex-col gap-4'>
      <p className='text-xs text-subtle'>{t('配置搜索与抓取所使用的上游提供商。调用方不再传递 provider，运行时始终使用这里保存的设置。')}</p>
      <Alert>{t('上游提供商配置由各接入协议共享，改动后所有协议同时生效。')}</Alert>
      {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
      {notice && <Alert variant='success'>{notice}</Alert>}

      <div className='grid grid-cols-1 gap-4 md:grid-cols-2'>
        <Field>
          <FieldLabel>{t('批量网页搜索提供商')}</FieldLabel>
          <MultiSelect value={selection.search_providers} onValueChange={value => setSelection(current => ({ ...current, search_providers: value }))}
            options={options} placeholder={t('请选择提供商')} aria-label={t('批量网页搜索提供商')} />
          <FieldDescription>{t('支持多选。每次搜索会并发调用所选提供商并合并重复来源。')}</FieldDescription>
        </Field>
        <Field>
          <FieldLabel>{t('批量网页抓取提供商')}</FieldLabel>
          <Select value={selection.fetch_provider} onValueChange={value => { if (typeof value === 'string') setSelection(current => ({ ...current, fetch_provider: value })) }}>
            <SelectTrigger className='w-full'><SelectValue>{fetchOptions.find(option => option.value === selection.fetch_provider)?.label || selection.fetch_provider}</SelectValue></SelectTrigger>
            <SelectContent>{fetchOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select>
          <FieldDescription>{t('仅支持单选，默认为 Tavily。')}</FieldDescription>
        </Field>
      </div>
      <div><Button disabled={saving} onClick={() => void saveProviders()}>{saving ? <Spinner /> : null}{t('保存能力配置')}</Button></div>

      {aggregateReady
        ? <Alert variant='success'>{missing.length
          ? t('批量网页搜索可用：将使用 {ready}，未配置的 {missing} 会自动跳过。', { ready: readyNames, missing: missingNames })
          : t('批量网页搜索可用：所选来源均已启用并配置完成。')}</Alert>
        : <Alert variant='warning'>{t('批量网页搜索不可用：请启用来源并配置密钥或绑定可用账号。')}</Alert>}
      <p className='text-xs text-subtle'>{t('已有账号搜索只支持搜索，不支持网页抓取；请求会使用绑定账号的额度与出网设置。')}</p>

      <div className='grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4'>
        {providers.map(provider => <div key={provider.provider} className='flex flex-col gap-2 rounded-md border border-border p-3'>
          <div className='flex items-center gap-2'>
            <span className='font-semibold'>{provider.display_name}</span>
            <Switch className='ml-auto' checked={provider.enabled} disabled={provider.available === false && !provider.enabled} onCheckedChange={checked => void toggle(provider, checked === true)} aria-label={t('启用')} />
          </div>
          <div className='flex flex-wrap gap-1.5 text-[11px]'>
            <span className={provider.configured ? 'text-success' : 'text-subtle'}>{provider.configured ? t('已配置') : t('未配置')}</span>
            <span className='text-subtle'>·</span>
            <span className={provider.enabled ? 'text-primary' : 'text-subtle'}>{provider.enabled ? t('已启用') : t('已停用')}</span>
          </div>
          <div className='text-[11.5px] text-subtle'>
            {provider.auth_type === 'account' ? <>
              <div>{t('已有账号')} · {t('仅搜索')}</div>
              <div>{t('搜索账号')}: <span className='text-foreground'>{provider.account_name || t('未选择账号')}</span></div>
              {provider.max_query_chars ? <div>{t('每条搜索词最多 {count} 字', { count: provider.max_query_chars })}</div> : null}
              {provider.unavailable_reason ? <div className='mt-1 text-warning'>{t(provider.unavailable_reason)}</div> : null}
            </> : <div>{t('密钥提示')}: <span className='text-foreground'>{provider.key_hint || '-'}</span></div>}
            <div>{t('更新时间')}: {formatTime(provider.updated_at)}</div>
          </div>
          <div className='mt-auto flex flex-wrap gap-1.5'>
            <Button size='sm' variant='outline' onClick={() => setEditing(provider)}>{t('编辑')}</Button>
            <Button size='sm' variant='outline' disabled={!provider.configured || testing === provider.provider} onClick={() => void test(provider)}>
              {testing === provider.provider ? <Spinner /> : null}{t('测试连接')}
            </Button>
            {provider.auth_type !== 'account' ? <Button size='sm' variant='ghost' disabled={!provider.configured} onClick={() => {
              void confirmDialog(t('确定清除提供商 {name} 的 API Key？清除后该提供商将不可用，直到重新保存新密钥。', { name: provider.display_name }), { destructive: true })
                .then(ok => { if (ok) void clearKey(provider) })
            }}>{t('清除密钥')}</Button> : null}
          </div>
        </div>)}
        {!providers.length && <Empty>{t('没有可配置的提供商')}</Empty>}
      </div>
    </CardContent>
    {editing && <ProviderDialog provider={editing} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); void load(true) }} />}
  </Card>
}

function ProviderDialog({ provider, onClose, onSaved }: { provider: ProviderRow; onClose: () => void; onSaved: () => void }) {
  const [apiKey, setApiKey] = React.useState('')
  const [accountId, setAccountId] = React.useState(provider.account_id || '')
  const isAccount = provider.auth_type === 'account'
  const accounts = (provider.accounts || []).filter(account => account.enabled)
  const accountLabel = accounts.find(account => account.id === accountId)?.name
    || (accountId ? t('已绑定账号不可用，请重新选择') : t('未选择账号'))
  const [enabled, setEnabled] = React.useState(provider.enabled)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  const save = async () => {
    setBusy(true); setError('')
    try {
      await saveProvider(provider.provider, isAccount
        ? { account_id: accountId || null, enabled }
        : { api_key: apiKey.trim() || undefined, enabled })
      onSaved()
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      setBusy(false)
    }
  }

  return <Dialog open onOpenChange={(open, details) => { if (!open) { if (busy) details.cancel(); else onClose() } }}>
    <DialogContent className='max-w-lg'>
      <DialogHeader><DialogTitle>{t('编辑提供商 {name}', { name: provider.display_name })}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        {isAccount ? <Field>
          <FieldLabel>{t('搜索账号')}</FieldLabel>
          <Select value={accountId || '__none__'} onValueChange={value => { if (typeof value === 'string') setAccountId(value === '__none__' ? '' : value) }}>
            <SelectTrigger className='w-full'><SelectValue>{accountLabel}</SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value='__none__'>{t('未选择账号')}</SelectItem>
              {accountId && !accounts.some(account => account.id === accountId) ? <SelectItem value={accountId} disabled>{accountLabel}</SelectItem> : null}
              {accounts.map(account => <SelectItem key={account.id} value={account.id}>{account.name}</SelectItem>)}
            </SelectContent>
          </Select>
          <FieldDescription>{t('复用「账号」页的登录态，不需要额外填写搜索密钥。账号停用或删除后不会自动换号。')}</FieldDescription>
          {!accounts.length ? <Alert variant='warning'>{t('没有可绑定的启用账号，请先到「账号」页添加或启用。')}</Alert> : null}
          {provider.available === false && provider.unavailable_reason ? <Alert variant='warning'>{t(provider.unavailable_reason)}</Alert> : null}
        </Field> : <Field>
          <FieldLabel>{t('API Key')}</FieldLabel>
          <Input type='password' autoComplete='new-password' value={apiKey} placeholder={t('输入新密钥；留空保持不变')} onChange={event => setApiKey(event.currentTarget.value)} />
          <FieldDescription>{t('空白的 API Key 不会覆盖已保存的密钥。')}</FieldDescription>
        </Field>}
        {PROVIDER_KEY_PAGES[provider.provider] ? <div>
          <Button variant='link' size='sm' className='h-auto px-0' onClick={() => void openExternal(PROVIDER_KEY_PAGES[provider.provider])}>
            {t('获取 API Key')}
          </Button>
        </div> : null}
        <div className='flex items-center gap-2'>
          <span className='text-sm'>{t('启用')}</span>
          <Switch checked={enabled} disabled={isAccount && provider.available === false && !enabled} onCheckedChange={checked => setEnabled(checked === true)} aria-label={t('启用')} />
        </div>
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button><Button disabled={busy} onClick={() => void save()}>{busy ? <Spinner /> : null}{t('保存')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}

/* ─── 视觉分析 ─────────────────────────────────────────────── */

type ManageModel = { id: string; provider: string; enabled: boolean; kind?: string }
type ManageProvider = { provider: string; providerLabel?: string; count?: number }

const REASONING_LABELS: Record<string, string> = {
  off: 'off', none: 'none', minimal: 'minimal', low: 'low', medium: 'medium', high: 'high', xhigh: 'xhigh', max: 'max',
}

export function VisionConfigPanel() {
  const [config, setConfig] = React.useState<VisionConfigRow | null>(null)
  const [keys, setKeys] = React.useState<VisionKeyOption[]>([])
  const [providers, setProviders] = React.useState<ManageProvider[]>([])
  const [models, setModels] = React.useState<ManageModel[]>([])
  const [reasoningLevels, setReasoningLevels] = React.useState<string[]>(Object.keys(REASONING_LABELS))
  const [provider, setProvider] = React.useState('')
  const [model, setModel] = React.useState('')
  const [reasoning, setReasoning] = React.useState('')
  const [keyId, setKeyId] = React.useState('')
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [testing, setTesting] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')

  // 与 WebSearchConfig 的 load 同一理由：保存后只刷新数据，不切骨架
  const load = React.useCallback(async (silent = false) => {
    if (!silent) setLoading(true)
    setError('')
    try {
      const [visionResult, manage] = await Promise.all([
        fetchVision(),
        callJson<{ providers?: ManageProvider[]; models?: ManageModel[]; reasoningLevels?: string[] }>('GET', '/api/models/manage'),
      ])
      setConfig(visionResult.config)
      setKeys(visionResult.keys || [])
      setProviders((manage.providers || []).filter(item => (item.count ?? 0) > 0))
      setModels((manage.models || []).filter(item => item.enabled && item.kind !== 'image'))
      if (manage.reasoningLevels?.length) setReasoningLevels(manage.reasoningLevels)
      const stored = visionResult.config
      setProvider(stored?.provider || '')
      setModel(stored?.upstream_model || '')
      setReasoning(stored?.reasoning || '')
      setKeyId(stored?.api_key_id && visionResult.keys.some(key => key.id === stored.api_key_id) ? stored.api_key_id : (visionResult.keys[0]?.id || ''))
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      if (!silent) setLoading(false)
    }
  }, [])

  React.useEffect(() => { void load() }, [load])

  const providerOptions = providers.map(item => ({ value: item.provider, label: item.providerLabel || item.provider }))
  const modelOptions = models.filter(item => item.provider === provider).map(item => ({ value: item.id, label: item.id }))
  const keyOptions = keys.map(key => ({ value: key.id, label: `${key.name}（${key.key_prefix}）` }))
  const savedKeyMissing = config?.api_key_id && !keys.some(key => key.id === config.api_key_id)

  const save = async () => {
    if (!provider || !model) { setError(t('请选择提供商与上游模型')); return }
    if (!keyId) { setError(t('请选择内部调用 API Key')); return }
    setSaving(true); setError(''); setNotice('')
    try {
      const saved = await saveVision({ provider, upstream_model: model, reasoning_effort: reasoning, api_key_id: keyId })
      setConfig(saved)
      setNotice(t('视觉服务配置已保存'))
      await load(true)
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      setSaving(false)
    }
  }

  const test = async () => {
    setTesting(true); setError(''); setNotice('')
    try {
      const result = await testVision()
      if (result.success) setNotice(t('视觉链路连通：耗时 {duration}ms', { duration: result.duration_ms ?? 0 }))
      else setError(result.error || t('视觉链路测试失败'))
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    } finally {
      setTesting(false)
    }
  }

  const clear = async () => {
    setError(''); setNotice('')
    try {
      await clearVision()
      setConfig(null); setProvider(''); setModel(''); setReasoning('')
      setNotice(t('视觉服务配置已清除'))
    } catch (failure) {
      setError(t(failure instanceof Error ? failure.message : String(failure)))
    }
  }

  if (loading) return <div className='flex items-center gap-2 p-4 text-sm text-subtle'><Spinner />{t('正在加载工具服务配置…')}</div>

  return <Card>
    <CardHeader>
      <CardTitle>{t('视觉模型配置')}</CardTitle>
      <div className='ml-auto flex gap-2'>
        <Button variant='outline' disabled={!config?.configured || testing} onClick={() => void test()}>{testing ? <Spinner /> : null}{t('测试连接')}</Button>
        {config?.configured ? <Button variant='ghost' disabled={saving} onClick={() => void clear()}>{t('清除配置')}</Button> : null}
      </div>
    </CardHeader>
    <CardContent className='flex flex-col gap-4'>
      <p className='text-xs text-subtle'>{t('选择处理图片的上游提供商与模型。调用时会用内部 Key 经本站网关请求该模型，协议转换、重试与故障转移全部复用网关。')}</p>
      {!config?.configured
        ? <Alert variant='warning'>{t('尚未配置视觉模型，配置前调用方会收到“未配置”错误。')}</Alert>
        : config.key_available === false
          ? <Alert variant='destructive'>{t('已配置的 API Key 已失效（被删除或禁用），请重新选择并保存。')}</Alert>
          : <Alert variant='success'>{t('当前模型：{provider} / {model}', { provider: config.provider_label || config.provider, model: config.upstream_model })}</Alert>}
      {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
      {notice && <Alert variant='success'>{notice}</Alert>}
      {!providers.length && <Alert variant='warning'>{t('还没有可用的提供商，请先在「账号」页添加账号并在「模型管理」页确认有已启用的模型。')}</Alert>}

      <div className='grid grid-cols-1 gap-4 md:grid-cols-2'>
        <Field>
          <FieldLabel>{t('上游提供商')}</FieldLabel>
          <Select value={provider} onValueChange={value => { if (typeof value === 'string') { setProvider(value); setModel('') } }}>
            <SelectTrigger className='w-full'><SelectValue>{providerOptions.find(option => option.value === provider)?.label || t('请选择提供商')}</SelectValue></SelectTrigger>
            <SelectContent>{providerOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select>
          <FieldDescription>{t('仅显示已启用且至少有一个可用模型的提供商。')}</FieldDescription>
        </Field>
        <Field>
          <FieldLabel>{t('上游模型')}</FieldLabel>
          <Select value={model} onValueChange={value => { if (typeof value === 'string') setModel(value) }}>
            <SelectTrigger className='w-full' disabled={!provider}><SelectValue>{model || (provider ? t('请选择模型') : t('请先选择提供商'))}</SelectValue></SelectTrigger>
            <SelectContent>{modelOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select>
          <FieldDescription>{t('仅显示该提供商下已启用的模型。')}</FieldDescription>
        </Field>
        <Field>
          <FieldLabel>{t('思考等级')}</FieldLabel>
          <Select value={reasoning} onValueChange={value => { if (typeof value === 'string') setReasoning(value) }}>
            <SelectTrigger className='w-full'><SelectValue>{reasoning || t('不指定')}</SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{t('不指定')}</SelectItem>
              {reasoningLevels.map(level => <SelectItem key={level} value={level}>{REASONING_LABELS[level] || level}</SelectItem>)}
            </SelectContent>
          </Select>
          <FieldDescription>{t('可选。留空时不传，由模型映射的「思考覆盖」决定。')}</FieldDescription>
        </Field>
        <Field>
          <FieldLabel>{t('内部调用 API Key')}</FieldLabel>
          <Select value={keyId} onValueChange={value => { if (typeof value === 'string') setKeyId(value) }}>
            <SelectTrigger className='w-full'><SelectValue>{keyOptions.find(option => option.value === keyId)?.label || t('请选择 API Key')}</SelectValue></SelectTrigger>
            <SelectContent>
              {savedKeyMissing ? <SelectItem value={config?.api_key_id || ''} disabled>{config?.api_key_name || config?.api_key_id}（{t('已失效')}）</SelectItem> : null}
              {keyOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <FieldDescription>{t('内部请求本站网关时使用的网关 Key；额度、限流与请求日志记在该 Key 上。')}</FieldDescription>
        </Field>
      </div>
      <div><Button disabled={saving} onClick={() => void save()}>{saving ? <Spinner /> : null}{t('保存')}</Button></div>
    </CardContent>
  </Card>
}
