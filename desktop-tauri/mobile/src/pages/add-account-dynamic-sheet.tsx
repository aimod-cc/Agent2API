/** 动态登录入口；列表只从 session 实际运行时摘要派生，不登记第二份 provider。 */
import { useEffect, useRef, useState } from 'react'
import {
  Alert, AlertDescription, AlertTitle, Badge, Button, Card, Input, Label, Select,
  SelectContent, SelectItem, SelectTrigger, SelectValue, Sheet, SheetBody, SheetContent,
  SheetHeader, SheetTitle, Spinner,
} from '@ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { providerLabel } from '../lib/providers'
import { serverIdentity } from '../lib/servers'
import { store, useStore } from '../state/store'
import { fetchProxyPool, type PoolItem } from '../api/gateway'
import {
  addDynamicApiAccount, cancelDynamicLogin, dynamicLoginError, methodsUnavailable,
  readDynamicMethods, saveDynamicAccountProxy, startDynamicLogin, submitDynamicCallback, waitDynamicLogin,
} from '../api/add-account-dynamic'
import {
  loginText, mergeLoginInstructions, methodLabel, parseCallbackInput, parseMethods, pluginBrowserInstructions, promptEnabled, promptValue, readInputs,
  type CallbackInput, type MethodsState,
} from '../../../ui-islands/src/islands/add-account-dynamic-fields'
import { usePluginPrompts } from '../../../ui-islands/src/islands/add-account-plugin-prompts'
import { isAddAccountProviderHidden } from '../../../ui-shared/provider-availability'
import { ANTIGRAVITY_ACP_PROVIDER, isAntigravityAccountProvider, groupAntigravityCards, antigravityLoginProviders } from '../../../ui-shared/antigravity-login'
import { PluginLoginInteraction } from '../../../ui-shared/plugin-login-interaction'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { readSystemProxy } from '../../../ui-shared/proxy-system'
import { apiCall } from '../api/client'
import { t } from '../i18n'

export function DynamicProviderSheet({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const { session, sessionError } = useStore()
  const providers = sessionError ? [] : (session?.accounts?.providers || [])
    .filter(item => Boolean(item.id) && !isAddAccountProviderHidden(item.id || '') && item.loginAvailable !== false)
  const [selected, setSelected] = useState('')
  const [search, setSearch] = useState('')
  const keyword = search.trim().toLowerCase()
  const selectedProvider = providers.find(item => item.id === selected)
  const cards = groupAntigravityCards(providers)
  const identity = serverIdentity()

  return <Sheet open onOpenChange={open => { if (!open) onClose() }}>
    <SheetContent>
      <SheetHeader><SheetTitle>{selectedProvider
        ? t('登录 / 添加 {name} 账号', { name: isAntigravityAccountProvider(selected) ? 'Antigravity' : providerLabel(selected, selectedProvider.label) })
        : t('选择提供商')}</SheetTitle></SheetHeader>
      <SheetBody>
        {selectedProvider ? <>
          <Button size='sm' variant='outline' className='self-start' onClick={() => setSelected('')}>{t('返回提供商列表')}</Button>
          {isAntigravityAccountProvider(selected) ? <AntigravityMethods key={`${identity}:${selected}`}
            initialProvider={selected} providers={providers} onAdded={onAdded} />
            : <DynamicForm key={`${identity}:${selected}`} provider={selected} label={selectedProvider.label || selected}
              source={selectedProvider.source} onAdded={onAdded} />}
        </> : <>
          <p className='text-xs text-subtle'>{t('提供商仅在已启用且加载后出现。登录与本机引用均作用在所连接网关，不是手机。')}</p>
          <Input type='search' value={search} placeholder={t('搜索提供商…')}
            onChange={event => setSearch(event.currentTarget.value)} />
          {sessionError ? <Alert variant='destructive'><AlertDescription>{sessionError}</AlertDescription></Alert> : null}
          <div className='grid grid-cols-1 gap-2 min-[420px]:grid-cols-2'>
            {cards.filter(item => !keyword || `${item.id} ${providerLabel(item.id, item.label)}`.toLowerCase().includes(keyword))
              .map(item => <Button key={item.id} variant='outline' className='h-auto min-h-16 justify-start whitespace-normal text-left'
                onClick={() => setSelected(item.id || '')}>
                <ProviderLogo id={item.id} label={item.label} />
                <span className='flex min-w-0 flex-col gap-1'>
                  <span>{providerLabel(item.id, item.label)}</span>
                  <span className='text-[11px] text-muted-foreground'>{t('{n} 个账号', { n: item.count || 0 })}</span>
                </span>
              </Button>)}
          </div>
          {!providers.length ? <p className='text-xs text-subtle'>{t('请安装并启用对应的提供商插件；列表为空时请刷新后重试。')}</p> : null}
          <Button variant='outline' onClick={() => void store.refresh(true)}>{t('重新读取提供商')}</Button>
        </>}
      </SheetBody>
    </SheetContent>
  </Sheet>
}

function AntigravityMethods({ initialProvider, providers, onAdded }: {
  initialProvider: string
  providers: Array<{ id?: string; label?: string; source?: string; loginAvailable?: boolean }>
  onAdded: () => void
}) {
  const [selected, setSelected] = useState(initialProvider)
  const available = antigravityLoginProviders(providers)
  const provider = available.some(item => item.id === selected) ? selected : available[0]?.id || ''
  const current = available.find(item => item.id === provider)
  const label = provider === ANTIGRAVITY_ACP_PROVIDER ? current?.label || provider : t('网页登录 / 填写凭证')
  return <>
    {available.length > 1 ? <>
      <Label>{t('添加方式')}</Label>
      <Select value={provider} onValueChange={value => { if (value) setSelected(value) }}>
        <SelectTrigger className='w-full'><SelectValue>{label}</SelectValue></SelectTrigger>
        <SelectContent>{available.map(item => <SelectItem key={item.id} value={item.id!}>
          {item.id === ANTIGRAVITY_ACP_PROVIDER ? item.label || item.id : t('网页登录 / 填写凭证')}
        </SelectItem>)}</SelectContent>
      </Select>
    </> : null}
    {current ? <DynamicForm key={provider} provider={provider} label={current.label || provider}
      source={current.source} onAdded={onAdded} hideMethodPicker={provider === ANTIGRAVITY_ACP_PROVIDER} />
      : <Alert variant='warning'><AlertDescription>
        {t('请安装并启用对应的提供商插件；列表为空时请刷新后重试。')}
      </AlertDescription></Alert>}
  </>
}

function DynamicForm({ provider, label, source, onAdded, hideMethodPicker = false }: {
  provider: string; label: string; source?: string; onAdded: () => void; hideMethodPicker?: boolean
}) {
  const prefix = `mobile-add-${provider}`
  const [methods, setMethods] = useState<MethodsState>({ phase: 'loading', methods: [], instructions: '', reason: '' })
  const [selected, setSelected] = useState('')
  const [values, setValues] = useState<Record<string, Record<string, string>>>({})
  const [name, setName] = useState('')
  const [proxyMode, setProxyMode] = useState('direct')
  const [readingSystemProxy, setReadingSystemProxy] = useState(false)
  const [systemProxyError, setSystemProxyError] = useState('')
  const [proxyId, setProxyId] = useState('')
  const [proxyPool, setProxyPool] = useState<PoolItem[] | null>(null)
  const [proxyPoolError, setProxyPoolError] = useState('')
  const [poolReload, setPoolReload] = useState(0)
  const [protocol, setProtocol] = useState('http')
  const [host, setHost] = useState('')
  const [port, setPort] = useState('')
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [progress, setProgress] = useState('')
  const [flow, setFlow] = useState<{ state: string; url: string; manual: boolean; callbackInput: CallbackInput } | null>(null)
  const [hasCallback, setHasCallback] = useState(false)
  const [callbackBusy, setCallbackBusy] = useState(false)
  const [callbackError, setCallbackError] = useState('')
  const [callbackNotice, setCallbackNotice] = useState('')
  const callbackInputRef = useRef<HTMLInputElement>(null)
  const callbackRequest = useRef(false)
  const callbackGeneration = useRef(0)
  const [canceling, setCanceling] = useState(false)
  const flowRef = useRef('')
  const loginProxy = useRef<unknown>(undefined)
  const mounted = useRef(false)
  const submitting = useRef(false)
  const identity = serverIdentity()
  const declaredMethod = methods.methods.find(item => item.id === selected) || methods.methods[0]
  const inputs = declaredMethod ? values[declaredMethod.id] || {} : {}
  const resolvedPrompts = usePluginPrompts(provider, declaredMethod, inputs, !flow && !busy,
    (path, body) => apiCall(path, { method: 'POST', body }))
  const method = resolvedPrompts.method
  const hasNamePrompt = Boolean(method?.prompts.some(prompt => prompt.key === 'name'))
  const locked = busy || Boolean(flow) || readingSystemProxy
  const submitLocked = locked || resolvedPrompts.pending || Boolean(resolvedPrompts.error)

  function clearCallback(): void {
    if (callbackInputRef.current) callbackInputRef.current.value = ''
    callbackGeneration.current += 1
    callbackRequest.current = false
    if (!mounted.current) return
    setHasCallback(false)
    setCallbackBusy(false)
    setCallbackError('')
    setCallbackNotice('')
  }

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      clearCallback()
      if (flowRef.current && serverIdentity() === identity) void cancelDynamicLogin(flowRef.current).catch(() => {})
      flowRef.current = ''
      loginProxy.current = undefined
    }
  }, [identity])

  useEffect(() => {
    const controller = new AbortController()
    setMethods({ phase: 'loading', methods: [], instructions: '', reason: '' })
    setSelected('')
    setValues({})
    clearCallback()
    setError('')
    void readDynamicMethods(provider, controller.signal).then(result => {
      if (controller.signal.aborted) return
      const parsed = parseMethods(result, provider)
      parsed.methods = parsed.methods.filter(method => !method.desktopOnly)
      if (provider === 'deepseek-web') parsed.methods = parsed.methods.filter(method => method.type !== 'oauth')
      setMethods(parsed)
    }).catch(failure => {
      if (controller.signal.aborted) return
      const builtin = source === 'builtin' || provider.endsWith('-subscription')
      setMethods({ phase: 'error', methods: [], instructions: '', reason: methodsUnavailable(failure)
        ? builtin ? t('本体订阅未返回动态登录方式（501）；不会猜测导入接口或显示插件手动凭证入口。请更新网关后重试。')
          : t('此提供商未声明动态登录方式，请在网关桌面面板使用其既有入口；手机端不会猜测凭证字段。')
        : dynamicLoginError(failure) })
    })
    return () => controller.abort()
  }, [provider, source])

  useEffect(() => {
    if (!flow) return
    let live = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const state = flow.state
    const poll = async () => {
      if (!live || !mounted.current || flowRef.current !== state || serverIdentity() !== identity) return
      try {
        const reply = await waitDynamicLogin(state)
        if (!live || !mounted.current || flowRef.current !== state || serverIdentity() !== identity) return
        if (reply.error) throw new Error(reply.error)
        if (reply.done === true) {
          if (!reply.session || !(reply.session.accountUid || reply.session.accountId)
            || reply.session.provider && reply.session.provider !== provider) {
            throw new Error(t('登录桥返回了空会话，无法确认账号已添加'))
          }
          const warning = await saveDynamicAccountProxy(
            reply.session.accountUid || reply.session.accountId!, loginProxy.current,
            reply.session.proxySaved === true, reply.session.warning || '', identity,
          )
          if (!live || !mounted.current || serverIdentity() !== identity) return
          flowRef.current = ''
          loginProxy.current = undefined
          setFlow(null)
          await finish(warning)
          return
        }
        if (reply.pending !== true) throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
        setProgress(mergeLoginInstructions(loginText(reply.instructions), loginText(reply.hint)))
        timer = setTimeout(() => void poll(), 1000)
      } catch (failure) {
        if (!live || !mounted.current || serverIdentity() !== identity) return
        if (flowRef.current && flowRef.current !== state) return
        flowRef.current = ''
        loginProxy.current = undefined
        clearCallback()
        setValues({})
        setPassword('')
        setFlow(null)
        setProgress('')
        setError(dynamicLoginError(failure))
        void cancelDynamicLogin(state).catch(() => {})
      }
    }
    void poll()
    return () => { live = false; if (timer) clearTimeout(timer) }
  }, [flow, identity, provider])

  useEffect(() => {
    if (proxyMode !== 'pool') return
    const controller = new AbortController()
    setProxyPoolError('')
    void fetchProxyPool(controller.signal).then(result => {
      if (!controller.signal.aborted && serverIdentity() === identity) setProxyPool(result.items || [])
    }).catch(failure => {
      if (!controller.signal.aborted && serverIdentity() === identity) {
        setProxyPool([])
        setProxyPoolError(dynamicLoginError(failure))
      }
    })
    return () => controller.abort()
  }, [proxyMode, identity, poolReload])

  async function importSystemProxy() {
    if (locked || submitting.current) return
    submitting.current = true
    setReadingSystemProxy(true)
    setSystemProxyError('')
    try {
      const proxy = readSystemProxy(await apiCall('/api/proxies/system'), t)
      if (!mounted.current || serverIdentity() !== identity) return
      setProxyMode('custom')
      setProtocol(proxy.protocol)
      setHost(proxy.host)
      setPort(String(proxy.port))
      setUsername(proxy.username)
      setPassword(proxy.password)
    } catch (failure) {
      if (mounted.current && serverIdentity() === identity) setSystemProxyError(dynamicLoginError(failure))
    } finally {
      submitting.current = false
      if (mounted.current && serverIdentity() === identity) setReadingSystemProxy(false)
    }
  }

  function proxyPayload(): unknown {
    if (proxyMode === 'direct') return null
    if (proxyMode === 'pool') {
      if (!proxyId) throw new Error(t('请先选择已保存的代理'))
      const selected = proxyPool?.find(item => item.id === proxyId)
      if (!selected || selected.enabled === false || selected.resolveError) {
        throw new Error(t('当前代理不可用，请重新选择'))
      }
      return { source: 'pool', proxyId }
    }
    const number = Number(port)
    if (!host.trim()) throw new Error(t('请填写代理主机地址'))
    if (!Number.isInteger(number) || number < 1 || number > 65535) throw new Error(t('代理端口必须是 1-65535 的整数'))
    return { source: 'custom', protocol, host: host.trim(), port: number, username: username.trim(), password }
  }

  async function finish(warning = '') {
    clearCallback()
    loginProxy.current = undefined
    setValues({})
    setPassword('')
    setProgress('')
    await store.refresh(true)
    if (serverIdentity() !== identity) return
    store.toast(t('{label}账号已添加', { label: t(label) }))
    if (warning) store.toast(warning, 'warn')
    if (store.get().sessionError) store.toast(t('账号已添加，但账号列表同步失败，请刷新重试。'), 'warn')
    if (mounted.current) onAdded()
  }

  async function submit() {
    if (submitLocked || submitting.current || !method || methods.phase !== 'ready') return
    submitting.current = true
    setBusy(true)
    setError('')
    setProgress('')
    clearCallback()
    try {
      const fields = readInputs(method, inputs)
      const proxy = proxyPayload()
      const input = { provider, methodIndex: method.methodIndex!, inputs: fields, proxy }
      if (method.type === 'api') {
        if (method.localOnly) throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
        const remark = hasNamePrompt ? fields.name : name.trim()
        await finish(await addDynamicApiAccount({ ...input, ...(remark ? { name: remark } : {}) }))
        return
      }
      if (method.type !== 'oauth') throw new Error(t('请先选择有效的 OAuth 登录方式'))
      loginProxy.current = proxy
      const started = await startDynamicLogin(input)
      const state = started.state || started.flow
      if (!state || typeof state !== 'string') throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
      if (!mounted.current || serverIdentity() !== identity) {
        if (serverIdentity() === identity) await cancelDynamicLogin(state)
        return
      }
      const rawUrl = started.authUrl ?? started.url ?? ''
      let callbackInput: CallbackInput
      try {
        callbackInput = parseCallbackInput(started.callbackInput === undefined ? method.callbackInput : started.callbackInput)
        if (started.manualCallback !== undefined && typeof started.manualCallback !== 'boolean') throw new Error()
        if (method.manualCallback && (started.manualCallback === false || callbackInput !== method.callbackInput)) throw new Error()
        if (callbackInput === 'code' && !(started.manualCallback === true || method.manualCallback)) throw new Error()
      } catch {
        await cancelDynamicLogin(state)
        throw new Error(t('网关未返回有效的手动回调输入契约；请更新网关后重新发起。'))
      }
      const manual = started.manualCallback === true || method.manualCallback
      if ((started.provider && started.provider !== provider) || typeof rawUrl !== 'string'
        || (!rawUrl && !(started.localOnly === true || method.localOnly || manual || (started.pollIntervalMs || 0) > 0))) {
        await cancelDynamicLogin(state)
        throw new Error(t('登录授权地址无效，已取消本次等待。'))
      }
      if (rawUrl) {
        try { const url = new URL(rawUrl); if (url.protocol !== 'https:' || url.username || url.password) throw new Error() }
        catch { await cancelDynamicLogin(state); throw new Error(t('登录授权地址无效，已取消本次等待。')) }
      }
      flowRef.current = state
      setProgress(loginText(started.instructions))
      setFlow({ state, url: rawUrl, manual, callbackInput })
    } catch (failure) {
      if (mounted.current && serverIdentity() === identity) setError(dynamicLoginError(failure))
    } finally {
      submitting.current = false
      if (mounted.current && serverIdentity() === identity) setBusy(false)
    }
  }

  async function cancel() {
    if (!flow || canceling) return
    const state = flow.state
    clearCallback()
    setValues({})
    setPassword('')
    setCanceling(true)
    try {
      await cancelDynamicLogin(state)
      if (!mounted.current || serverIdentity() !== identity || flowRef.current !== state) return
      flowRef.current = ''
      loginProxy.current = undefined
      setFlow(null)
      setProgress('')
      store.toast(t('已取消登录等待'))
    } catch (failure) {
      if (mounted.current && serverIdentity() === identity) setError(dynamicLoginError(failure))
    } finally { if (mounted.current && serverIdentity() === identity) setCanceling(false) }
  }

  async function sendCallback() {
    if (!flow?.manual || callbackRequest.current || canceling || flowRef.current !== flow.state) return
    const raw = callbackInputRef.current?.value || ''
    if (!raw) return
    const state = flow.state
    const generation = callbackGeneration.current
    const isCurrent = () => mounted.current && serverIdentity() === identity && flowRef.current === state
      && callbackGeneration.current === generation
    callbackRequest.current = true
    setCallbackBusy(true)
    setCallbackError('')
    setCallbackNotice('')
    try {
      const next = await submitDynamicCallback(state, raw)
      if (!isCurrent()) return
      if (callbackInputRef.current) callbackInputRef.current.value = ''
      setHasCallback(false)
      if (next.retryable === true) {
        setCallbackError(loginText(next.hint) || t('回调未被接受，请重新复制本次登录的完整回调地址'))
        return
      }
      if (next.nextUrl) {
        const url = new URL(next.nextUrl)
        if (url.protocol !== 'https:' || url.username || url.password || /[\u0000-\u0020\u007f]/.test(next.nextUrl)) throw new Error()
        setFlow(current => current?.state === state ? { ...current, url: next.nextUrl! } : current)
        setCallbackNotice(t('还需继续授权，请打开下一步授权页，完成后回填新的完整回调地址。'))
        return
      }
      setCallbackNotice(t('回调已提交，仍需等待网关确认账号登录完成。'))
      // 回调收到不等于完成；账号添加只认 wait 的 done+session。
    } catch {
      if (isCurrent()) setCallbackError(t('授权回调提交失败；请检查输入并重试，登录仍在等待。'))
    } finally {
      if (isCurrent()) {
        callbackRequest.current = false
        setCallbackBusy(false)
      }
    }
  }

  const instructions = mergeLoginInstructions(methods.instructions, method?.instructions,
    method?.browserMode === 'plugin' ? pluginBrowserInstructions(provider) : '', progress)

  return <div className='flex min-w-0 flex-col gap-3'>
    {flow?.state ? <PluginLoginInteraction key={flow.state} flow={flow.state} t={t}
      request={(method, path, body) => apiCall(path, { method, body })} /> : null}
    {methods.phase === 'loading' ? <p><Spinner /> {t('正在读取插件登录方式…')}</p> : null}
    {methods.reason ? <Alert variant={methods.phase === 'error' ? 'destructive' : 'warning'}>
      <AlertDescription>{methods.reason}</AlertDescription></Alert> : null}
    {method ? <>
      {!hideMethodPicker ? <Select value={method.id} disabled={locked} onValueChange={value => {
        setSelected(value || ''); setValues({}); setPassword(''); clearCallback(); setError('')
      }}>
        <SelectTrigger className='w-full max-w-full' aria-label={t('账号登录方式')} title={methodLabel(method)}><SelectValue>{methodLabel(method)}</SelectValue></SelectTrigger>
        <SelectContent className='w-[min(var(--anchor-width),var(--available-width))]'>
          {methods.methods.map(item => <SelectItem key={item.id} value={item.id}
            className='[&>:first-child]:whitespace-normal [&>:first-child]:break-words'>{methodLabel(item)}</SelectItem>)}
        </SelectContent>
      </Select> : null}
      {method.reason ? <Alert variant='warning'><AlertDescription>{method.reason}</AlertDescription></Alert> : null}
      {instructions ? <Alert>
        <AlertTitle>{t('登录指引 / 设备码授权')}</AlertTitle>
        <AlertDescription className='whitespace-pre-wrap break-words'>{instructions}</AlertDescription>
      </Alert> : null}
      {method.localOnly ? <Alert variant='warning'><AlertTitle>{t('需要网关本机环境')}</AlertTitle>
        <AlertDescription>{t('“本机”指运行网关的机器，不是打开面板的电脑。远程浏览器也可以发起，但网关本机可能需要安装客户端并已登录；是否可用由网关返回结果决定。')}</AlertDescription></Alert> : null}
      {method.prompts.map((prompt, index) => {
        if (!promptEnabled(prompt, method, inputs)) return null
        const id = `${prefix}-${index}`
        const value = promptValue(prompt, inputs)
        const change = (value: string) => setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [prompt.key]: value } }))
        return <div key={`${method.id}:${prompt.key}`} className='flex flex-col gap-1.5'>
          <Label htmlFor={id}>{prompt.label}{prompt.optional ? t('（可选）') : t('（必填）')}</Label>
          {prompt.type === 'select' ? <SearchableSelect value={value} disabled={locked} onValueChange={change}
            id={id} className='w-full' searchable={prompt.options.length > 8}
            displayValue={prompt.options.find(option => option.value === value)?.label || prompt.placeholder || t('请选择')}
            options={[
              ...(prompt.optional ? [{ value: '', label: t('不填写（可选）') }] : []),
              ...prompt.options,
            ]} /> : <Input id={id} value={value} type={prompt.secret ? 'password' : 'text'} disabled={locked} spellCheck={false}
            autoComplete={prompt.secret ? 'new-password' : 'off'} placeholder={prompt.placeholder} onChange={event => change(event.currentTarget.value)} />}
        </div>
      })}
      {method.type === 'api' && !hasNamePrompt ? <Label className='flex flex-col items-start gap-1.5'>{t('备注名')}
        <Input value={name} maxLength={100} disabled={locked} onChange={event => setName(event.currentTarget.value)} placeholder={t('可选，留空使用账号昵称')} />
      </Label> : method.type === 'oauth' && !hasNamePrompt ? <p className='text-xs text-subtle'>{t('账号名由插件自动获取，添加后可修改。')}</p> : null}
      <Label>{t('账号 / 登录出网代理')}</Label>
      <Select value={proxyMode} disabled={locked} onValueChange={value => setProxyMode(value || 'direct')}>
        <SelectTrigger className='w-full'><SelectValue>{proxyMode === 'pool' ? t('已保存的代理') : proxyMode === 'custom' ? t('自定义') : t('无代理（直连）')}</SelectValue></SelectTrigger>
        <SelectContent><SelectItem value='direct'>{t('无代理（直连）')}</SelectItem><SelectItem value='pool'>{t('已保存的代理')}</SelectItem><SelectItem value='custom'>{t('自定义')}</SelectItem></SelectContent>
      </Select>
      <Button size='sm' variant='outline' disabled={locked} className='self-start' onClick={() => void importSystemProxy()}>
        {readingSystemProxy ? <Spinner /> : null}{t('填入系统代理')}
      </Button>
      <p className='text-xs text-subtle'>{t('读取网关电脑当前的 HTTPS 系统代理，填入自定义地址；后续系统设置变化不会自动同步。')}</p>
      {systemProxyError ? <p className='text-xs text-destructive' role='alert'>{systemProxyError}</p> : null}
      {proxyMode === 'pool' ? <div className='flex flex-col gap-2'>
        <Select value={proxyId} disabled={locked || !proxyPool?.length} onValueChange={value => setProxyId(value || '')}>
          <SelectTrigger className='w-full' aria-label={t('代理')}><SelectValue>{proxyPool?.find(item => item.id === proxyId)?.name
            || (proxyId ? t('当前代理不可用，请重新选择') : proxyPool === null ? t('正在读取…') : t('请选择代理'))}</SelectValue></SelectTrigger>
          <SelectContent>{(proxyPool || []).map(item => <SelectItem key={item.id} value={item.id}
            disabled={item.enabled === false || Boolean(item.resolveError)}>{item.name}</SelectItem>)}</SelectContent>
        </Select>
        {proxyPoolError ? <p className='text-xs text-destructive'>{proxyPoolError}</p>
          : proxyPool?.length === 0 ? <p className='text-xs text-subtle'>{t('还没有保存的代理')}</p> : null}
        <Button size='sm' variant='outline' disabled={locked} className='self-start'
          onClick={() => setPoolReload(value => value + 1)}>{t('重新读取')}</Button>
      </div> : null}
      {proxyMode === 'custom' ? <Card className='flex flex-col gap-2 p-3'>
        <Select value={protocol} disabled={locked} onValueChange={value => setProtocol(value || 'http')}>
          <SelectTrigger className='w-full' aria-label={t('协议')}><SelectValue>{protocol === 'socks5' ? 'SOCKS5' : 'HTTP'}</SelectValue></SelectTrigger>
          <SelectContent><SelectItem value='http'>HTTP</SelectItem><SelectItem value='socks5'>SOCKS5</SelectItem></SelectContent>
        </Select>
        <Label className='flex flex-col items-start gap-1'>{t('主机')}<Input value={host} disabled={locked} onChange={event => setHost(event.currentTarget.value)} /></Label>
        <Label className='flex flex-col items-start gap-1'>{t('端口')}<Input value={port} type='number' min={1} max={65535} disabled={locked} onChange={event => setPort(event.currentTarget.value)} /></Label>
        <Label className='flex flex-col items-start gap-1'>{t('用户名')}<Input value={username} disabled={locked} autoComplete='off' onChange={event => setUsername(event.currentTarget.value)} /></Label>
        <Label className='flex flex-col items-start gap-1'>{t('密码')}<Input value={password} type='password' disabled={locked} autoComplete='new-password' onChange={event => setPassword(event.currentTarget.value)} /></Label>
      </Card> : null}
      <Button disabled={submitLocked || Boolean(method.reason)} onClick={() => void submit()}>{busy || resolvedPrompts.pending ? <Spinner /> : null}{method.type === 'api' ? t('添加账号') : t('登录并添加')}</Button>
      <p className='text-xs text-subtle'>{t('所选代理会保存到账号设置，后续请求继续使用。')}</p>
    </> : null}
    {flow ? <Alert><AlertTitle>{t('等待账号登录完成…')}</AlertTitle>
      <AlertDescription className='flex flex-col gap-2 whitespace-pre-wrap break-words'>
        {flow.url ? <Button variant='outline' onClick={() => window.open(flow.url, '_blank', 'noopener,noreferrer')}>{t('打开官方授权页')}</Button> : null}
        {flow.manual ? <>
          <Badge variant='outline'>{t('需要手动提交授权回调')}</Badge>
          <p>{flow.callbackInput === 'code'
            ? t('此方式需要原样提交授权码或令牌；输入会隐藏，不会当作 URL 或 API 凭证解析。提交后仍需等待网关确认。')
            : t('此方式需要完整回调地址；请原样粘贴，不拆分参数或重建地址。提交后仍需等待网关确认。')}</p>
          <Label htmlFor={`${prefix}-callback`}>{flow.callbackInput === 'code'
            ? t('授权码或令牌（原样提交）') : t('完整回调地址（原样提交）')}</Label>
          <Input key={`${flow.state}:${flow.callbackInput}`} id={`${prefix}-callback`} ref={callbackInputRef}
            type='password' defaultValue=''
            autoComplete='new-password' spellCheck={false} autoCapitalize='none'
            placeholder={flow.callbackInput === 'code' ? t('授权码或令牌（原样提交）') : t('完整回调地址（原样提交）')}
            onChange={event => { setHasCallback(event.currentTarget.value.length > 0); setCallbackError(''); setCallbackNotice('') }}
            disabled={callbackBusy || canceling} />
          <Button variant='outline' disabled={!hasCallback || callbackBusy || canceling} onClick={() => void sendCallback()}>
            {callbackBusy ? <Spinner /> : null}{t('提交授权回调')}</Button>
          {callbackNotice ? <p role='status'>{callbackNotice}</p> : null}
          {callbackError ? <Alert variant='destructive'><AlertDescription>{callbackError}</AlertDescription></Alert> : null}
        </> : flow.url ? <p>{t('如官方页面跳回的是网关本机地址，请在桌面面板完成回调；手机端不会猜测未声明的回调输入。')}</p> : null}
        <Button variant='outline' disabled={canceling} onClick={() => void cancel()}>{t('取消等待')}</Button>
      </AlertDescription>
    </Alert> : null}
    {error || resolvedPrompts.error ? <Alert variant='destructive'><AlertDescription className='break-words'>{error || resolvedPrompts.error}</AlertDescription></Alert> : null}
  </div>
}
