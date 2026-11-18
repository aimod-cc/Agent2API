/** 动态登录入口；列表只从 session 实际运行时摘要派生，不登记第二份 provider。 */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import {
  Alert, AlertDescription, AlertTitle, Badge, Button, Card, Input, Label, Select,
  SelectContent, SelectItem, SelectTrigger, SelectValue, Sheet, SheetBody, SheetContent,
  SheetHeader, SheetTitle, SheetFooter, Spinner,
} from '@ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { ProviderSponsorCards } from '../components/ProviderSponsorCards'
import { providerLabel } from '../lib/providers'
import { serverIdentity } from '../lib/servers'
import { openExternal } from '../lib/community'
import { store, useStore } from '../state/store'
import { fetchProxyPool, type PoolItem } from '../api/gateway'
import {
  addDynamicApiAccount, cancelDynamicLogin, dynamicLoginError, methodsUnavailable,
  readDynamicMethods, saveDynamicAccountProxy, startDynamicLogin, submitDynamicCallback, waitDynamicLogin,
} from '../api/add-account-dynamic'
import {
  loginText, mergeLoginInstructions, parseCallbackInput, parseMethods, pluginBrowserInstructions, readInputs,
  type CallbackInput, type MethodsState,
} from '../../../ui-islands/src/islands/add-account-dynamic-fields'
import { usePluginPrompts } from '../../../ui-islands/src/islands/add-account-plugin-prompts'
import { isAddAccountProviderHidden } from '../../../ui-shared/provider-availability'
import { PluginLoginInteraction, type LoginRequest } from '../../../ui-shared/plugin-login-interaction'
import {
  MANAGEMENT_SESSION_CHANGE, managedCaptchaBusy, managedLoginSourceGeneration, managedLoginWaitMessage, subscribeManagedCaptchaBusy,
  type ManagedLoginScope, type ManagedLoginWait, type ManagedLoginWaitPhase,
} from '../../../ui-shared/plugin-login-managed'
import { getPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { PluginLoginActions, PluginLoginBrowser, PluginLoginFields, PluginLoginMethodPicker, PluginLoginOptions, PluginLoginSecurity } from '../../../ui-shared/plugin-login-form'
import { PluginLoginDiagnostic } from '../../../ui-shared/plugin-login-diagnostic-view'
import { loginDiagnosticOf, loginFailure, type LoginDiagnostic } from '../../../ui-shared/plugin-login-diagnostic'
import { readSystemProxy } from '../../../ui-shared/proxy-system'
import { apiCall } from '../api/client'
import { locale, t } from '../i18n'
import '../styles/add-account.css'

export function DynamicProviderSheet({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const { session, sessionError } = useStore()
  const providers = sessionError ? [] : (session?.accounts?.providers || [])
    .filter(item => Boolean(item.id) && !isAddAccountProviderHidden(item.id || '') && item.loginAvailable !== false
      && session?.pluginStartup?.providers[item.id || '']?.available !== false)
  const [selected, setSelected] = useState('')
  const [visited, setVisited] = useState<string[]>([])
  const verifying = useSyncExternalStore(subscribeManagedCaptchaBusy, managedCaptchaBusy)
  const selectProvider = (id: string) => { setVisited(previous => previous.includes(id) ? previous : [...previous, id]); setSelected(id) }
  const [search, setSearch] = useState('')
  const keyword = search.trim().toLowerCase()
  const selectedProvider = providers.find(item => item.id === selected)
  const cards = providers
  const identity = serverIdentity()
  const [footerHost] = useState(() => document.createElement('div'))

  return <Sheet open modal={!verifying} onOpenChange={(open, details) => {
    if (!open && verifying && ['outside-press', 'focus-out'].includes(details.reason)) { details.cancel(); return }
    if (!open) onClose()
  }}>
    <SheetContent className='mobile-account-add'>
      <SheetHeader>{selectedProvider ? <ProviderLogo id={selected} label={selectedProvider.label} /> : null}<SheetTitle>{selectedProvider
        ? t('添加 {name} 账号', { name: providerLabel(selected, selectedProvider.label) })
        : t('选择提供商')}</SheetTitle></SheetHeader>
      <SheetBody>
        {selectedProvider ? null : <>
          <p className='text-xs text-subtle'>{t('提供商仅在已启用且加载后出现。登录与本机引用均作用在所连接网关，不是手机。')}</p>
          <Input type='search' value={search} placeholder={t('搜索提供商…')}
            onChange={event => setSearch(event.currentTarget.value)} />
          {sessionError ? <Alert variant='destructive'><AlertDescription>{sessionError}</AlertDescription></Alert> : null}
          <div className='grid grid-cols-1 gap-2 min-[420px]:grid-cols-2'>
            {cards.filter(item => !keyword || `${item.id} ${providerLabel(item.id, item.label)}`.toLowerCase().includes(keyword))
              .map(item => <Button key={item.id} variant='outline' className='h-auto min-h-16 justify-start whitespace-normal text-left'
                onClick={() => selectProvider(item.id || '')}>
                <ProviderLogo id={item.id} label={item.label} />
                <span className='flex min-w-0 flex-col gap-1'>
                  <span>{providerLabel(item.id, item.label)}</span>
                  <span className='text-[11px] text-muted-foreground'>{t('{n} 个账号', { n: item.count || 0 })}</span>
                </span>
              </Button>)}
          </div>
          {!providers.length ? <p className='text-xs text-subtle'>{t('请安装并启用对应的提供商插件；列表为空时请刷新后重试。')}</p> : null}
          <Button variant='outline' onClick={() => void store.refresh(true)}>{t('重新读取提供商')}</Button>
          <ProviderSponsorCards />
        </>}
        {visited.map(id => {
          const item = providers.find(item => item.id === id)
          if (!item) return null
          return <div key={`${identity}:${id}`} hidden={selected !== id}>
            <DynamicForm provider={id} label={item.label || id} source={item.source} onAdded={onAdded}
              active={selected === id} footerHost={footerHost} />
          </div>
        })}
      </SheetBody>
      <SheetFooter className='mobile-account-add-footer' hidden={!selectedProvider}>
        <Button variant='ghost' onClick={() => setSelected('')}>{t('更换提供商')}</Button>
        <div className='mobile-account-add-foot-mount' ref={node => { if (node && footerHost.parentElement !== node) node.append(footerHost) }} />
      </SheetFooter>
    </SheetContent>
  </Sheet>
}

type DynamicFormProps = {
  provider: string
  label: string
  source?: string
  onAdded(): void
  active: boolean
  footerHost: HTMLElement
}

function DynamicForm({ provider, label, source, onAdded, active, footerHost }: DynamicFormProps) {
  const prefix = `mobile-add-${provider}`
  const [methods, setMethods] = useState<MethodsState>({ phase: 'loading', methods: [], instructions: '', reason: '' })
  const [selected, setSelected] = useState('')
  const [values, setValues] = useState<Record<string, Record<string, string>>>({})
  const [name, setName] = useState('')
  const loginRemark = useRef('')
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
  const [error, updateError] = useState('')
  const [diagnostic, setDiagnostic] = useState<LoginDiagnostic>()
  const setError = (message: string, source?: unknown): void => {
    updateError(message)
    setDiagnostic(message ? loginDiagnosticOf(source) : undefined)
  }
  const [progress, setProgress] = useState('')
  const [flow, setFlow] = useState<{
    state: string; url: string; manual: boolean; callbackInput: CallbackInput;
    managed: boolean; methodIndex: number; bootId: string; deadline: number; interval: number; managementGeneration: number;
  } | null>(null)
  const [managedPhase, setManagedPhase] = useState<ManagedLoginWaitPhase | ''>('')
  const [waitUnconfirmed, setWaitUnconfirmed] = useState(false)
  const [readingStatus, setReadingStatus] = useState(false)
  const [statusRequest, setStatusRequest] = useState(0)
  const waitStatusSeen = useRef(0)
  const waitBusy = useRef(false)
  const waitReadOnly = useRef(false)
  const waitController = useRef<AbortController | null>(null)
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
  const lifecycle = useRef(0)
  const identity = serverIdentity()
  const originBoot = () => getPluginStartup().value?.bootId || store.get().session?.pluginStartup?.bootId || ''
  const currentFlow = useRef(flow)
  currentFlow.current = flow
  const flowCurrent = (owner: NonNullable<typeof flow>) => mounted.current && flowRef.current === owner.state
    && serverIdentity() === identity && (!owner.managed || originBoot() === owner.bootId
      && owner.managementGeneration === managedLoginSourceGeneration() && !store.get().authLost)
  const interactionCurrent = (owner: NonNullable<typeof flow>) => flowCurrent(owner)
    && (!owner.managed || !waitReadOnly.current && !waitUnconfirmed && ['waiting', ''].includes(managedPhase) && performance.now() < owner.deadline)
  const declaredMethod = methods.methods.find(item => item.id === selected) || methods.methods.find(item => !item.reason) || methods.methods[0]
  const inputs = declaredMethod ? values[declaredMethod.id] || {} : {}
  const resolvedPrompts = usePluginPrompts(provider, declaredMethod, inputs, active && !flow && !busy,
    (path, body) => apiCall(path, { method: 'POST', body }))
  const method = resolvedPrompts.method
  const hasNamePrompt = Boolean(method?.prompts.some(prompt => prompt.key === 'name'))
  const locked = busy || canceling || Boolean(flow) || readingSystemProxy
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
    const stop = () => {
      lifecycle.current += 1
      const state = flowRef.current
      flowRef.current = ''
      clearCallback()
      const owner = currentFlow.current
      if (state && serverIdentity() === identity && (!owner?.managed || !waitReadOnly.current && owner.bootId === originBoot()
        && owner.managementGeneration === managedLoginSourceGeneration() && !store.get().authLost)) {
        void cancelDynamicLogin(state, identity).catch(() => {})
      }
      waitController.current?.abort(); loginProxy.current = undefined
      setFlow(null); setManagedPhase(''); setWaitUnconfirmed(false)
    }
    const sourceLost = () => {
      const owner = currentFlow.current
      if (!owner?.managed) return
      lifecycle.current += 1
      flowRef.current = ''; waitController.current?.abort(); clearCallback(); loginProxy.current = undefined
      setFlow(null); setValues({}); setPassword(''); setProgress(''); setManagedPhase(''); setWaitUnconfirmed(false)
      setError(t('管理会话已变化，本次登录资格已失效；不会继续查询或重发原流程。'))
    }
    const guard = () => {
      const owner = currentFlow.current
      if (owner?.managed && (serverIdentity() !== identity || originBoot() !== owner.bootId
        || owner.managementGeneration !== managedLoginSourceGeneration() || store.get().authLost)) sourceLost()
    }
    const unsubscribe = store.subscribe(guard)
    const unsubscribeBoot = subscribePluginStartup(guard)
    window.addEventListener('pagehide', stop)
    window.addEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost)
    return () => {
      mounted.current = false; lifecycle.current += 1
      unsubscribe(); unsubscribeBoot(); window.removeEventListener('pagehide', stop)
      window.removeEventListener(MANAGEMENT_SESSION_CHANGE, sourceLost); waitController.current?.abort()
      clearCallback()
      const owner = currentFlow.current
      if (flowRef.current && serverIdentity() === identity && (!owner?.managed || !waitReadOnly.current && owner.bootId === originBoot()
        && owner.managementGeneration === managedLoginSourceGeneration() && !store.get().authLost)) {
        void cancelDynamicLogin(flowRef.current, identity).catch(() => {})
      }
      flowRef.current = ''; loginProxy.current = undefined
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
      if (provider === 'trae') parsed.methods = parsed.methods.filter(method => method.type !== 'oauth')
      if (provider === 'deepseek-web') parsed.methods = parsed.methods.filter(method => method.type !== 'oauth')
      setMethods(parsed)
    }).catch(failure => {
      if (controller.signal.aborted) return
      const builtin = source === 'builtin' || provider.endsWith('-subscription')
      setMethods({ phase: 'error', methods: [], instructions: '', reason: methodsUnavailable(failure)
        ? builtin ? t('本体订阅未返回动态登录方式（501）；不会猜测导入接口或显示插件手动凭证入口。请更新网关后重试。')
          : t('该提供商未声明动态登录方式，请更新插件或网关后重试；不会使用旧版兼容入口。')
        : dynamicLoginError(failure) })
    })
    return () => controller.abort()
  }, [provider, source])

  useEffect(() => {
    if (!flow) return
    if (flow.managed) {
      let live = true
      let expired = performance.now() >= flow.deadline
      const manualStatus = statusRequest !== waitStatusSeen.current
      waitStatusSeen.current = statusRequest
      let timer: ReturnType<typeof setTimeout> | undefined
      const current = () => live && flowCurrent(flow)
      const unknown = () => {
        if (!current()) return
        waitReadOnly.current = true
        clearCallback(); setValues({}); setPassword(''); loginProxy.current = undefined
        setManagedPhase('unconfirmed'); setWaitUnconfirmed(true); setProgress('')
        setError(t('账号保存未确认，请查询本次状态；不会再次保存。'))
      }
      const poll = async (manual = false) => {
        if (!current() || waitBusy.current) return
        if (!manual && (expired || performance.now() >= flow.deadline)) { expired = true; unknown(); return }
        const controller = new AbortController()
        waitController.current = controller; waitBusy.current = true; setReadingStatus(true)
        try {
          const reply = await waitDynamicLogin(flow.state, { provider, methodIndex: flow.methodIndex }, controller.signal) as ManagedLoginWait
          if (!current() || controller.signal.aborted) return
          setManagedPhase(reply.phase)
          if (reply.saved === null || reply.phase === 'saved') waitReadOnly.current = true
          if (reply.phase === 'saved' && reply.saved === true && reply.done === true && reply.session) {
            // 保存已确认才刷新列表；managed 不走旧 PATCH 补偿保存或再次提交账号。
            const warning = [reply.session.warning ? t(reply.session.warning) : '',
              reply.session.proxySaved === true ? '' : t('账号已添加，但代理选择未确认保存；请在账号设置中重新检查。')].filter(Boolean).join('\n')
            const remarkWarning = await saveLoginRemark(reply.session.accountUid || reply.session.accountId!, flow)
            if (!current()) return
            flowRef.current = ''; loginProxy.current = undefined; clearCallback()
            setFlow(null); setWaitUnconfirmed(false)
            await finish([warning, remarkWarning].filter(Boolean).join('\n'), flow.bootId, flow.managementGeneration)
            return
          }
          if (reply.phase === 'unconfirmed') { unknown(); return }
          if (reply.phase === 'failed' || reply.phase === 'canceled') {
            flowRef.current = ''; loginProxy.current = undefined; clearCallback(); setValues({}); setPassword('')
            setFlow(null); setProgress(''); setWaitUnconfirmed(false)
            if (reply.phase === 'canceled') store.toast(t('已取消登录等待'))
            else setError(managedLoginWaitMessage(reply, t))
            return
          }
          setWaitUnconfirmed(false); setError('')
          if (waitReadOnly.current && reply.phase === 'waiting') { unknown(); return }
          setProgress(reply.phase === 'saving' ? managedLoginWaitMessage(reply, t)
            : mergeLoginInstructions(loginText(reply.instructions), loginText(reply.hint)))
          if (expired || performance.now() >= flow.deadline) { expired = true; unknown(); return }
          timer = setTimeout(() => void poll(), flow.interval)
        } catch {
          // 读取失败或未知终态只留原 flow 的只读查询；不取消、补保存或重发任何登录材料。
          if (!controller.signal.aborted) unknown()
        } finally {
          if (waitController.current === controller) waitController.current = null
          waitBusy.current = false
          if (mounted.current && serverIdentity() === identity) setReadingStatus(false)
        }
      }
      void poll(manualStatus)
      const expiry = !expired ? setTimeout(() => {
        if (!current()) return
        expired = true
        if (timer) { clearTimeout(timer); timer = undefined }
        // 到期只停止后续自动查询；已开始的原同源 GET 仍能交付真实保存回执。
        unknown()
      }, Math.max(0, flow.deadline - performance.now())) : undefined
      return () => {
        live = false; waitController.current?.abort()
        if (timer) clearTimeout(timer)
        if (expiry) clearTimeout(expiry)
      }
    }
    let live = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const state = flow.state
    const poll = async () => {
      if (!live || !mounted.current || flowRef.current !== state || serverIdentity() !== identity) return
      try {
        if (flow.managed && !flowCurrent(flow)) throw new Error(t('本次登录步骤已过期，请取消后重新发起登录。'))
        const reply = await waitDynamicLogin(state)
        if (!live || !mounted.current || flowRef.current !== state || serverIdentity() !== identity) return
        if (flow.managed && !flowCurrent(flow)) throw new Error(t('本次登录步骤已过期，请取消后重新发起登录。'))
        if (reply.error) throw loginFailure(reply.error, reply)
        if (reply.done === true) {
          if (!reply.session || !(reply.session.accountUid || reply.session.accountId)
            || (flow.managed ? reply.session.provider !== provider : reply.session.provider && reply.session.provider !== provider)) {
            throw new Error(t('登录桥返回了空会话，无法确认账号已添加'))
          }
          const warning = await saveDynamicAccountProxy(
            reply.session.accountUid || reply.session.accountId!, loginProxy.current,
            reply.session.proxySaved === true, reply.session.warning || '', identity,
          )
          const remarkWarning = await saveLoginRemark(reply.session.accountUid || reply.session.accountId!, flow)
          if (!live || !mounted.current || serverIdentity() !== identity || flow.managed && !flowCurrent(flow)) return
          flowRef.current = ''
          loginProxy.current = undefined
          setFlow(null)
          await finish([warning, remarkWarning].filter(Boolean).join('\n'), flow.managed ? flow.bootId : undefined)
          return
        }
        if (reply.pending !== true) throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
        setProgress(mergeLoginInstructions(loginText(reply.instructions), loginText(reply.hint)))
        timer = setTimeout(() => void poll(), flow.interval)
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
        setError(dynamicLoginError(failure), failure)
        if (!flow.managed || flow.bootId === originBoot()) void cancelDynamicLogin(state, identity).catch(() => {})
      }
    }
    void poll()
    const expiry = flow.managed ? setTimeout(() => {
      if (!live || !mounted.current || flowRef.current !== state || serverIdentity() !== identity) return
      flowRef.current = ''; loginProxy.current = undefined; clearCallback(); setFlow(null)
      setError(t('本次登录步骤已过期，请取消后重新发起登录。'))
      if (flow.bootId === originBoot()) void cancelDynamicLogin(state, identity).catch(() => {})
    }, Math.max(0, flow.deadline - performance.now())) : undefined
    return () => { live = false; if (timer) clearTimeout(timer); if (expiry) clearTimeout(expiry) }
  }, [flow, identity, provider, statusRequest])

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

  async function saveLoginRemark(accountId: string, owner: NonNullable<typeof flow>): Promise<string> {
    const remark = loginRemark.current
    if (!remark || !flowCurrent(owner) || owner.managementGeneration !== managedLoginSourceGeneration() || owner.bootId !== originBoot()) return ''
    loginRemark.current = ''
    try {
      await apiCall(`/api/accounts/${encodeURIComponent(accountId)}`, { method: 'PATCH', body: { name: remark }, noAuthReplay: true })
      return ''
    } catch { return t('账号已添加，但备注名未保存，请在账号设置中修改。') }
  }

  async function finish(warning = '', expectedBoot?: string, expectedGeneration?: number) {
    clearCallback()
    loginProxy.current = undefined
    setValues({})
    setPassword('')
    setProgress('')
    setManagedPhase(''); setWaitUnconfirmed(false)
    await store.refresh(true)
    if (serverIdentity() !== identity || expectedBoot !== undefined && expectedBoot !== originBoot()
      || expectedGeneration !== undefined && expectedGeneration !== managedLoginSourceGeneration()) return
    store.toast(t('{label}账号已添加', { label: t(label) }))
    if (warning) store.toast(warning, 'warn')
    if (store.get().sessionError) store.toast(t('账号已添加，但账号列表同步失败，请刷新重试。'), 'warn')
    if (mounted.current) onAdded()
  }

  async function submit() {
    if (submitLocked || submitting.current || canceling || !method || methods.phase !== 'ready') return
    const epoch = lifecycle.current
    submitting.current = true
    setBusy(true)
    setError('')
    setProgress('')
    setManagedPhase(''); setWaitUnconfirmed(false); waitReadOnly.current = false
    clearCallback()
    try {
      const fields = readInputs(method, inputs)
      const proxy = proxyPayload()
      const input = { provider, methodIndex: method.methodIndex!, inputs: fields, proxy }
      if (method.type === 'api' || method.type === 'desktop') {
        if (method.type === 'api' && method.localOnly) throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
        const remark = hasNamePrompt ? fields.name : name.trim()
        await finish(await addDynamicApiAccount({ ...input, ...(method.type === 'desktop' ? { importDesktop: true } : {}), ...(remark ? { name: remark } : {}) }))
        return
      }
      if (method.type !== 'oauth' && method.type !== 'sms') throw new Error(t('请先选择有效的登录方式'))
      loginProxy.current = proxy
      loginRemark.current = hasNamePrompt ? '' : name.trim()
      const startedAt = performance.now()
      const initialBoot = originBoot()
      const managementGeneration = managedLoginSourceGeneration()
      const loginInput = { ...input, locale: locale() }
      const started = await startDynamicLogin(loginInput)
      const state = started.state || started.flow
      if (!state || typeof state !== 'string') throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
      if (!mounted.current || lifecycle.current !== epoch || serverIdentity() !== identity) {
        if (serverIdentity() === identity && initialBoot === originBoot()
          && (started.managedLogin !== true || managementGeneration === managedLoginSourceGeneration())) await cancelDynamicLogin(state, identity)
        return
      }
      if (started.managedLogin === true && managementGeneration !== managedLoginSourceGeneration()) {
        throw new Error(t('管理会话已变化，本次登录资格已失效；不会继续查询或重发原流程。'))
      }
      const rawUrl = started.authUrl ?? started.url ?? ''
      const managed = started.managedLogin === true
      if (managed && (started.provider !== provider || started.methodIndex !== method.methodIndex || started.flow !== state
        || rawUrl !== '' || started.browserMode !== 'none' || started.manualCallback !== false || started.callbackInput !== 'url'
        || typeof started.remainingMs !== 'number' || !Number.isFinite(started.remainingMs) || started.remainingMs <= 0
        || !initialBoot || initialBoot !== originBoot())) {
        if (initialBoot === originBoot()) await cancelDynamicLogin(state, identity)
        throw new Error(t('登录步骤与当前流程不匹配，请重新发起登录。'))
      }
      let callbackInput: CallbackInput
      try {
        callbackInput = parseCallbackInput(started.callbackInput === undefined ? method.callbackInput : started.callbackInput)
        if (started.manualCallback !== undefined && typeof started.manualCallback !== 'boolean') throw new Error()
        if (!managed && method.manualCallback && (started.manualCallback === false || callbackInput !== method.callbackInput)) throw new Error()
        if (callbackInput === 'code' && !(started.manualCallback === true || method.manualCallback)) throw new Error()
      } catch {
        await cancelDynamicLogin(state)
        throw new Error(t('网关未返回有效的手动回调输入契约；请更新网关后重新发起。'))
      }
      const manual = !managed && (started.manualCallback === true || method.manualCallback)
      if ((started.provider && started.provider !== provider) || typeof rawUrl !== 'string'
        || (!rawUrl && !(managed || started.localOnly === true || method.localOnly || manual
          || method.type === 'sms' && started.interactionType === 'sms' || (started.pollIntervalMs || 0) > 0))) {
        await cancelDynamicLogin(state)
        throw new Error(t('登录授权地址无效，已取消本次等待。'))
      }
      if (rawUrl) {
        try { const url = new URL(rawUrl); if (url.protocol !== 'https:' || url.username || url.password) throw new Error() }
        catch { await cancelDynamicLogin(state); throw new Error(t('登录授权地址无效，已取消本次等待。')) }
      }
      flowRef.current = state
      setProgress(loginText(started.instructions))
      setManagedPhase(managed ? 'waiting' : '')
      setFlow({ state, url: rawUrl, manual, callbackInput, managed, methodIndex: method.methodIndex!, bootId: initialBoot,
        managementGeneration, deadline: managed ? startedAt + started.remainingMs! : Infinity,
        interval: managed ? 1500 : 1000 })
    } catch (failure) {
      if (mounted.current && serverIdentity() === identity && lifecycle.current === epoch) setError(dynamicLoginError(failure), failure)
    } finally {
      submitting.current = false
      if (mounted.current && serverIdentity() === identity) setBusy(false)
    }
  }

  async function cancel() {
    if (!flow || canceling) return
    const sameSource = flowCurrent(flow)
    const readOnly = flow.managed && waitReadOnly.current
    const state = flow.state
    flowRef.current = ''
    waitController.current?.abort()
    setFlow(null)
    setManagedPhase(''); setWaitUnconfirmed(false)
    clearCallback()
    setValues({})
    setPassword('')
    setCanceling(true)
    try {
      if (readOnly) {
        setProgress(''); setError(t('账号保存未确认，请查询本次状态；不会再次保存。'))
        return
      }
      if (flow.managed && !sameSource) return
      await cancelDynamicLogin(state, identity)
      if (!mounted.current || serverIdentity() !== identity || flowRef.current && flowRef.current !== state) return
      flowRef.current = ''
      loginProxy.current = undefined
      setFlow(null)
      setProgress('')
      store.toast(t('已取消登录等待'))
    } catch (failure) {
      if (mounted.current && serverIdentity() === identity) setError(dynamicLoginError(failure), failure)
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

  const managedScope: ManagedLoginScope | undefined = flow?.managed ? {
    provider, methodIndex: flow.methodIndex, key: JSON.stringify([identity, flow.bootId, provider, flow.state]),
    deadline: flow.deadline, current: () => interactionCurrent(flow),
  } : undefined
  const interactionRequest: LoginRequest = async <T,>(verb: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown, options?: { signal?: AbortSignal }) => {
    const owner = currentFlow.current
    if (!owner || !interactionCurrent(owner) || options?.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
    const response = await apiCall<T>(path, { method: verb, body, signal: options?.signal,
      ...(owner.managed ? { noAuthReplay: true } : {}),
      quiet: verb === 'POST' && (path === '/api/session/login/callback' || path === '/api/session/login/interaction') })
    if (!interactionCurrent(owner) || options?.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
    return response
  }
  const optionalName = method?.prompts.find(prompt => prompt.key === 'name' && prompt.type === 'text' && !prompt.secret && prompt.optional && !prompt.when)
  const formPrompts = method?.prompts.filter(prompt => prompt !== optionalName) || []
  const accountName = optionalName ? inputs[optionalName.key] ?? optionalName.default ?? '' : name
  const proxyLabel = proxyMode === 'pool' ? t('已保存的代理') : proxyMode === 'custom' ? t('自定义代理') : t('无代理（直连）')
  const instructions = mergeLoginInstructions(methods.instructions, method?.instructions,
    method?.browserMode === 'plugin' ? pluginBrowserInstructions(provider) : '', progress)

  return <div className='plugin-account-form' hidden={!active}>
    {flow?.state && (!flow.managed || interactionCurrent(flow)) ? <PluginLoginInteraction key={flow.state} flow={flow.state} t={t} openExternal={openExternal}
      managed={managedScope} request={interactionRequest} /> : null}
    {methods.phase === 'loading' ? <p><Spinner /> {t('正在读取插件登录方式…')}</p> : null}
    {methods.reason ? <Alert variant={methods.phase === 'error' ? 'destructive' : 'warning'}>
      <AlertDescription>{methods.reason}</AlertDescription></Alert> : null}
    {method ? <>
      <PluginLoginMethodPicker methods={methods.methods} value={method.id} disabled={locked}
        onChange={value => { setSelected(value); setValues({}); setPassword(''); clearCallback(); setError('') }} />
      {method.reason ? <Alert variant='warning'><AlertDescription>{method.reason}</AlertDescription></Alert> : null}
      {instructions ? <p className='plugin-login-instructions'>{instructions}</p> : null}
      <PluginLoginBrowser method={method} provider={provider} desktop={false} value='external' onChange={() => undefined} disabled={locked} />
      {method.localOnly ? <Alert variant='warning'><AlertTitle>{t('需要网关本机环境')}</AlertTitle>
        <AlertDescription>{t('“本机”指运行网关的机器，不是打开面板的电脑。远程浏览器也可以发起，但网关本机可能需要安装客户端并已登录；是否可用由网关返回结果决定。')}</AlertDescription></Alert> : null}
      <PluginLoginFields method={method} values={inputs} prompts={formPrompts} prefix={prefix} disabled={locked}
        onChange={(key, value) => setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [key]: value } }))} />
      <PluginLoginOptions disabled={locked} summary={`${accountName || t('默认账号名')} · ${proxyLabel}`}>
        {optionalName || !hasNamePrompt ? <div className='plugin-login-field'>
          <Label htmlFor={`${prefix}-name`}>{t('备注名')}<span className='plugin-login-optional'>{t('可选')}</span></Label>
          <Input id={`${prefix}-name`} value={accountName} maxLength={100} disabled={locked}
            onChange={event => {
              const value = event.currentTarget.value
              if (optionalName) setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [optionalName.key]: value } }))
              else setName(value)
            }} placeholder={t('可选，留空使用账号昵称')} />
        </div> : null}
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
      <p className='plugin-login-note'>{t('所选代理会保存到账号设置，后续请求继续使用。')}</p>
      </PluginLoginOptions>
      <PluginLoginActions host={footerHost} active={active}>
        <Button disabled={submitLocked || Boolean(method.reason)} hidden={method.type === 'sms' && Boolean(flow)} onClick={() => void submit()}>
          {busy || resolvedPrompts.pending ? <Spinner /> : null}
          {method.type === 'api' || method.type === 'desktop' ? t('添加账号') : method.type === 'sms' ? t('开始短信登录') : t('登录并添加')}
        </Button>
      </PluginLoginActions>
    </> : null}
    {flow ? <Alert variant={flow.managed && waitUnconfirmed ? 'warning' : undefined}><AlertTitle>{flow.managed && waitUnconfirmed
      ? t('保存未确认') : flow.managed && managedPhase === 'saving' ? t('正在保存账号，保存结果尚未确认') : t('等待账号登录完成…')}</AlertTitle>
      <AlertDescription className='flex flex-col gap-2 whitespace-pre-wrap break-words'>
        {flow.managed && waitUnconfirmed ? <>
          <p>{t('账号保存未确认，请查询本次状态；不会再次保存。')}</p>
          <Button variant='outline' disabled={readingStatus || canceling || !flowCurrent(flow)}
            onClick={() => { if (flowCurrent(flow) && !waitBusy.current) setStatusRequest(value => value + 1) }}>
            {readingStatus ? <Spinner /> : null}{t('查询本次登录状态（只读）')}</Button>
        </> : null}
        {flow.url ? <Button variant='outline' onClick={() => { if (flowCurrent(flow)) openExternal(flow.url) }}>{t('打开官方授权页')}</Button> : null}
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
        <Button variant='outline' disabled={canceling} onClick={() => void cancel()}>{flow.managed && waitReadOnly.current ? t('关闭') : t('取消等待')}</Button>
      </AlertDescription>
    </Alert> : null}
    {error || resolvedPrompts.error ? <Alert variant='destructive'>
      {diagnostic ? <AlertTitle>{t('登录失败')}</AlertTitle> : null}
      <AlertDescription className='whitespace-pre-wrap break-words'>{error || resolvedPrompts.error}</AlertDescription>
      <PluginLoginDiagnostic diagnostic={diagnostic} />
    </Alert> : null}
    <PluginLoginSecurity />
  </div>
}
